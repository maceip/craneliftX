//! A real Rust executable containing a deliberately supported x86-64 leaf.
//! A naked function fixes its instruction/ABI contract across compiler versions;
//! the pipeline does not yet support general compiler-generated prologues.
#[unsafe(no_mangle)]
#[unsafe(naked)]
pub extern "C" fn pipeline_entry(_a: u64, _b: u64) -> u64 {
    core::arch::naked_asm!(
        "mov rax, rdi",
        "add rax, rsi",
        "xor rax, 85",
        "shl rax, 2",
        "ret",
    );
}
fn main() {
    let value = pipeline_entry(std::hint::black_box(35), std::hint::black_box(7));
    assert_eq!(value, (42u64 ^ 85) << 2);
    println!("native Rust binary: rax={value}");
}
