use cranelift_codegen::{
    ir::{types, AbiParam, Function, InstBuilder, MemFlags, Signature, Type, Value},
    isa::CallConv,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use iced_x86::{Instruction, Mnemonic, OpKind, Register};

use crate::{
    context::{REGISTER_COUNT, RIP_OFFSET},
    error::EngineError,
    x86::{control_boundary, immediate, memory_width, register_view},
};

fn unsupported(insn: &Instruction, reason: &'static str) -> EngineError {
    EngineError::Lower {
        rip: insn.ip(),
        reason,
    }
}

fn read_register(
    builder: &mut FunctionBuilder<'_>,
    reg: Register,
    insn: &Instruction,
) -> Result<Value, EngineError> {
    let (index, width, shift) = register_view(reg)
        .ok_or_else(|| unsupported(insn, "only integer registers are supported"))?;
    let mut value = builder.use_var(Variable::from_u32(index as u32));
    if shift != 0 {
        value = builder.ins().ushr_imm(value, i64::from(shift));
    }
    if width < 64 {
        value = builder.ins().band_imm(value, ((1u64 << width) - 1) as i64);
    }
    Ok(value)
}

fn write_register(
    builder: &mut FunctionBuilder<'_>,
    reg: Register,
    mut value: Value,
    insn: &Instruction,
) -> Result<(), EngineError> {
    let (index, width, shift) = register_view(reg)
        .ok_or_else(|| unsupported(insn, "only integer registers are supported"))?;
    let var = Variable::from_u32(index as u32);
    if width < 64 {
        let mask = (1u64 << width) - 1;
        value = builder.ins().band_imm(value, mask as i64);
        if width < 32 {
            if shift != 0 {
                value = builder.ins().ishl_imm(value, i64::from(shift));
            }
            let old = builder.use_var(var);
            let preserved = builder.ins().band_imm(old, !(mask << shift) as i64);
            value = builder.ins().bor(preserved, value);
        }
        // 32-bit writes zero the upper half; 8/16-bit writes preserve it.
    }
    builder.def_var(var, value);
    Ok(())
}

fn address(builder: &mut FunctionBuilder<'_>, insn: &Instruction) -> Result<Value, EngineError> {
    if insn.segment_prefix() != Register::None || insn.memory_index() != Register::None {
        return Err(unsupported(
            insn,
            "segment overrides and indexed addressing are outside reg+offset memory support",
        ));
    }
    if insn.is_ip_rel_memory_operand() {
        return Ok(builder
            .ins()
            .iconst(types::I64, insn.ip_rel_memory_address() as i64));
    }
    let base = insn.memory_base();
    if base == Register::None {
        return Ok(builder
            .ins()
            .iconst(types::I64, insn.memory_displacement64() as i64));
    }
    if register_view(base).is_none_or(|(_, width, _)| width != 64) {
        return Err(unsupported(
            insn,
            "memory base must be a 64-bit integer register",
        ));
    }
    let value = read_register(builder, base, insn)?;
    Ok(builder
        .ins()
        .iadd_imm(value, insn.memory_displacement64() as i64))
}

fn memory_type(insn: &Instruction) -> Result<Type, EngineError> {
    match memory_width(insn) {
        Some(1) => Ok(types::I8),
        Some(2) => Ok(types::I16),
        Some(4) => Ok(types::I32),
        Some(8) => Ok(types::I64),
        _ => Err(unsupported(
            insn,
            "only 8/16/32/64-bit integer memory is supported",
        )),
    }
}

fn source(builder: &mut FunctionBuilder<'_>, insn: &Instruction) -> Result<Value, EngineError> {
    if insn.op1_kind() == OpKind::Register {
        return read_register(builder, insn.op1_register(), insn);
    }
    if let Some(value) = immediate(insn, 1) {
        return Ok(builder.ins().iconst(types::I64, value as i64));
    }
    Err(unsupported(insn, "expected a register or immediate source"))
}

/// Build a single-block CLIF function with exactly one I64 context argument.
/// The caller must audit before lowering. Unsupported instructions fail explicitly.
/// A final control-flow boundary remains unexecuted at next_rip.
pub fn lower_to_clif(trace: &[Instruction], next_rip: u64) -> Result<Function, EngineError> {
    let mut signature = Signature::new(CallConv::SystemV);
    signature.params.push(AbiParam::new(types::I64));
    let mut function = Function::with_name_signature(Default::default(), signature);
    let mut builder_context = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut function, &mut builder_context);
    let entry = builder.create_block();
    builder.append_block_params_for_function_params(entry);
    builder.switch_to_block(entry);
    builder.seal_block(entry);
    let ctx_ptr = builder.block_params(entry)[0];
    for index in 0..REGISTER_COUNT {
        let var = Variable::from_u32(index as u32);
        builder.declare_var(var, types::I64);
        let value = builder
            .ins()
            .load(types::I64, MemFlags::new(), ctx_ptr, (index * 8) as i32);
        builder.def_var(var, value);
    }
    for (index, insn) in trace.iter().enumerate() {
        if control_boundary(insn) {
            if index + 1 != trace.len() || next_rip != insn.ip() {
                return Err(unsupported(
                    insn,
                    "control-flow boundary must be last and equal exit RIP",
                ));
            }
            break;
        }
        if insn.has_lock_prefix() || insn.has_rep_prefix() || insn.has_repne_prefix() {
            return Err(unsupported(insn, "instruction prefixes are not supported"));
        }
        if insn.op_count() != 2 {
            return Err(unsupported(
                insn,
                "instruction is outside the supported matrix",
            ));
        }
        if insn.mnemonic() == Mnemonic::Mov
            && insn.op0_kind() == OpKind::Memory
            && insn.op1_kind() == OpKind::Register
        {
            let ty = memory_type(insn)?;
            let ptr = address(&mut builder, insn)?;
            let mut value = read_register(&mut builder, insn.op1_register(), insn)?;
            if ty != types::I64 {
                value = builder.ins().ireduce(ty, value);
            }
            builder.ins().store(MemFlags::new(), value, ptr, 0);
            continue;
        }
        if insn.op0_kind() != OpKind::Register {
            return Err(unsupported(insn, "expected a register destination"));
        }
        let destination = insn.op0_register();
        let (_, width, _) = register_view(destination)
            .ok_or_else(|| unsupported(insn, "only integer registers are supported"))?;
        let value = match insn.mnemonic() {
            Mnemonic::Mov if insn.op1_kind() == OpKind::Memory => {
                let ty = memory_type(insn)?;
                let ptr = address(&mut builder, insn)?;
                let value = builder.ins().load(ty, MemFlags::new(), ptr, 0);
                if ty == types::I64 {
                    value
                } else {
                    builder.ins().uextend(types::I64, value)
                }
            }
            Mnemonic::Mov => source(&mut builder, insn)?,
            Mnemonic::Add | Mnemonic::Sub | Mnemonic::Xor | Mnemonic::And | Mnemonic::Or => {
                let left = read_register(&mut builder, destination, insn)?;
                let right = source(&mut builder, insn)?;
                match insn.mnemonic() {
                    Mnemonic::Add => builder.ins().iadd(left, right),
                    Mnemonic::Sub => builder.ins().isub(left, right),
                    Mnemonic::Xor => builder.ins().bxor(left, right),
                    Mnemonic::And => builder.ins().band(left, right),
                    Mnemonic::Or => builder.ins().bor(left, right),
                    _ => unreachable!(),
                }
            }
            Mnemonic::Shl | Mnemonic::Shr => {
                let left = read_register(&mut builder, destination, insn)?;
                let count = immediate(insn, 1)
                    .ok_or_else(|| unsupported(insn, "only immediate shifts are supported"))?;
                // x86 masks with 31 even for 8/16-bit operands. Widening before
                // shifting also avoids CLIF's modulo-width shift semantics.
                let count = builder.ins().iconst(
                    types::I64,
                    (count & if width == 64 { 63 } else { 31 }) as i64,
                );
                if insn.mnemonic() == Mnemonic::Shl {
                    builder.ins().ishl(left, count)
                } else {
                    builder.ins().ushr(left, count)
                }
            }
            _ => {
                return Err(unsupported(
                    insn,
                    "instruction is outside the supported matrix",
                ))
            }
        };
        write_register(&mut builder, destination, value, insn)?;
    }
    for index in 0..REGISTER_COUNT {
        let value = builder.use_var(Variable::from_u32(index as u32));
        builder
            .ins()
            .store(MemFlags::new(), value, ctx_ptr, (index * 8) as i32);
    }
    let rip = builder.ins().iconst(types::I64, next_rip as i64);
    builder
        .ins()
        .store(MemFlags::new(), rip, ctx_ptr, RIP_OFFSET);
    builder.ins().return_(&[]);
    builder.finalize();
    Ok(function)
}
