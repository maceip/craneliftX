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
    Rv64,
    /// Host-native (x86-64 on Intel, aarch64 on Apple Silicon), executed
    /// directly on the CPU with checked memory helpers. This is the default.
    #[default]
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

fn xmm_index(reg: Register) -> Option<usize> {
    match reg {
        Register::XMM0 => Some(0),
        Register::XMM1 => Some(1),
        Register::XMM2 => Some(2),
        Register::XMM3 => Some(3),
        Register::XMM4 => Some(4),
        Register::XMM5 => Some(5),
        Register::XMM6 => Some(6),
        Register::XMM7 => Some(7),
        Register::XMM8 => Some(8),
        Register::XMM9 => Some(9),
        Register::XMM10 => Some(10),
        Register::XMM11 => Some(11),
        Register::XMM12 => Some(12),
        Register::XMM13 => Some(13),
        Register::XMM14 => Some(14),
        Register::XMM15 => Some(15),
        _ => None,
    }
}

struct Lower<'a> {
    b: FunctionBuilder<'a>,
    stack_fault: Option<Block>,
    mem_fault: Option<Block>,
    rip: u64,
    backend: Backend,
    div: &'a Diversification,
    cfi: Option<&'a EntryBitmap>,
    /// Variable index that holds RSP after diversification permutation.
    rsp_var: u32,
    /// The block context (NativeGuest / register file).
    ctx: Value,
    /// Cranelift function reference to `isr_cfi_check` (x86_64 only).
    cfi_check_fn: Option<cranelift_codegen::ir::FuncRef>,
    /// Cranelift signature reference for `isr_cfi_check` (x86_64 only).
    cfi_check_sig: Option<cranelift_codegen::ir::SigRef>,
    /// Signatures for the native checked memory helpers.
    load_sig: Option<cranelift_codegen::ir::SigRef>,
    store_sig: Option<cranelift_codegen::ir::SigRef>,
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

    fn get_xmm(&mut self, idx: usize) -> (Value, Value) {
        (self.var(19 + idx), self.var(35 + idx))
    }
    fn set_xmm(&mut self, idx: usize, lo: Value, hi: Value) {
        self.b.def_var(Variable::from_u32((19 + idx) as u32), lo);
        self.b.def_var(Variable::from_u32((35 + idx) as u32), hi);
    }

    fn read_xmm(&mut self, i: &Instruction, op: u32) -> Result<(Value, Value), EngineError> {
        if i.op_kind(op) == OpKind::Register {
            let idx = xmm_index(i.op_register(op)).ok_or_else(|| fail(i, "expected XMM register"))?;
            Ok(self.get_xmm(idx))
        } else if i.op_kind(op) == OpKind::Memory {
            let p = self.address(i)?;
            let lo = self.load(p, 64)?;
            let p8 = self.b.ins().iadd_imm(p, 8);
            let hi = self.load(p8, 64)?;
            Ok((lo, hi))
        } else {
            Err(fail(i, "unsupported XMM operand"))
        }
    }

    fn pcmpeqb_u64(&mut self, a: Value, b: Value) -> Value {
        let mut res = self.c(0);
        let c_ff = self.c(0xff);
        let c_0 = self.c(0);
        for byte_idx in 0..8 {
            let shift = (byte_idx * 8) as i64;
            let ba = if shift > 0 { self.b.ins().ushr_imm(a, shift) } else { a };
            let ba = self.b.ins().band_imm(ba, 0xff);
            let bb = if shift > 0 { self.b.ins().ushr_imm(b, shift) } else { b };
            let bb = self.b.ins().band_imm(bb, 0xff);
            let eq = self.b.ins().icmp(IntCC::Equal, ba, bb);
            let mask = self.b.ins().select(eq, c_ff, c_0);
            let shifted = if shift > 0 { self.b.ins().ishl_imm(mask, shift) } else { mask };
            res = self.b.ins().bor(res, shifted);
        }
        res
    }

    fn pcmpeqd_u64(&mut self, a: Value, b: Value) -> Value {
        let mut res = self.c(0);
        let c_ffff = self.c(0xffffffff);
        let c_0 = self.c(0);
        for dword_idx in 0..2 {
            let shift = (dword_idx * 32) as i64;
            let da = if shift > 0 { self.b.ins().ushr_imm(a, shift) } else { a };
            let da = self.b.ins().band_imm(da, 0xffffffff);
            let db = if shift > 0 { self.b.ins().ushr_imm(b, shift) } else { b };
            let db = self.b.ins().band_imm(db, 0xffffffff);
            let eq = self.b.ins().icmp(IntCC::Equal, da, db);
            let mask = self.b.ins().select(eq, c_ffff, c_0);
            let shifted = if shift > 0 { self.b.ins().ishl_imm(mask, shift) } else { mask };
            res = self.b.ins().bor(res, shifted);
        }
        res
    }

    fn paddb_u64(&mut self, a: Value, b: Value) -> Value {
        let mut res = self.c(0);
        for byte_idx in 0..8 {
            let shift = (byte_idx * 8) as i64;
            let ba = if shift > 0 { self.b.ins().ushr_imm(a, shift) } else { a };
            let ba = self.b.ins().band_imm(ba, 0xff);
            let bb = if shift > 0 { self.b.ins().ushr_imm(b, shift) } else { b };
            let bb = self.b.ins().band_imm(bb, 0xff);
            let sum = self.b.ins().iadd(ba, bb);
            let sum = self.b.ins().band_imm(sum, 0xff);
            let shifted = if shift > 0 { self.b.ins().ishl_imm(sum, shift) } else { sum };
            res = self.b.ins().bor(res, shifted);
        }
        res
    }

    fn paddw_u64(&mut self, a: Value, b: Value) -> Value {
        let mut res = self.c(0);
        for idx in 0..4 {
            let shift = (idx * 16) as i64;
            let wa = if shift > 0 { self.b.ins().ushr_imm(a, shift) } else { a };
            let wa = self.b.ins().band_imm(wa, 0xffff);
            let wb = if shift > 0 { self.b.ins().ushr_imm(b, shift) } else { b };
            let wb = self.b.ins().band_imm(wb, 0xffff);
            let sum = self.b.ins().iadd(wa, wb);
            let sum = self.b.ins().band_imm(sum, 0xffff);
            let shifted = if shift > 0 { self.b.ins().ishl_imm(sum, shift) } else { sum };
            res = self.b.ins().bor(res, shifted);
        }
        res
    }

    fn paddd_u64(&mut self, a: Value, b: Value) -> Value {
        let mut res = self.c(0);
        for idx in 0..2 {
            let shift = (idx * 32) as i64;
            let da = if shift > 0 { self.b.ins().ushr_imm(a, shift) } else { a };
            let da = self.b.ins().band_imm(da, 0xffffffff);
            let db = if shift > 0 { self.b.ins().ushr_imm(b, shift) } else { b };
            let db = self.b.ins().band_imm(db, 0xffffffff);
            let sum = self.b.ins().iadd(da, db);
            let sum = self.b.ins().band_imm(sum, 0xffffffff);
            let shifted = if shift > 0 { self.b.ins().ishl_imm(sum, shift) } else { sum };
            res = self.b.ins().bor(res, shifted);
        }
        res
    }

    fn psubb_u64(&mut self, a: Value, b: Value) -> Value {
        let mut res = self.c(0);
        for byte_idx in 0..8 {
            let shift = (byte_idx * 8) as i64;
            let ba = if shift > 0 { self.b.ins().ushr_imm(a, shift) } else { a };
            let ba = self.b.ins().band_imm(ba, 0xff);
            let bb = if shift > 0 { self.b.ins().ushr_imm(b, shift) } else { b };
            let bb = self.b.ins().band_imm(bb, 0xff);
            let diff = self.b.ins().isub(ba, bb);
            let diff = self.b.ins().band_imm(diff, 0xff);
            let shifted = if shift > 0 { self.b.ins().ishl_imm(diff, shift) } else { diff };
            res = self.b.ins().bor(res, shifted);
        }
        res
    }

    fn psubw_u64(&mut self, a: Value, b: Value) -> Value {
        let mut res = self.c(0);
        for idx in 0..4 {
            let shift = (idx * 16) as i64;
            let wa = if shift > 0 { self.b.ins().ushr_imm(a, shift) } else { a };
            let wa = self.b.ins().band_imm(wa, 0xffff);
            let wb = if shift > 0 { self.b.ins().ushr_imm(b, shift) } else { b };
            let wb = self.b.ins().band_imm(wb, 0xffff);
            let diff = self.b.ins().isub(wa, wb);
            let diff = self.b.ins().band_imm(diff, 0xffff);
            let shifted = if shift > 0 { self.b.ins().ishl_imm(diff, shift) } else { diff };
            res = self.b.ins().bor(res, shifted);
        }
        res
    }

    fn psubd_u64(&mut self, a: Value, b: Value) -> Value {
        let mut res = self.c(0);
        for idx in 0..2 {
            let shift = (idx * 32) as i64;
            let da = if shift > 0 { self.b.ins().ushr_imm(a, shift) } else { a };
            let da = self.b.ins().band_imm(da, 0xffffffff);
            let db = if shift > 0 { self.b.ins().ushr_imm(b, shift) } else { b };
            let db = self.b.ins().band_imm(db, 0xffffffff);
            let diff = self.b.ins().isub(da, db);
            let diff = self.b.ins().band_imm(diff, 0xffffffff);
            let shifted = if shift > 0 { self.b.ins().ishl_imm(diff, shift) } else { diff };
            res = self.b.ins().bor(res, shifted);
        }
        res
    }

    fn pmaxub_u64(&mut self, a: Value, b: Value) -> Value {
        let mut res = self.c(0);
        for byte_idx in 0..8 {
            let shift = (byte_idx * 8) as i64;
            let ba = if shift > 0 { self.b.ins().ushr_imm(a, shift) } else { a };
            let ba = self.b.ins().band_imm(ba, 0xff);
            let bb = if shift > 0 { self.b.ins().ushr_imm(b, shift) } else { b };
            let bb = self.b.ins().band_imm(bb, 0xff);
            let cmp = self.b.ins().icmp(IntCC::UnsignedGreaterThan, ba, bb);
            let m = self.b.ins().select(cmp, ba, bb);
            let shifted = if shift > 0 { self.b.ins().ishl_imm(m, shift) } else { m };
            res = self.b.ins().bor(res, shifted);
        }
        res
    }

    fn pminub_u64(&mut self, a: Value, b: Value) -> Value {
        let mut res = self.c(0);
        for byte_idx in 0..8 {
            let shift = (byte_idx * 8) as i64;
            let ba = if shift > 0 { self.b.ins().ushr_imm(a, shift) } else { a };
            let ba = self.b.ins().band_imm(ba, 0xff);
            let bb = if shift > 0 { self.b.ins().ushr_imm(b, shift) } else { b };
            let bb = self.b.ins().band_imm(bb, 0xff);
            let cmp = self.b.ins().icmp(IntCC::UnsignedLessThan, ba, bb);
            let m = self.b.ins().select(cmp, ba, bb);
            let shifted = if shift > 0 { self.b.ins().ishl_imm(m, shift) } else { m };
            res = self.b.ins().bor(res, shifted);
        }
        res
    }


    fn address(&mut self, i: &Instruction) -> Result<Value, EngineError> {
        let seg = i.segment_prefix();
        if seg != Register::None
            && seg != Register::FS
            && seg != Register::CS
            && seg != Register::DS
            && seg != Register::SS
            && seg != Register::ES
        {
            return Err(fail(i, "segment addressing unsupported"));
        }
        let mut v = if i.is_ip_rel_memory_operand() {
            self.c(i.ip_rel_memory_address())
        } else {
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
            v
        };
        if seg == Register::FS {
            let fs_base = self.var(18);
            v = self.b.ins().iadd(v, fs_base);
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
        if self.backend == Backend::X86_64 {
            return self.native_load(p, w);
        }
        let p = self.ptr(p, w);
        let v = self.b.ins().load(ty(w), MemFlags::new(), p, 0);
        Ok(if w < 64 {
            self.b.ins().uextend(types::I64, v)
        } else {
            v
        })
    }
    fn store(&mut self, p: Value, w: u16, v: Value, constant: bool) {
        if self.backend == Backend::X86_64 {
            self.native_store(p, w, v, constant);
            return;
        }
        let p = self.ptr(p, w);
        let v = if w < 64 {
            self.b.ins().ireduce(ty(w), v)
        } else {
            v
        };
        self.b.ins().store(MemFlags::new(), v, p, 0);
    }

    /// Load through the runtime's checked helper so tags and PAC apply.
    fn native_load(&mut self, p: Value, w: u16) -> Result<Value, EngineError> {
        self.bounds_check(p, w);
        let load_sig = self.load_sig.expect("x86_64 load signature");
        let cookie = self.b.ins().load(
            types::I64,
            MemFlags::new(),
            self.ctx,
            super::native::NATIVE_COOKIE_OFFSET,
        );
        let load_fn = self.b.ins().load(
            types::I64,
            MemFlags::new(),
            self.ctx,
            super::native::NATIVE_LOAD_FN_OFFSET,
        );
        let size = self.c(u64::from(w / 8));
        let pc = self.c(self.rip);
        let call = self.b.ins().call_indirect(load_sig, load_fn, &[cookie, p, size, pc]);
        let raw = self.b.inst_results(call)[0];
        self.check_native_fault();
        // Helper returns the zero-extended load already.
        Ok(raw)
    }

    /// Store through the runtime's checked helper so tags and PAC apply.
    /// `constant` marks immediate/lea provenance for PAC signing.
    fn native_store(&mut self, p: Value, w: u16, v: Value, constant: bool) {
        self.bounds_check(p, w);
        let store_sig = self.store_sig.expect("x86_64 store signature");
        let cookie = self.b.ins().load(
            types::I64,
            MemFlags::new(),
            self.ctx,
            super::native::NATIVE_COOKIE_OFFSET,
        );
        let store_fn = self.b.ins().load(
            types::I64,
            MemFlags::new(),
            self.ctx,
            super::native::NATIVE_STORE_FN_OFFSET,
        );
        let size = self.c(u64::from(w / 8));
        // Helpers write only `size` bytes of the value; high bytes are ignored.
        let constant_v = self.c(u64::from(constant));
        let pc = self.c(self.rip);
        self.b.ins().call_indirect(
            store_sig,
            store_fn,
            &[cookie, p, size, v, constant_v, pc],
        );
        self.check_native_fault();
    }

    fn bounds_check(&mut self, v: Value, w: u16) {
        let plain = self.b.ins().band_imm(v, POINTER_ADDRESS_MASK as i64);
        let bad = self.b.ins().icmp_imm(
            IntCC::UnsignedGreaterThan,
            plain,
            (ADDRESS_LIMIT - u64::from(w / 8)) as i64,
        );
        self.b.ins().trapnz(bad, TrapCode::HEAP_OUT_OF_BOUNDS);
    }

    /// After a checked helper call, leave the block if the fault slot is set.
    fn check_native_fault(&mut self) {
        let kind = self.b.ins().load(
            types::I64,
            MemFlags::new(),
            self.ctx,
            super::native::NATIVE_FAULT_KIND_OFFSET,
        );
        let fault = self.b.ins().icmp_imm(IntCC::NotEqual, kind, 0);
        let leave = self.mem_fault_block();
        let resume = self.b.create_block();
        self.b.ins().brif(fault, leave, &[], resume, &[]);
        self.b.switch_to_block(resume);
        self.b.seal_block(resume);
    }

    fn mem_fault_block(&mut self) -> Block {
        *self.mem_fault.get_or_insert_with(|| self.b.create_block())
    }
    fn write(&mut self, i: &Instruction, n: u32, v: Value) -> Result<(), EngineError> {
        match i.op_kind(n) {
            OpKind::Register => self.put_reg(i.op_register(n), v, i),
            OpKind::Memory => {
                let w = self.width(i, n)?;
                let p = self.address(i)?;
                // An immediate (or lea of a code address) is the only source
                // with constant provenance for PAC signing.
                let constant = immediate(i, 1).is_some() || i.mnemonic() == Mnemonic::Lea;
                self.store(p, w, v, constant);
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
        let sub = matches!(mnemonic, Sub | Cmp | Dec | Neg | Sbb);
        let logical = matches!(mnemonic, Test | And | Or | Xor);
        let preserve_carry = matches!(mnemonic, Inc | Dec);
        let mut f = self.status(result, w);
        if !logical {
            let c = if sub {
                if mnemonic == Sbb {
                    let ult = self.b.ins().icmp(IntCC::UnsignedLessThan, a, b);
                    let ule = self.b.ins().icmp(IntCC::UnsignedLessThanOrEqual, a, b);
                    let old_cf = self.flag(0);
                    let old_cf_bool = self.b.ins().icmp_imm(IntCC::NotEqual, old_cf, 0);
                    let borrow = self.b.ins().select(old_cf_bool, ule, ult);
                    self.bool64(borrow)
                } else {
                    let c = self.b.ins().icmp(IntCC::UnsignedLessThan, a, b);
                    self.bool64(c)
                }
            } else if mnemonic == Adc {
                let ult = self.b.ins().icmp(IntCC::UnsignedLessThan, result, a);
                let ule = self.b.ins().icmp(IntCC::UnsignedLessThanOrEqual, result, a);
                let old_cf = self.flag(0);
                let old_cf_bool = self.b.ins().icmp_imm(IntCC::NotEqual, old_cf, 0);
                let carry = self.b.ins().select(old_cf_bool, ule, ult);
                self.bool64(carry)
            } else {
                let c = self.b.ins().icmp(IntCC::UnsignedLessThan, result, a);
                self.bool64(c)
            };
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
        // Return addresses on the stack are not PAC-covered; constant is irrelevant.
        self.store(sp, 64, v, true);
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

    // Compute which Cranelift variable holds guest RSP after diversification.
    // div.variable(7) is the variable index for guest register 7 (RSP).
    let rsp_var = if backend == Backend::X86_64 {
        div.variable(7)
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

    // Checked memory helper signatures for x86_64 (tags + PAC on every access).
    let (load_sig, store_sig) = if backend == Backend::X86_64 {
        let mut load_sig = Signature::new(CallConv::SystemV);
        for _ in 0..4 {
            load_sig.params.push(AbiParam::new(types::I64));
        }
        load_sig.returns.push(AbiParam::new(types::I64));
        let load_sig = b.import_signature(load_sig);
        let mut store_sig = Signature::new(CallConv::SystemV);
        for _ in 0..6 {
            store_sig.params.push(AbiParam::new(types::I64));
        }
        let store_sig = b.import_signature(store_sig);
        (Some(load_sig), Some(store_sig))
    } else {
        (None, None)
    };

    // Declare variables 0..50 (0..15 GPR, 16 RIP, 17 RFLAGS, 18 FS_BASE, 19..34 XMM lo, 35..50 XMM hi).
    for j in 0..51 {
        let var = Variable::from_u32(j);
        b.declare_var(var, types::I64);
    }

    // Load context: guest register i from ctx[i*8] -> variable permute[i] (or i for RIP/RFLAGS/FS_BASE)
    for i in 0..16 {
        let var_idx = div.variable(i);
        let var = Variable::from_u32(var_idx);
        let v = b
            .ins()
            .load(types::I64, MemFlags::new(), ctx, (i * 8) as i32);
        b.def_var(var, v);
    }
    // RIP (16), RFLAGS (17), and FS_BASE (18) are not diversified.
    for i in 16..19 {
        let var = Variable::from_u32(i);
        let v = b
            .ins()
            .load(types::I64, MemFlags::new(), ctx, (i * 8) as i32);
        b.def_var(var, v);
    }
    let zero = b.ins().iconst(types::I64, 0);
    for i in 19..51 {
        b.def_var(Variable::from_u32(i), zero);
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
        mem_fault: None,
        rip: 0,
        backend,
        div,
        cfi,
        rsp_var,
        ctx,
        cfi_check_fn,
        cfi_check_sig,
        load_sig,
        store_sig,
    };

    for i in trace {
        use Mnemonic::*;
        l.rip = i.ip();
        let next = l.c(i.next_ip());
        l.set(16, next);
        if matches!(i.mnemonic(), Nop | Endbr64 | Endbr32) {
            continue;
        }
        if i.has_rep_prefix() || i.has_repne_prefix() {
            if !matches!(
                i.mnemonic(),
                Stosb | Stosw | Stosd | Stosq | Movsb | Movsw | Movsd | Movsq | Ret
            ) {
                return Err(fail(i, "atomic and string prefixes unsupported"));
            }
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
            Adc | Sbb => {
                let w = l.width(i, 0)?;
                let a = l.read(i, 0)?;
                let b = l.read(i, 1)?;
                let b = l.mask(b, w);
                let cf = l.flag(0);
                let v = if i.mnemonic() == Sbb {
                    let sub = l.b.ins().isub(a, b);
                    l.b.ins().isub(sub, cf)
                } else {
                    let add = l.b.ins().iadd(a, b);
                    l.b.ins().iadd(add, cf)
                };
                let v = l.mask(v, w);
                l.flags(a, b, v, w, i.mnemonic());
                l.write(i, 0, v)?;
            }
            Bswap => {
                let w = l.width(i, 0)?;
                let a = l.read(i, 0)?;
                let v = if w == 32 {
                    let a32 = l.b.ins().ireduce(types::I32, a);
                    let rev = l.b.ins().bswap(a32);
                    l.b.ins().uextend(types::I64, rev)
                } else if w == 64 {
                    l.b.ins().bswap(a)
                } else {
                    return Err(fail(i, "unsupported bswap width"));
                };
                l.write(i, 0, v)?;
            }
            Bt | Bts => {
                let w = l.width(i, 0)?;
                let a = l.read(i, 0)?;
                let bit_idx_raw = l.read(i, 1)?;
                let mask = (w - 1) as i64;
                let bit_idx = l.b.ins().band_imm(bit_idx_raw, mask);
                let shifted = l.b.ins().ushr(a, bit_idx);
                let cf = l.b.ins().band_imm(shifted, 1);
                let old = l.var(17);
                let old = l.b.ins().band_imm(old, !1);
                let new_flags = l.b.ins().bor(old, cf);
                l.set(17, new_flags);
                if i.mnemonic() == Bts {
                    let one = l.c(1);
                    let mask_bit = l.b.ins().ishl(one, bit_idx);
                    let v = l.b.ins().bor(a, mask_bit);
                    let v = l.mask(v, w);
                    l.write(i, 0, v)?;
                }
            }
            Bsf | Bsr | Tzcnt => {
                let w = l.width(i, 0)?;
                let a = l.read(i, 0)?;
                let is_zero = l.b.ins().icmp_imm(IntCC::Equal, a, 0);
                let v = match i.mnemonic() {
                    Bsf => {
                        let tz = l.b.ins().ctz(a);
                        l.mask(tz, w)
                    }
                    Tzcnt => {
                        let tz = l.b.ins().ctz(a);
                        let w_val = l.c(w as u64);
                        l.b.ins().select(is_zero, w_val, tz)
                    }
                    Bsr => {
                        let clz = l.b.ins().clz(a);
                        let limit = l.c((64 - 1) as u64);
                        let top = l.b.ins().isub(limit, clz);
                        l.mask(top, w)
                    }
                    _ => unreachable!(),
                };
                let z = l.bool64(is_zero);
                let old = l.var(17);
                let old = l.b.ins().band_imm(old, !0x41);
                let z_shift = l.b.ins().ishl_imm(z, 6);
                let mut new_flags = l.b.ins().bor(old, z_shift);
                if i.mnemonic() == Tzcnt {
                    new_flags = l.b.ins().bor(new_flags, z);
                }
                l.set(17, new_flags);
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
            Mul => {
                let w = l.width(i, 0)?;
                let b = l.read(i, 0)?;
                let rax_var = l.div.variable(0) as usize;
                let rdx_var = l.div.variable(3) as usize;
                let rax_val = l.var(rax_var);
                let a = l.mask(rax_val, w);
                let b = l.mask(b, w);
                if w == 64 {
                    let lo = l.b.ins().imul(a, b);
                    let hi = l.b.ins().umulhi(a, b);
                    l.set(rax_var, lo);
                    l.set(rdx_var, hi);
                    let overflow = l.b.ins().icmp_imm(IntCC::NotEqual, hi, 0);
                    let c = l.bool64(overflow);
                    let o = l.b.ins().ishl_imm(c, 11);
                    let f = l.b.ins().bor(c, o);
                    let old = l.var(17);
                    let old = l.b.ins().band_imm(old, !0x801);
                    let f = l.b.ins().bor(old, f);
                    l.set(17, f);
                } else if w == 32 {
                    let prod = l.b.ins().imul(a, b);
                    let lo = l.mask(prod, 32);
                    let hi = l.b.ins().ushr_imm(prod, 32);
                    let hi = l.mask(hi, 32);
                    l.set(rax_var, lo);
                    l.set(rdx_var, hi);
                    let overflow = l.b.ins().icmp_imm(IntCC::NotEqual, hi, 0);
                    let c = l.bool64(overflow);
                    let o = l.b.ins().ishl_imm(c, 11);
                    let f = l.b.ins().bor(c, o);
                    let old = l.var(17);
                    let old = l.b.ins().band_imm(old, !0x801);
                    let f = l.b.ins().bor(old, f);
                    l.set(17, f);
                } else if w == 16 {
                    let prod = l.b.ins().imul(a, b);
                    let lo = l.mask(prod, 16);
                    let hi = l.b.ins().ushr_imm(prod, 16);
                    let hi = l.mask(hi, 16);
                    let old_rax = l.var(rax_var);
                    let old_rdx = l.var(rdx_var);
                    let kept_rax = l.b.ins().band_imm(old_rax, !0xffff);
                    let kept_rdx = l.b.ins().band_imm(old_rdx, !0xffff);
                    let new_rax = l.b.ins().bor(kept_rax, lo);
                    let new_rdx = l.b.ins().bor(kept_rdx, hi);
                    l.set(rax_var, new_rax);
                    l.set(rdx_var, new_rdx);
                    let overflow = l.b.ins().icmp_imm(IntCC::NotEqual, hi, 0);
                    let c = l.bool64(overflow);
                    let o = l.b.ins().ishl_imm(c, 11);
                    let f = l.b.ins().bor(c, o);
                    let old = l.var(17);
                    let old = l.b.ins().band_imm(old, !0x801);
                    let f = l.b.ins().bor(old, f);
                    l.set(17, f);
                } else if w == 8 {
                    let prod = l.b.ins().imul(a, b);
                    let lo = l.mask(prod, 16);
                    let old_rax = l.var(rax_var);
                    let kept_rax = l.b.ins().band_imm(old_rax, !0xffff);
                    let new_rax = l.b.ins().bor(kept_rax, lo);
                    l.set(rax_var, new_rax);
                    let hi = l.b.ins().ushr_imm(prod, 8);
                    let overflow = l.b.ins().icmp_imm(IntCC::NotEqual, hi, 0);
                    let c = l.bool64(overflow);
                    let o = l.b.ins().ishl_imm(c, 11);
                    let f = l.b.ins().bor(c, o);
                    let old = l.var(17);
                    let old = l.b.ins().band_imm(old, !0x801);
                    let f = l.b.ins().bor(old, f);
                    l.set(17, f);
                } else {
                    return Err(fail(i, "unsupported operand width for mul"));
                }
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
            Shl | Shr | Sar => {
                let w = l.width(i, 0)?;
                let a = l.read(i, 0)?;
                let count_raw = l.read(i, 1)?;
                let mask = if w == 64 { 63 } else { 31 };
                let count = l.b.ins().band_imm(count_raw, mask);
                let is_zero = l.b.ins().icmp_imm(IntCC::Equal, count, 0);

                let res = match i.mnemonic() {
                    Shl => l.b.ins().ishl(a, count),
                    Shr => l.b.ins().ushr(a, count),
                    Sar => {
                        let signed = l.sign(a, w);
                        l.b.ins().sshr(signed, count)
                    }
                    _ => unreachable!(),
                };
                let res = l.mask(res, w);

                let shift_one = l.b.ins().iadd_imm(count, -1);
                let cf = match i.mnemonic() {
                    Shl => {
                        let w_minus_one = l.c((w - 1) as u64);
                        let shift = l.b.ins().isub(w_minus_one, shift_one);
                        let bit = l.b.ins().ushr(a, shift);
                        l.b.ins().band_imm(bit, 1)
                    }
                    Shr => {
                        let bit = l.b.ins().ushr(a, shift_one);
                        l.b.ins().band_imm(bit, 1)
                    }
                    Sar => {
                        let signed = l.sign(a, w);
                        let bit = l.b.ins().sshr(signed, shift_one);
                        l.b.ins().band_imm(bit, 1)
                    }
                    _ => unreachable!(),
                };

                let status = l.status(res, w);
                let old_flags = l.var(17);
                let new_flags = l.b.ins().band_imm(old_flags, !0x8c5);
                let new_flags = l.b.ins().bor(new_flags, cf);
                let new_flags = l.b.ins().bor(new_flags, status);

                let final_res = l.b.ins().select(is_zero, a, res);
                let final_flags = l.b.ins().select(is_zero, old_flags, new_flags);
                l.set(17, final_flags);
                l.write(i, 0, final_res)?;
            }
            Cld | Pause => {
                if i.mnemonic() == Cld {
                    let old = l.var(17);
                    let cleared = l.b.ins().band_imm(old, !(1 << 10));
                    l.set(17, cleared);
                }
            }
            Cdqe => {
                let rax_var = l.div.variable(0) as usize;
                let rax = l.var(rax_var);
                let eax = l.mask(rax, 32);
                let extended = l.sign(eax, 32);
                l.set(rax_var, extended);
            }
            Cqo => {
                let rax_var = l.div.variable(0) as usize;
                let rdx_var = l.div.variable(3) as usize;
                let rax = l.var(rax_var);
                let sign = l.b.ins().sshr_imm(rax, 63);
                l.set(rdx_var, sign);
            }
            Syscall => {
                // Guest RCX gets the return address and R11 gets RFLAGS, as on
                // Linux x86-64. Use the diversified variable indices.
                let rcx = l.div.variable(2) as usize;
                l.set(rcx, next);
                let f = l.var(17);
                let r11 = l.div.variable(11) as usize;
                l.set(r11, f);
            }
            Stosb | Stosw | Stosd | Stosq => {
                let w: u16 = match i.mnemonic() {
                    Stosb => 8,
                    Stosw => 16,
                    Stosd => 32,
                    Stosq => 64,
                    _ => unreachable!(),
                };
                let step = (w / 8) as i64;
                let rdi_var = l.div.variable(5) as usize;
                let rax_var = l.div.variable(0) as usize;
                let rax_val = l.var(rax_var);
                let val = l.mask(rax_val, w);

                if i.has_rep_prefix() {
                    let rcx_var = l.div.variable(2) as usize;
                    let header = l.b.create_block();
                    let body = l.b.create_block();
                    let exit = l.b.create_block();

                    l.b.ins().jump(header, &[]);
                    l.b.switch_to_block(header);
                    let count = l.var(rcx_var);
                    let zero = l.c(0);
                    let is_zero = l.b.ins().icmp(IntCC::Equal, count, zero);
                    l.b.ins().brif(is_zero, exit, &[], body, &[]);

                    l.b.switch_to_block(body);
                    l.b.seal_block(body);
                    let rdi = l.var(rdi_var);
                    l.store(rdi, w, val, false);
                    let new_rdi = l.b.ins().iadd_imm(rdi, step);
                    l.set(rdi_var, new_rdi);
                    let new_count = l.b.ins().iadd_imm(count, -1);
                    l.set(rcx_var, new_count);
                    l.b.ins().jump(header, &[]);

                    l.b.seal_block(header);
                    l.b.switch_to_block(exit);
                    l.b.seal_block(exit);
                } else {
                    let rdi = l.var(rdi_var);
                    l.store(rdi, w, val, false);
                    let new_rdi = l.b.ins().iadd_imm(rdi, step);
                    l.set(rdi_var, new_rdi);
                }
            }
            Movsb | Movsw | Movsd | Movsq => {
                let w: u16 = match i.mnemonic() {
                    Movsb => 8,
                    Movsw => 16,
                    Movsd => 32,
                    Movsq => 64,
                    _ => unreachable!(),
                };
                let step = (w / 8) as i64;
                let rdi_var = l.div.variable(5) as usize;
                let rsi_var = l.div.variable(4) as usize;

                if i.has_rep_prefix() {
                    let rcx_var = l.div.variable(2) as usize;
                    let header = l.b.create_block();
                    let body = l.b.create_block();
                    let exit = l.b.create_block();

                    l.b.ins().jump(header, &[]);
                    l.b.switch_to_block(header);
                    let count = l.var(rcx_var);
                    let zero = l.c(0);
                    let is_zero = l.b.ins().icmp(IntCC::Equal, count, zero);
                    l.b.ins().brif(is_zero, exit, &[], body, &[]);

                    l.b.switch_to_block(body);
                    l.b.seal_block(body);
                    let rsi = l.var(rsi_var);
                    let val = l.load(rsi, w)?;
                    let rdi = l.var(rdi_var);
                    l.store(rdi, w, val, false);
                    let new_rsi = l.b.ins().iadd_imm(rsi, step);
                    l.set(rsi_var, new_rsi);
                    let new_rdi = l.b.ins().iadd_imm(rdi, step);
                    l.set(rdi_var, new_rdi);
                    let new_count = l.b.ins().iadd_imm(count, -1);
                    l.set(rcx_var, new_count);
                    l.b.ins().jump(header, &[]);

                    l.b.seal_block(header);
                    l.b.switch_to_block(exit);
                    l.b.seal_block(exit);
                } else {
                    let rsi = l.var(rsi_var);
                    let val = l.load(rsi, w)?;
                    let rdi = l.var(rdi_var);
                    l.store(rdi, w, val, false);
                    let new_rsi = l.b.ins().iadd_imm(rsi, step);
                    l.set(rsi_var, new_rsi);
                    let new_rdi = l.b.ins().iadd_imm(rdi, step);
                    l.set(rdi_var, new_rdi);
                }
            }
            Cmpxchg => {
                let w = l.width(i, 0)?;
                let dst = l.read(i, 0)?;
                let src = l.read(i, 1)?;
                let rax_var = l.div.variable(0) as usize;
                let rax_val = l.var(rax_var);
                let rax = l.mask(rax_val, w);
                let eq = l.b.ins().icmp(IntCC::Equal, rax, dst);
                let diff = l.b.ins().isub(rax, dst);
                l.flags(rax, dst, diff, w, Cmp);
                let then_block = l.b.create_block();
                let else_block = l.b.create_block();
                let merge_block = l.b.create_block();
                l.b.ins().brif(eq, then_block, &[], else_block, &[]);

                l.b.switch_to_block(then_block);
                l.b.seal_block(then_block);
                l.write(i, 0, src)?;
                l.b.ins().jump(merge_block, &[]);

                l.b.switch_to_block(else_block);
                l.b.seal_block(else_block);
                let old_rax = l.var(rax_var);
                let new_rax = if w == 64 {
                    dst
                } else {
                    let masked = l.mask(dst, w);
                    if w == 32 {
                        masked
                    } else {
                        let kept = l.b.ins().band_imm(old_rax, !((1i64 << w) - 1));
                        l.b.ins().bor(kept, masked)
                    }
                };
                l.set(rax_var, new_rax);
                l.b.ins().jump(merge_block, &[]);

                l.b.switch_to_block(merge_block);
                l.b.seal_block(merge_block);
            }
            Xadd => {
                let w = l.width(i, 0)?;
                let dst = l.read(i, 0)?;
                let src = l.read(i, 1)?;
                let sum = l.b.ins().iadd(dst, src);
                l.flags(dst, src, sum, w, Add);
                l.write(i, 1, dst)?;
                l.write(i, 0, sum)?;
            }
            Xchg => {
                let v0 = l.read(i, 0)?;
                let v1 = l.read(i, 1)?;
                l.write(i, 0, v1)?;
                l.write(i, 1, v0)?;
            }
            Pxor | Xorps => {
                let op0 = xmm_index(i.op0_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                if i.op1_kind() == OpKind::Register {
                    let op1 = xmm_index(i.op1_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                    if op0 == op1 {
                        let zero = l.c(0);
                        l.set_xmm(op0, zero, zero);
                    } else {
                        let (lo0, hi0) = l.get_xmm(op0);
                        let (lo1, hi1) = l.get_xmm(op1);
                        let new_lo = l.b.ins().bxor(lo0, lo1);
                        let new_hi = l.b.ins().bxor(hi0, hi1);
                        l.set_xmm(op0, new_lo, new_hi);
                    }
                } else if i.op1_kind() == OpKind::Memory {
                    let p = l.address(i)?;
                    let mem_lo = l.load(p, 64)?;
                    let p8 = l.b.ins().iadd_imm(p, 8);
                    let mem_hi = l.load(p8, 64)?;
                    let (lo0, hi0) = l.get_xmm(op0);
                    let new_lo = l.b.ins().bxor(lo0, mem_lo);
                    let new_hi = l.b.ins().bxor(hi0, mem_hi);
                    l.set_xmm(op0, new_lo, new_hi);
                } else {
                    return Err(fail(i, "unsupported pxor operand"));
                }
            }
            Movaps | Movups | Movdqa | Movdqu => {
                if i.op0_kind() == OpKind::Register && i.op1_kind() == OpKind::Register {
                    let dst = xmm_index(i.op0_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                    let src = xmm_index(i.op1_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                    let (lo, hi) = l.get_xmm(src);
                    l.set_xmm(dst, lo, hi);
                } else if i.op0_kind() == OpKind::Register && i.op1_kind() == OpKind::Memory {
                    let dst = xmm_index(i.op0_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                    let p = l.address(i)?;
                    let lo = l.load(p, 64)?;
                    let p8 = l.b.ins().iadd_imm(p, 8);
                    let hi = l.load(p8, 64)?;
                    l.set_xmm(dst, lo, hi);
                } else if i.op0_kind() == OpKind::Memory && i.op1_kind() == OpKind::Register {
                    let src = xmm_index(i.op1_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                    let (lo, hi) = l.get_xmm(src);
                    let p = l.address(i)?;
                    l.store(p, 64, lo, false);
                    let p8 = l.b.ins().iadd_imm(p, 8);
                    l.store(p8, 64, hi, false);
                } else {
                    return Err(fail(i, "unsupported movaps/movups operand combination"));
                }
            }
            Movd | Movq => {
                let w = if i.mnemonic() == Movd { 32 } else { 64 };
                if i.op0_kind() == OpKind::Register && xmm_index(i.op0_register()).is_some() {
                    let dst = xmm_index(i.op0_register()).unwrap();
                    let zero = l.c(0);
                    let val = if i.op1_kind() == OpKind::Register {
                        let v = l.reg(i.op1_register(), i)?;
                        l.mask(v, w)
                    } else if i.op1_kind() == OpKind::Memory {
                        let p = l.address(i)?;
                        l.load(p, w)?
                    } else {
                        return Err(fail(i, "unsupported operand for movd/movq to xmm"));
                    };
                    l.set_xmm(dst, val, zero);
                } else if i.op1_kind() == OpKind::Register && xmm_index(i.op1_register()).is_some() {
                    let src = xmm_index(i.op1_register()).unwrap();
                    let (lo, _) = l.get_xmm(src);
                    let val = l.mask(lo, w);
                    if i.op0_kind() == OpKind::Register {
                        l.put_reg(i.op0_register(), val, i)?;
                    } else if i.op0_kind() == OpKind::Memory {
                        let p = l.address(i)?;
                        l.store(p, w, val, false);
                    } else {
                        return Err(fail(i, "unsupported operand for movd/movq from xmm"));
                    }
                } else {
                    let v = l.read(i, 1)?;
                    l.write(i, 0, v)?;
                }
            }
            Pcmpeqb | Pcmpeqd => {
                let dst = xmm_index(i.op0_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                let (dst_lo, dst_hi) = l.get_xmm(dst);
                let (src_lo, src_hi) = l.read_xmm(i, 1)?;
                if i.mnemonic() == Pcmpeqb {
                    let res_lo = l.pcmpeqb_u64(dst_lo, src_lo);
                    let res_hi = l.pcmpeqb_u64(dst_hi, src_hi);
                    l.set_xmm(dst, res_lo, res_hi);
                } else {
                    let res_lo = l.pcmpeqd_u64(dst_lo, src_lo);
                    let res_hi = l.pcmpeqd_u64(dst_hi, src_hi);
                    l.set_xmm(dst, res_lo, res_hi);
                }
            }
            Pmovmskb => {
                let src = xmm_index(i.op1_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                let (lo, hi) = l.get_xmm(src);
                let mut mask = l.c(0);
                for k in 0..8 {
                    let bit = l.b.ins().ushr_imm(lo, (k * 8 + 7) as i64);
                    let bit = l.b.ins().band_imm(bit, 1);
                    let shifted = l.b.ins().ishl_imm(bit, k as i64);
                    mask = l.b.ins().bor(mask, shifted);
                }
                for k in 0..8 {
                    let bit = l.b.ins().ushr_imm(hi, (k * 8 + 7) as i64);
                    let bit = l.b.ins().band_imm(bit, 1);
                    let shifted = l.b.ins().ishl_imm(bit, (8 + k) as i64);
                    mask = l.b.ins().bor(mask, shifted);
                }
                l.put_reg(i.op0_register(), mask, i)?;
            }
            Movmskps => {
                let src = xmm_index(i.op1_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                let (lo, hi) = l.get_xmm(src);
                let bit0 = l.b.ins().ushr_imm(lo, 31);
                let b0 = l.b.ins().band_imm(bit0, 1);
                let bit1 = l.b.ins().ushr_imm(lo, 63);
                let b1_raw = l.b.ins().band_imm(bit1, 1);
                let b1 = l.b.ins().ishl_imm(b1_raw, 1);
                let bit2 = l.b.ins().ushr_imm(hi, 31);
                let b2_raw = l.b.ins().band_imm(bit2, 1);
                let b2 = l.b.ins().ishl_imm(b2_raw, 2);
                let bit3 = l.b.ins().ushr_imm(hi, 63);
                let b3_raw = l.b.ins().band_imm(bit3, 1);
                let b3 = l.b.ins().ishl_imm(b3_raw, 3);
                let mut m = l.b.ins().bor(b0, b1);
                m = l.b.ins().bor(m, b2);
                m = l.b.ins().bor(m, b3);
                l.put_reg(i.op0_register(), m, i)?;
            }
            Pand | Pandn | Por => {
                let dst = xmm_index(i.op0_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                let (lo0, hi0) = l.get_xmm(dst);
                let (lo1, hi1) = l.read_xmm(i, 1)?;
                let (new_lo, new_hi) = match i.mnemonic() {
                    Pand => (l.b.ins().band(lo0, lo1), l.b.ins().band(hi0, hi1)),
                    Pandn => {
                        let not0 = l.b.ins().bnot(lo0);
                        let not1 = l.b.ins().bnot(hi0);
                        (l.b.ins().band(not0, lo1), l.b.ins().band(not1, hi1))
                    }
                    Por => (l.b.ins().bor(lo0, lo1), l.b.ins().bor(hi0, hi1)),
                    _ => unreachable!(),
                };
                l.set_xmm(dst, new_lo, new_hi);
            }
            Paddb | Paddw | Paddd | Paddq | Psubb | Psubw | Psubd | Psubq | Pmaxub | Pminub => {
                let dst = xmm_index(i.op0_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                let (dst_lo, dst_hi) = l.get_xmm(dst);
                let (src_lo, src_hi) = l.read_xmm(i, 1)?;
                let (res_lo, res_hi) = match i.mnemonic() {
                    Paddb => (l.paddb_u64(dst_lo, src_lo), l.paddb_u64(dst_hi, src_hi)),
                    Paddw => (l.paddw_u64(dst_lo, src_lo), l.paddw_u64(dst_hi, src_hi)),
                    Paddd => (l.paddd_u64(dst_lo, src_lo), l.paddd_u64(dst_hi, src_hi)),
                    Paddq => (l.b.ins().iadd(dst_lo, src_lo), l.b.ins().iadd(dst_hi, src_hi)),
                    Psubb => (l.psubb_u64(dst_lo, src_lo), l.psubb_u64(dst_hi, src_hi)),
                    Psubw => (l.psubw_u64(dst_lo, src_lo), l.psubw_u64(dst_hi, src_hi)),
                    Psubd => (l.psubd_u64(dst_lo, src_lo), l.psubd_u64(dst_hi, src_hi)),
                    Psubq => (l.b.ins().isub(dst_lo, src_lo), l.b.ins().isub(dst_hi, src_hi)),
                    Pmaxub => (l.pmaxub_u64(dst_lo, src_lo), l.pmaxub_u64(dst_hi, src_hi)),
                    Pminub => (l.pminub_u64(dst_lo, src_lo), l.pminub_u64(dst_hi, src_hi)),
                    _ => unreachable!(),
                };
                l.set_xmm(dst, res_lo, res_hi);
            }
            Pshufd => {
                let dst = xmm_index(i.op0_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                let (src_lo, src_hi) = l.read_xmm(i, 1)?;
                let imm = i.immediate8() as u64;
                let d0 = l.b.ins().band_imm(src_lo, 0xffffffff);
                let d1 = l.b.ins().ushr_imm(src_lo, 32);
                let d2 = l.b.ins().band_imm(src_hi, 0xffffffff);
                let d3 = l.b.ins().ushr_imm(src_hi, 32);
                let dwords = [d0, d1, d2, d3];
                let sel0 = (imm & 3) as usize;
                let sel1 = ((imm >> 2) & 3) as usize;
                let sel2 = ((imm >> 4) & 3) as usize;
                let sel3 = ((imm >> 6) & 3) as usize;
                let shift_hi1 = l.b.ins().ishl_imm(dwords[sel1], 32);
                let new_lo = l.b.ins().bor(dwords[sel0], shift_hi1);
                let shift_hi3 = l.b.ins().ishl_imm(dwords[sel3], 32);
                let new_hi = l.b.ins().bor(dwords[sel2], shift_hi3);
                l.set_xmm(dst, new_lo, new_hi);
            }
            Punpcklbw => {
                let dst = xmm_index(i.op0_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                let (dst_lo, _) = l.get_xmm(dst);
                let (src_lo, _) = l.read_xmm(i, 1)?;
                let mut new_lo = l.c(0);
                for k in 0..4 {
                    let d = l.b.ins().ushr_imm(dst_lo, (k * 8) as i64);
                    let d = l.b.ins().band_imm(d, 0xff);
                    let s = l.b.ins().ushr_imm(src_lo, (k * 8) as i64);
                    let s = l.b.ins().band_imm(s, 0xff);
                    let d_shift = l.b.ins().ishl_imm(d, (k * 16) as i64);
                    let s_shift = l.b.ins().ishl_imm(s, (k * 16 + 8) as i64);
                    new_lo = l.b.ins().bor(new_lo, d_shift);
                    new_lo = l.b.ins().bor(new_lo, s_shift);
                }
                let mut new_hi = l.c(0);
                for k in 0..4 {
                    let d = l.b.ins().ushr_imm(dst_lo, ((4 + k) * 8) as i64);
                    let d = l.b.ins().band_imm(d, 0xff);
                    let s = l.b.ins().ushr_imm(src_lo, ((4 + k) * 8) as i64);
                    let s = l.b.ins().band_imm(s, 0xff);
                    let d_shift = l.b.ins().ishl_imm(d, (k * 16) as i64);
                    let s_shift = l.b.ins().ishl_imm(s, (k * 16 + 8) as i64);
                    new_hi = l.b.ins().bor(new_hi, d_shift);
                    new_hi = l.b.ins().bor(new_hi, s_shift);
                }
                l.set_xmm(dst, new_lo, new_hi);
            }
            Punpckhbw => {
                let dst = xmm_index(i.op0_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                let (_, dst_hi) = l.get_xmm(dst);
                let (_, src_hi) = l.read_xmm(i, 1)?;
                let mut new_lo = l.c(0);
                for k in 0..4 {
                    let d = l.b.ins().ushr_imm(dst_hi, (k * 8) as i64);
                    let d = l.b.ins().band_imm(d, 0xff);
                    let s = l.b.ins().ushr_imm(src_hi, (k * 8) as i64);
                    let s = l.b.ins().band_imm(s, 0xff);
                    let d_shift = l.b.ins().ishl_imm(d, (k * 16) as i64);
                    let s_shift = l.b.ins().ishl_imm(s, (k * 16 + 8) as i64);
                    new_lo = l.b.ins().bor(new_lo, d_shift);
                    new_lo = l.b.ins().bor(new_lo, s_shift);
                }
                let mut new_hi = l.c(0);
                for k in 0..4 {
                    let d = l.b.ins().ushr_imm(dst_hi, ((4 + k) * 8) as i64);
                    let d = l.b.ins().band_imm(d, 0xff);
                    let s = l.b.ins().ushr_imm(src_hi, ((4 + k) * 8) as i64);
                    let s = l.b.ins().band_imm(s, 0xff);
                    let d_shift = l.b.ins().ishl_imm(d, (k * 16) as i64);
                    let s_shift = l.b.ins().ishl_imm(s, (k * 16 + 8) as i64);
                    new_hi = l.b.ins().bor(new_hi, d_shift);
                    new_hi = l.b.ins().bor(new_hi, s_shift);
                }
                l.set_xmm(dst, new_lo, new_hi);
            }
            Punpcklwd => {
                let dst = xmm_index(i.op0_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                let (dst_lo, _) = l.get_xmm(dst);
                let (src_lo, _) = l.read_xmm(i, 1)?;
                let mut new_lo = l.c(0);
                for k in 0..2 {
                    let d = l.b.ins().ushr_imm(dst_lo, (k * 16) as i64);
                    let d = l.b.ins().band_imm(d, 0xffff);
                    let s = l.b.ins().ushr_imm(src_lo, (k * 16) as i64);
                    let s = l.b.ins().band_imm(s, 0xffff);
                    let d_shift = l.b.ins().ishl_imm(d, (k * 32) as i64);
                    let s_shift = l.b.ins().ishl_imm(s, (k * 32 + 16) as i64);
                    new_lo = l.b.ins().bor(new_lo, d_shift);
                    new_lo = l.b.ins().bor(new_lo, s_shift);
                }
                let mut new_hi = l.c(0);
                for k in 0..2 {
                    let d = l.b.ins().ushr_imm(dst_lo, ((2 + k) * 16) as i64);
                    let d = l.b.ins().band_imm(d, 0xffff);
                    let s = l.b.ins().ushr_imm(src_lo, ((2 + k) * 16) as i64);
                    let s = l.b.ins().band_imm(s, 0xffff);
                    let d_shift = l.b.ins().ishl_imm(d, (k * 32) as i64);
                    let s_shift = l.b.ins().ishl_imm(s, (k * 32 + 16) as i64);
                    new_hi = l.b.ins().bor(new_hi, d_shift);
                    new_hi = l.b.ins().bor(new_hi, s_shift);
                }
                l.set_xmm(dst, new_lo, new_hi);
            }
            Punpckhwd => {
                let dst = xmm_index(i.op0_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                let (_, dst_hi) = l.get_xmm(dst);
                let (_, src_hi) = l.read_xmm(i, 1)?;
                let mut new_lo = l.c(0);
                for k in 0..2 {
                    let d = l.b.ins().ushr_imm(dst_hi, (k * 16) as i64);
                    let d = l.b.ins().band_imm(d, 0xffff);
                    let s = l.b.ins().ushr_imm(src_hi, (k * 16) as i64);
                    let s = l.b.ins().band_imm(s, 0xffff);
                    let d_shift = l.b.ins().ishl_imm(d, (k * 32) as i64);
                    let s_shift = l.b.ins().ishl_imm(s, (k * 32 + 16) as i64);
                    new_lo = l.b.ins().bor(new_lo, d_shift);
                    new_lo = l.b.ins().bor(new_lo, s_shift);
                }
                let mut new_hi = l.c(0);
                for k in 0..2 {
                    let d = l.b.ins().ushr_imm(dst_hi, ((2 + k) * 16) as i64);
                    let d = l.b.ins().band_imm(d, 0xffff);
                    let s = l.b.ins().ushr_imm(src_hi, ((2 + k) * 16) as i64);
                    let s = l.b.ins().band_imm(s, 0xffff);
                    let d_shift = l.b.ins().ishl_imm(d, (k * 32) as i64);
                    let s_shift = l.b.ins().ishl_imm(s, (k * 32 + 16) as i64);
                    new_hi = l.b.ins().bor(new_hi, d_shift);
                    new_hi = l.b.ins().bor(new_hi, s_shift);
                }
                l.set_xmm(dst, new_lo, new_hi);
            }
            Punpckldq => {
                let dst = xmm_index(i.op0_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                let (dst_lo, _) = l.get_xmm(dst);
                let (src_lo, _) = l.read_xmm(i, 1)?;
                let d0 = l.b.ins().band_imm(dst_lo, 0xffffffff);
                let s0 = l.b.ins().band_imm(src_lo, 0xffffffff);
                let s0_shift = l.b.ins().ishl_imm(s0, 32);
                let new_lo = l.b.ins().bor(d0, s0_shift);

                let d1 = l.b.ins().ushr_imm(dst_lo, 32);
                let s1 = l.b.ins().ushr_imm(src_lo, 32);
                let s1_shift = l.b.ins().ishl_imm(s1, 32);
                let new_hi = l.b.ins().bor(d1, s1_shift);
                l.set_xmm(dst, new_lo, new_hi);
            }
            Punpckhdq => {
                let dst = xmm_index(i.op0_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                let (_, dst_hi) = l.get_xmm(dst);
                let (_, src_hi) = l.read_xmm(i, 1)?;
                let d2 = l.b.ins().band_imm(dst_hi, 0xffffffff);
                let s2 = l.b.ins().band_imm(src_hi, 0xffffffff);
                let s2_shift = l.b.ins().ishl_imm(s2, 32);
                let new_lo = l.b.ins().bor(d2, s2_shift);

                let d3 = l.b.ins().ushr_imm(dst_hi, 32);
                let s3 = l.b.ins().ushr_imm(src_hi, 32);
                let s3_shift = l.b.ins().ishl_imm(s3, 32);
                let new_hi = l.b.ins().bor(d3, s3_shift);
                l.set_xmm(dst, new_lo, new_hi);
            }
            Pshuflw => {
                let dst = xmm_index(i.op0_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                let (src_lo, src_hi) = l.read_xmm(i, 1)?;
                let imm = i.immediate8() as usize;
                let w0 = l.b.ins().band_imm(src_lo, 0xffff);
                let w1_raw = l.b.ins().ushr_imm(src_lo, 16);
                let w1 = l.b.ins().band_imm(w1_raw, 0xffff);
                let w2_raw = l.b.ins().ushr_imm(src_lo, 32);
                let w2 = l.b.ins().band_imm(w2_raw, 0xffff);
                let w3 = l.b.ins().ushr_imm(src_lo, 48);
                let words = [w0, w1, w2, w3];
                let s0 = imm & 3;
                let s1 = (imm >> 2) & 3;
                let s2 = (imm >> 4) & 3;
                let s3 = (imm >> 6) & 3;
                let o0 = words[s0];
                let o1 = l.b.ins().ishl_imm(words[s1], 16);
                let o2 = l.b.ins().ishl_imm(words[s2], 32);
                let o3 = l.b.ins().ishl_imm(words[s3], 48);
                let mut new_lo = l.b.ins().bor(o0, o1);
                new_lo = l.b.ins().bor(new_lo, o2);
                new_lo = l.b.ins().bor(new_lo, o3);
                l.set_xmm(dst, new_lo, src_hi);
            }
            Pshufhw => {
                let dst = xmm_index(i.op0_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                let (src_lo, src_hi) = l.read_xmm(i, 1)?;
                let imm = i.immediate8() as usize;
                let w0 = l.b.ins().band_imm(src_hi, 0xffff);
                let w1_raw = l.b.ins().ushr_imm(src_hi, 16);
                let w1 = l.b.ins().band_imm(w1_raw, 0xffff);
                let w2_raw = l.b.ins().ushr_imm(src_hi, 32);
                let w2 = l.b.ins().band_imm(w2_raw, 0xffff);
                let w3 = l.b.ins().ushr_imm(src_hi, 48);
                let words = [w0, w1, w2, w3];
                let s0 = imm & 3;
                let s1 = (imm >> 2) & 3;
                let s2 = (imm >> 4) & 3;
                let s3 = (imm >> 6) & 3;
                let o0 = words[s0];
                let o1 = l.b.ins().ishl_imm(words[s1], 16);
                let o2 = l.b.ins().ishl_imm(words[s2], 32);
                let o3 = l.b.ins().ishl_imm(words[s3], 48);
                let mut new_hi = l.b.ins().bor(o0, o1);
                new_hi = l.b.ins().bor(new_hi, o2);
                new_hi = l.b.ins().bor(new_hi, o3);
                l.set_xmm(dst, src_lo, new_hi);
            }
            Punpcklqdq => {
                let dst = xmm_index(i.op0_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                let (lo0, _) = l.get_xmm(dst);
                let (lo1, _) = l.read_xmm(i, 1)?;
                l.set_xmm(dst, lo0, lo1);
            }
            Punpckhqdq => {
                let dst = xmm_index(i.op0_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                let (_, hi0) = l.get_xmm(dst);
                let (_, hi1) = l.read_xmm(i, 1)?;
                l.set_xmm(dst, hi0, hi1);
            }
            Movlhps => {
                let dst = xmm_index(i.op0_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                let (lo0, _) = l.get_xmm(dst);
                let (lo1, _) = l.read_xmm(i, 1)?;
                l.set_xmm(dst, lo0, lo1);
            }
            Movhps => {
                if i.op0_kind() == OpKind::Register {
                    let dst = xmm_index(i.op0_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                    let (lo, _) = l.get_xmm(dst);
                    let p = l.address(i)?;
                    let hi = l.load(p, 64)?;
                    l.set_xmm(dst, lo, hi);
                } else if i.op0_kind() == OpKind::Memory {
                    let src = xmm_index(i.op1_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                    let (_, hi) = l.get_xmm(src);
                    let p = l.address(i)?;
                    l.store(p, 64, hi, false);
                }
            }
            Pinsrw | Pinsrb | Pinsrd | Pinsrq => {
                let dst = xmm_index(i.op0_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                let (lo, hi) = l.get_xmm(dst);
                match i.mnemonic() {
                    Pinsrw => {
                        let imm = (i.immediate8() & 7) as u32;
                        let val = if i.op1_kind() == OpKind::Register {
                            let r = l.reg(i.op1_register(), i)?;
                            l.b.ins().band_imm(r, 0xffff)
                        } else {
                            let p = l.address(i)?;
                            l.load(p, 16)?
                        };
                        if imm < 4 {
                            let shift = (imm * 16) as i64;
                            let mask = !(0xffffu64 << (imm * 16)) as i64;
                            let cleared = l.b.ins().band_imm(lo, mask);
                            let inserted = l.b.ins().ishl_imm(val, shift);
                            let new_lo = l.b.ins().bor(cleared, inserted);
                            l.set_xmm(dst, new_lo, hi);
                        } else {
                            let shift = ((imm - 4) * 16) as i64;
                            let mask = !(0xffffu64 << ((imm - 4) * 16)) as i64;
                            let cleared = l.b.ins().band_imm(hi, mask);
                            let inserted = l.b.ins().ishl_imm(val, shift);
                            let new_hi = l.b.ins().bor(cleared, inserted);
                            l.set_xmm(dst, lo, new_hi);
                        }
                    }
                    Pinsrb => {
                        let imm = (i.immediate8() & 15) as u32;
                        let val = if i.op1_kind() == OpKind::Register {
                            let r = l.reg(i.op1_register(), i)?;
                            l.b.ins().band_imm(r, 0xff)
                        } else {
                            let p = l.address(i)?;
                            l.load(p, 8)?
                        };
                        if imm < 8 {
                            let shift = (imm * 8) as i64;
                            let mask = !(0xffu64 << (imm * 8)) as i64;
                            let cleared = l.b.ins().band_imm(lo, mask);
                            let inserted = l.b.ins().ishl_imm(val, shift);
                            let new_lo = l.b.ins().bor(cleared, inserted);
                            l.set_xmm(dst, new_lo, hi);
                        } else {
                            let shift = ((imm - 8) * 8) as i64;
                            let mask = !(0xffu64 << ((imm - 8) * 8)) as i64;
                            let cleared = l.b.ins().band_imm(hi, mask);
                            let inserted = l.b.ins().ishl_imm(val, shift);
                            let new_hi = l.b.ins().bor(cleared, inserted);
                            l.set_xmm(dst, lo, new_hi);
                        }
                    }
                    Pinsrd => {
                        let imm = (i.immediate8() & 3) as u32;
                        let val = if i.op1_kind() == OpKind::Register {
                            let r = l.reg(i.op1_register(), i)?;
                            l.b.ins().band_imm(r, 0xffffffff)
                        } else {
                            let p = l.address(i)?;
                            l.load(p, 32)?
                        };
                        match imm {
                            0 => {
                                let cleared = l.b.ins().band_imm(lo, !(0xffffffffu64) as i64);
                                let new_lo = l.b.ins().bor(cleared, val);
                                l.set_xmm(dst, new_lo, hi);
                            }
                            1 => {
                                let cleared = l.b.ins().band_imm(lo, 0xffffffff);
                                let shifted = l.b.ins().ishl_imm(val, 32);
                                let new_lo = l.b.ins().bor(cleared, shifted);
                                l.set_xmm(dst, new_lo, hi);
                            }
                            2 => {
                                let cleared = l.b.ins().band_imm(hi, !(0xffffffffu64) as i64);
                                let new_hi = l.b.ins().bor(cleared, val);
                                l.set_xmm(dst, lo, new_hi);
                            }
                            _ => {
                                let cleared = l.b.ins().band_imm(hi, 0xffffffff);
                                let shifted = l.b.ins().ishl_imm(val, 32);
                                let new_hi = l.b.ins().bor(cleared, shifted);
                                l.set_xmm(dst, lo, new_hi);
                            }
                        }
                    }
                    Pinsrq => {
                        let imm = (i.immediate8() & 1) as u32;
                        let val = if i.op1_kind() == OpKind::Register {
                            l.reg(i.op1_register(), i)?
                        } else {
                            let p = l.address(i)?;
                            l.load(p, 64)?
                        };
                        if imm == 0 {
                            l.set_xmm(dst, val, hi);
                        } else {
                            l.set_xmm(dst, lo, val);
                        }
                    }
                    _ => unreachable!(),
                }
            }
            Pextrw | Pextrb | Pextrd | Pextrq => {
                let src = xmm_index(i.op1_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                let (lo, hi) = l.get_xmm(src);
                match i.mnemonic() {
                    Pextrw => {
                        let imm = (i.immediate8() & 7) as u32;
                        let word = if imm < 4 {
                            l.b.ins().ushr_imm(lo, (imm * 16) as i64)
                        } else {
                            l.b.ins().ushr_imm(hi, ((imm - 4) * 16) as i64)
                        };
                        let word = l.b.ins().band_imm(word, 0xffff);
                        if i.op0_kind() == OpKind::Register {
                            l.put_reg(i.op0_register(), word, i)?;
                        } else {
                            let p = l.address(i)?;
                            l.store(p, 16, word, false);
                        }
                    }
                    Pextrb => {
                        let imm = (i.immediate8() & 15) as u32;
                        let byte = if imm < 8 {
                            l.b.ins().ushr_imm(lo, (imm * 8) as i64)
                        } else {
                            l.b.ins().ushr_imm(hi, ((imm - 8) * 8) as i64)
                        };
                        let byte = l.b.ins().band_imm(byte, 0xff);
                        if i.op0_kind() == OpKind::Register {
                            l.put_reg(i.op0_register(), byte, i)?;
                        } else {
                            let p = l.address(i)?;
                            l.store(p, 8, byte, false);
                        }
                    }
                    Pextrd => {
                        let imm = (i.immediate8() & 3) as u32;
                        let dword = match imm {
                            0 => lo,
                            1 => l.b.ins().ushr_imm(lo, 32),
                            2 => hi,
                            _ => l.b.ins().ushr_imm(hi, 32),
                        };
                        let dword = l.b.ins().band_imm(dword, 0xffffffff);
                        if i.op0_kind() == OpKind::Register {
                            l.put_reg(i.op0_register(), dword, i)?;
                        } else {
                            let p = l.address(i)?;
                            l.store(p, 32, dword, false);
                        }
                    }
                    Pextrq => {
                        let imm = (i.immediate8() & 1) as u32;
                        let qword = if imm == 0 { lo } else { hi };
                        if i.op0_kind() == OpKind::Register {
                            l.put_reg(i.op0_register(), qword, i)?;
                        } else {
                            let p = l.address(i)?;
                            l.store(p, 64, qword, false);
                        }
                    }
                    _ => unreachable!(),
                }
            }
            Ptest => {
                let dst = xmm_index(i.op0_register()).ok_or_else(|| fail(i, "expected XMM register"))?;
                let (dst_lo, dst_hi) = l.get_xmm(dst);
                let (src_lo, src_hi) = l.read_xmm(i, 1)?;
                let and_lo = l.b.ins().band(dst_lo, src_lo);
                let and_hi = l.b.ins().band(dst_hi, src_hi);
                let and_any = l.b.ins().bor(and_lo, and_hi);
                let zf = l.b.ins().icmp_imm(IntCC::Equal, and_any, 0);

                let not_dst_lo = l.b.ins().bnot(dst_lo);
                let not_dst_hi = l.b.ins().bnot(dst_hi);
                let not_and_lo = l.b.ins().band(not_dst_lo, src_lo);
                let not_and_hi = l.b.ins().band(not_dst_hi, src_hi);
                let not_and_any = l.b.ins().bor(not_and_lo, not_and_hi);
                let cf = l.b.ins().icmp_imm(IntCC::Equal, not_and_any, 0);

                let c_40 = l.c(0x40);
                let c_01 = l.c(0x01);
                let c_0 = l.c(0);
                let zf_val = l.b.ins().select(zf, c_40, c_0);
                let cf_val = l.b.ins().select(cf, c_01, c_0);
                let flags = l.b.ins().bor(zf_val, cf_val);
                l.set(17, flags);
            }
            Shrd => {
                let w = l.width(i, 0)?;
                let dst = l.read(i, 0)?;
                let src = l.read(i, 1)?;
                let count = l.read(i, 2)?;
                let mask = (w - 1) as i64;
                let count_masked = l.b.ins().band_imm(count, mask);
                let is_zero = l.b.ins().icmp_imm(IntCC::Equal, count_masked, 0);

                let shifted_dst = l.b.ins().ushr(dst, count_masked);
                let shift_src = l.b.ins().irsub_imm(count_masked, w as i64);
                let safe_shift = l.b.ins().band_imm(shift_src, mask);
                let shifted_src = l.b.ins().ishl(src, safe_shift);
                let res = l.b.ins().bor(shifted_dst, shifted_src);
                let res = l.mask(res, w);
                let final_res = l.b.ins().select(is_zero, dst, res);

                let cf_shift = l.b.ins().iadd_imm(count_masked, -1);
                let safe_cf_shift = l.b.ins().band_imm(cf_shift, mask);
                let cf_bit = l.b.ins().ushr(dst, safe_cf_shift);
                let cf = l.b.ins().band_imm(cf_bit, 1);
                let status = l.status(res, w);
                let new_flags = l.b.ins().bor(status, cf);
                let old_flags = l.var(17);
                let final_flags = l.b.ins().select(is_zero, old_flags, new_flags);
                l.set(17, final_flags);

                l.write(i, 0, final_res)?;
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
    for i in 16..19 {
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
    if let Some(fault) = l.mem_fault {
        l.b.switch_to_block(fault);
        l.b.seal_block(fault);
        // Persist the register file so the runtime can inspect RIP at the fault.
        for i in 0..16 {
            let var_idx = div.variable(i);
            let v = l.var(var_idx as usize);
            l.b.ins().store(MemFlags::new(), v, ctx, (i * 8) as i32);
        }
        for i in 16..19 {
            let v = l.var(i);
            l.b.ins().store(MemFlags::new(), v, ctx, (i * 8) as i32);
        }
        if backend == Backend::X86_64 {
            let three = l.c(super::native::FAULT_MEMORY);
            l.b.ins().return_(&[three]);
        } else {
            l.b.ins().return_(&[]);
        }
    }
    l.b.finalize();
    Ok(function)
}
