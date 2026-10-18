#!/usr/bin/env python3
"""Build a real Rust x86-64 ELF and require its server to execute through RV64.
No native server is started. All responses must come from isr-process. Failure
produces a failed receipt and nonzero exit; native continuity cannot pass this.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import select
import shutil
import socket
import struct
import subprocess
import sys
import time
import uuid
from live_probe import LiveConsumer, endpoint_from, stalled_consumer

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "tests/fixtures/rv64_network_server.rs"
REQUEST_SIZES = json.loads((ROOT / "webapp/network_workload.json").read_text())["request_sizes"]


def sha(data):
    return hashlib.sha256(data).hexdigest()


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def exact(sock, length):
    data = bytearray()
    while len(data) < length:
        chunk = sock.recv(length - len(data))
        if not chunk:
            raise AssertionError("connection closed before complete response")
        data.extend(chunk)
    return bytes(data)


def oracle(data, sequence):
    output = bytearray()
    value = 0xCBF29CE484222325 ^ sequence
    for index, byte in enumerate(data):
        byte = (byte + index) & 255
        byte = ((byte << 1) | (byte >> 7)) & 255
        byte ^= 0xA5
        output.append(byte)
        value = ((value ^ byte) * 0x100000001B3) & ((1 << 64) - 1)
    return bytes(output), value


def exchange(sock, connection, sequence, size):
    data = bytes((index * 73 + sequence * 29 + connection * 11) & 255 for index in range(size))
    expected, checksum = oracle(data, sequence)
    before = time.time_ns()
    frame = struct.pack("<I", size) + data
    # Separate writes with a delay force partial-header and partial-payload reads.
    sock.sendall(frame[:1])
    time.sleep(0.003)
    sock.sendall(frame[1:4])
    for start in range(4, len(frame), 997):
        sock.sendall(frame[start:start + 997])
    result = exact(sock, size + 12)
    assert result == struct.pack("<IQ", size, checksum) + expected, (connection, sequence, size)
    return {"connection": connection, "sequence": sequence, "payload_bytes": size,
            "started_unix_ns": str(before), "completed_unix_ns": str(time.time_ns()),
            "request_sha256": sha(data), "response_sha256": sha(result),
            "expected_checksum": hex(checksum), "verified": True, "backend": "RV64"}


def ledger_from(stderr):
    line = next(line for line in stderr.splitlines() if line.startswith("ledger: "))
    path = Path(line[len("ledger: "):])
    events = [json.loads(line) for line in path.read_text().splitlines()]
    assert [e["seq"] for e in events] == list(range(1, len(events) + 1))
    assert all(isinstance(e["timestamp_unix_ns"], str) and int(e["timestamp_unix_ns"]) > 0 for e in events)
    assert all(a["run_elapsed_ns"] <= b["run_elapsed_ns"] for a, b in zip(events, events[1:]))
    return path, events


def verify(args, out, report):
    runner = args.runner.resolve()
    rustc = args.rustc or (subprocess.check_output(["rustup", "which", "--toolchain", "stable", "rustc"], text=True).strip()
                           if shutil.which("rustup") else shutil.which("rustc"))
    binary = out / "rust-network-server-x86_64.elf"
    command = [str(rustc), "--edition=2021", "--target", "x86_64-unknown-linux-gnu",
               "-C", "opt-level=2", "-C", "panic=abort", "-C", "relocation-model=static",
               "-C", f"linker={shutil.which('ld.lld') or 'rust-lld'}", "-C", "link-arg=-nostdlib", "-C", "link-arg=--static",
               "-C", "no-vectorize-loops", "-C", "no-vectorize-slp", str(SOURCE), "-o", str(binary)]
    build = subprocess.run(command, capture_output=True, text=True, timeout=90)
    save(out / "build.json", {"command": command, "returncode": build.returncode,
         "stdout": build.stdout, "stderr": build.stderr})
    assert build.returncode == 0, build.stderr
    report["input"] = {"source": str(SOURCE), "source_sha256": sha(SOURCE.read_bytes()),
                       "binary": str(binary), "binary_sha256": sha(binary.read_bytes()),
                       "runner": str(runner), "runner_sha256": sha(runner.read_bytes()),
                       "compiler": subprocess.check_output([str(rustc), "--version"], text=True).strip(),
                       "profile": "static Linux x86-64, freestanding Rust, scalar codegen"}
    command = [str(runner), str(binary), "--ledger-dir", str(out / "pipeline"),
               "--recompile-every-syscalls", "25", "--max-seconds", "120",
               "--backend", getattr(args, "backend", "rv64"),
               "--events-bind", "127.0.0.1:0"]
    report["command"] = command
    responses = report["responses"] = []
    stderr_path = out / "runner.stderr"
    with stderr_path.open("w") as stderr:
        process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=stderr)
        stalled = None
        try:
            endpoint = endpoint_from(stderr_path, process)
            consumer = LiveConsumer(endpoint, process, out / "live-events.jsonl")
            stalled = stalled_consumer(endpoint)
            deadline = time.monotonic() + 45
            ready = bytearray()
            while len(ready) < 6:
                assert select.select([process.stdout], [], [], max(0, deadline - time.monotonic()))[0], "no guest readiness"
                part = os.read(process.stdout.fileno(), 6 - len(ready))
                assert part, "guest exited before readiness: " + stderr_path.read_text()
                ready.extend(part)
            assert ready[:4] == b"RV64", ready
            port = struct.unpack("!H", ready[4:])[0]
            report["guest_ready_record_hex"] = ready.hex()
            with socket.create_connection(("127.0.0.1", port), timeout=30) as stream:
                peer, local, fd = stream.getpeername(), stream.getsockname(), stream.fileno()
                connected = time.time_ns()
                assert consumer.first_compile.wait(timeout=5), "no live compilation events"
                for seq, size in enumerate(REQUEST_SIZES[0], 1):
                    responses.append(exchange(stream, 0, seq, size))
                    if seq == 6:
                        time.sleep(0.2)
                    assert (peer, local, fd) == (stream.getpeername(), stream.getsockname(), stream.fileno())
                stream.shutdown(socket.SHUT_WR)
                assert stream.recv(1) == b"", "guest did not honor clean EOF"
                disconnected = time.time_ns()
                report["persistent_connection"] = {"status": "passed", "reconnects": 0,
                    "peer": peer, "local": local, "client_fd": fd,
                    "connected_unix_ns": str(connected), "disconnected_unix_ns": str(disconnected),
                    "verified_responses": len(REQUEST_SIZES[0]), "request_sequence_preserved": True}
            with socket.create_connection(("127.0.0.1", port), timeout=30) as stream:
                for seq, size in enumerate(REQUEST_SIZES[1], 1):
                    responses.append(exchange(stream, 1, seq, size))
                stream.shutdown(socket.SHUT_WR)
                assert stream.recv(1) == b""
            with socket.create_connection(("127.0.0.1", port), timeout=30) as stream:
                # Oversize input is rejected by the translated Rust handler.
                stream.sendall(struct.pack("<I", 65537))
                assert stream.recv(1) == b"", "oversize frame was not rejected"
            report["oversize_frame_rejected"] = True
            assert process.wait(timeout=30) == 0, stderr_path.read_text()
            report["runner_exit_code"] = process.returncode
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            process.stdout.close()
            if stalled is not None:
                stalled.close()
    ledger, events = ledger_from(stderr_path.read_text())
    report["live_events"] = consumer.finish(events)
    received_while_connected = [r for r in consumer.receipts
        if r["event"]["type"] == "compilation_progress"
        and connected < int(r["received_unix_ns"]) < disconnected]
    assert received_while_connected, "no live translation updates while guest connection was open"
    report["live_events"].update({"compilation_updates_while_guest_connected": len(received_while_connected),
        "stalled_subscriber_present": True})
    live_audits = [r for r in consumer.receipts if r["event"]["type"] == "security"
        and connected < int(r["received_unix_ns"]) < disconnected]
    assert live_audits, "no live inspection findings while guest connection was open"
    report["live_events"]["inspection_updates_while_guest_connected"] = len(live_audits)
    report["ledger"] = str(ledger)
    terminal = events[-1]
    assert terminal["type"] == "result" and terminal["status"] == "completed", terminal
    counters = terminal["report"]
    assert counters["exit_code"] == 0 and counters["accepted_connections"] == 3
    backend = getattr(args, "backend", "rv64")
    if backend == "x86_64":
        assert counters["native_x86_instructions_executed"] > 0
    else:
        assert counters["rv64_steps"] > 0 and counters["native_x86_instructions_executed"] == 0
    assert counters["network_read_bytes"] == sum(r["payload_bytes"] + 4 for r in responses) + 4
    assert counters["network_written_bytes"] == sum(r["payload_bytes"] + 12 for r in responses)
    totals = [block for e in events if e["type"] == "block_execution_totals" for block in e["blocks"]]
    assert sum(b["rv64_steps"] for b in totals) == counters["rv64_steps"]
    assert sum(b["executions"] for b in totals) == counters["block_executions"]
    audits = [e for e in events if e["type"] == "security"]
    assert len(audits) == counters["compiled_blocks"]
    assert all(e["verdict"] == "accepted" and e["policy"] == "process-block-v1"
        and 0 < e["instruction_count"] <= 32 for e in audits)
    lower_starts = {e["block_id"]: e["seq"] for e in events
        if e["type"] == "stage" and e["stage"] == "lower" and e["status"] == "started"}
    assert all(e["seq"] < lower_starts[e["block_id"]] for e in audits)
    report["instruction_policy"] = {"policy": "process-block-v1", "blocks_inspected": len(audits),
        "all_accepted": True, "before_lowering": True}
    artifacts = [e for e in events if e["type"] == "artifact"]
    assert len(artifacts) == counters["compiled_blocks"] * 3
    unique = {}
    for event in events:
        if event["type"] == "harvest":
            for instruction in event["instructions"]:
                encoded = bytes.fromhex(instruction["bytes"])
                assert instruction["ip"] not in unique or unique[instruction["ip"]] == encoded
                unique[instruction["ip"]] = encoded
    assert len(unique) == counters["unique_lifted_instructions"]
    assert sum(map(len, unique.values())) == counters["unique_lifted_bytes"]
    short_reads = [e for e in events if e["type"] == "syscall_completed" and e["detail"]["name"] == "read"
                   and 0 < e["detail"]["result"] < int(e["detail"]["args"][2], 16)]
    assert short_reads, "fragmentation did not exercise a partial guest read"
    report["observed_partial_reads"] = len(short_reads)
    for stage in ("ingest", "load", "initialize", "decode", "audit", "lift", "lower", "codegen", "artifacts", "execute"):
        starts = sum(e["type"] == "stage" and e["stage"] == stage and e["status"] == "started" for e in events)
        ends = sum(e["type"] == "stage" and e["stage"] == stage and e["status"] == "completed" for e in events)
        assert starts == ends and starts > 0, stage
    for artifact in artifacts:
        data = Path(artifact["path"]).read_bytes()
        assert len(data) == artifact["bytes"] and sha(data) == artifact["sha256"]
    cold_lifts = [e for e in events if e["type"] == "stage" and e["stage"] == "lift" and e["status"] == "completed"
                  and connected < int(e["timestamp_unix_ns"]) < disconnected]
    invalidations = [e for e in events if e["type"] == "cache_invalidation"
                     and connected < int(e["timestamp_unix_ns"]) < disconnected and e["open_descriptors"] == 2]
    assert invalidations and cold_lifts, "no recompilation while persistent connection was open"
    assert any(int(e["timestamp_unix_ns"]) > int(responses[0]["completed_unix_ns"]) for e in invalidations)
    assert any(int(e["timestamp_unix_ns"]) < int(responses[11]["started_unix_ns"]) for e in invalidations)
    report["persistent_connection"].update({"cache_invalidations_while_open": len(invalidations),
        "blocks_compiled_while_open": len(cold_lifts)})
    report["runtime_counters"] = counters
    report["artifact_hashes_verified"] = len(artifacts)
    report["verified_payload_bytes"] = sum(r["payload_bytes"] for r in responses)
    report["native_fallback"] = False
    # Corrupt actual ELF entry with UD2: same runner must fail, without starting
    # a native server or manufacturing a response from the Python oracle.
    damaged = bytearray(binary.read_bytes())
    entry, phoff = struct.unpack_from("<QQ", damaged, 24)
    phsize, phnum = struct.unpack_from("<HH", damaged, 54)
    patched = False
    for at in range(phoff, phoff + phsize * phnum, phsize):
        kind, _, offset, address, _, filesz = struct.unpack_from("<IIQQQQ", damaged, at)
        if kind == 1 and address <= entry < address + filesz - 1:
            location = offset + entry - address
            damaged[location:location + 2] = b"\x0f\x0b"
            patched = True
            break
    assert patched
    broken = out / "unsupported-entry.elf"
    broken.write_bytes(damaged)
    failure = subprocess.run([str(runner), str(broken), "--ledger-dir", str(out / "negative")], capture_output=True, timeout=30)
    assert failure.returncode != 0 and failure.stdout == b""
    failure_path, rejected = ledger_from(failure.stderr.decode())
    assert rejected[-1]["type"] == "result" and rejected[-1]["status"] == "failed"
    assert "unsupported" in rejected[-1]["error"]
    assert any(e["type"] == "stage" and e["stage"] == "lower" and e["status"] == "failed" for e in rejected)
    assert any(e["type"] == "stage" and e["stage"] == "lift" and e["status"] == "failed" for e in rejected)
    report["unsupported_instruction_fails_closed"] = {"status": "passed", "ledger": str(failure_path)}
    report["status"] = "passed"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runner", type=Path, default=ROOT / "target/release/isr-process")
    parser.add_argument("--rustc")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--backend", default="rv64", choices=["rv64", "x86_64"])
    args = parser.parse_args()
    out = (args.output or ROOT / ".isr-state/rv64-network" / f"{time.time_ns()}-{uuid.uuid4().hex[:8]}").resolve()
    out.mkdir(parents=True, exist_ok=False)
    report = {"status": "failed", "started_unix_ns": str(time.time_ns()),
              "scope": "entire freestanding Rust server: Linux entry, accept loop, framing, transformation and state through RV64; host kernel bridges socket syscalls"}
    try:
        verify(args, out, report)
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
    report["completed_unix_ns"] = str(time.time_ns())
    save(out / "report.json", report)
    print(json.dumps({"status": report["status"], "report": str(out / "report.json"),
                      "verified_responses": len(report.get("responses", [])),
                      "payload_bytes": report.get("verified_payload_bytes"),
                      "runtime": report.get("runtime_counters"), "error": report.get("error")}, indent=2))
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
