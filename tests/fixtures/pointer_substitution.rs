//! Real freestanding Linux x86-64 guest that performs a pointer substitution.
//!
//! Two heap objects each hold a 16-byte buffer and a function pointer at offset
//! 16. Both pointers are initialized from constants, exactly as a constructor
//! would. The "attacker" then copies the second object's signed function
//! pointer over the first object's -- the shape of a substitution attack once
//! an overflow can reach the pointer field.
//!
//! Exit codes: 10 = the original handler ran, 20 = the substituted handler ran.
#![no_std]
#![no_main]
use core::arch::global_asm;

global_asm!(
    ".global _start",
    "_start:",
    // obj1 = isr_malloc(32), obj2 = isr_malloc(32)
    "mov rax, 0x49535200",
    "mov rdi, 32",
    "syscall",
    "mov r12, rax",
    "mov rax, 0x49535200",
    "mov rdi, 32",
    "syscall",
    "mov r13, rax",
    // obj1->cb = handler_a, obj2->cb = handler_b (constant provenance).
    "lea rax, [rip + handler_a]",
    "mov qword ptr [r12 + 16], rax",
    "lea rax, [rip + handler_b]",
    "mov qword ptr [r13 + 16], rax",
    // Substitution: copy the signed pointer from obj2 into obj1's slot.
    "mov rax, qword ptr [r13 + 16]",
    "mov qword ptr [r12 + 16], rax",
    // Call through obj1->cb.
    "mov rax, qword ptr [r12 + 16]",
    "call rax",
    "mov rax, 60",
    "mov rdi, 9",
    "syscall",
    "handler_a:",
    "mov rax, 60",
    "mov rdi, 10",
    "syscall",
    "handler_b:",
    "mov rax, 60",
    "mov rdi, 20",
    "syscall",
);

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
