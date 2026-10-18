//! Use-after-free patterned on CVE-2026-42530 (Nginx HTTP/3 QPACK lifecycle
//! mismatch: a connection-level struct holds a pointer into a short-lived
//! stream pool that is freed when the stream closes).
//!
//! What is replicated: a long-lived struct caches a pointer to a short-lived
//! buffer; the buffer is freed; a new allocation may reuse the slot; the
//! cached pointer is then read.
//!
//! What is NOT replicated: QPACK, HTTP/3, or the real CVE.
//!
//! Control run (no tags): the dangling read completes. Exit 3 if the slot was
//! reused (the new value is observed), exit 4 if the freed slot still holds
//! its old bytes. Both are use-after-free: the access went through a pointer
//! the runtime had already freed.
//! Hardened run: quarantine tag on the freed granule → UseAfterFree.
#![no_std]
#![no_main]
use core::arch::global_asm;

global_asm!(
    ".global _start",
    "_start:",
    // Connection struct (long-lived).
    "mov rax, 0x49535200",
    "mov rdi, 64",
    "syscall",
    "mov r14, rax",
    // Stream pool insert_buffer (short-lived).
    "mov rax, 0x49535200",
    "mov rdi, 32",
    "syscall",
    "mov r15, rax",
    "mov dword ptr [r15], 0xDEADBEEF",
    // Connection caches the stream pointer.
    "mov qword ptr [r14], r15",
    // Stream closes: free the insert_buffer.
    "mov rax, 0x49535203",
    "mov rdi, r15",
    "syscall",
    // New stream allocates from the same pool.
    "mov rax, 0x49535200",
    "mov rdi, 32",
    "syscall",
    "mov r15, rax",
    "mov dword ptr [r15], 0xCAFEBABE",
    // Connection struct accesses its CACHED (stale) pointer.
    "mov r12, qword ptr [r14]",
    "mov eax, dword ptr [r12]",
    "cmp eax, 0xDEADBEEF",
    "je not_reused",
    // Stale read observed the new occupant: use-after-free with reuse.
    "mov rax, 60",
    "mov rdi, 3",
    "syscall",
    "not_reused:",
    // Freed but not reused: still a UAF (access to freed memory succeeded).
    "mov rax, 60",
    "mov rdi, 4",
    "syscall",
);

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
