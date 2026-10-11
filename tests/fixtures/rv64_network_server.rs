//! Real freestanding Linux x86-64 TCP server. All application logic is ordinary
//! Rust. Assembly is confined to the Linux entry and syscall ABI, as libc would
//! provide it. There are no engine imports, lifting callbacks, or native handlers.
#![no_std]
#![no_main]
use core::arch::{asm, global_asm};

global_asm!(".global _start", "_start:", "xor rbp, rbp", "call server_main", "ud2");

#[inline(always)]
unsafe fn syscall(n: usize, a: usize, b: usize, c: usize) -> isize {
    let result: isize;
    unsafe {
        asm!("syscall", inlateout("rax") n => result, in("rdi") a, in("rsi") b,
             in("rdx") c, lateout("rcx") _, lateout("r11") _, options(nostack));
    }
    result
}
fn exit(code: usize) -> ! {
    unsafe { syscall(60, code, 0, 0); }
    loop { core::hint::spin_loop(); }
}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! { exit(101) }

fn read_exact(fd: usize, bytes: &mut [u8]) -> bool {
    let mut at = 0;
    while at < bytes.len() {
        let n = unsafe { syscall(0, fd, bytes.as_mut_ptr().add(at) as usize, bytes.len() - at) };
        if n == -4 { continue; }
        if n <= 0 { return false; }
        at += n as usize;
    }
    true
}
fn write_all(fd: usize, bytes: &[u8]) -> bool {
    let mut at = 0;
    while at < bytes.len() {
        let n = unsafe { syscall(1, fd, bytes.as_ptr().add(at) as usize, bytes.len() - at) };
        if n == -4 { continue; }
        if n <= 0 { return false; }
        at += n as usize;
    }
    true
}

#[unsafe(no_mangle)]
#[inline(never)]
pub fn transform_payload(payload: &mut [u8], request: u64) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64 ^ request;
    for (index, byte) in payload.iter_mut().enumerate() {
        *byte = byte.wrapping_add(index as u8).rotate_left(1) ^ 0xa5;
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3);
    }
    hash
}

#[unsafe(no_mangle)]
#[inline(never)]
pub fn handle_connection(fd: usize) -> u64 {
    // Fixed BSS buffer avoids requiring a heap allocator in this freestanding
    // server. Only one connection is handled at a time.
    static mut BUFFER: [u8; 65536] = [0; 65536];
    let buffer = unsafe { &mut *core::ptr::addr_of_mut!(BUFFER) };
    let mut request = 0u64;
    loop {
        let mut header = [0u8; 4];
        if !read_exact(fd, &mut header) { break; }
        let len = u32::from_le_bytes(header) as usize;
        if len > buffer.len() { break; }
        let payload = &mut buffer[..len];
        if !read_exact(fd, payload) { break; }
        request += 1;
        let hash = transform_payload(payload, request);
        // Including the request sequence in the hash proves server state survives
        // lazy compilation, cache eviction, calls, and multiple requests.
        if !write_all(fd, &header) || !write_all(fd, &hash.to_le_bytes()) || !write_all(fd, payload) { break; }
    }
    request
}

#[unsafe(no_mangle)]
pub extern "C" fn server_main() -> ! {
    let listener = unsafe { syscall(41, 2, 1, 0) }; // AF_INET, SOCK_STREAM
    if listener < 0 { exit(1); }
    let mut address = [0u8; 16];
    address[0] = 2; // Linux sockaddr_in, port 0, 127.0.0.1
    address[4] = 127;
    address[7] = 1;
    if unsafe { syscall(49, listener as usize, address.as_ptr() as usize, 16) } < 0 { exit(2); }
    if unsafe { syscall(50, listener as usize, 16, 0) } < 0 { exit(3); }
    let mut address_len = 16u32;
    if unsafe { syscall(51, listener as usize, address.as_mut_ptr() as usize, &mut address_len as *mut u32 as usize) } < 0 { exit(4); }
    // Binary readiness record: magic, then the actual two network-order port bytes.
    if !write_all(1, b"RV64") || !write_all(1, &address[2..4]) { exit(5); }
    for _ in 0..3 {
        let fd = unsafe { syscall(43, listener as usize, 0, 0) };
        if fd < 0 { exit(6); }
        handle_connection(fd as usize);
        if unsafe { syscall(3, fd as usize, 0, 0) } < 0 { exit(7); }
    }
    unsafe { syscall(3, listener as usize, 0, 0); }
    exit(0)
}
