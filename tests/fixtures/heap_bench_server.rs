//! Heap-churn HTTP guest for Skillet benchmarking — freestanding Linux x86-64.
//!
//! Every request does real allocator work:
//!   * scratch + header table + parse state + response buffer
//!   * a 32-slot live session ring with frees
//!   * periodic bulk alloc/free
//!
//! `malloc`/`free` are ordinary global symbols:
//!   * native Linux — mmap/munmap-backed allocator (works without libc)
//!   * under isr-process — the runtime intercepts these names and uses the
//!     tagged randomized heap (same binary, no relink)
//!
//! Build (same flags as bench_server.rs):
//!   rustc --edition=2021 --target x86_64-unknown-linux-gnu \
//!     -C opt-level=2 -C panic=abort -C relocation-model=static \
//!     -C linker=rust-lld -C link-arg=-nostdlib -C link-arg=--static \
//!     heap_bench_server.rs -o heap_bench_server.elf
#![no_std]
#![no_main]
use core::arch::global_asm;

const REQUEST_COUNT: u64 = 200;

global_asm!(
    // ---------------- malloc / free (mmap-backed) ----------------
    ".global malloc",
    ".global free",
    // ---------------- malloc / free (mmap-backed + freelist) ----------------
    // Header: [0]=map_len or 0 if freelist node, [8]=user_size.
    // Freelist head is the global `heap_free_list`.
    ".global malloc",
    ".global free",
    ".data",
    "heap_free_list:",
    ".quad 0",
    ".text",
    "malloc:",
    "push r8",
    "push r9",
    "push r10",
    "push r11",
    "push rdi",
    // walk freelist for a block large enough (first-fit)
    "mov rax, [rip + heap_free_list]",
    "xor rcx, rcx",
    "walk:",
    "test rax, rax",
    "jz mmap_path",
    "mov rdx, [rax+8]",        // user_size of free block
    "cmp rdx, rdi",
    "jae reuse",
    "mov rcx, rax",
    "mov rax, [rax]",          // next (stored in header map_len field when free)
    "jmp walk",
    "reuse:",
    // unlink rax from list
    "mov rdx, [rax]",          // next
    "test rcx, rcx",
    "jz reuse_head",
    "mov [rcx], rdx",
    "jmp reuse_ok",
    "reuse_head:",
    "mov [rip + heap_free_list], rdx",
    "reuse_ok:",
    "add rax, 64",
    "pop rdi",
    "pop r11",
    "pop r10",
    "pop r9",
    "pop r8",
    "ret",
    "mmap_path:",
    "pop rdi",
    "push rdi",
    "add rdi, 64",
    "mov rsi, rdi",
    "mov rax, 9",
    "xor rdi, rdi",
    "mov rdx, 3",
    "mov r10, 0x22",
    "mov r8, -1",
    "xor r9, r9",
    "syscall",
    "pop rcx",
    "cmp rax, -4096",
    "jae malloc_fail",
    "mov [rax], rsi",          // map_len (nonzero = real mapping)
    "mov [rax+8], rcx",
    "add rax, 64",
    "pop r11",
    "pop r10",
    "pop r9",
    "pop r8",
    "ret",
    "malloc_fail:",
    "xor rax, rax",
    "pop r11",
    "pop r10",
    "pop r9",
    "pop r8",
    "ret",
    "free:",
    "test rdi, rdi",
    "jz free_done",
    "push r8",
    "push r9",
    "push r10",
    "push r11",
    "sub rdi, 64",
    "mov rsi, [rdi]",          // map_len
    "cmp rsi, 64",
    "jbe not_mapped",          // freelist node: map_len field already used as next
    // real mapping: if it is large, push onto freelist instead of munmap
    // (reclaim when process exits). Freelist next stored in [rdi].
    "mov rax, [rip + heap_free_list]",
    "mov [rdi], rax",
    "mov [rip + heap_free_list], rdi",
    "jmp free_done2",
    "not_mapped:",
    // already a freelist node being freed twice? ignore
    "free_done2:",
    "pop r11",
    "pop r10",
    "pop r9",
    "pop r8",
    "free_done:",
    "ret",

    // ---------------- _start ----------------
    ".global _start",
    "_start:",

    // warmup: allocate and free a few blocks so the allocator is live
    "mov r8, 32",
    "warmup:",
    "mov rdi, 48",
    "call malloc",
    "mov rdi, rax",
    "call free",
    "mov rdi, 768",
    "call malloc",
    "mov rdi, rax",
    "call free",
    "dec r8",
    "jnz warmup",

    // response template on stack
    "sub rsp, 32",
    "mov dword ptr [rsp+0], 0x50545448",
    "mov dword ptr [rsp+4], 0x312e312f",
    "mov dword ptr [rsp+8], 0x30303220",
    "mov dword ptr [rsp+12], 0x0d4b4f20",
    "mov dword ptr [rsp+16], 0x4f0a0d0a",
    "mov dword ptr [rsp+20], 0x00000a4b",
    "mov r14, rsp",

    "sub rsp, 1024",
    "mov r12, rsp",

    // socket
    "mov rax, 41",
    "mov rdi, 2",
    "mov rsi, 1",
    "xor rdx, rdx",
    "syscall",
    "mov rbx, rax",
    "test rax, rax",
    "js fail",

    // SO_REUSEADDR
    "sub rsp, 16",
    "mov dword ptr [rsp], 1",
    "mov rax, 54",
    "mov rdi, rbx",
    "mov rsi, 1",
    "mov rdx, 2",
    "mov r10, rsp",
    "mov r8, 4",
    "syscall",
    "add rsp, 16",

    // bind 127.0.0.1:29876
    "sub rsp, 16",
    "mov dword ptr [rsp], 0x0002",
    "mov word ptr [rsp+2], 0xb474",
    "mov dword ptr [rsp+4], 0x0100007f",
    "mov rax, 49",
    "mov rdi, rbx",
    "mov rsi, rsp",
    "mov rdx, 16",
    "syscall",
    "test rax, rax",
    "js fail",
    "add rsp, 16",

    // listen
    "mov rax, 50",
    "mov rdi, rbx",
    "mov rsi, 128",
    "syscall",
    "test rax, rax",
    "js fail",

    // 32-slot session ring on stack
    "sub rsp, 256",
    "mov rbp, rsp",
    "xor rcx, rcx",
    "clear_ring:",
    "mov qword ptr [rbp + rcx*8], 0",
    "inc rcx",
    "cmp rcx, 32",
    "jne clear_ring",
    "mov r9d, 0",

    "mov r13, {REQUEST_COUNT}",

    "loop_start:",
    "test r13, r13",
    "jz cleanup",
    "dec r13",

    "mov rax, 43",
    "mov rdi, rbx",
    "xor rsi, rsi",
    "xor rdx, rdx",
    "syscall",
    "test rax, rax",
    "js loop_start",
    "mov r10, rax",

    "mov rax, 0",
    "mov rdi, r10",
    "mov rsi, r12",
    "mov rdx, 1024",
    "syscall",
    "mov r11, rax",

    // scratch 256 -> r8
    "mov rdi, 256",
    "call malloc",
    "mov r8, rax",

    // header table 128 (stays live in session ring)
    "mov rdi, 128",
    "call malloc",
    "push rax",

    // parse state 64
    "mov rdi, 64",
    "call malloc",
    "push rax",

    // response buffer 256 + read len
    "mov rdi, r11",
    "add rdi, 256",
    "call malloc",
    "mov rdx, rax",

    // copy template into heap response
    "xor ecx, ecx",
    "copy_resp:",
    "cmp ecx, 22",
    "jge copy_done",
    "mov al, [r14 + rcx]",
    "mov [rdx + rcx], al",
    "inc ecx",
    "jmp copy_resp",
    "copy_done:",

    // copy up to 64 request bytes into scratch
    "xor ecx, ecx",
    "copy_req:",
    "cmp ecx, 64",
    "jge copy_req_done",
    "cmp ecx, r11d",
    "jge copy_req_done",
    "mov al, [r12 + rcx]",
    "mov [r8 + rcx], al",
    "inc ecx",
    "jmp copy_req",
    "copy_req_done:",

    // write HTTP 200 from heap buffer
    "push rdx",
    "mov rax, 1",
    "mov rdi, r10",
    "mov rsi, rdx",
    "mov rdx, 22",
    "syscall",
    "pop rdx",

    // free scratch
    "mov rdi, r8",
    "call free",
    // free parse state (top of stack)
    "pop rdi",
    "call free",
    // free response
    "mov rdi, rdx",
    "call free",

    // session ring: free previous slot, store header table
    "pop rdi",                 // header table
    "mov ecx, r9d",
    "and ecx, 31",
    "mov rax, [rbp + rcx*8]",
    "test rax, rax",
    "jz ring_store",
    "push rdi",
    "mov rdi, rax",
    "call free",
    "pop rdi",
    "ring_store:",
    "mov [rbp + rcx*8], rdi",
    "inc r9d",

    // close conn
    "mov rax, 3",
    "mov rdi, r10",
    "syscall",

    // every 16 requests: bulk alloc/free 16×96
    "mov rax, r13",
    "and rax, 15",
    "jnz loop_start",
    "mov rcx, 16",
    "bulk:",
    "push rcx",
    "mov rdi, 96",
    "call malloc",
    "pop rcx",
    "push rax",
    "dec rcx",
    "jnz bulk",
    "mov rcx, 16",
    "bulk_free:",
    "pop rdi",
    "push rcx",
    "call free",
    "pop rcx",
    "dec rcx",
    "jnz bulk_free",
    "jmp loop_start",

    "cleanup:",
    "xor ecx, ecx",
    "drain:",
    "mov rdi, [rbp + rcx*8]",
    "test rdi, rdi",
    "jz drain_next",
    "call free",
    "drain_next:",
    "inc ecx",
    "cmp ecx, 32",
    "jne drain",

    "mov rax, 3",
    "mov rdi, rbx",
    "syscall",
    "mov rax, 60",
    "xor rdi, rdi",
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
