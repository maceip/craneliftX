#!/usr/bin/env python3
"""Benchmark the HTTP guest: bare native vs ISR host-native vs ISR + optional LAM.

Configurations (same freestanding Linux x86-64 guest, same request mix):
  1. Native          — guest ELF executed directly (Linux x86-64 host)
  2. ISR x86_64      — lifted + host-native JIT, default hardening
                       (shadow tags, PAC, mid-instruction CFI, randomized heap)
  3. ISR + LAM       — same as (2) with Intel LAM_U57 requested
                       (hardware tag mask when the kernel allows it)

Outputs benchmark_results.json and benchmark_results.svg.

Usage:
    python3 scripts/benchmark.py [--requests 50] [--backend x86_64]
"""
from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
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


def build_guest() -> Path:
    OUT.mkdir(parents=True, exist_ok=True)
    src = FIXTURES / "bench_server.rs"
    elf = OUT / "bench_server.elf"
    rustc = (
        subprocess.check_output(
            ["rustup", "which", "--toolchain", "stable", "rustc"], text=True
        ).strip()
        if shutil.which("rustup")
        else shutil.which("rustc")
    )
    cmd = [
        rustc,
        "--edition=2021",
        "--target",
        "x86_64-unknown-linux-gnu",
        "-C",
        "opt-level=2",
        "-C",
        "panic=abort",
        "-C",
        "relocation-model=static",
        "-C",
        f"linker={shutil.which('ld.lld') or 'rust-lld'}",
        "-C",
        "link-arg=-nostdlib",
        "-C",
        "link-arg=--static",
        "-C",
        "no-vectorize-loops",
        "-C",
        "no-vectorize-slp",
        str(src),
        "-o",
        str(elf),
    ]
    subprocess.run(cmd, check=True, timeout=120)
    return elf


def curl_n(n: int) -> tuple[float, list[float]]:
    lat = []
    start = time.perf_counter()
    for _ in range(n):
        t0 = time.perf_counter()
        subprocess.run(
            ["curl", "-s", "-o", "/dev/null", f"http://127.0.0.1:{PORT}/"],
            capture_output=True,
        )
        lat.append((time.perf_counter() - t0) * 1000)
    total = time.perf_counter() - start
    return total, lat


def stats(config: str, total: float, lat: list[float], extra: dict | None = None) -> dict:
    row = {
        "config": config,
        "total_seconds": total,
        "rps": len(lat) / total if total > 0 else 0,
        "latency_ms": {
            "mean": statistics.mean(lat),
            "p50": statistics.median(lat),
            "p99": sorted(lat)[max(0, int(len(lat) * 0.99) - 1)],
            "max": max(lat),
        },
    }
    if extra:
        row.update(extra)
    return row



def drain_port(timeout: float = 8.0) -> None:
    """Wait until the guest port is free so the next config can bind."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        r = subprocess.run(["ss", "-ltn"], capture_output=True, text=True)
        if f":{PORT}" not in r.stdout:
            return
        subprocess.run(["fuser", "-k", "-9", f"{PORT}/tcp"], capture_output=True)
        time.sleep(0.25)
    time.sleep(0.5)


def wait_ready(proc: subprocess.Popen, timeout: float = 20.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(f"guest exited early: {proc.returncode}")
        probe = subprocess.run(
            ["curl", "-s", "-o", "/dev/null", "-m", "1", f"http://127.0.0.1:{PORT}/"],
            capture_output=True,
        )
        if probe.returncode == 0:
            return
        time.sleep(0.15)
    raise RuntimeError("guest did not become ready")


def run_native(elf: Path, n: int) -> dict:
    """Direct execution of the freestanding Linux ELF (Linux x86-64 only)."""
    if platform.system() != "Linux" or platform.machine() not in ("x86_64", "amd64"):
        return stats(
            "Native (direct)",
            0.0,
            [0.0],
            {"skipped": "requires a Linux x86-64 host to exec the guest ELF", "notes": "skipped"},
        )
    proc = subprocess.Popen([str(elf)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        wait_ready(proc)
        # one warmup
        curl_n(1)
        total, lat = curl_n(n)
        return stats("Native (direct)", total, lat, {"hardening": "none"})
    finally:
        proc.kill()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            pass
        subprocess.run(["fuser", "-k", "-9", f"{PORT}/tcp"], capture_output=True)
        drain_port()


def run_isr(elf: Path, n: int, backend: str, label: str, extra_flags: list[str]) -> dict:
    ledger = ROOT / ".isr-state" / f"bench-{backend}-{'-'.join(extra_flags) or 'default'}"
    if ledger.exists():
        shutil.rmtree(ledger)
    cmd = [
        str(ISR_PROCESS),
        str(elf),
        "--ledger-dir",
        str(ledger),
        "--max-seconds",
        "180",
        "--backend",
        backend,
        *extra_flags,
    ]
    ledger.parent.mkdir(parents=True, exist_ok=True)
    stderr = open(ledger.parent / f"{label.replace(' ', '_').replace('(', '').replace(')', '')}.err", "w")
    proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=stderr)
    try:
        wait_ready(proc, timeout=30)
        curl_n(1)
        total, lat = curl_n(n)
        row = stats(label, total, lat, {"hardening": " ".join(extra_flags) or "default", "backend": backend})
        # ledger counters
        for path in sorted(ledger.glob("*/events.jsonl")):
            for line in path.read_text().splitlines():
                if not line.strip():
                    continue
                e = json.loads(line)
                if e.get("type") == "result" and isinstance(e.get("report"), dict):
                    r = e["report"]
                    row["compiled_blocks"] = r.get("compiled_blocks", 0)
                    row["native_x86_instructions"] = r.get("native_x86_instructions_executed", 0)
                    row["x86_instructions"] = r.get("x86_instructions_executed", 0)
                    row["rv64_steps"] = r.get("rv64_steps", 0)
                if e.get("type") == "mitigations":
                    row["lam"] = e.get("tags", {}).get("lam", "?")
                    row["tag_scheme"] = e.get("tags", {}).get("scheme", "?")
        return row
    finally:
        proc.kill()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            pass
        subprocess.run(["fuser", "-k", "-9", f"{PORT}/tcp"], capture_output=True)
        drain_port()
        stderr.close()


def generate_svg(results: list[dict], output: Path) -> None:
    """Clear grouped bar chart: three configs × three metrics."""
    results = [r for r in results if not r.get("skipped")]
    if not results:
        return
    w, h = 960, 520
    margin = {"top": 70, "right": 30, "bottom": 100, "left": 80}
    chart_w = w - margin["left"] - margin["right"]
    chart_h = h - margin["top"] - margin["bottom"]
    colors = ["#2e7d32", "#1565c0", "#ef6c00"]
    plots = [
        ("rps", "Requests / second (higher is better)"),
        ("total_seconds", "Total time for N requests (s, lower is better)"),
        ("latency_ms", "Mean latency (ms, lower is better)"),
    ]

    def value(r, key):
        v = r[key]
        return v["mean"] if isinstance(v, dict) else v

    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">',
        f'<rect width="{w}" height="{h}" fill="#fafafa"/>',
        f'<text x="{w/2}" y="28" text-anchor="middle" fill="#111" font-size="18" font-family="system-ui,sans-serif" font-weight="600">'
        "HTTP guest: bare metal vs ISR vs ISR + optional LAM</text>",
        f'<text x="{w/2}" y="50" text-anchor="middle" fill="#555" font-size="12" font-family="system-ui,sans-serif">'
        f"{len(results)} configs · {results[0].get('requests', '')} requests · host {platform.system()} {platform.machine()}</text>",
    ]

    n = len(results)
    group_w = chart_w / len(plots)
    bar_w = min(56, group_w * 0.7 / n)
    for pi, (key, label) in enumerate(plots):
        gx = margin["left"] + pi * group_w + group_w * 0.15
        vals = [value(r, key) for r in results]
        max_val = max(vals) * 1.15 or 1.0
        for i in range(5):
            y = margin["top"] + chart_h - (i / 4) * chart_h
            lab = (i / 4) * max_val
            parts.append(
                f'<line x1="{gx - 10}" y1="{y:.1f}" x2="{gx + group_w * 0.7:.1f}" y2="{y:.1f}" stroke="#ddd"/>'
            )
            parts.append(
                f'<text x="{gx - 14}" y="{y + 4:.1f}" text-anchor="end" fill="#888" font-size="10" font-family="system-ui,sans-serif">{lab:.1f}</text>'
            )
        for ci, r in enumerate(results):
            v = value(r, key)
            bh = (v / max_val) * chart_h
            bx = gx + ci * (bar_w + 10)
            by = margin["top"] + chart_h - bh
            parts.append(
                f'<rect x="{bx:.1f}" y="{by:.1f}" width="{bar_w:.1f}" height="{bh:.1f}" fill="{colors[ci]}" rx="3"/>'
            )
            parts.append(
                f'<text x="{bx + bar_w/2:.1f}" y="{by - 6:.1f}" text-anchor="middle" fill="#222" font-size="11" font-family="system-ui,sans-serif" font-weight="600">{v:.1f}</text>'
            )
        parts.append(
            f'<text x="{gx + group_w * 0.35:.1f}" y="{h - 48}" text-anchor="middle" fill="#333" font-size="12" font-family="system-ui,sans-serif">{label}</text>'
        )

    for ci, r in enumerate(results):
        lx = margin["left"] + ci * 220
        ly = h - 22
        parts.append(f'<rect x="{lx}" y="{ly - 10}" width="14" height="14" fill="{colors[ci]}" rx="2"/>')
        parts.append(
            f'<text x="{lx + 22}" y="{ly + 2}" fill="#222" font-size="12" font-family="system-ui,sans-serif">{r["config"]}</text>'
        )
    parts.append("</svg>")
    output.write_text("\n".join(parts))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--requests", type=int, default=50)
    ap.add_argument("--backend", default="x86_64")
    ap.add_argument("--json", type=Path, default=ROOT / "benchmark_results.json")
    ap.add_argument("--svg", type=Path, default=ROOT / "benchmark_results.svg")
    args = ap.parse_args()

    if not ISR_PROCESS.exists():
        subprocess.run(
            ["cargo", "build", "--locked", "--release", "--bin", "isr-process"],
            cwd=ROOT,
            check=True,
        )

    elf = build_guest()
    n = args.requests
    results = []
    print(f"host={platform.system()} {platform.machine()}  requests={n}  guest={elf}")

    print("1/3 Native (direct) ...")
    r = run_native(elf, n)
    r["requests"] = n
    results.append(r)
    if r.get("skipped"):
        print(f"   skipped: {r['skipped']}")
    else:
        print(f"   {r['rps']:.1f} rps, {r['latency_ms']['mean']:.2f} ms")

    print("2/3 ISR host-native, hardening on, LAM off ...")
    r = run_isr(elf, n, args.backend, "ISR + hardening (LAM off)", ["--no-lam"])
    r["requests"] = n
    results.append(r)
    print(f"   {r['rps']:.1f} rps, {r['latency_ms']['mean']:.2f} ms  scheme={r.get('tag_scheme')}")

    print("3/3 ISR host-native, hardening on, LAM requested ...")
    r = run_isr(elf, n, args.backend, "ISR + hardening + LAM", [])
    r["requests"] = n
    r["optional"] = "Intel LAM_U57 requested via arch_prctl/prctl"
    results.append(r)
    print(f"   {r['rps']:.1f} rps, {r['latency_ms']['mean']:.2f} ms  lam={r.get('lam')} scheme={r.get('tag_scheme')}")

    args.json.write_text(json.dumps(results, indent=2) + "\n")
    generate_svg(results, args.svg)
    print(f"wrote {args.json}")
    print(f"wrote {args.svg}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
