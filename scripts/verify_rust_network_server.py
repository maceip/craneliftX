#!/usr/bin/env python3
"""Exercise a real Rust TCP service and gate translated connection continuity.

This is an acceptance probe, not a mock-success test. Exit 1 means the server
cannot yet run through the lifting pipeline, even when native networking passes.
Only the standard libraries of Python and Rust are required for the fixture.
"""
import argparse
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import socket
import subprocess
import sys
import threading
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "tests/fixtures/rust_network_server.rs"
MASK = (1 << 64) - 1
SIZES = [[2, 64, 8192, 65536], [3, 65, 16384, 262144], [4, 127, 32768, 1048576]]


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def reference(payload):
    result = bytearray(len(payload))
    checksum = 0xCBF29CE484222325
    for index, byte in enumerate(payload):
        value = (byte + index) & 255
        value = ((value << 1) | (value >> 7)) & 255
        value ^= 0xA5
        result[index] = value
        checksum = ((checksum ^ value) * 0x100000001B3) & MASK
    return bytes(result), checksum


def receive(stream, length):
    result = bytearray()
    while len(result) < length:
        chunk = stream.recv(length - len(result))
        if not chunk:
            raise RuntimeError("connection closed before a complete response")
        result.extend(chunk)
    return bytes(result)


def connect(address):
    stream = socket.create_connection(address, timeout=15)
    stream.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    return stream


def request(stream, payload, phase):
    expected, checksum = reference(payload)
    before = time.time_ns()
    started = time.monotonic_ns()
    # Split the header so the server must handle real TCP stream framing.
    header = len(payload).to_bytes(4, "little")
    stream.sendall(header[:2])
    stream.sendall(header[2:] + payload)
    length = int.from_bytes(receive(stream, 4), "little")
    observed_checksum = int.from_bytes(receive(stream, 8), "little")
    if length != len(payload):
        raise RuntimeError(f"response length mismatch: {length} != {len(payload)}")
    observed = receive(stream, length)
    if observed != expected or observed_checksum != checksum:
        raise RuntimeError("native TCP response disagrees with the independent Python oracle")
    return {"phase": phase, "timestamp_unix_ns": str(before),
            "start_monotonic_ns": started, "end_monotonic_ns": time.monotonic_ns(),
            "bytes": len(payload), "request_sha256": hashlib.sha256(payload).hexdigest(),
            "response_sha256": hashlib.sha256(observed).hexdigest(),
            "checksum": f"0x{checksum:016x}", "verified": True}


def load_connection(address, index):
    with connect(address) as stream:
        identity = {"local": stream.getsockname(), "peer": stream.getpeername()}
        records = []
        for size in SIZES[index]:
            payload = bytes((i * 17 + index * 31 + size) & 255 for i in range(size))
            records.append(request(stream, payload, "concurrent_native_load"))
        return {"socket": identity, "requests": records}


def lift_case(runner, binary, symbol, output):
    command = [str(runner), str(binary), "--ledger-dir", str(output / "pipeline")]
    if symbol:
        command += ["--symbol", symbol]
    start = time.monotonic_ns()
    process = subprocess.run(command, capture_output=True, text=True, timeout=30)
    end = time.monotonic_ns()
    path = next((line[len("ledger: "):] for line in process.stderr.splitlines()
                 if line.startswith("ledger: ")), None)
    if not path:
        raise RuntimeError(f"runner emitted no ledger: {process.stderr}")
    ledger = Path(path)
    events = [json.loads(line) for line in ledger.read_text().splitlines()]
    if [e["seq"] for e in events] != list(range(1, len(events) + 1)):
        raise RuntimeError("pipeline ledger has a sequence gap")
    inputs = [e for e in events if e["type"] == "input"]
    if len(inputs) != 1 or inputs[0]["input_sha256"] != digest(binary):
        raise RuntimeError("pipeline did not ingest the actual server binary")
    harvest = next((e for e in events if e.get("stage") == "harvest"
                    and e["state"] == "completed"), None)
    failure = next((e for e in events if e.get("state") == "failed"), None)
    failed_instruction = None
    if failure and harvest:
        location = re.search(r"rip: (\d+)", failure.get("error", ""))
        if location:
            failed_instruction = next((i for i in harvest["details"]["instructions"]
                                       if int(i["rip"], 16) == int(location[1])), None)
    case = {"selection": symbol or "file entry", "command": command,
            "start_monotonic_ns": start, "end_monotonic_ns": end,
            "exit_code": process.returncode, "stdout": process.stdout, "stderr": process.stderr,
            "ledger": str(ledger), "ledger_sha256": digest(ledger), "result": events[-1],
            "failed_stage": failure["stage"] if failure else None,
            "failed_instruction": failed_instruction,
            "decoded_instructions": harvest["details"]["decoded_instructions"] if harvest else 0,
            "decoded_bytes": harvest["details"]["decoded_bytes"] if harvest else 0,
            "lowering_reached": any(e.get("stage") == "lower" for e in events),
            "execution_reached": any(e.get("stage") == "execute" for e in events)}
    (output / ("lift-" + (symbol or "entry") + ".json")).write_text(json.dumps(case, indent=2) + "\n")
    return case


def exercise(runner, binary, output, target):
    stdout_path, stderr_path = output / "native-events.jsonl", output / "native-stderr.txt"
    persistent_records, heartbeat_errors = [], []
    stop = threading.Event()
    server = None
    heartbeat = None
    persistent = None
    try:
        with stdout_path.open("x") as stdout, stderr_path.open("x") as stderr:
            server = subprocess.Popen([str(binary), "4"], stdout=stdout, stderr=stderr)
        deadline = time.monotonic() + 15
        while True:
            text = stdout_path.read_text()
            if "\n" in text:
                ready = json.loads(text.splitlines()[0])
                break
            if server.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError(f"server never became ready: {stderr_path.read_text()}")
            time.sleep(0.01)
        if ready["type"] != "listening" or ready["host_arch"] != "x86_64":
            raise RuntimeError(f"unexpected server identity: {ready}")
        host, port = ready["address"].rsplit(":", 1)
        address = (host, int(port))
        persistent = connect(address)
        identity = {"local": persistent.getsockname(), "peer": persistent.getpeername(),
                    "client_fd": persistent.fileno()}
        persistent_records.append(request(persistent, b"before lifting: same connection", "before_lift"))

        def heartbeat_loop():
            sequence = 0
            try:
                while not stop.is_set():
                    payload = (f"while lifting request {sequence}: ".encode() + bytes(range(128)))
                    persistent_records.append(request(persistent, payload, "during_lift"))
                    sequence += 1
                    stop.wait(0.01)
            except Exception as error:
                heartbeat_errors.append(str(error))

        with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
            loads = [pool.submit(load_connection, address, i) for i in range(3)]
            heartbeat = threading.Thread(target=heartbeat_loop, name="persistent-tcp-heartbeat")
            heartbeat.start()
            prefix = "_" if "apple" in target else ""
            cases = [lift_case(runner, binary, symbol, output) for symbol in
                     [None, prefix + "network_server_run", prefix + "handle_connection",
                      prefix + "transform_payload"]]
            stop.set()
            heartbeat.join(timeout=20)
            if heartbeat.is_alive() or heartbeat_errors:
                raise RuntimeError(f"persistent connection failed during lifting: {heartbeat_errors}")
            persistent_records.append(request(persistent, b"after lifting: still same connection", "after_lift"))
            after = {"local": persistent.getsockname(), "peer": persistent.getpeername(),
                     "client_fd": persistent.fileno()}
            if after != identity:
                raise RuntimeError("persistent socket identity changed")
            persistent.shutdown(socket.SHUT_WR)
            persistent.close()
            persistent = None
            load_results = [future.result(timeout=20) for future in loads]
        if server.wait(timeout=20) != 0:
            raise RuntimeError(f"native server failed: {stderr_path.read_text()}")
        native_events = [json.loads(line) for line in stdout_path.read_text().splitlines()]
        if [e["seq"] for e in native_events] != list(range(1, len(native_events) + 1)):
            raise RuntimeError("native event ledger has a sequence gap")
        all_requests = persistent_records + [r for load in load_results for r in load["requests"]]
        completed = native_events[-1]
        if (completed["type"] != "completed" or completed["requests"] != len(all_requests)
                or completed["bytes"] != sum(r["bytes"] for r in all_requests)):
            raise RuntimeError("server/client accounting mismatch")
        # The persistent connection was opened before any other client, so the
        # server's accept index is zero throughout. Require independent server
        # evidence of the same request count on that connection as well.
        server_persistent = [e for e in native_events if e.get("connection") == 0]
        if len(server_persistent) != len(persistent_records):
            raise RuntimeError("server-side persistent connection accounting mismatch")
        for case in cases:
            case["concurrent_verified_responses"] = sum(
                r["start_monotonic_ns"] < case["end_monotonic_ns"]
                and r["end_monotonic_ns"] > case["start_monotonic_ns"]
                for r in persistent_records if r["phase"] == "during_lift")
        continuity = all(case["concurrent_verified_responses"] > 0 for case in cases)
        rejected = [case for case in cases if case["exit_code"] != 0]
        result = {
            "status": "failed" if rejected else "not_verified",
            "native_network": {"status": "passed", "process_id": server.pid,
                               "execution": "x86-64 executable; Rosetta on ARM macOS" if platform.machine() == "arm64" else "native x86-64",
                               "connections": 4, "requests": len(all_requests),
                               "payload_bytes": completed["bytes"], "mismatches": 0,
                               "events": str(stdout_path), "events_sha256": digest(stdout_path)},
            "connection_continuity": {"status": "passed" if continuity else "not_verified",
                                      "socket_before": identity, "socket_after": after,
                                      "reconnects": 0, "server_connection_id": 0,
                                      "requests": persistent_records,
                                      "execution_backend": "native Rust throughout"},
            "load_clients": load_results, "lift_attempts": cases,
            "translated_network": {
                "status": "failed" if rejected else "not_verified", "verified_rv64_network_requests": 0,
                "socket_handoff_occurred": False,
                "reason": "The pipeline did not run the server or a request handler through RV64. Native TCP continuity alone is not translated-runtime acceptance."},
        }
        return result
    finally:
        stop.set()
        if persistent is not None:
            persistent.close()
        if server is not None and server.poll() is None:
            server.kill()
            server.wait(timeout=5)
        if heartbeat is not None:
            heartbeat.join(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runner", type=Path, default=ROOT / "target/release/isr-run")
    parser.add_argument("--target", default="x86_64-apple-darwin" if sys.platform == "darwin" else "x86_64-unknown-linux-gnu")
    parser.add_argument("--rustc", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    output = args.output or ROOT / ".isr-state/network-server" / f"{time.time_ns()}-{uuid.uuid4().hex[:8]}"
    output.mkdir(parents=True, exist_ok=False)
    output = output.resolve()
    report = {"v": 1, "started_unix_ns": str(time.time_ns()), "status": "failed",
              "source": str(SOURCE), "source_sha256": digest(SOURCE), "target": args.target,
              "acceptance": "same TCP connection before/during/after actual lifted request execution"}
    print(f"Evidence: {output}", flush=True)
    try:
        runner = args.runner.resolve(strict=True)
        if not os.access(runner, os.X_OK):
            raise RuntimeError("runner is not executable; build --release --bin isr-run first")
        report["runner"] = {"path": str(runner), "sha256": digest(runner)}
        rustc = args.rustc
        if rustc is None:
            if shutil.which("rustup"):
                rustc = Path(subprocess.check_output(["rustup", "which", "--toolchain", "stable", "rustc"], text=True).strip())
            else:
                rustc = Path(shutil.which("rustc") or "rustc")
        report["rustc"] = subprocess.check_output([str(rustc), "--version"], text=True).strip()
        binary = output / "rust-network-server"
        command = [str(rustc), "--edition=2021", "--target", args.target,
                   "-C", "opt-level=2", "-C", "debuginfo=1", "-C", "strip=none",
                   str(SOURCE), "-o", str(binary)]
        compile_result = subprocess.run(command, capture_output=True, text=True, timeout=120)
        (output / "build.stdout").write_text(compile_result.stdout)
        (output / "build.stderr").write_text(compile_result.stderr)
        report["build"] = {"command": command, "exit_code": compile_result.returncode}
        if compile_result.returncode:
            raise RuntimeError(f"server build failed: {compile_result.stderr}")
        report["binary"] = {"path": str(binary), "bytes": binary.stat().st_size, "sha256": digest(binary)}
        report.update(exercise(runner, binary, output, args.target))
        print(f"Native TCP: {report['native_network']['requests']} verified requests / {report['native_network']['payload_bytes']} bytes", flush=True)
        print(f"Persistent native connection: {report['connection_continuity']['status']}; reconnects=0", flush=True)
        for case in report["lift_attempts"]:
            print(f"Lift {case['selection']}: exit={case['exit_code']}, stage={case['failed_stage']}, instruction={case['failed_instruction']}", flush=True)
        print("FAIL: no server requests executed through RV64; no native-to-lifted handoff occurred.", flush=True)
    except Exception as error:
        report["verification_error"] = str(error)
        print(f"FAIL: {error}", file=sys.stderr, flush=True)
    finally:
        report["finished_unix_ns"] = str(time.time_ns())
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(f"Report: {output / 'report.json'}", flush=True)
    # A future single-trace success still needs actual translated TCP evidence.
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
