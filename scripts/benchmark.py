#!/usr/bin/env python3
"""Benchmark ISR runtime overhead: Native vs RV64 interpreter.

Runs a minimal HTTP server under three configurations and compares
latency, throughput, and resource usage.

NOTE: ISR x86_64 native backend is excluded because it lacks syscall
interception — guest syscalls execute directly on the host CPU with
Linux numbers, which fails on macOS/Rosetta. It works for pure compute
but not for I/O workloads. This is a known limitation.

Configurations:
  1. Native (macOS x86-64 via Rosetta) — direct execution, no ISR
  2. ISR RV64 — x86-64 lifted to RISC-V, interpreted

Metrics collected:
  - Total runtime for N requests
  - Requests per second
  - Average latency per request
  - CPU time (user + sys)
  - Peak RSS memory
  - ISR-specific: compiled blocks, RV64 steps, x86 instructions executed

Usage:
    python3 scripts/benchmark.py
    python3 scripts/benchmark.py --requests 50 --output benchmark_results.svg
"""
from __future__ import annotations

import argparse
import json
import os
import statistics
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "tests" / "fixtures"
OUT = ROOT / "target" / "fixtures"
ISR_PROCESS = ROOT / "target" / "release" / "isr-process"
PORT = 29876


def build_servers():
    """Compile both native macOS and ISR Linux versions of the benchmark server."""
    OUT.mkdir(parents=True, exist_ok=True)

    # Native macOS x86-64 server
    native_src = FIXTURES / "bench_server_native.c"
    native_bin = OUT / "bench_server_native"
    if not native_bin.exists():
        subprocess.run(
            [
                "arch", "-x86_64", "cc", "-O2", "-target", "x86_64-apple-macos11",
                f"-DREQUEST_COUNT={REQUEST_COUNT}", f"-DPORT={PORT}",
                str(native_src), "-o", str(native_bin)
            ],
            check=True,
        )
        print(f"Built native server: {native_bin}")

    # ISR Linux static server
    isr_src = FIXTURES / "bench_server.rs"
    isr_bin = OUT / "bench_server.elf"
    if not isr_bin.exists():
        rustc = subprocess.check_output(
            ["rustup", "which", "--toolchain", "stable", "rustc"], text=True
        ).strip()
        subprocess.run(
            [
                rustc,
                "--edition=2021",
                "--target", "x86_64-unknown-linux-gnu",
                "-C", "opt-level=2",
                "-C", "panic=abort",
                "-C", "relocation-model=static",
                "-C", "linker=ld.lld",
                "-C", "link-arg=-nostdlib",
                "-C", "link-arg=--static",
                "-C", "no-vectorize-loops",
                "-C", "no-vectorize-slp",
                str(isr_src),
                "-o", str(isr_bin),
            ],
            check=True,
        )
        print(f"Built ISR server: {isr_bin}")

    return native_bin, isr_bin


def run_native_server(native_bin: Path) -> dict:
    """Run the native server and measure."""
    env = {k: v for k, v in os.environ.items() if k not in ("http_proxy", "https_proxy", "HTTP_PROXY", "HTTPS_PROXY")}
    
    # Start server
    proc = subprocess.Popen([str(native_bin)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=env)
    time.sleep(0.3)  # Let server start listening

    # Warmup
    _ = subprocess.run(["curl", "-s", f"http://127.0.0.1:{PORT}/"], capture_output=True, env=env)

    latencies = []
    start = time.perf_counter()
    for _ in range(REQUEST_COUNT):
        req_start = time.perf_counter()
        r = subprocess.run(
            ["curl", "-s", "-o", "/dev/null", f"http://127.0.0.1:{PORT}/"],
            capture_output=True,
            env=env,
        )
        latencies.append((time.perf_counter() - req_start) * 1000)
    total = time.perf_counter() - start

    proc.terminate()
    proc.wait()

    return {
        "config": "Native (macOS x86-64)",
        "total_seconds": total,
        "rps": REQUEST_COUNT / total,
        "latency_ms": {
            "mean": statistics.mean(latencies),
            "p50": statistics.median(latencies),
            "p99": sorted(latencies)[int(len(latencies) * 0.99)],
            "max": max(latencies),
        },
        "compiled_blocks": 0,
        "rv64_steps": 0,
        "x86_instructions": 0,
        "native_x86_instructions": 0,
    }


def run_isr_server(isr_bin: Path, backend: str) -> dict:
    """Run the server under ISR and measure."""
    env = {k: v for k, v in os.environ.items() if k not in ("http_proxy", "https_proxy", "HTTP_PROXY", "HTTPS_PROXY")}
    ledger_dir = ROOT / ".isr-state" / f"bench-{backend}"
    if ledger_dir.exists():
        import shutil
        shutil.rmtree(ledger_dir)

    # Start ISR process
    proc = subprocess.Popen(
        [str(ISR_PROCESS), str(isr_bin), "--ledger-dir", str(ledger_dir),
         "--max-seconds", "120", "--backend", backend],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        env=env,
    )
    time.sleep(6)  # ISR needs time to compile blocks on first request

    latencies = []
    start = time.perf_counter()
    for _ in range(REQUEST_COUNT):
        req_start = time.perf_counter()
        r = subprocess.run(
            ["curl", "-s", "-o", "/dev/null", f"http://127.0.0.1:{PORT}/"],
            capture_output=True,
            env=env,
        )
        latencies.append((time.perf_counter() - req_start) * 1000)
    total = time.perf_counter() - start

    proc.terminate()
    proc.wait()

    # Extract metrics from ledger
    metrics = {
        "config": f"ISR {backend}",
        "total_seconds": total,
        "rps": REQUEST_COUNT / total,
        "latency_ms": {
            "mean": statistics.mean(latencies),
            "p50": statistics.median(latencies),
            "p99": sorted(latencies)[int(len(latencies) * 0.99)],
            "max": max(latencies),
        },
        "compiled_blocks": 0,
        "rv64_steps": 0,
        "x86_instructions": 0,
        "native_x86_instructions": 0,
    }

    # Parse ledger events
    events_file = None
    for subdir in ledger_dir.iterdir():
        if subdir.is_dir():
            ef = subdir / "events.jsonl"
            if ef.exists():
                events_file = ef
                break

    if events_file:
        for line in events_file.read_text().splitlines():
            if not line.strip():
                continue
            try:
                e = json.loads(line)
                if e.get("type") == "compilation_progress":
                    r = e.get("counters", {})
                    metrics["compiled_blocks"] = r.get("compiled_blocks", 0)
                    metrics["rv64_steps"] = r.get("rv64_steps", 0)
                    metrics["x86_instructions"] = r.get("x86_instructions_executed", 0)
                    metrics["native_x86_instructions"] = r.get("native_x86_instructions_executed", 0)
                if e.get("type") == "result":
                    r = e.get("report", {})
                    metrics["compiled_blocks"] = max(metrics["compiled_blocks"], r.get("compiled_blocks", 0))
                    metrics["rv64_steps"] = max(metrics["rv64_steps"], r.get("rv64_steps", 0))
                    metrics["x86_instructions"] = max(metrics["x86_instructions"], r.get("x86_instructions_executed", 0))
                    metrics["native_x86_instructions"] = max(metrics["native_x86_instructions"], r.get("native_x86_instructions_executed", 0))
            except json.JSONDecodeError:
                continue

    return metrics


def generate_svg(results: list[dict], output: Path):
    """Generate an SVG bar chart comparing the configurations."""
    w, h = 900, 600
    margin = {"top": 60, "right": 40, "bottom": 80, "left": 120}
    chart_w = w - margin["left"] - margin["right"]
    chart_h = h - margin["top"] - margin["bottom"]

    # Colors
    colors = ["#4CAF50", "#2196F3"]

    # Metrics to plot
    plots = [
        ("total_seconds", "Total Runtime (seconds)", 1.0),
        ("rps", "Requests / Second", 1.0),
        ("latency_ms", "Avg Latency (ms)", 1.0),
    ]

    svg_parts = []
    svg_parts.append(f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">')
    svg_parts.append(f'<rect width="{w}" height="{h}" fill="#1e1e1e"/>')
    svg_parts.append(f'<text x="{w//2}" y="35" text-anchor="middle" fill="#ffffff" font-size="18" font-family="sans-serif">ISR Benchmark: Native vs RV64 Interpreter</text>')

    n_configs = len(results)
    group_w = chart_w / len(plots)
    bar_w = group_w * 0.6 / n_configs
    gap = group_w * 0.1

    for plot_idx, (key, label, scale) in enumerate(plots):
        gx = margin["left"] + plot_idx * group_w + gap
        max_val = max(r[key] if isinstance(r[key], (int, float)) else r[key]["mean"] for r in results) * 1.1

        # Y-axis for this group
        for i in range(6):
            y = margin["top"] + chart_h - (i / 5) * chart_h
            val = (i / 5) * max_val
            svg_parts.append(f'<line x1="{gx-5}" y1="{y}" x2="{gx + group_w - gap*2}" y2="{y}" stroke="#444" stroke-width="0.5"/>')
            svg_parts.append(f'<text x="{gx-10}" y="{y+4}" text-anchor="end" fill="#aaa" font-size="9">{val:.1f}</text>')

        # Label
        svg_parts.append(f'<text x="{gx + (group_w - gap*2)/2}" y="{h - 20}" text-anchor="middle" fill="#ccc" font-size="11">{label}</text>')

        # Bars
        for cfg_idx, r in enumerate(results):
            val = r[key] if isinstance(r[key], (int, float)) else r[key]["mean"]
            bh = (val / max_val) * chart_h if max_val > 0 else 0
            bx = gx + cfg_idx * (bar_w + 4)
            by = margin["top"] + chart_h - bh
            svg_parts.append(f'<rect x="{bx}" y="{by}" width="{bar_w}" height="{bh}" fill="{colors[cfg_idx]}" rx="2"/>')
            svg_parts.append(f'<text x="{bx + bar_w/2}" y="{by - 5}" text-anchor="middle" fill="#fff" font-size="9">{val:.2f}</text>')

    # Legend
    for cfg_idx, r in enumerate(results):
        lx = margin["left"] + cfg_idx * 200
        ly = h - 50
        svg_parts.append(f'<rect x="{lx}" y="{ly}" width="12" height="12" fill="{colors[cfg_idx]}" rx="2"/>')
        svg_parts.append(f'<text x="{lx + 18}" y="{ly + 10}" fill="#ccc" font-size="11">{r["config"]}</text>')

    # Annotation about x86_64
    svg_parts.append(f'<text x="{w//2}" y="{h - 8}" text-anchor="middle" fill="#ff9800" font-size="9">Note: ISR x86_64 backend excluded — lacks syscall interception for I/O workloads</text>')

    svg_parts.append('</svg>')
    output.write_text('\n'.join(svg_parts))
    print(f"Chart saved to: {output}")


def main():
    global REQUEST_COUNT
    parser = argparse.ArgumentParser()
    parser.add_argument("--requests", type=int, default=100)
    parser.add_argument("--output", type=Path, default=ROOT / "benchmark_results.svg")
    args = parser.parse_args()
    REQUEST_COUNT = args.requests

    print(f"Building servers (requests={REQUEST_COUNT})...")
    native_bin, isr_bin = build_servers()

    print(f"\n1/2 Running NATIVE server...")
    native_result = run_native_server(native_bin)
    print(f"   Total: {native_result['total_seconds']:.3f}s | RPS: {native_result['rps']:.1f} | Latency: {native_result['latency_ms']['mean']:.2f}ms")

    print(f"\n2/2 Running ISR RV64 server...")
    isr_result = run_isr_server(isr_bin, "rv64")
    print(f"   Total: {isr_result['total_seconds']:.3f}s | RPS: {isr_result['rps']:.1f} | Latency: {isr_result['latency_ms']['mean']:.2f}ms")
    print(f"   Compiled blocks: {isr_result['compiled_blocks']} | RV64 steps: {isr_result['rv64_steps']:,} | X86 instrs: {isr_result['x86_instructions']:,}")

    results = [native_result, isr_result]

    # Print summary table
    print("\n" + "=" * 70)
    print(f"{'Config':<30} {'Total(s)':<10} {'RPS':<10} {'Latency(ms)':<12} {'Blocks':<8}")
    print("=" * 70)
    for r in results:
        print(f"{r['config']:<30} {r['total_seconds']:<10.3f} {r['rps']:<10.1f} {r['latency_ms']['mean']:<12.2f} {r['compiled_blocks']:<8}")

    overhead = isr_result["total_seconds"] / native_result["total_seconds"]
    print(f"\nISR RV64 overhead: {overhead:.1f}x slower than native")

    generate_svg(results, args.output)

    # Save JSON
    json_path = args.output.with_suffix(".json")
    json_path.write_text(json.dumps(results, indent=2))
    print(f"Data saved to: {json_path}")


if __name__ == "__main__":
    raise SystemExit(main())
