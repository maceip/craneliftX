//! Heap-overflow patterned on CVE-2026-53362 (IPv6 fragment reassembly
//! overflowing an adjacent slab object).
//!
//! What is replicated: a fixed-size reassembly buffer is filled past its end
//! and corrupts a neighbouring object of the same size class, the shape of a
//! slab overflow into an adjacent `sk_buff`.
//!
//! What is NOT replicated: the kernel IPv6 stack, the real CVE patch, or a
//! call through the corrupted function pointer.
//!
//! Allocation order matters: with ordered placement the first same-class slot
//! is the higher address, so the neighbour is allocated first and the
//! overflow runs forward from the lower buffer into it. The overflow also
//! writes behind the buffer so the pair works under randomized placement.
//!
//! Control run (no tags, ordered placement): neighbour corrupted → exit 3.
//! Hardened run: shadow tag mismatch on the overflowed granule → caught.
#![no_std]
#![no_main]
use core::arch::global_asm;

global_asm!(
    ".global _start",
    "_start:",
    // skb_struct first (higher slot under ordered placement).
    "mov rax, 0x49535200",
    "mov rdi, 32",
    "syscall",
    "mov r13, rax",
    // reassembly_buffer second (lower slot) — same size class.
    "mov rax, 0x49535200",
    "mov rdi, 32",
    "syscall",
    "mov r12, rax",
    // skb->func = a plausible destructor pointer.
    "mov rax, 0x1111111111111111",
    "mov qword ptr [r13], rax",
    // Overflow past the end of the 32-byte buffer (32 bytes forward).
    "xor rcx, rcx",
    "1:",
    "mov byte ptr [r12 + 32 + rcx], 0xCC",
    "add rcx, 1",
    "cmp rcx, 32",
    "jne 1b",
    // Also write behind the buffer so a neighbour on the low side is hit.
    "xor rcx, rcx",
    "2:",
    "mov byte ptr [r12 - 32 + rcx], 0xCC",
    "add rcx, 1",
    "cmp rcx, 32",
    "jne 2b",
    // Did the neighbour's function pointer change?
    "mov rax, qword ptr [r13]",
    "mov rbx, 0x1111111111111111",
    "cmp rax, rbx",
    "jne corrupted",
    // Overflow did not reach the neighbour.
    "mov rax, 60",
    "mov rdi, 4",
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
