//! Real freestanding Linux x86-64 guest that performs a linear heap overflow.
//!
//! Two 32-byte chunks are allocated from the runtime allocator, the second is
//! filled with a known pattern, and the first is then written 16 bytes past its
//! end. Nothing here is synthetic: the write is an ordinary `mov` loop, and the
//! allocator is reached through the runtime's heap syscall.
//!
//! Exit codes: 0 = neighbour intact, 3 = neighbour corrupted (overflow landed).
#![no_std]
#![no_main]
use core::arch::global_asm;

global_asm!(
    ".global _start",
    "_start:",
    // A = isr_malloc(32)
    "mov rax, 0x49535200",
    "mov rdi, 32",
    "syscall",
    "mov r12, rax",
    // B = isr_malloc(32)
    "mov rax, 0x49535200",
    "mov rdi, 32",
    "syscall",
    "mov r13, rax",
    // Fill B with 0xAA.
    "xor rcx, rcx",
    "fill_b:",
    "mov byte ptr [r13 + rcx], 0xAA",
    "add rcx, 1",
    "cmp rcx, 32",
    "jne fill_b",
    // Overflow: 16 bytes past the end of A, and 16 bytes before it. Which side
    // the neighbour sits on depends on placement, and the fixture must corrupt
    // it either way or the control run proves nothing.
    "xor rcx, rcx",
    "overflow_hi:",
    "mov byte ptr [r12 + 32 + rcx], 0xBB",
    "add rcx, 1",
    "cmp rcx, 16",
    "jne overflow_hi",
    "xor rcx, rcx",
    "overflow_lo:",
    "mov byte ptr [r12 - 16 + rcx], 0xBB",
    "add rcx, 1",
    "cmp rcx, 16",
    "jne overflow_lo",
    // Did the neighbour change? Either of its two granules may be the one the
    // overflow reached, depending on which side of A it was placed.
    "mov al, byte ptr [r13]",
    "cmp al, 0xAA",
    "jne corrupted",
    "mov al, byte ptr [r13 + 16]",
    "cmp al, 0xAA",
    "jne corrupted",
    "mov rax, 60",
    "mov rdi, 0",
    "syscall",
    "corrupted:",
    "mov rax, 60",
    "mov rdi, 3",
    "syscall",
);

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
