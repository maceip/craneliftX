//! Cranelift drop runner.
//!
//! The wasm module is compiled with Cranelift. The default target is Pulley,
//! Cranelift's portable ISA, executed by the Pulley interpreter (the emulator
//! this pipeline starts on). A lifted remill function is an ordinary export;
//! guest memory, when the function needs it, is the wasm linear memory.

use std::env;
use std::fs;
use std::process::ExitCode;

use wasmtime::{Config, Engine, Instance, Module, Store, Val};

struct MemOp {
    kind: MemKind,
    bytes: Vec<u8>,
}

enum MemKind {
    /// Write `bytes` and pass the offset as this argument.
    Store,
    /// Reserve a zeroed slot, pass its offset, and read it back after the call.
    Out { ty: String },
}

struct Request {
    wasm: String,
    func: String,
    /// Immediate arguments, in order. Memory operands are filled later.
    imm_args: Vec<Option<i64>>,
    mem_ops: Vec<(usize, MemOp)>,
    expected: Option<i64>,
    expected_outs: Vec<Option<u64>>,
}

fn usage() -> ! {
    eprintln!(
        "usage:\n  \
         ceremony-wasm <wasm> [func] [a] [b] [expected]\n  \
         ceremony-wasm --wasm <wasm> --func <name> \\\n      \
         [--arg <i64>] [--mem <hex>] [--out u32|u64] [--expect <i64>] [--expect-out <u64>]"
    );
    std::process::exit(2);
}

fn parse_args() -> Request {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|a| a.starts_with("--")) {
        return parse_flags(&args);
    }
    let wasm = args.first().cloned().unwrap_or_else(|| {
        format!("{}/../ceremony/sign.wasm", env!("CARGO_MANIFEST_DIR"))
    });
    let func = args.get(1).cloned().unwrap_or_else(|| "ceremony_op".into());
    let a: i64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3);
    let b: i64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(4);
    let expected = args.get(4).and_then(|s| s.parse().ok());
    Request {
        wasm,
        func,
        imm_args: vec![Some(a), Some(b)],
        mem_ops: Vec::new(),
        expected,
        expected_outs: Vec::new(),
    }
}

fn parse_flags(args: &[String]) -> Request {
    let mut wasm = None;
    let mut func = None;
    let mut imm_args: Vec<Option<i64>> = Vec::new();
    let mut mem_ops = Vec::new();
    let mut expected = None;
    let mut expected_outs = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let flag = &args[i];
        let val = args.get(i + 1).unwrap_or_else(|| usage());
        match flag.as_str() {
            "--wasm" => wasm = Some(val.clone()),
            "--func" => func = Some(val.clone()),
            "--arg" => {
                let n: i64 = val.parse().unwrap_or_else(|_| usage());
                imm_args.push(Some(n));
            }
            "--mem" => {
                let bytes = decode_hex(val);
                let idx = imm_args.len();
                imm_args.push(None);
                mem_ops.push((
                    idx,
                    MemOp {
                        kind: MemKind::Store,
                        bytes,
                    },
                ));
            }
            "--out" => {
                let ty = val.to_ascii_lowercase();
                if ty != "u32" && ty != "u64" {
                    usage();
                }
                let width = if ty == "u32" { 4 } else { 8 };
                let idx = imm_args.len();
                imm_args.push(None);
                mem_ops.push((
                    idx,
                    MemOp {
                        kind: MemKind::Out { ty },
                        bytes: vec![0; width],
                    },
                ));
            }
            "--expect" => {
                expected = Some(val.parse().unwrap_or_else(|_| usage()));
            }
            "--expect-out" => {
                expected_outs.push(Some(val.parse().unwrap_or_else(|_| usage())));
            }
            _ => usage(),
        }
        i += 2;
    }
    Request {
        wasm: wasm.unwrap_or_else(|| usage()),
        func: func.unwrap_or_else(|| usage()),
        imm_args,
        mem_ops,
        expected,
        expected_outs,
    }
}

fn decode_hex(s: &str) -> Vec<u8> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        eprintln!("memory hex has an odd number of digits");
        std::process::exit(2);
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap_or_else(|_| usage()))
        .collect()
}

fn align_up(n: usize, align: usize) -> usize {
    (n + align - 1) & !(align - 1)
}

fn run(req: &Request) -> Result<(), Box<dyn std::error::Error>> {
    let wasm = fs::read(&req.wasm)?;
    let mut cfg = Config::new();
    // DROP: Cranelift emits Pulley bytecode. The Pulley interpreter executes it.
    cfg.target("pulley64")?;
    cfg.cranelift_opt_level(wasmtime::OptLevel::Speed);
    let engine = Engine::new(&cfg)?;
    let module = Module::new(&engine, &wasm[..])?;
    let mut store = Store::new(&engine, ());
    let instance = Instance::new(&mut store, &module, &[])?;

    let mut args: Vec<i64> = req.imm_args.iter().map(|a| a.unwrap_or(0)).collect();
    let mut out_slots: Vec<(usize, String, usize)> = Vec::new();

    if !req.mem_ops.is_empty() {
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or("wasm module does not export memory")?;
        let base: usize = if let Some(top_fn) = instance.get_func(&mut store, "guest_stack_top") {
            let mut results = [Val::I64(0)];
            top_fn.call(&mut store, &[], &mut results)?;
            match results[0] {
                Val::I64(v) => v as usize,
                other => return Err(format!("guest_stack_top returned {other:?}").into()),
            }
        } else {
            32768
        };
        // Data sits just above the guest stack top, inside guest_stack's upper half.
        let mut cursor = align_up(base.saturating_add(256), 8);
        for (idx, op) in &req.mem_ops {
            cursor = align_up(cursor, 8);
            let end = cursor + op.bytes.len();
            let pages_needed = (end + 65535) / 65536;
            let have = memory.size(&store) as usize;
            if pages_needed > have {
                memory.grow(&mut store, (pages_needed - have) as u64)?;
            }
            memory.write(&mut store, cursor, &op.bytes)?;
            args[*idx] = cursor as i64;
            if let MemKind::Out { ty } = &op.kind {
                out_slots.push((*idx, ty.clone(), cursor));
            }
            cursor = end;
        }
    }

    let func = instance
        .get_func(&mut store, &req.func)
        .ok_or_else(|| format!("export `{}` not found", req.func))?;
    let params: Vec<Val> = args.iter().copied().map(Val::I64).collect();
    let mut results = [Val::I64(0)];
    func.call(&mut store, &params, &mut results)?;
    let result = match results[0] {
        Val::I64(v) => v,
        other => return Err(format!("function returned {other:?}, expected i64").into()),
    };

    let mut outs = Vec::new();
    if !out_slots.is_empty() {
        let memory = instance.get_memory(&mut store, "memory").unwrap();
        for (_idx, ty, offset) in &out_slots {
            let width = if ty == "u32" { 4 } else { 8 };
            let mut buf = vec![0u8; width];
            memory.read(&store, *offset, &mut buf)?;
            let value = if width == 4 {
                u32::from_le_bytes(buf.try_into().unwrap()) as u64
            } else {
                u64::from_le_bytes(buf.try_into().unwrap())
            };
            outs.push((ty.clone(), value));
        }
    }

    println!("  artifact : {}", req.wasm);
    println!("  target   : pulley64 (Cranelift portable ISA, interpreter)");
    let arg_s = args
        .iter()
        .map(|a| a.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    for (ty, value) in &outs {
        println!("  OUT {ty} {value}");
    }
    match req.expected {
        Some(e) => {
            println!("  {}({}) = {}  (expected {e})", req.func, arg_s, result);
            let outs_ok = req.expected_outs.iter().enumerate().all(|(i, exp)| {
                exp.map(|v| outs.get(i).map(|(_, got)| *got == v).unwrap_or(false))
                    .unwrap_or(true)
            });
            let ok = result == e && outs_ok;
            println!("  RESULT   : {}", if ok { "OK" } else { "WRONG" });
            if !ok {
                return Err("lifted result did not match the expected value".into());
            }
        }
        None => {
            println!("  {}({}) = {result}", req.func, arg_s);
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let req = parse_args();
    println!(
        "[DROP] wasm -> Cranelift -> Pulley : {}  func={}",
        req.wasm, req.func
    );
    if let Err(e) = run(&req) {
        eprintln!("  ERROR: {e}");
        return ExitCode::from(1);
    }
    println!();
    ExitCode::SUCCESS
}
