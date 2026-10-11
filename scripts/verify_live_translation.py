#!/usr/bin/env python3
"""Exercise live translation updates with a 16 MiB synthetic static ELF.

1,024 reached blocks perform 31 integer additions apiece before exit. Padding
is deliberately not counted as lifted code. This complements, rather than
replaces, the real Rust network server acceptance.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
import time
import uuid
from live_probe import LiveConsumer, endpoint_from, stalled_consumer

ROOT = Path(__file__).resolve().parents[1]
BLOCKS = 1024
INPUT_BYTES = 16 * 1024 * 1024


def fixture():
    # xor edi,edi; then 31 ADDs + direct JMP per block; finally exit(0).
    block = b"\x48\x83\xc0\x01" * 31 + b"\xe9\x00\x00\x00\x00"
    code = b"\x31\xff\xe9\x00\x00\x00\x00" + block * BLOCKS + b"\xb8\x3c\x00\x00\x00\x0f\x05"
    image = bytearray(INPUT_BYTES)
    image[:7] = b"\x7fELF\x02\x01\x01"
    struct.pack_into("<HHIQQQ", image, 16, 2, 62, 1, 0x20100, 64, 0)
    struct.pack_into("<HHHHHH", image, 52, 64, 56, 1, 64, 0, 0)
    struct.pack_into("<IIQQQQQQ", image, 64, 1, 5, 0, 0x20000, 0x20000,
                     INPUT_BYTES, INPUT_BYTES, 4096)
    image[0x100:0x100 + len(code)] = code
    return image, len(code)


def verify(args, out, report):
    binary = out / "many-blocks.elf"
    image, code_bytes = fixture()
    binary.write_bytes(image)
    runner = args.runner.resolve()
    command = [str(runner), str(binary), "--ledger-dir", str(out / "pipeline"),
               "--events-bind", "127.0.0.1:0", "--max-seconds", "300"]
    report.update({"command": command, "input_bytes": len(image),
                   "input_sha256": hashlib.sha256(image).hexdigest(),
                   "runner_sha256": hashlib.sha256(runner.read_bytes()).hexdigest(),
                   "reachable_source_bytes": code_bytes,
                   "padding_bytes": len(image) - 0x100 - code_bytes})
    stderr_path = out / "runner.stderr"
    with stderr_path.open("w") as stderr:
        process = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=stderr)
        stalled = None
        try:
            endpoint = endpoint_from(stderr_path, process)
            consumer = LiveConsumer(endpoint, process, out / "live-events.jsonl", reconnect_after=100)
            stalled = stalled_consumer(endpoint)
            assert consumer.first_compile.wait(timeout=10), "no early compilation update"
            assert process.poll() is None, "first translation update arrived after process exit"
            assert process.wait(timeout=330) == 0, stderr_path.read_text()
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            if stalled is not None:
                stalled.close()
    ledger = Path(next(line[len("ledger: "):] for line in stderr_path.read_text().splitlines()
                       if line.startswith("ledger: ")))
    events = [json.loads(line) for line in ledger.read_text().splitlines()]
    report["ledger"] = str(ledger)
    report["live_events"] = consumer.finish(events)
    terminal = events[-1]
    assert terminal["status"] == "completed", terminal
    counters = terminal["report"]
    assert counters["compiled_blocks"] == BLOCKS + 2
    assert counters["unique_lifted_bytes"] == code_bytes
    assert counters["unique_lifted_instructions"] == BLOCKS * 32 + 4
    assert counters["rv64_steps"] > 0 and counters["native_x86_instructions_executed"] == 0
    progress = [e for e in events if e["type"] == "compilation_progress"]
    assert [e["counters"]["compiled_blocks"] for e in progress] == list(range(1, BLOCKS + 3))
    assert all(a["counters"]["unique_lifted_bytes"] < b["counters"]["unique_lifted_bytes"]
               for a, b in zip(progress, progress[1:]))
    execution = [r for r in consumer.receipts if r["event"]["type"] == "progress"
                 and int(r["received_unix_ns"]) < int(terminal["timestamp_unix_ns"])]
    assert execution, "no periodic execution update during translation"
    report["live_events"].update({"execution_updates_before_result": len(execution),
                                   "stalled_subscriber_present": True})
    report["runtime_counters"] = counters
    report["status"] = "passed"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runner", type=Path, default=ROOT / "target/release/isr-process")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    out = (args.output or ROOT / ".isr-state/live-translation" / f"{time.time_ns()}-{uuid.uuid4().hex[:8]}").resolve()
    out.mkdir(parents=True, exist_ok=False)
    report = {"status": "failed", "started_unix_ns": str(time.time_ns()),
              "scope": "16 MiB synthetic ELF with 1024 arithmetic blocks plus entry/exit; padding is not translated"}
    try:
        verify(args, out, report)
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
    report["completed_unix_ns"] = str(time.time_ns())
    (out / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"status": report["status"], "report": str(out / "report.json"),
                      "runtime": report.get("runtime_counters"), "error": report.get("error")}, indent=2))
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
