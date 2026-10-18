//! Real freestanding Linux x86-64 guest that writes to a freed chunk.
//!
//! A 32-byte chunk is allocated, filled, freed, and then written again. Under
//! the hardened heap the freed slot's granules are re-tagged with the
//! quarantine value, so the first write after `free` faults.
//!
//! Exit codes: 0 = the write-after-free completed, 4 = never reached.
#![no_std]
#![no_main]
use core::arch::global_asm;

global_asm!(
    ".global _start",
    "_start:",
    "mov rax, 0x49535200", // isr_malloc(32)
    "mov rdi, 32",
    "syscall",
    "mov r12, rax",
    "xor rcx, rcx",
    "fill:",
    "mov byte ptr [r12 + rcx], 0x11",
    "add rcx, 1",
    "cmp rcx, 32",
    "jne fill",
    // free(A)
    "mov rax, 0x49535203",
    "mov rdi, r12",
    "syscall",
    // Use after free: write the first granule again. Reaching the exit below
    // means the write-after-free completed unhindered.
    "mov byte ptr [r12], 0xCC",
    "mov rax, 60",
    "mov rdi, 0",
    "syscall",
);

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
