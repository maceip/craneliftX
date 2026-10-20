#!/usr/bin/env python3
"""Benchmark real multi-worker, multi-threaded Nginx (glibc & musl) on Linux x86-64:
Native execution vs LOCKSTEP memory-hardened execution, sampling real /proc Memory (MB)
and CPU (%) time-series over time as Nginx allocates request pools and serves load.
"""
from __future__ import annotations

import argparse
import concurrent.futures
import hashlib
import http.client
import json
import os
import platform
import shutil
import signal
import socket
import statistics
import subprocess
import sys
import threading
import time
from pathlib import Path

CARGO_BIN = str(Path.home() / ".cargo" / "bin")
if CARGO_BIN not in os.environ.get("PATH", ""):
    os.environ["PATH"] = f"{CARGO_BIN}:{os.environ.get('PATH', '')}"

ROOT = Path(__file__).resolve().parents[1]
ISR_PROCESS = ROOT / "target" / "release" / "isr-process"

GLIBC_CANDIDATES = [
    Path("/home/devuser/nginx-last-mile.EEFJLS/install/sbin/nginx"),
    ROOT / "tests" / "fixtures" / "nginx_glibc_static",
]
MUSL_CANDIDATES = [
    Path("/home/devuser/nginx-last-mile.EEFJLS/musl-install/sbin/nginx"),
    ROOT / "tests" / "fixtures" / "nginx_musl_static",
]


def find_nginx(candidates: list[Path]) -> Path:
    for p in candidates:
        if p.exists():
            return p.resolve()
    raise FileNotFoundError(f"Could not find Nginx binary in {candidates}")


def proc_group_pids(leader_pid: int) -> list[int]:
    """Return all PIDs belonging to the process group or child tree of leader_pid."""
    pids = {leader_pid}
    try:
        pgid = os.getpgid(leader_pid)
    except OSError:
        pgid = leader_pid

    proc_root = Path("/proc")
    if not proc_root.exists():
        return list(pids)

    for entry in proc_root.iterdir():
        if not entry.name.isdigit():
            continue
        pid = int(entry.name)
        try:
            stat_text = (entry / "stat").read_text()
            rparen = stat_text.rfind(")")
            fields = stat_text[rparen + 2 :].split()
            # fields[1] is ppid, fields[2] is pgrp
            ppid = int(fields[1])
            pgrp = int(fields[2])
            if pgrp == pgid or ppid in pids:
                pids.add(pid)
        except OSError:
            continue
    return sorted(pids)


def sample_proc_tree(leader_pid: int) -> tuple[float, int]:
    """Return (total_rss_mb, total_cpu_jiffies) across all processes in the tree."""
    pids = proc_group_pids(leader_pid)
    total_rss_kb = 0
    total_jiffies = 0
    for pid in pids:
        try:
            status = Path(f"/proc/{pid}/status").read_text()
            for line in status.splitlines():
                if line.startswith("VmRSS:"):
                    parts = line.split()
                    total_rss_kb += int(parts[1])
                    break
        except OSError:
            pass
        try:
            stat_text = Path(f"/proc/{pid}/stat").read_text()
            rparen = stat_text.rfind(")")
            fields = stat_text[rparen + 2 :].split()
            utime = int(fields[11])
            stime = int(fields[12])
            total_jiffies += utime + stime
        except OSError:
            pass
    return total_rss_kb / 1024.0, total_jiffies


def run_nginx_config(
    label: str,
    short_label: str,
    libc_name: str,
    binary: Path,
    mode: str,
    extra_flags: list[str],
    duration_sec: float = 2.0,
    concurrency: int = 8,
) -> dict:
    shm = Path("/dev/shm")
    bench_base = (shm / "lockstep-nginx-bench") if shm.exists() else (ROOT / ".isr-state" / "nginx-bench")
    work_dir = bench_base / short_label.lower().replace(" ", "-").replace("+", "plus").replace("(", "").replace(")", "")
    if work_dir.exists():
        shutil.rmtree(work_dir)
    work_dir.mkdir(parents=True, exist_ok=True)
    for sub in ("client_body_temp", "proxy_temp", "fastcgi_temp", "uwsgi_temp", "scgi_temp", "logs"):
        (work_dir / sub).mkdir(exist_ok=True)

    payload = bytes(range(256)) * 1024  # 256 KiB deterministic payload
    (work_dir / "payload.bin").write_bytes(payload)
    expected_hash = hashlib.sha256(payload).hexdigest()

    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        port = s.getsockname()[1]

    conf_path = work_dir / "nginx.conf"
    conf_path.write_text(
        f"""daemon off;
master_process on;
worker_processes 2;
thread_pool default threads=4 max_queue=1024;
pid "{work_dir}/nginx.pid";
error_log stderr notice;
events {{
    worker_connections 256;
}}
http {{
    access_log off;
    sendfile off;
    aio threads=default;
    server {{
        listen 127.0.0.1:{port};
        add_header X-Worker $pid;
        location / {{
            root "{work_dir}";
        }}
    }}
}}
"""
    )

    guest_args = ["-p", str(work_dir) + "/", "-c", str(conf_path)]
    if mode == "native":
        cmd = [str(binary), *guest_args]
    else:
        cmd = [
            str(ISR_PROCESS),
            str(binary),
            "--ledger-dir",
            str(work_dir / "ledger"),
            "--max-seconds",
            "90",
            "--backend",
            "x86_64",
            *extra_flags,
            "--",
            *guest_args,
        ]

    stderr_log = (work_dir / "stderr.log").open("wb")
    proc = subprocess.Popen(cmd, stdout=stderr_log, stderr=stderr_log, start_new_session=True)

    def one_request(req_idx: int) -> tuple[float, str]:
        t0 = time.perf_counter()
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
        try:
            # Unique query string forces fresh URI/request buffer allocation in Nginx request pool
            conn.request("GET", f"/payload.bin?seq={req_idx}&pad={'x' * 128}")
            resp = conn.getresponse()
            body = resp.read()
            if resp.status != 200 or len(body) != len(payload):
                raise RuntimeError(f"bad status={resp.status} len={len(body)}")
            if hashlib.sha256(body).hexdigest() != expected_hash:
                raise RuntimeError("SHA-256 mismatch")
            worker = resp.getheader("X-Worker") or "?"
            return (time.perf_counter() - t0) * 1000.0, worker
        finally:
            conn.close()

    try:
        # Wait for readiness
        deadline = time.monotonic() + 45.0
        while True:
            if proc.poll() is not None:
                raise RuntimeError(f"Nginx exited before readiness: {proc.returncode}")
            try:
                one_request(0)
                break
            except Exception:
                if time.monotonic() >= deadline:
                    raise RuntimeError("Readiness timeout")
                time.sleep(0.05)

        # Warm up both workers
        for i in range(6):
            one_request(i + 1)

        clk_tck = os.sysconf("SC_CLK_TCK") if hasattr(os, "sysconf") else 100
        base_rss_mb, prev_jiffies = sample_proc_tree(proc.pid)
        prev_sample_t = time.perf_counter()

        timeseries: list[dict] = []
        latencies: list[float] = []
        workers_seen: set[str] = set()
        completed_count = 0
        lock = threading.Lock()
        stop_flag = threading.Event()

        start_t = time.perf_counter()
        timeseries.append(
            {
                "t_sec": 0.0,
                "rss_mb": round(base_rss_mb, 2),
                "mem_Growth_mb": 0.0,
                "cpu_pct": 0.0,
                "requests": 0,
            }
        )

        def sampler_loop() -> None:
            nonlocal prev_jiffies, prev_sample_t
            step = 0.05  # 50ms resolution
            next_tick = start_t + step
            while not stop_flag.is_set():
                now = time.perf_counter()
                if now < next_tick:
                    time.sleep(max(0.005, next_tick - now))
                    continue
                t_rel = time.perf_counter() - start_t
                if t_rel > duration_sec + 0.02:
                    break
                rss_mb, jiffies = sample_proc_tree(proc.pid)
                dt = max(0.005, time.perf_counter() - prev_sample_t)
                dj = max(0, jiffies - prev_jiffies)
                cpu_pct = (dj / (dt * clk_tck)) * 100.0
                prev_jiffies = jiffies
                prev_sample_t = time.perf_counter()
                with lock:
                    reqs = completed_count
                timeseries.append(
                    {
                        "t_sec": round(min(duration_sec, t_rel), 3),
                        "rss_mb": round(rss_mb, 2),
                        "mem_growth_mb": round(max(0.0, rss_mb - base_rss_mb), 2),
                        "cpu_pct": round(min(400.0, cpu_pct), 1),
                        "requests": reqs,
                    }
                )
                next_tick += step

        sampler_thread = threading.Thread(target=sampler_loop, daemon=True)
        sampler_thread.start()

        req_counter = 100

        def client_worker() -> None:
            nonlocal completed_count, req_counter
            conn: http.client.HTTPConnection | None = None
            while time.perf_counter() - start_t < duration_sec:
                with lock:
                    req_counter += 1
                    idx = req_counter
                try:
                    t0 = time.perf_counter()
                    if conn is None:
                        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
                    conn.request("GET", f"/payload.bin?seq={idx}&pad={'x' * 128}")
                    resp = conn.getresponse()
                    body = resp.read()
                    if resp.status != 200 or len(body) != len(payload):
                        raise RuntimeError(f"bad status={resp.status} len={len(body)}")
                    if hashlib.sha256(body).hexdigest() != expected_hash:
                        raise RuntimeError("SHA-256 mismatch")
                    w_id = resp.getheader("X-Worker") or "?"
                    lat_ms = (time.perf_counter() - t0) * 1000.0
                    # Reconnect periodically so Nginx exercises both connection & request pool allocation
                    if idx % (16 if mode == "native" else 2) == 0:
                        conn.close()
                        conn = None
                    with lock:
                        if time.perf_counter() - start_t <= duration_sec + 0.25:
                            latencies.append(lat_ms)
                            workers_seen.add(w_id)
                            completed_count += 1
                except Exception:
                    if conn is not None:
                        try:
                            conn.close()
                        except Exception:
                            pass
                        conn = None
                    if time.perf_counter() - start_t >= duration_sec:
                        break
            if conn is not None:
                try:
                    conn.close()
                except Exception:
                    pass

        with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as pool:
            futures = [pool.submit(client_worker) for _ in range(concurrency)]
            concurrent.futures.wait(futures, timeout=duration_sec + 5.0)

        elapsed = time.perf_counter() - start_t
        stop_flag.set()
        sampler_thread.join(timeout=2.0)

        if not latencies:
            raise RuntimeError("No requests completed during benchmark window")

        lat_sorted = sorted(latencies)
        peak_rss = max(pt["rss_mb"] for pt in timeseries) if timeseries else base_rss_mb
        peak_growth = max(pt.get("mem_growth_mb", 0.0) for pt in timeseries) if timeseries else 0.0
        mean_cpu = statistics.mean(pt["cpu_pct"] for pt in timeseries[1:]) if len(timeseries) > 1 else 0.0
        peak_cpu = max(pt["cpu_pct"] for pt in timeseries[1:]) if len(timeseries) > 1 else 0.0

        return {
            "config": label,
            "short_label": short_label,
            "libc": libc_name,
            "mode": mode,
            "hardening": " ".join(extra_flags) if extra_flags else ("none" if mode == "native" else "full"),
            "duration_seconds": round(elapsed, 3),
            "verified_responses": len(latencies),
            "payload_bytes": len(payload),
            "workers_observed": sorted(workers_seen),
            "rps": len(latencies) / elapsed,
            "latency_ms": {
                "mean": statistics.mean(lat_sorted),
                "median": statistics.median(lat_sorted),
                "p95": lat_sorted[int((len(lat_sorted) - 1) * 0.95)],
                "p99": lat_sorted[int((len(lat_sorted) - 1) * 0.99)],
            },
            "memory_mb": {
                "base_rss": round(base_rss_mb, 2),
                "peak_rss": round(peak_rss, 2),
                "peak_growth": round(peak_growth, 2),
            },
            "cpu_pct": {
                "mean": round(mean_cpu, 1),
                "peak": round(peak_cpu, 1),
            },
            "timeseries": timeseries,
        }
    finally:
        try:
            os.killpg(proc.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            proc.wait()
        stderr_log.close()
        shutil.rmtree(work_dir, ignore_errors=True)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--duration", type=float, default=2.0, help="Benchmark window per config in seconds")
    ap.add_argument("--concurrency", type=int, default=8, help="Concurrent HTTP clients")
    ap.add_argument("--json", type=Path, default=ROOT / "benchmark_results.json")
    ap.add_argument("--svg", type=Path, default=ROOT / "benchmark_results.svg")
    args = ap.parse_args()

    if not ISR_PROCESS.exists():
        subprocess.run(
            ["cargo", "build", "--locked", "--release", "--bin", "isr-process"],
            cwd=ROOT,
            check=True,
        )

    glibc_nginx = find_nginx(GLIBC_CANDIDATES)
    musl_nginx = find_nginx(MUSL_CANDIDATES)

    suite = [
        (
            "Nginx (glibc) — LOCKSTEP Full Defense",
            "LOCKSTEP Full (glibc)",
            "glibc 2.39",
            glibc_nginx,
            "lockstep",
            [],
        ),
        (
            "Nginx (musl) — LOCKSTEP Full Defense",
            "LOCKSTEP Full (musl)",
            "musl 1.2.5",
            musl_nginx,
            "lockstep",
            [],
        ),
        (
            "Nginx (glibc) — LOCKSTEP Shadow Tags + Quarantine",
            "LOCKSTEP Tags Only (glibc)",
            "glibc 2.39",
            glibc_nginx,
            "lockstep",
            ["--no-pac", "--no-cfi"],
        ),
        (
            "Nginx (glibc) — LOCKSTEP PAC + CFI Only",
            "LOCKSTEP PAC+CFI (glibc)",
            "glibc 2.39",
            glibc_nginx,
            "lockstep",
            ["--no-tags", "--no-random-placement"],
        ),
        (
            "Nginx (glibc) — Unprotected Native",
            "Native Nginx (glibc)",
            "glibc 2.39",
            glibc_nginx,
            "native",
            [],
        ),
        (
            "Nginx (musl) — Unprotected Native",
            "Native Nginx (musl)",
            "musl 1.2.5",
            musl_nginx,
            "native",
            [],
        ),
    ]

    results = []
    print(f"Benchmarking real multi-worker Nginx on {platform.system()} {platform.machine()} (duration={args.duration}s, concurrency={args.concurrency})")
    for idx, (label, short_label, libc_name, binary, mode, flags) in enumerate(suite, 1):
        print(f"[{idx}/{len(suite)}] {label} ...")
        r = run_nginx_config(
            label=label,
            short_label=short_label,
            libc_name=libc_name,
            binary=binary,
            mode=mode,
            extra_flags=flags,
            duration_sec=args.duration,
            concurrency=args.concurrency,
        )
        results.append(r)
        print(
            f"   -> {r['rps']:.1f} req/s | median {r['latency_ms']['median']:.1f} ms | "
            f"peak RSS {r['memory_mb']['peak_rss']:.1f} MB (+{r['memory_mb']['peak_growth']:.1f} MB) | "
            f"mean CPU {r['cpu_pct']['mean']:.1f}% | {len(r['timeseries'])} samples"
        )

    args.json.write_text(json.dumps(results, indent=2) + "\n")
    print(f"Wrote {args.json}")
    subprocess.run([sys.executable, str(ROOT / "scripts" / "generate_graphic.py")], check=False)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
