//! Real freestanding Linux x86-64 guest that measures heap grooming.
//!
//! Each round allocates a victim chunk and then an attacker chunk and records
//! whether the attacker landed immediately after the victim. That adjacency is
//! the precondition for "overflow from my buffer into the object I want"; if
//! placement is randomized it becomes a coin flip instead of a guarantee.
//!
//! Exit code: how many of 16 rounds placed the attacker adjacent to the victim.
#![no_std]
#![no_main]
use core::arch::global_asm;

global_asm!(
    ".global _start",
    "_start:",
    "xor r14, r14", // rounds
    "xor r15, r15", // adjacent count
    "round:",
    "mov rax, 0x49535200", // victim
    "mov rdi, 32",
    "syscall",
    "mov r12, rax",
    "mov rax, 0x49535200", // attacker
    "mov rdi, 32",
    "syscall",
    "mov r13, rax",
    // Tag bits ride in the top byte, so compare the low 32 bits of the delta.
    "mov rax, r13",
    "sub rax, r12",
    // Slots may be handed out in either direction, so accept +/- one slot.
    "cmp eax, 32",
    "je adjacent",
    "cmp eax, 0xFFFFFFE0",
    "jne next",
    "adjacent:",
    "add r15, 1",
    "next:",
    "add r14, 1",
    "cmp r14, 16",
    "je done",
    "jmp round",
    "done:",
    "mov rax, 60",
    "mov rdi, r15",
    "syscall",
);

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
