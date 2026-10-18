//! Basic-block lowering for the isolated process runtime. Every reached x86
//! instruction, including branches/calls/returns and flag effects, is lowered
//! to Cranelift IR and then compiled to either RV64 (interpreted) or x86-64
//! (native) machine code.
use super::{
    cfi::EntryBitmap,
    diversify::Diversification,
    memory::{ADDRESS_LIMIT, GUEST_BIAS, STACK_BOTTOM, STACK_TOP},
};
use crate::{
    x86::{immediate, register_view},
    EngineError,
};
use cranelift_codegen::{
    ir::{
        condcodes::IntCC, types, AbiParam, Block, ExternalName, ExtFuncData, Function,
        InstBuilder, MemFlags, Signature, StackSlotData, StackSlotKind, TrapCode, Type,
        UserExternalNameRef, Value,
    },
    isa::CallConv,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use iced_x86::{Code, Instruction, Mnemonic, OpKind, Register};

/// Which backend the lowered block will be compiled for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub enum Backend {
    /// RISC-V 64-bit, executed by the in-process interpreter.
    #[default]
    Rv64,
    /// x86-64 native, executed directly on the host CPU.
    X86_64,
}

impl Backend {
    pub fn label(&self) -> &'static str {
        match self {
            Backend::Rv64 => "riscv64",
            Backend::X86_64 => "x86_64",
        }
    }
}

/// Bits reserved for pointer metadata under every tag scheme: the software tag
/// occupies 56:63 and the LAM_U57 tag occupies 57:62, so clearing the whole top
/// byte is correct for both and leaves a 48-bit guest address intact.
pub const POINTER_ADDRESS_MASK: u64 = !(0xffu64 << 56);

fn fail(i: &Instruction, reason: &'static str) -> EngineError {
    EngineError::Lower {
        rip: i.ip(),
        reason,
    }
}
fn ty(width: u16) -> Type {
    match width {
        8 => types::I8,
        16 => types::I16,
        32 => types::I32,
        _ => types::I64,
    }
}

struct Lower<'a> {
    b: FunctionBuilder<'a>,
    stack_fault: Option<Block>,
    rip: u64,
    backend: Backend,
    div: &'a Diversification,
    cfi: Option<&'a EntryBitmap>,
    /// Variable index that holds RSP after diversification permutation.
    rsp_var: u32,
    /// Cranelift function reference to `isr_cfi_check` (x86_64 only).
    cfi_check_fn: Option<cranelift_codegen::ir::FuncRef>,
    /// Cranelift signature reference for `isr_cfi_check` (x86_64 only).
    cfi_check_sig: Option<cranelift_codegen::ir::SigRef>,
}

impl Lower<'_> {
    fn c(&mut self, v: u64) -> Value {
        self.b.ins().iconst(types::I64, v as i64)
    }
    fn var(&mut self, index: usize) -> Value {
        self.b.use_var(Variable::from_u32(index as u32))
    }
    fn set(&mut self, index: usize, v: Value) {
        if index == self.rsp_var as usize {
            // Stop at the first invalid RSP, including transient pivots that a
            // later instruction would restore. The failure exit saves the
            // offending RSP/RIP; the runtime rejects it before any syscall.
            let fault = *self.stack_fault.get_or_insert_with(|| {
                let block = self.b.create_block();
                self.b.append_block_param(block, types::I64);
                self.b.append_block_param(block, types::I64);
                block
            });
            let below = self
                .b
                .ins()
                .icmp_imm(IntCC::UnsignedLessThan, v, STACK_BOTTOM as i64);
            let above = self
                .b
                .ins()
                .icmp_imm(IntCC::UnsignedGreaterThan, v, STACK_TOP as i64);
            let bad = self.b.ins().bor(below, above);
            let rip = self.c(self.rip);
            let resume = self.b.create_block();
            self.b.ins().brif(bad, fault, &[v, rip], resume, &[]);
            self.b.switch_to_block(resume);
            self.b.seal_block(resume);
        }
        self.b.def_var(Variable::from_u32(index as u32), v);
    }
    fn mask(&mut self, v: Value, w: u16) -> Value {
        if w == 64 {
            v
        } else {
            self.b.ins().band_imm(v, ((1u64 << w) - 1) as i64)
        }
    }
    fn sign(&mut self, v: Value, w: u16) -> Value {
        if w == 64 {
            v
        } else {
            let n = self.b.ins().ireduce(ty(w), v);
            self.b.ins().sextend(types::I64, n)
        }
    }

    /// Map a guest register to its Cranelift variable, applying diversification.
    fn reg(&mut self, r: Register, i: &Instruction) -> Result<Value, EngineError> {
        let (index, w, shift) = register_view(r).ok_or_else(|| fail(i, "non-integer register"))?;
        let var_idx = if index < 16 {
            self.div.variable(index) as usize
        } else {
            index
        };
        let mut v = self.var(var_idx);
        if shift != 0 {
            v = self.b.ins().ushr_imm(v, i64::from(shift));
        }
        Ok(self.mask(v, w))
    }

    /// Write a value to a guest register, applying diversification.
    fn put_reg(&mut self, r: Register, v: Value, i: &Instruction) -> Result<(), EngineError> {
        let (index, w, shift) = register_view(r).ok_or_else(|| fail(i, "non-integer register"))?;
        let var_idx = if index < 16 {
            self.div.variable(index) as usize
        } else {
            index
        };
        let mut v = self.mask(v, w);
        if w < 32 {
            if shift != 0 {
                v = self.b.ins().ishl_imm(v, i64::from(shift));
            }
            let old = self.var(var_idx);
            let old = self
                .b
                .ins()
                .band_imm(old, !(((1u64 << w) - 1) << shift) as i64);
            v = self.b.ins().bor(old, v);
        }
        self.set(var_idx, v);
        Ok(())
    }

    fn address(&mut self, i: &Instruction) -> Result<Value, EngineError> {
        if i.segment_prefix() != Register::None {
            return Err(fail(i, "segment addressing unsupported"));
        }
        if i.is_ip_rel_memory_operand() {
            return Ok(self.c(i.ip_rel_memory_address()));
        }
        let mut v = self.c(i.memory_displacement64());
        for (r, scale) in [
            (i.memory_base(), 1),
            (i.memory_index(), i.memory_index_scale()),
        ] {
            if r != Register::None {
                let index = self.reg(r, i)?;
                let index = self.b.ins().imul_imm(index, i64::from(scale));
                v = self.b.ins().iadd(v, index);
            }
        }
        // Address-size override wraps at 32 bits before mapping.
        if register_view(i.memory_base()).is_some_and(|(_, w, _)| w == 32)
            || register_view(i.memory_index()).is_some_and(|(_, w, _)| w == 32)
        {
            v = self.mask(v, 32);
        }
        Ok(v)
    }

    /// Bound-check a guest address and bias it into the bus's window.
    fn ptr(&mut self, v: Value, w: u16) -> Value {
        let plain = self.b.ins().band_imm(v, POINTER_ADDRESS_MASK as i64);
        let bad = self.b.ins().icmp_imm(
            IntCC::UnsignedGreaterThan,
            plain,
            (ADDRESS_LIMIT - u64::from(w / 8)) as i64,
        );
        self.b.ins().trapnz(bad, TrapCode::HEAP_OUT_OF_BOUNDS);
        self.b.ins().iadd_imm(v, GUEST_BIAS as i64)
    }
    fn width(&self, i: &Instruction, n: u32) -> Result<u16, EngineError> {
        match i.op_kind(n) {
            OpKind::Register => register_view(i.op_register(n))
                .map(|(_, w, _)| w)
                .ok_or_else(|| fail(i, "non-integer operand")),
            OpKind::Memory => crate::x86::memory_width(i)
                .map(|w| (w * 8) as u16)
                .ok_or_else(|| fail(i, "non-integer memory operand")),
            _ => Err(fail(i, "operand has no integer width")),
        }
    }
    fn read(&mut self, i: &Instruction, n: u32) -> Result<Value, EngineError> {
        if let Some(v) = immediate(i, n) {
            return Ok(self.c(v));
        }
        match i.op_kind(n) {
            OpKind::Register => self.reg(i.op_register(n), i),
            OpKind::Memory => {
                let w = self.width(i, n)?;
                let p = self.address(i)?;
                self.load(p, w)
            }
            OpKind::NearBranch64 => Ok(self.c(i.near_branch_target())),
            _ => Err(fail(i, "unsupported source operand")),
        }
    }
    fn load(&mut self, p: Value, w: u16) -> Result<Value, EngineError> {
        let p = self.ptr(p, w);
        let v = self.b.ins().load(ty(w), MemFlags::new(), p, 0);
        Ok(if w < 64 {
            self.b.ins().uextend(types::I64, v)
        } else {
            v
        })
    }
    fn store(&mut self, p: Value, w: u16, v: Value) {
        let p = self.ptr(p, w);
        let v = if w < 64 {
            self.b.ins().ireduce(ty(w), v)
        } else {
            v
        };
        self.b.ins().store(MemFlags::new(), v, p, 0);
    }
    fn write(&mut self, i: &Instruction, n: u32, v: Value) -> Result<(), EngineError> {
        match i.op_kind(n) {
            OpKind::Register => self.put_reg(i.op_register(n), v, i),
            OpKind::Memory => {
                let w = self.width(i, n)?;
                let p = self.address(i)?;
                self.store(p, w, v);
                Ok(())
            }
            _ => Err(fail(i, "unsupported destination operand")),
        }
    }
    fn bit(&mut self, v: Value, n: u16) -> Value {
        let v = self.b.ins().ushr_imm(v, i64::from(n));
        self.b.ins().band_imm(v, 1)
    }
    fn bool64(&mut self, v: Value) -> Value {
        self.b.ins().uextend(types::I64, v)
    }
    fn flag(&mut self, n: u16) -> Value {
        // RFLAGS is always variable 17 (not diversified)
        let f = self.var(17);
        self.bit(f, n)
    }
    fn status(&mut self, result: Value, w: u16) -> Value {
        let z = self.b.ins().icmp_imm(IntCC::Equal, result, 0);
        let z = self.bool64(z);
        let z = self.b.ins().ishl_imm(z, 6);
        let s = self.bit(result, w - 1);
        let s = self.b.ins().ishl_imm(s, 7);
        // PF is even parity of the low byte, not the whole operand.
        let mut p = result;
        for shift in [4, 2, 1] {
            let high = self.b.ins().ushr_imm(p, shift);
            p = self.b.ins().bxor(p, high);
        }
        let p = self.b.ins().bnot(p);
        let p = self.b.ins().band_imm(p, 1);
        let p = self.b.ins().ishl_imm(p, 2);
        let f = self.b.ins().bor(z, s);
        self.b.ins().bor(f, p)
    }
    fn flags(&mut self, a: Value, b: Value, result: Value, w: u16, mnemonic: Mnemonic) {
        use Mnemonic::*;
        let sub = matches!(mnemonic, Sub | Cmp | Dec | Neg);
        let logical = matches!(mnemonic, Test | And | Or | Xor);
        let preserve_carry = matches!(mnemonic, Inc | Dec);
        let mut f = self.status(result, w);
        if !logical {
            let c = if sub {
                self.b.ins().icmp(IntCC::UnsignedLessThan, a, b)
            } else {
                self.b.ins().icmp(IntCC::UnsignedLessThan, result, a)
            };
            let c = self.bool64(c);
            let ar = self.b.ins().bxor(a, result);
            let ab = self.b.ins().bxor(a, b);
            let overflow = if sub {
                self.b.ins().band(ab, ar)
            } else {
                let br = self.b.ins().bxor(b, result);
                self.b.ins().band(ar, br)
            };
            let o = self.bit(overflow, w - 1);
            let o = self.b.ins().ishl_imm(o, 11);
            let af = self.b.ins().bxor(ab, result);
            let af = self.b.ins().band_imm(af, 16);
            f = self.b.ins().bor(f, c);
            f = self.b.ins().bor(f, o);
            f = self.b.ins().bor(f, af);
        }
        // RFLAGS is always variable 17 (not diversified)
        let old = self.var(17);
        if preserve_carry {
            f = self.b.ins().band_imm(f, !1);
            let c = self.b.ins().band_imm(old, 1);
            f = self.b.ins().bor(f, c);
        }
        let old = self.b.ins().band_imm(old, !0x8d5);
        f = self.b.ins().bor(f, old);
        self.set(17, f);
    }
    fn condition(&mut self, m: Mnemonic, i: &Instruction) -> Result<Value, EngineError> {
        use Mnemonic::*;
        let z = self.flag(6);
        let s = self.flag(7);
        let o = self.flag(11);
        let c = self.flag(0);
        let p = self.flag(2);
        let so = self.b.ins().bxor(s, o);
        let (v, invert) = match m {
            Je | Cmove | Sete => (z, false),
            Jne | Cmovne | Setne => (z, true),
            Jb | Cmovb | Setb => (c, false),
            Jae | Cmovae | Setae => (c, true),
            Jbe | Cmovbe | Setbe => (self.b.ins().bor(c, z), false),
            Ja | Cmova | Seta => (self.b.ins().bor(c, z), true),
            Jl | Cmovl | Setl => (so, false),
            Jge | Cmovge | Setge => (so, true),
            Jle | Cmovle | Setle => (self.b.ins().bor(so, z), false),
            Jg | Cmovg | Setg => (self.b.ins().bor(so, z), true),
            Js | Cmovs | Sets => (s, false),
            Jns | Cmovns | Setns => (s, true),
            Jo | Cmovo | Seto => (o, false),
            Jno | Cmovno | Setno => (o, true),
            Jp | Cmovp | Setp => (p, false),
            Jnp | Cmovnp | Setnp => (p, true),
            _ => return Err(fail(i, "unsupported condition")),
        };
        Ok(self.b.ins().icmp_imm(
            if invert {
                IntCC::Equal
            } else {
                IntCC::NotEqual
            },
            v,
            0,
        ))
    }
    fn push(&mut self, v: Value) {
        let sp = self.var(self.rsp_var as usize);
        let sp = self.b.ins().iadd_imm(sp, -8);
        self.set(self.rsp_var as usize, sp);
        self.store(sp, 64, v);
    }
    fn pop(&mut self) -> Result<Value, EngineError> {
        let sp = self.var(self.rsp_var as usize);
        let v = self.load(sp, 64)?;
        let sp = self.b.ins().iadd_imm(sp, 8);
        self.set(self.rsp_var as usize, sp);
        Ok(v)
    }

    /// Insert an inline CFI check for an indirect branch target.
    /// Only active for x86_64 backend with a CFI bitmap.
    fn cfi_check(&mut self, target: Value) {
        if self.backend != Backend::X86_64 {
            return;
        }
        let Some(bitmap) = self.cfi else {
            return;
        };
        let Some(_cfi_fn) = self.cfi_check_fn else {
            return;
        };

        // Call isr_cfi_check(bitmap_ptr, target) -> u32
        let bitmap_ptr = self.c(bitmap.base());
        // For the inline check, we use a simple bitmap lookup.
        // offset = target - base
        let offset = self.b.ins().isub(target, bitmap_ptr);
        let len = self.c(bitmap.len_bytes() as u64 * 8);
        let in_bounds = self.b.ins().icmp(IntCC::UnsignedLessThan, offset, len);
        self.b.ins().trapz(in_bounds, TrapCode::STACK_OVERFLOW);

        // byte_idx = offset / 8
        let byte_idx = self.b.ins().ushr_imm(offset, 3);
        // bit_idx = offset & 7
        let bit_idx = self.b.ins().band_imm(offset, 7);

        // Load the byte from the bitmap
        let bitmap_bytes = self.c(bitmap.as_slice().as_ptr() as u64);
        let byte_addr = self.b.ins().iadd(bitmap_bytes, byte_idx);
        let byte = self.b.ins().load(types::I8, MemFlags::new(), byte_addr, 0);

        // mask = 1 << bit_idx
        let one = self.b.ins().iconst(types::I8, 1);
        let bit_idx_i8 = self.b.ins().ireduce(types::I8, bit_idx);
        let mask = self.b.ins().ishl(one, bit_idx_i8);
        let masked = self.b.ins().band(byte, mask);
        let is_valid = self.b.ins().icmp_imm(IntCC::NotEqual, masked, 0);
        self.b.ins().trapz(is_valid, TrapCode::STACK_OVERFLOW);
    }
}

pub fn lower_block(
    trace: &[Instruction],
    backend: Backend,
    div: &Diversification,
    cfi: Option<&EntryBitmap>,
) -> Result<Function, EngineError> {
    let mut sig = Signature::new(CallConv::SystemV);
    sig.params.push(AbiParam::new(types::I64)); // ctx
    if backend == Backend::X86_64 {
        // x86_64 native blocks receive the CFI bitmap pointer as arg 1.
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
    }
    let mut function = Function::with_name_signature(Default::default(), sig);
    let mut fb = FunctionBuilderContext::new();
    let mut b = FunctionBuilder::new(&mut function, &mut fb);
    let entry = b.create_block();
    b.append_block_params_for_function_params(entry);
    b.switch_to_block(entry);
    b.seal_block(entry);

    let ctx = b.block_params(entry)[0];

    // Compute which variable holds RSP after diversification.
    let rsp_var = if backend == Backend::X86_64 {
        // Find which variable index maps to guest register 7 (RSP).
        let mut v = 7u32;
        for guest in 0..16 {
            if div.variable(guest) == 7 {
                v = guest as u32;
                break;
            }
        }
        v
    } else {
        7 // RSP is always variable 7 for RV64
    };

    // Import CFI check function for x86_64.
    let (cfi_check_fn, cfi_check_sig) = if backend == Backend::X86_64 && cfi.is_some() {
        let mut cfi_sig = Signature::new(CallConv::SystemV);
        cfi_sig.params.push(AbiParam::new(types::I64));
        cfi_sig.params.push(AbiParam::new(types::I64));
        cfi_sig.returns.push(AbiParam::new(types::I32));
        let sig_ref = b.import_signature(cfi_sig);
        let func_ref = b.import_function(ExtFuncData {
            name: ExternalName::user(UserExternalNameRef::from_u32(0)),
            signature: sig_ref,
            colocated: false,
        });
        (Some(func_ref), Some(sig_ref))
    } else {
        (None, None)
    };

    // Declare variables 0..17.
    for j in 0..18 {
        let var = Variable::from_u32(j);
        b.declare_var(var, types::I64);
    }

    // Load context: guest register i from ctx[i*8] -> variable permute[i] (or i for RIP/RFLAGS)
    for i in 0..16 {
        let var_idx = div.variable(i);
        let var = Variable::from_u32(var_idx);
        let v = b
            .ins()
            .load(types::I64, MemFlags::new(), ctx, (i * 8) as i32);
        b.def_var(var, v);
    }
    // RIP (16) and RFLAGS (17) are not diversified.
    for i in 16..18 {
        let var = Variable::from_u32(i);
        let v = b
            .ins()
            .load(types::I64, MemFlags::new(), ctx, (i * 8) as i32);
        b.def_var(var, v);
    }

    // Stack padding for diversification (x86_64 only): create a dummy stack
    // slot so the function prologue reserves extra space. This shifts the
    // offsets of spilled registers and local temporaries, changing gadgets.
    if backend == Backend::X86_64 && div.stack_pad() > 0 {
        let data = StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            div.stack_pad() as u32,
            4, // 16-byte alignment
        );
        let slot = b.create_sized_stack_slot(data);
        // Force the slot to be "used" so Cranelift doesn't eliminate it.
        let addr = b.ins().stack_addr(types::I64, slot, 0);
        let zero = b.ins().iconst(types::I64, 0);
        b.ins().store(MemFlags::new(), zero, addr, 0);
    }

    let mut l = Lower {
        b,
        stack_fault: None,
        rip: 0,
        backend,
        div,
        cfi,
        rsp_var,
        cfi_check_fn,
        cfi_check_sig,
    };

    for i in trace {
        use Mnemonic::*;
        l.rip = i.ip();
        let next = l.c(i.next_ip());
        l.set(16, next);
        if i.mnemonic() == Nop {
            continue;
        }
        if i.has_lock_prefix() || i.has_rep_prefix() || i.has_repne_prefix() {
            return Err(fail(i, "atomic and string prefixes unsupported"));
        }
        match i.mnemonic() {
            Mov | Movzx | Movsx | Movsxd => {
                let mut v = l.read(i, 1)?;
                if matches!(i.mnemonic(), Movsx | Movsxd) {
                    v = l.sign(v, l.width(i, 1)?);
                }
                l.write(i, 0, v)?;
            }
            Lea => {
                let v = l.address(i)?;
                l.write(i, 0, v)?;
            }
            Push => {
                if !matches!(
                    i.code(),
                    Code::Push_r64 | Code::Push_rm64 | Code::Pushq_imm8 | Code::Pushq_imm32
                ) {
                    return Err(fail(i, "only 64-bit push supported"));
                }
                let v = l.read(i, 0)?;
                l.push(v);
            }
            Pop => {
                if i.op0_kind() != OpKind::Register || l.width(i, 0)? != 64 {
                    return Err(fail(i, "only 64-bit register pop supported"));
                }
                let v = l.pop()?;
                l.write(i, 0, v)?;
            }
            Call => {
                let target = l.read(i, 0)?;
                // CFI check for indirect calls.
                if !matches!(i.op0_kind(), OpKind::NearBranch64) {
                    l.cfi_check(target);
                }
                l.push(next);
                l.set(16, target);
            }
            Ret => {
                if !matches!(i.code(), Code::Retnq | Code::Retnq_imm16) {
                    return Err(fail(i, "only 64-bit near return supported"));
                }
                let target = l.pop()?;
                if i.op_count() != 0 {
                    let sp = l.var(l.rsp_var as usize);
                    let sp = l.b.ins().iadd_imm(sp, i.immediate16() as i64);
                    l.set(l.rsp_var as usize, sp);
                }
                l.set(16, target);
            }
            Jmp => {
                let target = l.read(i, 0)?;
                // CFI check for indirect jumps.
                if !matches!(i.op0_kind(), OpKind::NearBranch64) {
                    l.cfi_check(target);
                }
                l.set(16, target);
            }
            Je | Jne | Jb | Jae | Jbe | Ja | Jl | Jge | Jle | Jg | Js | Jns | Jo | Jno | Jp
            | Jnp => {
                let cond = l.condition(i.mnemonic(), i)?;
                let target = l.read(i, 0)?;
                let rip = l.b.ins().select(cond, target, next);
                l.set(16, rip);
            }
            Cmove | Cmovne | Cmovb | Cmovae | Cmovbe | Cmova | Cmovl | Cmovge | Cmovle | Cmovg
            | Cmovs | Cmovns | Cmovo | Cmovno | Cmovp | Cmovnp => {
                let cond = l.condition(i.mnemonic(), i)?;
                let a = l.read(i, 0)?;
                let b = l.read(i, 1)?;
                let v = l.b.ins().select(cond, b, a);
                l.write(i, 0, v)?;
            }
            Sete | Setne | Setb | Setae | Setbe | Seta | Setl | Setge | Setle | Setg | Sets
            | Setns | Seto | Setno | Setp | Setnp => {
                let cond = l.condition(i.mnemonic(), i)?;
                let v = l.bool64(cond);
                l.write(i, 0, v)?;
            }
            Add | Sub | Cmp | Test | And | Or | Xor | Inc | Dec | Neg => {
                let w = l.width(i, 0)?;
                let mut a = l.read(i, 0)?;
                let b = if matches!(i.mnemonic(), Inc | Dec) {
                    l.c(1)
                } else if i.mnemonic() == Neg {
                    let b = a;
                    a = l.c(0);
                    b
                } else {
                    let v = l.read(i, 1)?;
                    l.mask(v, w)
                };
                let v = match i.mnemonic() {
                    Sub | Cmp | Dec | Neg => l.b.ins().isub(a, b),
                    Test | And => l.b.ins().band(a, b),
                    Or => l.b.ins().bor(a, b),
                    Xor => l.b.ins().bxor(a, b),
                    _ => l.b.ins().iadd(a, b),
                };
                let v = l.mask(v, w);
                l.flags(a, b, v, w, i.mnemonic());
                if !matches!(i.mnemonic(), Cmp | Test) {
                    l.write(i, 0, v)?;
                }
            }
            Not => {
                let a = l.read(i, 0)?;
                let v = l.b.ins().bnot(a);
                l.write(i, 0, v)?;
            }
            Imul => {
                if i.op_count() < 2 {
                    return Err(fail(i, "one-operand multiply unsupported"));
                }
                let w = l.width(i, 0)?;
                let a = l.read(i, if i.op_count() == 3 { 1 } else { 0 })?;
                let b = l.read(i, if i.op_count() == 3 { 2 } else { 1 })?;
                let a = l.sign(a, w);
                let b = l.sign(b, w);
                let result = l.b.ins().imul(a, b);
                let truncated = l.mask(result, w);
                let overflow = if w == 64 {
                    let high = l.b.ins().smulhi(a, b);
                    let sign = l.b.ins().sshr_imm(result, 63);
                    l.b.ins().icmp(IntCC::NotEqual, high, sign)
                } else {
                    let signed = l.sign(truncated, w);
                    l.b.ins().icmp(IntCC::NotEqual, result, signed)
                };
                let c = l.bool64(overflow);
                let o = l.b.ins().ishl_imm(c, 11);
                let f = l.b.ins().bor(c, o);
                // RFLAGS is always variable 17
                let old = l.var(17);
                let old = l.b.ins().band_imm(old, !0x801);
                let f = l.b.ins().bor(old, f);
                l.set(17, f);
                l.write(i, 0, truncated)?;
            }
            Rol | Ror => {
                let w = l.width(i, 0)?;
                let a = l.read(i, 0)?;
                let count = immediate(i, 1)
                    .ok_or_else(|| fail(i, "only immediate rotations supported"))?
                    & if w == 64 { 63 } else { 31 };
                if count != 0 {
                    let effective = count % u64::from(w);
                    let v = if effective == 0 {
                        a
                    } else {
                        let (left, right) = if i.mnemonic() == Rol {
                            (effective, u64::from(w) - effective)
                        } else {
                            (u64::from(w) - effective, effective)
                        };
                        let lo = l.b.ins().ishl_imm(a, left as i64);
                        let hi = l.b.ins().ushr_imm(a, right as i64);
                        let v = l.b.ins().bor(lo, hi);
                        l.mask(v, w)
                    };
                    let c = l.bit(v, if i.mnemonic() == Rol { 0 } else { w - 1 });
                    // RFLAGS is always variable 17
                    let old = l.var(17);
                    let old =
                        l.b
                            .ins()
                            .band_imm(old, if count == 1 { !0x801 } else { !1 });
                    let mut f = l.b.ins().bor(old, c);
                    if count == 1 {
                        let top = l.bit(v, w - 1);
                        let other = if i.mnemonic() == Rol {
                            c
                        } else {
                            l.bit(v, w - 2)
                        };
                        let o = l.b.ins().bxor(top, other);
                        let o = l.b.ins().ishl_imm(o, 11);
                        f = l.b.ins().bor(f, o);
                    }
                    l.set(17, f);
                    l.write(i, 0, v)?;
                }
            }
            Syscall => {
                l.set(2, next);
                let f = l.var(17);
                l.set(11, f);
            }
            _ => return Err(fail(i, "instruction unsupported by process lowerer")),
        }
    }

    // Store context: variable permute[i] -> ctx[i*8]
    for i in 0..16 {
        let var_idx = div.variable(i);
        let v = l.var(var_idx as usize);
        l.b.ins().store(MemFlags::new(), v, ctx, (i * 8) as i32);
    }
    for i in 16..18 {
        let v = l.var(i);
        l.b.ins().store(MemFlags::new(), v, ctx, (i * 8) as i32);
    }

    if backend == Backend::X86_64 {
        let zero = l.c(0);
        l.b.ins().return_(&[zero]);
    } else {
        l.b.ins().return_(&[]);
    }

    if let Some(fault) = l.stack_fault {
        l.b.switch_to_block(fault);
        l.b.seal_block(fault);
        let rsp = l.b.block_params(fault)[0];
        let rip = l.b.block_params(fault)[1];
        l.b.ins().store(MemFlags::new(), rsp, ctx, 7 * 8);
        l.b.ins().store(MemFlags::new(), rip, ctx, 16 * 8);
        if backend == Backend::X86_64 {
            // Return sentinel for stack fault.
            let two = l.c(2);
            l.b.ins().return_(&[two]);
        } else {
            l.b.ins().return_(&[]);
        }
    }
    l.b.finalize();
    Ok(function)
}
