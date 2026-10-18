//! Minimal HTTP server for ISR benchmarking — pure assembly, no lea/rip-relative.
//!
//! All instructions are chosen to be supported by the ISR lowerer.
//! No lea, no rip-relative addressing, no push reg, no pop reg.
//!
//! Build:
//!   rustc --edition=2021 --target x86_64-unknown-linux-gnu \
//!     -C opt-level=2 -C panic=abort -C relocation-model=static \
//!     -C linker=rust-lld -C link-arg=-nostdlib -C link-arg=--static \
//!     -C no-vectorize-loops -C no-vectorize-slp \
//!     bench_server.rs -o bench_server.elf
#![no_std]
#![no_main]
use core::arch::global_asm;

const REQUEST_COUNT: u64 = 100;
const PORT: u64 = 29876;

global_asm!(
    ".global _start",
    "_start:",
    // --- Build response on stack (24 bytes) ---
    // Response: "HTTP/1.1 200 OK\r\n\r\nOK\n" (22 bytes)
    "sub rsp, 24",
    "mov dword ptr [rsp+0], 0x50545448",   // "HTTP"
    "mov dword ptr [rsp+4], 0x312e312f",   // "/1.1"
    "mov dword ptr [rsp+8], 0x30303220",   // " 200"
    "mov dword ptr [rsp+12], 0x0d4b4f20",  // " OK\r"
    "mov dword ptr [rsp+16], 0x4f0a0d0a",  // "\n\r\nO"
    "mov dword ptr [rsp+20], 0x00000a4b",  // "K\n\0\0"
    "mov r14, rsp",         // r14 = response buffer
    "mov r15, 22",          // r15 = response length

    // --- Allocate request buffer on stack ---
    "sub rsp, 1024",
    "mov r12, rsp",         // r12 = req buffer

    // socket(AF_INET, SOCK_STREAM, 0)
    "mov rax, 41",
    "mov rdi, 2",
    "mov rsi, 1",
    "mov rdx, 0",
    "syscall",
    "mov rbx, rax",          // rbx = sock fd
    "test rax, rax",
    "js fail",

    // bind(sock, &addr, 16) — addr on stack
    "sub rsp, 16",
    "mov dword ptr [rsp], 0x0002",       // AF_INET
    "mov word ptr [rsp+2], 0xb474",      // htons(29876) = 0xB474
    "mov dword ptr [rsp+4], 0x0100007f", // 127.0.0.1
    "mov rax, 49",
    "mov rdi, rbx",
    "mov rsi, rsp",
    "mov rdx, 16",
    "syscall",
    "test rax, rax",
    "js fail",
    "add rsp, 16",

    // listen(sock, 128)
    "mov rax, 50",
    "mov rdi, rbx",
    "mov rsi, 128",
    "mov rdx, 0",
    "syscall",
    "test rax, rax",
    "js fail",

    // Main loop
    "mov r13, {REQUEST_COUNT}",  // r13 = remaining requests

    "loop_start:",
    "test r13, r13",
    "jz cleanup",
    "dec r13",

    // accept(sock, NULL, NULL)
    "mov rax, 43",
    "mov rdi, rbx",
    "mov rsi, 0",
    "mov rdx, 0",
    "syscall",
    "test rax, rax",
    "js loop_start",
    "mov r10, rax",          // r10 = conn fd

    // read(conn, req_buf, 1024)
    "mov rax, 0",
    "mov rdi, r10",
    "mov rsi, r12",
    "mov rdx, 1024",
    "syscall",

    // write(conn, response, response_len)
    "mov rax, 1",
    "mov rdi, r10",
    "mov rsi, r14",
    "mov rdx, r15",
    "syscall",

    // close(conn)
    "mov rax, 3",
    "mov rdi, r10",
    "mov rsi, 0",
    "mov rdx, 0",
    "syscall",

    "jmp loop_start",

    "cleanup:",
    // close(sock)
    "mov rax, 3",
    "mov rdi, rbx",
    "syscall",
    // exit(0)
    "mov rax, 60",
    "mov rdi, 0",
    "syscall",

    "fail:",
    "mov rax, 60",
    "mov rdi, 1",
    "syscall",

    REQUEST_COUNT = const REQUEST_COUNT,
);

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
