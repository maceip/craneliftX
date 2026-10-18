//! Heap-overflow patterned on CVE-2026-42533 (Nginx `ngx_http_map_module`:
//! regex capture data copied into a fixed-size buffer without a length check).
//!
//! What is replicated: a small fixed-size buffer is overflowed past both ends
//! so it corrupts a neighbour of the same size class regardless of which side
//! the allocator placed it on — the shape of an unchecked capture copy into a
//! temporary heap buffer.
//!
//! What is NOT replicated: Nginx, the map module, regex captures, or the real
//! CVE. This is the generic linear-overflow primitive named after that bug.
//!
//! Control run (no tags, ordered placement): neighbour corrupted → exit 3.
//! Hardened run: shadow tag mismatch on the overflowed granule → caught.
#![no_std]
#![no_main]
use core::arch::global_asm;

global_asm!(
    ".global _start",
    "_start:",
    // capture_buffer = isr_malloc(32)
    "mov rax, 0x49535200",
    "mov rdi, 32",
    "syscall",
    "mov r12, rax",
    // neighbour = isr_malloc(32) — same size class.
    "mov rax, 0x49535200",
    "mov rdi, 32",
    "syscall",
    "mov r13, rax",
    // Fill neighbour with 0xAA.
    "xor rcx, rcx",
    "1:",
    "mov byte ptr [r13 + rcx], 0xAA",
    "add rcx, 1",
    "cmp rcx, 32",
    "jne 1b",
    // Unchecked copy past both ends of the capture buffer (placement-agnostic).
    "xor rcx, rcx",
    "2:",
    "mov byte ptr [r12 + 32 + rcx], 0xBB",
    "add rcx, 1",
    "cmp rcx, 16",
    "jne 2b",
    "xor rcx, rcx",
    "3:",
    "mov byte ptr [r12 - 16 + rcx], 0xBB",
    "add rcx, 1",
    "cmp rcx, 16",
    "jne 3b",
    // Neighbour corrupted?
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
