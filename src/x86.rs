use iced_x86::{Instruction, MemorySize, Mnemonic, OpKind, Register};

// Decoder-only iced-x86 deliberately omits the instr_info register tables.
// Each row is in GuestContext order: low byte, word, dword, qword.
const REGISTERS: [[Register; 4]; 16] = {
    use Register::*;
    [
        [AL, AX, EAX, RAX],
        [BL, BX, EBX, RBX],
        [CL, CX, ECX, RCX],
        [DL, DX, EDX, RDX],
        [SIL, SI, ESI, RSI],
        [DIL, DI, EDI, RDI],
        [BPL, BP, EBP, RBP],
        [SPL, SP, ESP, RSP],
        [R8L, R8W, R8D, R8],
        [R9L, R9W, R9D, R9],
        [R10L, R10W, R10D, R10],
        [R11L, R11W, R11D, R11],
        [R12L, R12W, R12D, R12],
        [R13L, R13W, R13D, R13],
        [R14L, R14W, R14D, R14],
        [R15L, R15W, R15D, R15],
    ]
};

/// (GuestContext register index, bit width, bit offset).
pub(crate) fn register_view(reg: Register) -> Option<(usize, u16, u8)> {
    for (index, row) in REGISTERS.iter().enumerate() {
        if let Some(column) = row.iter().position(|&candidate| candidate == reg) {
            return Some((index, 8 << column, 0));
        }
    }
    match reg {
        Register::AH => Some((0, 8, 8)),
        Register::BH => Some((1, 8, 8)),
        Register::CH => Some((2, 8, 8)),
        Register::DH => Some((3, 8, 8)),
        _ => None,
    }
}

pub(crate) fn immediate(insn: &Instruction, operand: u32) -> Option<u64> {
    Some(match insn.op_kind(operand) {
        OpKind::Immediate8 => insn.immediate8() as u64,
        OpKind::Immediate16 => insn.immediate16() as u64,
        OpKind::Immediate32 => insn.immediate32() as u64,
        OpKind::Immediate64 => insn.immediate64(),
        OpKind::Immediate8to16 => insn.immediate8to16() as u64,
        OpKind::Immediate8to32 => insn.immediate8to32() as u64,
        OpKind::Immediate8to64 => insn.immediate8to64() as u64,
        OpKind::Immediate32to64 => insn.immediate32to64() as u64,
        _ => return None,
    })
}

pub(crate) fn memory_width(insn: &Instruction) -> Option<u64> {
    match insn.memory_size() {
        MemorySize::UInt8 => Some(1),
        MemorySize::UInt16 => Some(2),
        MemorySize::UInt32 => Some(4),
        MemorySize::UInt64 => Some(8),
        _ => None,
    }
}

pub(crate) fn privileged(insn: &Instruction) -> bool {
    use Mnemonic::*;
    matches!(
        insn.mnemonic(),
        Syscall
            | Sysenter
            | Sysret
            | Sysexit
            | Int
            | Int1
            | Int3
            | Into
            | Iret
            | Iretd
            | Iretq
            | Hlt
            | Cli
            | Sti
    )
}

pub(crate) fn control_boundary(insn: &Instruction) -> bool {
    use Mnemonic::*;
    matches!(
        insn.mnemonic(),
        Jmp | Call
            | Ret
            | Retf
            | Ja
            | Jae
            | Jb
            | Jbe
            | Je
            | Jg
            | Jge
            | Jl
            | Jle
            | Jne
            | Jno
            | Jnp
            | Jns
            | Jo
            | Jp
            | Js
            | Jcxz
            | Jecxz
            | Jrcxz
            | Loop
            | Loope
            | Loopne
    )
}

pub(crate) fn boundary(insn: &Instruction) -> bool {
    control_boundary(insn) || privileged(insn)
}
