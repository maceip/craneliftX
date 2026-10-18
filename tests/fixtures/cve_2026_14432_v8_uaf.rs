//! Use-after-free patterned on CVE-2026-14432 (Chrome V8: a cached object
//! reference is not invalidated when the object is freed during GC).
//!
//! What is replicated: an object cache holds a pointer to a JSObject; the
//! object is freed; a new allocation may reuse the slot; the cache's stale
//! reference is then read.
//!
//! What is NOT replicated: V8, a real GC, or the real CVE.
//!
//! Control run (no tags): dangling read completes. Exit 3 if the slot was
//! reused, exit 4 if the freed slot still held its old bytes — both UAF.
//! Hardened run: quarantine tag on the freed granule → UseAfterFree.
#![no_std]
#![no_main]
use core::arch::global_asm;

global_asm!(
    ".global _start",
    "_start:",
    // JSObject (will be "GC'd").
    "mov rax, 0x49535200",
    "mov rdi, 48",
    "syscall",
    "mov r12, rax",
    "mov rax, 0x1122334455667788",
    "mov qword ptr [r12], rax",
    // Object cache holds a reference.
    "mov rax, 0x49535200",
    "mov rdi, 16",
    "syscall",
    "mov r13, rax",
    "mov qword ptr [r13], r12",
    // GC: free the JSObject.
    "mov rax, 0x49535203",
    "mov rdi, r12",
    "syscall",
    // New object may reuse the slot.
    "mov rax, 0x49535200",
    "mov rdi, 48",
    "syscall",
    "mov r12, rax",
    "mov rax, 0xAABBCCDDEEFF0011",
    "mov qword ptr [r12], rax",
    // Cache accesses its stale reference.
    "mov r14, qword ptr [r13]",
    "mov rax, qword ptr [r14]",
    "mov rbx, 0x1122334455667788",
    "cmp rax, rbx",
    "je not_reused",
    "mov rax, 60",
    "mov rdi, 3",
    "syscall",
    "not_reused:",
    "mov rax, 60",
    "mov rdi, 4",
    "syscall",
);

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
