//! File-to-RV64 trace runner. Every invocation writes its own durable ledger.
use cranelift_isr::{
    codegen::TARGET,
    ingest::{BinaryInput, Selection},
    ledger::RunLedger,
    pipeline::PreparedTrace,
    riscv_exec::RiscvExecutor,
    GuestContext,
};
use serde_json::json;
use std::{path::PathBuf, time::Instant};

const USAGE: &str = "Usage: isr-run <binary> [--symbol EXACT_NAME | --address VA | --raw]\n  [--set REGISTER=VALUE]... [--ledger-dir DIRECTORY]\n\nAddresses/values accept decimal or 0x hex. Default: file entry; raw base: 0x100000.\nExecutes one bounded trace (at most 32 decoded instructions); does not run a whole program.\nFile execution maps code, context, and interpreter stack only; external data access fails.\nEvery run writes events.jsonl, input-x86.bin, trace-x86.bin, trace.clif, and trace-rv64.bin.\nDefault ledger root: .isr-state/pipeline (or ISR_LEDGER_DIR).";
fn number(text: &str) -> Result<u64, String> {
    if let Some(hex) = text.strip_prefix("0x") {
        u64::from_str_radix(hex, 16)
    } else {
        text.parse()
    }
    .map_err(|e| format!("invalid integer {text:?}: {e}"))
}
fn set_register(ctx: &mut GuestContext, assignment: &str) -> Result<(), String> {
    let (name, value) = assignment
        .split_once('=')
        .ok_or("expected REGISTER=VALUE")?;
    let slot = match name {
        "rax" => &mut ctx.rax,
        "rbx" => &mut ctx.rbx,
        "rcx" => &mut ctx.rcx,
        "rdx" => &mut ctx.rdx,
        "rsi" => &mut ctx.rsi,
        "rdi" => &mut ctx.rdi,
        "rbp" => &mut ctx.rbp,
        "rsp" => &mut ctx.rsp,
        "r8" => &mut ctx.r8,
        "r9" => &mut ctx.r9,
        "r10" => &mut ctx.r10,
        "r11" => &mut ctx.r11,
        "r12" => &mut ctx.r12,
        "r13" => &mut ctx.r13,
        "r14" => &mut ctx.r14,
        "r15" => &mut ctx.r15,
        _ => {
            return Err(format!(
                "unknown register {name:?}; RIP comes from the input selection"
            ))
        }
    };
    *slot = number(value)?;
    Ok(())
}
struct Options {
    path: PathBuf,
    selection: Selection,
    ctx: GuestContext,
}
fn options(args: &[String]) -> Result<Options, String> {
    let mut path = None;
    let mut selection = None;
    let mut ctx = GuestContext {
        rsp: 0x2000,
        ..Default::default()
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let value = |args: &mut std::slice::Iter<'_, String>| {
            args.next()
                .cloned()
                .ok_or_else(|| format!("missing value for {arg}"))
        };
        match arg.as_str() {
            "--symbol" | "--address" | "--raw" => {
                if selection.is_some() {
                    return Err("choose only one of --symbol, --address, --raw".into());
                }
                selection = Some(match arg.as_str() {
                    "--symbol" => Selection::Symbol(value(&mut args)?),
                    "--address" => Selection::Address(number(&value(&mut args)?)?),
                    _ => Selection::Raw { address: 0x100000 },
                });
            }
            "--set" => set_register(&mut ctx, &value(&mut args)?)?,
            "--ledger-dir" => {
                value(&mut args)?;
            }
            s if s.starts_with('-') => return Err(format!("unknown option {s:?}")),
            _ => {
                if path.replace(PathBuf::from(arg)).is_some() {
                    return Err("only one input binary is allowed".into());
                }
            }
        }
    }
    Ok(Options {
        path: path.ok_or(USAGE)?,
        selection: selection.unwrap_or_default(),
        ctx,
    })
}
fn run(args: &[String], ledger: &mut RunLedger) -> Result<(), Box<dyn std::error::Error>> {
    let options = options(args)?;
    ledger.event("stage", json!({"stage":"ingest", "state":"started", "elapsed_ns":0, "path":options.path, "selection":format!("{:?}",options.selection)}))?;
    let start = Instant::now();
    let ingested = (|| -> Result<BinaryInput, Box<dyn std::error::Error>> {
        let data = BinaryInput::read_file(&options.path)?;
        let path = options.path.canonicalize()?;
        ledger.event("input", json!({"path":path, "input_bytes":data.len(), "input_sha256":cranelift_isr::ledger::sha256(&data)}))?;
        let mut input = BinaryInput::parse(&data, &options.selection)?;
        input.metadata["path"] = json!(path);
        Ok(input)
    })();
    let input = match ingested {
        Ok(input) => input,
        Err(error) => {
            ledger.event("stage", json!({"stage":"ingest", "state":"failed", "elapsed_ns":start.elapsed().as_nanos() as u64, "error":error.to_string()}))?;
            return Err(error);
        }
    };
    ledger.event("stage", json!({"stage":"ingest", "state":"completed", "elapsed_ns":start.elapsed().as_nanos() as u64, "details":input.metadata}))?;
    ledger.artifact("input-x86.bin", &input.bytes)?;
    let prepared = PreparedTrace::prepare_bytes(
        &input.bytes,
        input.address,
        0x1000,
        0x2000,
        None,
        &mut |event| ledger.stage(event),
    );
    ledger.check()?;
    let trace = prepared?;
    ledger.artifact("trace-x86.bin", trace.x86())?;
    ledger.artifact("trace.clif", trace.clif().as_bytes())?;
    ledger.artifact("trace-rv64.bin", trace.code())?;
    ledger.event("trace", json!({"summary":trace.summary(), "selected_bytes":input.bytes.len(),
        "unharvested_bytes":input.bytes.len()-trace.x86().len(), "whole_program":false, "memory_mode":"bounded"}))?;
    let mut ctx = GuestContext {
        rip: input.address,
        ..options.ctx
    };
    let result = trace.execute_bounded(&mut ctx, &mut RiscvExecutor::default(), &mut |event| {
        ledger.stage(event)
    });
    ledger.check()?;
    let steps = result?;
    ledger.event(
        "result",
        json!({"status":"completed", "scope":"single_trace", "whole_program":false,
        "trace_executions":1, "riscv_steps":steps, "summary":trace.summary()}),
    )?;
    println!("lifted {} instructions / {} x86 bytes -> {} RV64 code bytes; executed {steps} RV64 instructions; rax=0x{:016x}; exit_rip=0x{:016x}",trace.lifted_instruction_count(),trace.lifted_bytes(),trace.code().len(),ctx.rax,ctx.rip);
    Ok(())
}
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return;
    }
    let root = args
        .windows(2)
        .find(|p| p[0] == "--ledger-dir")
        .map(|p| PathBuf::from(&p[1]))
        .or_else(|| std::env::var_os("ISR_LEDGER_DIR").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(".isr-state/pipeline"));
    let mut ledger = match RunLedger::create(&root) {
        Ok(ledger) => ledger,
        Err(error) => {
            eprintln!("cannot create execution ledger: {error}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "ledger: {}",
        ledger.directory().join("events.jsonl").display()
    );
    let result = ledger.event("started", json!({"args":args, "host_arch":std::env::consts::ARCH,
        "decoder":"iced-x86", "backend":TARGET, "executor":"embedded RV64I", "scope":"single_trace"}))
        .map_err(|e| -> Box<dyn std::error::Error> {e.into()}).and_then(|()| run(&args, &mut ledger));
    if let Err(error) = result {
        if let Err(log_error) = ledger.event(
            "result",
            json!({"status":"failed", "error":error.to_string()}),
        ) {
            eprintln!("ledger write failed: {log_error}");
        }
        eprintln!("pipeline failed: {error}");
        std::process::exit(1);
    }
}
