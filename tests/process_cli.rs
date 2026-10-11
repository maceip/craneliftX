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
        if ["decode", "lower", "codegen", "artifacts", "lift"].contains(&stage) {
            assert_eq!(spans[0]["block_id"], spans[1]["block_id"]);
            assert!(spans[0]["block_id"].is_u64());
        }
    }
}
#[test]
fn process_failures_close_their_stage_and_never_fall_back() {
    let fixture = Fixture::new();
    for (code, dynamic, args, stage) in [
        (&[0x0f, 0x0b][..], false, &[][..], "lower"),
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
