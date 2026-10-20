#!/usr/bin/env python3
"""Exercise the same Nginx ELF directly or through ISR, preserving worker pools.

Every measured response must have the expected status, bytes, and SHA-256.
A failed startup or response produces a failed receipt, never a throughput row.
Use --runner only for translated execution. Linux /proc proves native workers;
translated runs additionally require guest lifecycle evidence in the ISR ledger.
"""
from __future__ import annotations

import argparse
import concurrent.futures
import hashlib
import http.client
import json
import os
from pathlib import Path
import platform
import signal
import socket
import statistics
import subprocess
import time


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(args: argparse.Namespace) -> int:
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    payload = bytes(range(256)) * 1024
    (root / "payload.bin").write_bytes(payload)
    expected_hash = hashlib.sha256(payload).hexdigest()
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    config = root / "nginx.conf"
    config.write_text(f'''daemon off;
master_process on;
worker_processes 2;
thread_pool default threads=4 max_queue=1024;
pid "{root}/nginx.pid";
error_log stderr notice;
events {{ worker_connections 128; }}
http {{
    access_log off;
    sendfile off;
    aio threads=default;
    server {{
        listen 127.0.0.1:{port};
        add_header X-Worker $pid;
        location / {{ root "{root}"; }}
    }}
}}
''')
    binary = args.binary.resolve()
    guest_args = ["-p", str(root) + "/", "-c", str(config)]
    command = [str(binary), *guest_args]
    if args.runner:
        command = [str(args.runner.resolve()), str(binary), "--ledger-dir",
                   str(root / "ledger"), "--max-seconds", str(args.timeout),
                   "--", *guest_args]
    report = {
        "status": "failed", "mode": "translated" if args.runner else "native",
        "command": command, "host": platform.uname()._asdict(),
        "binary_sha256": digest(binary), "config_sha256": digest(config),
        "payload_sha256": expected_hash, "payload_bytes": len(payload),
        "requests": args.requests, "concurrency": args.concurrency,
    }
    if args.runner:
        report["runner_sha256"] = digest(args.runner)
    process = None
    log = (root / "stderr.log").open("wb")

    def request() -> tuple[float, str]:
        started = time.perf_counter()
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
        try:
            conn.request("GET", "/payload.bin")
            response = conn.getresponse()
            body = response.read()
            if response.status != 200 or body != payload:
                raise RuntimeError(f"incorrect response: status={response.status}, bytes={len(body)}")
            if hashlib.sha256(body).hexdigest() != expected_hash:
                raise RuntimeError("response SHA-256 mismatch")
            worker = response.getheader("X-Worker")
            if not worker:
                raise RuntimeError("missing worker identity")
            return (time.perf_counter() - started) * 1000, worker
        finally:
            conn.close()

    try:
        process = subprocess.Popen(command, stdout=log, stderr=log, start_new_session=True)
        report["host_pid"] = process.pid
        deadline = time.monotonic() + args.timeout
        while True:
            if process.poll() is not None:
                raise RuntimeError(f"runner exited before readiness: {process.returncode}")
            try:
                request()
                break
            except (OSError, http.client.HTTPException):
                if time.monotonic() >= deadline:
                    raise RuntimeError("readiness deadline exceeded")
                time.sleep(0.1)

        if not args.runner:
            children = Path(f"/proc/{process.pid}/task/{process.pid}/children")
            worker_pids = [int(p) for p in children.read_text().split()]
            counts = {str(p): len(list(Path(f"/proc/{p}/task").iterdir())) for p in worker_pids}
            if len(counts) != 2 or any(n < 5 for n in counts.values()):
                raise RuntimeError(f"expected 2 workers with 4 pool threads each: {counts}")
            report["native_worker_task_counts"] = counts

        with concurrent.futures.ThreadPoolExecutor(args.concurrency) as pool:
            list(pool.map(lambda _: request(), range(args.concurrency * 2)))
            start = time.perf_counter()
            samples = list(pool.map(lambda _: request(), range(args.requests)))
            elapsed = time.perf_counter() - start
        if process.poll() is not None:
            raise RuntimeError("runner exited during benchmark")
        workers = sorted({worker for _, worker in samples})
        if len(workers) != 2:
            raise RuntimeError(f"responses did not exercise both workers: {workers}")
        if args.runner:
            events = [json.loads(line) for p in (root / "ledger").glob("*/events.jsonl")
                      for line in p.read_text().splitlines()]
            starts = [e for e in events if e.get("type") == "started"]
            if not starts or not all(starts[0]["options"]["hardening"].values()):
                raise RuntimeError("full hardening was not requested")
            # Serving HTTP alone does not prove thread or process semantics.
            created = [e for e in events if e.get("type") == "thread_started"]
            if len(created) < 8:
                raise RuntimeError("missing translated thread lifecycle evidence")
        latency = sorted(ms for ms, _ in samples)
        report.update(status="passed", verified_responses=len(samples),
                      response_workers=workers, seconds=elapsed,
                      requests_per_second=len(samples) / elapsed,
                      latency_ms={"median": statistics.median(latency),
                                  "p95": latency[int((len(latency) - 1) * .95)]})
    except Exception as error:
        report["error"] = str(error)
    finally:
        if process is not None:
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
            report["shutdown_returncode"] = process.returncode
        log.close()
        (root / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--runner", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--requests", type=int, default=120)
    parser.add_argument("--concurrency", type=int, default=4)
    parser.add_argument("--timeout", type=int, default=120)
    options = parser.parse_args()
    if options.requests < 2 or options.concurrency < 1 or not 1 <= options.timeout <= 3600:
        parser.error("invalid request count, concurrency, or timeout")
    raise SystemExit(run(options))
