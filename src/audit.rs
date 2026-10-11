use std::ops::Range;

use iced_x86::{Instruction, Mnemonic, OpKind, Register};

use crate::{
    error::SecurityViolation,
    x86::{control_boundary, immediate, memory_width, privileged, register_view},
};

/// Audit without an image mapping. Absolute and RIP-relative data operands fail closed.
pub fn audit_trace(
    trace: &[Instruction],
    stack_base: u64,
    stack_limit: u64,
) -> Result<(), SecurityViolation> {
    audit_trace_with_image(trace, stack_base, stack_limit, None)
}

/// Image and stack ranges are distinct. Image end is exclusive; every byte of
/// an absolute memory operand must fit. Register-derived host pointers remain
/// the caller's responsibility (this is an ISR filter, not a memory sandbox).
pub fn audit_trace_with_image(
    trace: &[Instruction],
    stack_base: u64,
    stack_limit: u64,
    image: Option<Range<u64>>,
) -> Result<(), SecurityViolation> {
    if stack_base >= stack_limit || image.as_ref().is_some_and(|r| r.start >= r.end) {
        return Err(SecurityViolation::InvalidBounds);
    }
    for insn in trace {
        let rip = insn.ip();
        if privileged(insn) {
            return Err(SecurityViolation::PrivilegedInstruction { rip });
        }
        // A boundary's explicit register is a target, not a destination. Its
        // absolute memory operands still go through the address policy below.
        let writes_sp = !control_boundary(insn)
            && insn.op0_kind() == OpKind::Register
            && register_view(insn.op0_register()).is_some_and(|(i, _, _)| i == 7);
        let exchanges_sp = insn.mnemonic() == Mnemonic::Xchg
            && insn.op1_kind() == OpKind::Register
            && register_view(insn.op1_register()).is_some_and(|(i, _, _)| i == 7);
        let adjusts_sp = matches!(insn.mnemonic(), Mnemonic::Add | Mnemonic::Sub)
            && insn.op0_register() == Register::RSP
            && immediate(insn, 1).is_some();
        if (writes_sp && !adjusts_sp)
            || exchanges_sp
            || matches!(
                insn.mnemonic(),
                Mnemonic::Push
                    | Mnemonic::Pop
                    | Mnemonic::Enter
                    | Mnemonic::Leave
                    | Mnemonic::Pushf
                    | Mnemonic::Pushfq
                    | Mnemonic::Popf
                    | Mnemonic::Popfq
            )
        {
            return Err(SecurityViolation::StackPivot { rip });
        }
        for operand in 0..insn.op_count() {
            if insn.op_kind(operand) != OpKind::Memory {
                continue;
            }
            let absolute = insn.is_ip_rel_memory_operand()
                || (insn.memory_base() == Register::None && insn.memory_index() == Register::None);
            if !absolute {
                continue;
            }
            let address = if insn.is_ip_rel_memory_operand() {
                insn.ip_rel_memory_address()
            } else {
                insn.memory_displacement64()
            };
            let allowed = memory_width(insn)
                .and_then(|size| address.checked_add(size))
                .zip(image.as_ref())
                .is_some_and(|(end, range)| address >= range.start && end <= range.end);
            if !allowed {
                return Err(SecurityViolation::AbsoluteAddress { rip, address });
            }
        }
    }
    Ok(())
}

/// Check each accepted immediate adjustment against the live initial RSP.
pub(crate) fn audit_stack_updates(
    trace: &[Instruction],
    mut rsp: u64,
    min: u64,
    max: u64,
) -> Result<(), SecurityViolation> {
    for insn in trace {
        if control_boundary(insn) {
            break;
        }
        if insn.op0_kind() != OpKind::Register || insn.op0_register() != Register::RSP {
            continue;
        }
        if rsp < min || rsp > max {
            return Err(SecurityViolation::StackOutOfBounds { rip: insn.ip() });
        }
        let value = immediate(insn, 1).ok_or(SecurityViolation::StackPivot { rip: insn.ip() })?
            as i64 as i128;
        let result = match insn.mnemonic() {
            Mnemonic::Add => rsp as i128 + value,
            Mnemonic::Sub => rsp as i128 - value,
            _ => return Err(SecurityViolation::StackPivot { rip: insn.ip() }),
        };
        if result < min as i128 || result > max as i128 {
            return Err(SecurityViolation::StackOutOfBounds { rip: insn.ip() });
        }
        rsp = result as u64;
    }
    Ok(())
}
