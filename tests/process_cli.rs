use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "isr-process-{}-{}-{}",
            std::process::id(),
            cranelift_isr::ledger::timestamp_ns(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn run(&self, bytes: &[u8], args: &[&str]) -> (std::process::Output, Vec<Value>) {
        let input = self.0.join("input.elf");
        fs::write(&input, bytes).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_isr-process"))
            .arg(input)
            .arg("--ledger-dir")
            .arg(&self.0)
            .args(args)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        let ledger = stderr
            .lines()
            .find_map(|line| line.strip_prefix("ledger: "))
            .unwrap();
        let events = fs::read_to_string(ledger)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        (output, events)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn elf(code: &[u8], dynamic: bool) -> Vec<u8> {
    let mut image = vec![0u8; 0x100];
    image[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    image[16..18].copy_from_slice(&2u16.to_le_bytes());
    image[18..20].copy_from_slice(&62u16.to_le_bytes());
    image[20..24].copy_from_slice(&1u32.to_le_bytes());
    image[24..32].copy_from_slice(&0x20100u64.to_le_bytes());
    image[32..40].copy_from_slice(&64u64.to_le_bytes());
    for (offset, value) in [
        (52, 64u16),
        (54, 56),
        (56, if dynamic { 2 } else { 1 }),
        (58, 64),
    ] {
        image[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    image[64..68].copy_from_slice(&1u32.to_le_bytes());
    image[68..72].copy_from_slice(&5u32.to_le_bytes());
    image[80..88].copy_from_slice(&0x20000u64.to_le_bytes());
    image[88..96].copy_from_slice(&0x20000u64.to_le_bytes());
    let length = (0x100 + code.len()) as u64;
    image[96..104].copy_from_slice(&length.to_le_bytes());
    image[104..112].copy_from_slice(&length.to_le_bytes());
    image[112..120].copy_from_slice(&4096u64.to_le_bytes());
    if dynamic {
        image[120..124].copy_from_slice(&3u32.to_le_bytes());
    } // PT_INTERP, no section table
    image.extend_from_slice(code);
    image
}
#[test]
fn process_exit_receipt_matches_actual_rv64_execution() {
    let fixture = Fixture::new();
    let (output, events) = fixture.run(
        &elf(&[0x31, 0xff, 0xb8, 60, 0, 0, 0, 0x0f, 0x05], false),
        &[],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    for (i, event) in events.iter().enumerate() {
        assert_eq!(event["seq"], i + 1);
        let group = cranelift_isr::groups::PipelineGroup::ALL
            .into_iter()
            .find(|g| event["group"] == g.id())
            .expect("every event has a known operation group");
        assert_eq!(event["group_order"], group.order());
        assert!(
            event["timestamp_unix_ns"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap()
                > 0
        );
    }
    let result = events.last().unwrap();
    assert_eq!(result["status"], "completed");
    assert!(result["report"]["rv64_steps"].as_u64().unwrap() > 0);
    assert_eq!(result["report"]["x86_instructions_executed"], 3);
    assert_eq!(result["report"]["unique_lifted_bytes"], 9);
    assert_eq!(result["report"]["syscalls"], 1);
    for (stage, group) in [
        ("ingest", "ingest"),
        ("load", "load"),
        ("initialize", "initialize"),
        ("decode", "decode"),
        ("audit", "audit"),
        ("lower", "lower"),
        ("codegen", "codegen"),
        ("artifacts", "artifacts"),
        ("lift", "cache"),
        ("execute", "execute"),
    ] {
        let spans: Vec<_> = events
            .iter()
            .filter(|e| e["type"] == "stage" && e["stage"] == stage)
            .collect();
        assert_eq!(spans.len(), 2, "{stage}");
        assert_eq!(spans[0]["status"], "started");
        assert_eq!(spans[1]["status"], "completed");
        assert!(spans.iter().all(|e| e["group"] == group));
        if ["decode", "audit", "lower", "codegen", "artifacts", "lift"].contains(&stage) {
            assert_eq!(spans[0]["block_id"], spans[1]["block_id"]);
            assert!(spans[0]["block_id"].is_u64());
        }
    }
    let audit = events.iter().find(|e| e["type"] == "security").unwrap();
    assert_eq!(audit["verdict"], "accepted");
    assert_eq!(audit["instruction_count"], 3);
    assert_eq!(audit["checks"][0]["verdict"], "guarded");
    assert!(
        audit["seq"].as_u64().unwrap()
            < events.iter().find(|e| e["stage"] == "lower").unwrap()["seq"]
                .as_u64()
                .unwrap()
    );
}

#[test]
fn process_policy_rejects_before_lowering_with_exact_findings() {
    let fixture = Fixture::new();
    for (code, rule) in [
        (&[0xcd, 0x80][..], "privileged_control"), // int 80h
        (&[0xf4][..], "privileged_control"),       // hlt
        (&[0x48, 0x89, 0xc4][..], "stack_policy"), // mov rsp,rax
        (&[0x48, 0x87, 0xc4][..], "stack_policy"), // xchg rsp,rax
        (
            &[0x48, 0xa1, 0xff, 0xff, 0xff, 0x7, 0, 0, 0, 0][..],
            "memory_policy",
        ),
        (&[0x48, 0xa3, 0, 0, 2, 0, 0, 0, 0, 0][..], "memory_policy"), // write RX image
        (&[0xeb, 0xff][..], "control_flow"),                          // jump into this instruction
        (&[0xe9, 0, 0, 0, 4][..], "control_flow"),                    // jump to unmapped address
    ] {
        let (output, events) = fixture.run(&elf(code, false), &[]);
        assert!(!output.status.success(), "{rule}");
        let audit = events.iter().find(|e| e["type"] == "security").unwrap();
        assert_eq!(audit["verdict"], "rejected", "{rule}: {audit}");
        let check = audit["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["rule"] == rule)
            .unwrap();
        assert_eq!(check["verdict"], "rejected");
        assert_eq!(check["findings"][0]["rip"], "0x20100");
        assert!(!events
            .iter()
            .any(|e| e["stage"] == "lower" || e["stage"] == "codegen"));
        assert_eq!(
            events.iter().find(|e| e["type"] == "execution").unwrap()["counters"]["rv64_steps"],
            0
        );
    }
}

#[test]
fn process_policy_accepts_stack_reads_frames_and_address_calculation() {
    let fixture = Fixture::new();
    // push rbp; mov rbp,rsp; sub rsp,32; cmp rsp,0; lea rax,[rip+64MiB];
    // mov rsp,rbp; pop rbp; exit(0). LEA does not access its unmapped target.
    let mut code = vec![
        0x55, 0x48, 0x89, 0xe5, 0x48, 0x83, 0xec, 32, 0x48, 0x83, 0xfc, 0, 0x48, 0x8d, 0x05, 0, 0,
        0, 4, 0x48, 0x89, 0xec, 0x5d,
    ];
    code.extend([0x31, 0xff, 0xb8, 60, 0, 0, 0, 0x0f, 0x05]);
    let (output, events) = fixture.run(&elf(&code, false), &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(events.last().unwrap()["report"]["syscalls"], 1);
    let audit = events.iter().find(|e| e["type"] == "security").unwrap();
    assert_eq!(audit["checks"][1]["findings"].as_array().unwrap().len(), 2);
    assert_eq!(audit["checks"][2]["verdict"], "passed");
}

#[test]
fn rv64_guard_stops_transient_stack_escape_before_restore_or_syscall() {
    let fixture = Fixture::new();
    // Both adjustments are allowed statically. The first leaves the 1MiB stack;
    // execution must stop there even though the next instruction restores RSP.
    let code = [
        0x48, 0x81, 0xec, 0, 0, 32, 0, 0x48, 0x81, 0xc4, 0, 0, 32, 0, 0x31, 0xff, 0xb8, 60, 0, 0,
        0, 0x0f, 0x05,
    ];
    let (output, events) = fixture.run(&elf(&code, false), &[]);
    assert!(!output.status.success());
    let audits: Vec<_> = events.iter().filter(|e| e["type"] == "security").collect();
    assert_eq!(audits.len(), 2);
    assert_eq!(audits[0]["verdict"], "accepted");
    assert_eq!(audits[1]["phase"], "execute");
    assert_eq!(audits[1]["verdict"], "rejected");
    assert_eq!(audits[1]["rip"], "0x20100");
    let report = &events.iter().find(|e| e["type"] == "execution").unwrap()["counters"];
    assert!(report["rv64_steps"].as_u64().unwrap() > 0);
    assert_eq!(report["syscalls"], 0);
    assert_eq!(report["block_executions"], 0);
}

#[test]
fn known_instruction_boundaries_survive_cache_invalidation() {
    let fixture = Fixture::new();
    // close(-1) creates a syscall boundary; the following jump targets byte 2
    // of the previously compiled MOV. Recompilation must retain that boundary.
    let code = [
        0xb8, 3, 0, 0, 0, 0xbf, 0xff, 0xff, 0xff, 0xff, 0x0f, 0x05, 0xeb, 0xf3,
    ];
    let (output, events) = fixture.run(&elf(&code, false), &["--recompile-every-syscalls", "1"]);
    assert!(!output.status.success());
    assert!(events.iter().any(|e| e["type"] == "cache_invalidation"));
    let rejected = events
        .iter()
        .find(|e| e["type"] == "security" && e["verdict"] == "rejected")
        .unwrap();
    assert_eq!(rejected["checks"][3]["rule"], "control_flow");
    assert_eq!(rejected["checks"][3]["verdict"], "rejected");
    assert_eq!(rejected["checks"][3]["findings"][0]["rip"], "0x2010c");
}
#[test]
fn process_failures_close_their_stage_and_never_fall_back() {
    let fixture = Fixture::new();
    for (code, dynamic, args, stage) in [
        (&[0x0f, 0x0b, 0xf4][..], false, &[][..], "lower"), // UD2 stops before HLT
        (&[0xc3][..], true, &[][..], "load"),
        (
            &[0xb8, 0xe7, 3, 0, 0, 0x0f, 0x05][..],
            false,
            &[][..],
            "syscall_failed",
        ),
        (
            &[0x31, 0xff, 0xb8, 60, 0, 0, 0, 0x0f, 0x05][..],
            false,
            &["--max-rv64-steps", "1"][..],
            "execution",
        ),
    ] {
        let (output, events) = fixture.run(&elf(code, dynamic), args);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(events.last().unwrap()["status"], "failed");
        assert!(
            events
                .iter()
                .any(|e| (e["stage"] == stage && e["status"] == "failed")
                    || (e["type"] == stage
                        && (stage == "syscall_failed" || e["status"] == "failed"))),
            "{stage}: {events:?}"
        );
        assert_eq!(events.iter().filter(|e| e["type"] == "started").count(), 1);
    }
}
