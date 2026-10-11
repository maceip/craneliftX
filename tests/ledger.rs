use cranelift_isr::{
    ingest::{BinaryInput, Selection},
    ledger::{sha256, RunLedger},
    pipeline::PreparedTrace,
    riscv_exec::RiscvExecutor,
    EngineError, GuestContext,
};
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "isr-ledger-{}-{}-{}",
            std::process::id(),
            cranelift_isr::ledger::timestamp_ns(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn run_bytes(bytes: &[u8], args: &[&str]) -> (Temp, Output, Vec<Value>) {
    let temp = Temp::new();
    let input = temp.0.join("input.bin");
    fs::write(&input, bytes).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_isr-run"))
        .arg(&input)
        .arg("--raw")
        .args(args)
        .arg("--ledger-dir")
        .arg(temp.0.join("runs"))
        .output()
        .unwrap();
    let run = fs::read_dir(temp.0.join("runs"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let events = read_events(&run.join("events.jsonl"));
    (temp, output, events)
}
fn read_events(path: &std::path::Path) -> Vec<Value> {
    let events: Vec<Value> = fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let id = events[0]["run_id"].clone();
    let mut last_elapsed = 0;
    for (i, event) in events.iter().enumerate() {
        assert_eq!(event["seq"], i + 1);
        assert_eq!(event["run_id"], id);
        assert!(
            event["timestamp_unix_ns"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap()
                > 0
        );
        let elapsed = event["run_elapsed_ns"].as_u64().unwrap();
        assert!(elapsed >= last_elapsed);
        last_elapsed = elapsed;
    }
    events
}
#[test]
fn cli_ledger_matches_executed_bytes_counts_state_and_artifacts() {
    // add rax,7; ret; trailing data must not count as lifted.
    let bytes = [0x48, 0x83, 0xc0, 7, 0xc3, 0xcc, 0xcc];
    let (_temp, output, events) = run_bytes(&bytes, &["--set", "rax=35"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result = events.last().unwrap();
    assert_eq!(result["status"], "completed");
    assert_eq!(result["summary"]["decoded_instructions"], 2);
    assert_eq!(result["summary"]["lifted_instructions"], 1);
    assert_eq!(result["summary"]["lifted_bytes"], 4);
    assert_eq!(result["summary"]["boundary"], "Ret");
    assert!(result["riscv_steps"].as_u64().unwrap() > 0);
    let trace = events.iter().find(|e| e["type"] == "trace").unwrap();
    assert_eq!(trace["unharvested_bytes"], 2);
    assert_eq!(trace["whole_program"], false);
    let executed = events
        .iter()
        .find(|e| e["stage"] == "execute" && e["state"] == "completed")
        .unwrap();
    assert_eq!(
        executed["details"]["registers_after"]["rax"],
        "0x000000000000002a"
    );
    assert_eq!(
        executed["details"]["registers_after"]["rip"],
        "0x0000000000100004"
    );
    let artifacts: Vec<_> = events.iter().filter(|e| e["type"] == "artifact").collect();
    assert_eq!(artifacts.len(), 4);
    for event in artifacts {
        let data = fs::read(event["path"].as_str().unwrap()).unwrap();
        assert_eq!(event["bytes"], data.len());
        assert_eq!(event["sha256"], sha256(&data));
    }
    let stages: Vec<_> = events.iter().filter(|e| e["type"] == "stage").collect();
    for pair in stages.as_chunks::<2>().0 {
        assert_eq!(pair[0]["state"], "started");
        assert_eq!(pair[1]["state"], "completed");
        assert_eq!(pair[0]["stage"], pair[1]["stage"]);
    }
}
#[test]
fn every_failure_is_terminal_and_stops_the_pipeline_at_its_real_stage() {
    for (bytes, stage, forbidden) in [
        (&[][..], "ingest", "harvest"),
        (&[0x48, 0x83][..], "harvest", "audit"),
        (&[0x48, 0x89, 0x07, 0x0f, 0x05][..], "audit", "lower"),
        (&[0x48, 0x0f, 0xaf, 0xc0, 0xc3][..], "lower", "codegen"),
        (&[0x48, 0x8b, 0x07, 0xc3][..], "execute", "never"),
    ] {
        let (_temp, output, events) = run_bytes(bytes, &["--set", "rdi=0xdeadbeef"]);
        assert!(!output.status.success());
        assert_eq!(events.last().unwrap()["status"], "failed");
        assert_eq!(events.last().unwrap()["type"], "result");
        assert!(
            events
                .iter()
                .any(|e| e["stage"] == stage && e["state"] == "failed"),
            "{events:?}"
        );
        assert!(!events.iter().any(|e| e["stage"] == forbidden));
        if stage == "execute" {
            assert!(events.last().unwrap()["error"]
                .as_str()
                .unwrap()
                .contains("unmapped guest memory"));
        }
    }
}
#[test]
fn bounded_execution_rejects_unmapped_pointers_and_context_padding() {
    let code = [0x48, 0x8b, 0x07, 0xc3]; // mov rax,[rdi]; ret
    let trace =
        PreparedTrace::prepare_bytes(&code, 0x100000, 0x1000, 0x2000, None, &mut |_| {}).unwrap();
    let mut ctx = GuestContext {
        rip: 0x100000,
        ..Default::default()
    };
    for address in [
        0,
        0xdeadbeef,
        u64::MAX - 3,
        &ctx as *const GuestContext as u64 + 136,
    ] {
        ctx.rdi = address;
        let before = ctx;
        let mut events = vec![];
        assert!(trace
            .execute_bounded(&mut ctx, &mut RiscvExecutor::default(), &mut |e| events
                .push(e))
            .is_err());
        assert_eq!(events.last().unwrap().state, "failed");
        assert_eq!(ctx, before);
    }
    ctx.rip += 1;
    let mut events = vec![];
    assert!(trace
        .execute_bounded(&mut ctx, &mut RiscvExecutor::default(), &mut |e| events
            .push(e))
        .is_err());
    assert_eq!(events.len(), 2);
    assert!(events[1].error.as_ref().unwrap().contains("RIP"));
}
#[test]
fn bounded_preparation_rejects_empty_overflow_and_reports_instruction_limit() {
    for (bytes, address) in [(&[][..], 0), (&[0x48, 0x89, 0xc0][..], u64::MAX)] {
        assert!(matches!(
            PreparedTrace::prepare_bytes(bytes, address, 1, 2, None, &mut |_| {}),
            Err(EngineError::Harvest { .. })
        ));
    }
    let bytes = [0x48, 0x89, 0xc0].repeat(40);
    let trace = PreparedTrace::prepare_bytes(&bytes, 0x100000, 1, 2, None, &mut |_| {}).unwrap();
    assert_eq!(trace.summary()["stop_reason"], "instruction_limit");
    assert_eq!(trace.instruction_count(), 32);
    assert_eq!(trace.lifted_instruction_count(), 32);
    assert_eq!(trace.x86().len(), 96);
}
#[test]
fn ingest_and_cli_reject_ambiguous_or_malformed_requests() {
    assert!(BinaryInput::parse(b"not a binary", &Selection::Entry).is_err());
    assert!(BinaryInput::parse(&[0xc3], &Selection::Raw { address: u64::MAX }).is_err());
    for args in [
        &["--symbol", "foo"][..],
        &["--typo"][..],
        &["--set", "rip=0"][..],
        &["--set", "rax=-1"][..],
    ] {
        let (_temp, output, events) = run_bytes(&[0xc3], args);
        assert!(!output.status.success());
        assert_eq!(events.last().unwrap()["status"], "failed");
        assert!(!events.iter().any(|e| e["stage"] == "execute"));
    }
}
#[test]
fn ledger_never_overwrites_an_existing_artifact() {
    let temp = Temp::new();
    let mut ledger = RunLedger::create(&temp.0).unwrap();
    ledger.artifact("trace.bin", b"original").unwrap();
    assert!(ledger.artifact("trace.bin", b"replacement").is_err());
    assert!(ledger.artifact("../outside", b"bad").is_err());
    assert_eq!(
        fs::read(ledger.directory().join("trace.bin")).unwrap(),
        b"original"
    );
}

#[test]
fn standalone_worker_persists_the_same_sequenced_events_as_stdout() {
    let temp = Temp::new();
    let output = Command::new(env!("CARGO_BIN_EXE_isr-demo"))
        .args(["--tier", "baby", "--seed", "42"])
        .env("ISR_LEDGER_DIR", &temp.0)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let streamed: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let path = fs::read_dir(&temp.0)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let saved = read_events(&path.join("events.jsonl"));
    assert_eq!(streamed.len(), saved.len());
    for (index, (stream, saved)) in streamed.iter().zip(&saved).enumerate() {
        assert_eq!(stream["worker_seq"], index + 1);
        for (key, value) in stream.as_object().unwrap() {
            assert_eq!(&saved[key], value);
        }
    }
    assert_eq!(saved.last().unwrap()["type"], "result");
    assert_eq!(saved.last().unwrap()["unique_lifted_bytes"], 64);
}

#[test]
fn demo_entry_points_never_silently_ignore_binary_paths_or_bad_options() {
    let temp = Temp::new();
    for args in [
        &["some-program"][..],
        &["--tier"][..],
        &["--tier", "baby", "--tier", "large"][..],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_isr-demo"))
            .args(args)
            .env("ISR_LEDGER_DIR", &temp.0)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let events: Vec<Value> = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "error");
    }
    let output = Command::new(env!("CARGO_BIN_EXE_cranelift-isr"))
        .arg("some-program")
        .env("ISR_LEDGER_DIR", &temp.0)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("isr-run"));
}

#[test]
#[cfg(all(target_arch = "x86_64", any(target_os = "macos", target_os = "linux")))]
fn linked_rust_binary_symbol_runs_through_file_ingestion_and_matches_native() {
    let temp = Temp::new();
    let binary = temp.0.join("rust-input");
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let compile = Command::new(compiler)
        .args([
            "--target",
            if cfg!(target_os = "macos") {
                "x86_64-apple-darwin"
            } else {
                "x86_64-unknown-linux-gnu"
            },
        ])
        .args([
            "--edition=2021",
            "-C",
            "opt-level=2",
            "-C",
            "force-frame-pointers=no",
        ])
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/pipeline_input.rs"
        ))
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let native = Command::new(&binary).output().unwrap();
    assert!(native.status.success());
    assert!(String::from_utf8_lossy(&native.stdout).contains("rax=508"));
    let symbol = if cfg!(target_vendor = "apple") {
        "_pipeline_entry"
    } else {
        "pipeline_entry"
    };
    let input = BinaryInput::read(&binary, &Selection::Symbol(symbol.into())).unwrap();
    assert_eq!(
        input.metadata["input_sha256"],
        sha256(&fs::read(&binary).unwrap())
    );
    let trace = PreparedTrace::prepare_bytes(
        &input.bytes,
        input.address,
        0x1000,
        0x2000,
        None,
        &mut |_| {},
    )
    .unwrap();
    let mut ctx = GuestContext {
        rip: input.address,
        rdi: 35,
        rsi: 7,
        ..Default::default()
    };
    trace
        .execute_bounded(&mut ctx, &mut RiscvExecutor::default(), &mut |_| {})
        .unwrap();
    assert_eq!(ctx.rax, 508);
    assert_eq!(trace.lifted_instruction_count(), 4);
    let by_address = BinaryInput::read(&binary, &Selection::Address(input.address)).unwrap();
    assert!(by_address.bytes.starts_with(trace.x86()));
    assert!(BinaryInput::read(&binary, &Selection::Symbol("missing_symbol".into())).is_err());
}
