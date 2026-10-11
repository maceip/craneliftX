use cranelift_isr::{
    codegen::TARGET, ledger::RunLedger, pipeline::PreparedTrace, riscv_exec::RiscvExecutor,
    GuestContext,
};
use serde_json::json;

fn demo(ledger: &mut RunLedger) -> Result<(), Box<dyn std::error::Error>> {
    // mov rax,rdi; add rax,rsi; mov [rdx+8],rax; mov rcx,[rdx+8];
    // xor rcx,0x55; shl rcx,3; shr rcx,1; mov rax,rcx; ret
    static X86_CODE: &[u8] = &[
        0x48, 0x89, 0xf8, 0x48, 0x01, 0xf0, 0x48, 0x89, 0x42, 0x08, 0x48, 0x8b, 0x4a, 0x08, 0x48,
        0x83, 0xf1, 0x55, 0x48, 0xc1, 0xe1, 0x03, 0x48, 0xd1, 0xe9, 0x48, 0x89, 0xc8, 0xc3,
    ];
    let mut data = [0u64; 2];
    let mut stack = [0u8; 128];
    let stack_min = stack.as_mut_ptr() as u64;
    let stack_max = stack_min + stack.len() as u64;
    let start_rip = X86_CODE.as_ptr() as u64;
    let mut ctx = GuestContext {
        rip: start_rip,
        rdi: 100,
        rsi: 23,
        rdx: data.as_mut_ptr() as u64,
        rsp: stack_max,
        ..GuestContext::default()
    };
    ledger.event("ingest", json!({"input_kind":"embedded x86 bytes", "input_bytes":X86_CODE.len(),
        "input_sha256":cranelift_isr::ledger::sha256(X86_CODE), "entry_rip":format!("0x{start_rip:016x}")}))?;
    let prepared =
        PreparedTrace::prepare_bytes(X86_CODE, start_rip, stack_min, stack_max, None, &mut |e| {
            ledger.stage(e)
        });
    ledger.check()?;
    let trace = prepared?;
    ledger.artifact("trace-x86.bin", trace.x86())?;
    ledger.artifact("trace.clif", trace.clif().as_bytes())?;
    ledger.artifact("trace-rv64.bin", trace.code())?;
    // SAFETY: the static trace ends in RET; all its data accesses fit data.
    let result = unsafe {
        trace.execute_observed(&mut ctx, &mut RiscvExecutor::default(), &mut |e| {
            ledger.stage(e)
        })
    };
    ledger.check()?;
    let steps = result?;
    assert_eq!(data[1], 123);
    assert_eq!(ctx.rax, (123u64 ^ 0x55) << 2);
    assert_eq!(ctx.rip, start_rip + X86_CODE.len() as u64 - 1);
    println!("x86-64 -> CLIF -> {TARGET} -> embedded RV64I");
    println!(
        "rax={}, memory[1]={}, exit RIP=trace+{} (RET boundary)",
        ctx.rax,
        data[1],
        ctx.rip - start_rip
    );

    let syscall = [0x0fu8, 0x05];
    ctx.rip = syscall.as_ptr() as u64;
    ledger.event("case", json!({"name":"expected-syscall-rejection"}))?;
    let rejected =
        PreparedTrace::prepare_bytes(&syscall, ctx.rip, stack_min, stack_max, None, &mut |e| {
            ledger.stage(e)
        });
    ledger.check()?;
    let error = rejected.err().ok_or("syscall was unexpectedly accepted")?;
    assert!(matches!(error, cranelift_isr::EngineError::Security(_)));
    println!("syscall rejected during audit, before CLIF/codegen");
    ledger.event(
        "result",
        json!({"status":"completed", "trace_executions":1, "riscv_steps":steps,
        "summary":trace.summary(), "syscall_rejected":true, "oracle":"Rust arithmetic assertions"}),
    )?;
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::var_os("ISR_LEDGER_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| ".isr-state/pipeline".into());
    let mut ledger = RunLedger::create(&root)?;
    eprintln!(
        "ledger: {}",
        ledger.directory().join("events.jsonl").display()
    );
    ledger.event("started", json!({"input":"built-in acceptance demo", "decoder":"iced-x86", "backend":TARGET, "host_arch":std::env::consts::ARCH}))?;
    let result = if std::env::args_os().len() > 1 {
        Err("the default command runs an embedded demo; use cargo run --bin isr-run -- <binary> to ingest a file".into())
    } else {
        demo(&mut ledger)
    };
    if let Err(error) = &result {
        ledger.event(
            "result",
            json!({"status":"failed", "error":error.to_string()}),
        )?;
    }
    result
}
