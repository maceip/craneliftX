//! Independent execution oracle: runs the same live x86 function both ways.
//! On Apple Silicon: cargo test --target x86_64-apple-darwin --test native_x86
#![cfg(all(target_arch = "x86_64", target_family = "unix"))]

use cranelift_isr::{
    audit::audit_trace, codegen::compile_to_riscv64, harvest::harvest_trace, lower::lower_to_clif,
    riscv_exec::execute_riscv, try_randomize_and_execute, GuestContext,
};

macro_rules! arithmetic_fixture {
    ($name:literal) => {
        core::arch::global_asm!(concat!(
            ".text\n.p2align 4\n.globl ",
            $name,
            "\n",
            $name,
            ":\n",
            "mov rax, rdi\nadd rax, rsi\nsub rax, 17\nxor rax, rdx\n",
            "and rax, -16\nor rax, 5\nshl rax, 3\nshr rax, 1\n",
            "mov [rcx + 8], rax\nmov rax, [rcx + 8]\nret\n"
        ));
    };
}

macro_rules! partial_fixture {
    ($name:literal) => {
        core::arch::global_asm!(concat!(
            ".text\n.p2align 4\n.globl ",
            $name,
            "\n",
            $name,
            ":\n",
            "mov rax, rdi\nmov ah, 0x91\nadd al, sil\nsub ax, 0x183\n",
            "xor eax, esi\nor ah, 0x12\nand ax, 0xff01\nshl al, 8\n",
            "shr ax, 16\nadd eax, 0x80000000\nsub rax, rdx\n",
            "and rax, rsi\nor rax, rdi\nxor rax, -127\nshl eax, 32\n",
            "mov [rcx + 1], ax\nmov ah, [rcx + 2]\nret\n"
        ));
    };
}

#[cfg(target_vendor = "apple")]
arithmetic_fixture!("_isr_arithmetic_fixture");
#[cfg(not(target_vendor = "apple"))]
arithmetic_fixture!("isr_arithmetic_fixture");
#[cfg(target_vendor = "apple")]
partial_fixture!("_isr_partial_fixture");
#[cfg(not(target_vendor = "apple"))]
partial_fixture!("isr_partial_fixture");

unsafe extern "C" {
    fn isr_arithmetic_fixture(a: u64, b: u64, c: u64, data: *mut u8) -> u64;
    fn isr_partial_fixture(a: u64, b: u64, c: u64, data: *mut u8) -> u64;
}

type NativeFunction = unsafe extern "C" fn(u64, u64, u64, *mut u8) -> u64;

fn compare_fixture(function: NativeFunction) {
    let rip = function as usize as u64;
    // SAFETY: these assembly fixtures are readable, complete, and end in RET.
    let (trace, next_rip) = unsafe { harvest_trace(rip, 32) }.unwrap();
    audit_trace(&trace, 1, u64::MAX).unwrap();
    let clif = lower_to_clif(&trace, next_rip).unwrap();
    let code = compile_to_riscv64(&clif).unwrap();
    let edges = [
        0,
        1,
        u64::MAX,
        1 << 63,
        (1 << 63) - 1,
        0xffff_ffff,
        0x8000_0000,
        0x1234_5678_9abc_def0,
    ];
    let mut state = 0x193a6754c03219afu64;
    for index in 0..512 {
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let a = if index < edges.len() {
            edges[index]
        } else {
            next()
        };
        let b = next();
        let c = next();
        let mut native_memory = [0xabu8; 24];
        let mut lifted_memory = native_memory;
        // SAFETY: the x86 fixture obeys SysV ABI, only writes its 24-byte data object,
        // and is being called on an x86-64 process (including Rosetta on macOS).
        let native_result = unsafe { function(a, b, c, native_memory.as_mut_ptr()) };
        let mut ctx = GuestContext {
            rax: next(),
            rbx: next(),
            rcx: lifted_memory.as_mut_ptr() as u64,
            rdx: c,
            rsi: b,
            rdi: a,
            rbp: next(),
            rsp: 0x1800,
            r8: next(),
            r9: next(),
            r10: next(),
            r11: next(),
            r12: next(),
            r13: next(),
            r14: next(),
            r15: next(),
            rip,
        };
        let mut expected = ctx;
        expected.rax = native_result;
        expected.rip = next_rip;
        if index == 0 {
            // Exercise the complete public live-pointer driver as well as the
            // compile-once path used to make the remaining differential cases fast.
            // SAFETY: readable fixture and valid exclusive data pointers.
            unsafe { try_randomize_and_execute(&mut ctx, 0x1000, 0x2000) }.unwrap();
        } else {
            // SAFETY: same compiled fixture, with valid exclusive data pointers.
            unsafe { execute_riscv(&code, &mut ctx) }.unwrap();
        }
        assert_eq!(ctx, expected, "register mismatch at vector {index}");
        assert_eq!(
            lifted_memory, native_memory,
            "memory mismatch at vector {index}"
        );
    }
}

#[test]
fn live_arithmetic_function_matches_x86_for_512_vectors() {
    compare_fixture(isr_arithmetic_fixture);
}

#[test]
fn live_partial_register_function_matches_x86_for_512_vectors() {
    compare_fixture(isr_partial_fixture);
}
