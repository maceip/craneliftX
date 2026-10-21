#!/usr/bin/env python3
"""Compile with the on-host libc toolchains; require exact native/translated outcomes."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
EXPECTED = "THREAD_SYNC_OK atomic=1600 cas=1600 mutex=4000 exchange=4000 tls=4 joins=4"

def main():
    p = argparse.ArgumentParser()
    p.add_argument("--runner", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--cc", action="append", help="Repeat for each compiler; defaults to gcc and musl-gcc")
    args = p.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    results = []
    for compiler in args.cc or ["gcc", "musl-gcc"]:
        name = Path(compiler).name
        binary = root / name
        subprocess.run([compiler, "-static", "-O1", "-pthread", "-fno-pie", "-no-pie", "-fno-tree-vectorize",
                        str(ROOT / "tests/fixtures/thread_synchronization.c"), "-o", str(binary)], check=True)
        for mode in ["native", "block", "access", "adaptive"]:
            command = [str(binary)] if mode == "native" else [str(args.runner.resolve()), str(binary),
                "--thread-sync", mode, "--max-seconds", "180", "--max-rv64-steps", "2000000000",
                "--ledger-dir", str(root / (name + "-" + mode))]
            started = time.monotonic()
            done = subprocess.run(command, capture_output=True, text=True, timeout=200)
            (root / (name + "-" + mode + ".stderr")).write_text(done.stderr)
            events = []
            if mode != "native":
                for ledger in (root / (name + "-" + mode)).glob("*/events.jsonl"):
                    events.extend(json.loads(line) for line in ledger.read_text().splitlines())
            reports = [e["report"] for e in events if e.get("type") == "result"]
            starts = [e for e in events if e.get("type") == "thread_started"]
            totals = [e["counters"] for e in events if e.get("type") == "thread_execution_totals"]
            concurrent = sum(r.get("native_concurrent_blocks", 0) for r in reports + totals)
            passed = done.returncode == 0 and done.stdout.strip() == EXPECTED
            if mode != "native":
                passed &= bool(reports) and reports[-1]["exit_code"] == 0 and len(starts) == 4
                passed &= concurrent > 0 if mode in ("access", "adaptive") else concurrent == 0
                passed &= any(e.get("type") == "started" and all(e["options"]["hardening"].values()) for e in events)
            result = {"compiler": compiler, "mode": mode, "seconds": time.monotonic() - started,
                      "returncode": done.returncode, "stdout": done.stdout.strip(), "thread_starts": len(starts),
                      "concurrent_blocks": concurrent, "passed": bool(passed), "command": command,
                      "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest()}
            results.append(result)
            (root / "results.json").write_text(json.dumps({"runner_sha256": hashlib.sha256(args.runner.read_bytes()).hexdigest(), "results": results}, indent=2) + "\n")
            print(json.dumps(result), flush=True)
            if not passed:
                raise RuntimeError("thread synchronization verification failed; inspect " + str(root))

if __name__ == "__main__":
    main()
