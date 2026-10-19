#!/usr/bin/env python3
"""Benchmark HTTP guest: bare native vs ISR host-native across all security configurations.

Configurations compared:
  1. Native (direct)                       — bare metal Linux x86-64 execution
  2. ISR Baseline (all mitigations OFF)    — pure translation & JIT, no security instrumentation
  3. ISR + PAC only                        — pointer authentication on heap pointers
  4. ISR + CFI only                        — fine-grained indirect branch bitmap checks
  5. ISR + Shadow Tags only                — 8-byte shadow memory granule verification
  6. ISR + Full Hardening (software mask)  — PAC + CFI + Shadow Tags + Randomized Heap (software mask)
  7. ISR + Full Hardening + LAM requested  — Full mitigations with Intel LAM_U57 requested

Outputs benchmark_results.json and benchmark_results.svg.
"""
from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import signal
import socket
import statistics
import subprocess
import sys
import time
from pathlib import Path

# Ensure cargo/rustc are in PATH even in non-login environments
CARGO_BIN = str(Path.home() / ".cargo" / "bin")
if CARGO_BIN not in os.environ.get("PATH", ""):
    os.environ["PATH"] = f"{CARGO_BIN}:{os.environ.get('PATH', '')}"

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "tests" / "fixtures"
OUT = ROOT / "target" / "fixtures"
ISR_PROCESS = ROOT / "target" / "release" / "isr-process"
PORT = 29876


def find_rustc() -> str:
    path_candidates = [
        shutil.which("rustc"),
        str(Path.home() / ".cargo" / "bin" / "rustc"),
    ]
    for c in path_candidates:
        if c and os.path.exists(c):
            return c
    return "rustc"


def build_guest(guest_type: str = "musl") -> Path:
    """Build the HTTP guest binary."""
    OUT.mkdir(parents=True, exist_ok=True)
    rustc = find_rustc()

    if guest_type == "musl":
        src = FIXTURES / "musl_server.rs"
        elf = OUT / "musl_server.elf"
        cmd = [
            rustc,
            "--edition=2021",
            "--target",
            "x86_64-unknown-linux-musl",
            "-C",
            "opt-level=2",
            "-C",
            "relocation-model=static",
            "-C",
            f"linker={shutil.which('rust-lld') or 'ld.lld' or 'rust-lld'}",
            "-C",
            "link-arg=-static",
            str(src),
            "-o",
            str(elf),
        ]
    else:
        src = FIXTURES / "heap_bench_server.rs"
        elf = OUT / "heap_bench_server.elf"
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
            f"linker={shutil.which('rust-lld') or 'ld.lld' or 'rust-lld'}",
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


def curl_n(n: int, endpoint: str = "") -> tuple[float, list[float]]:
    lat = []
    start = time.perf_counter()
    url = f"http://127.0.0.1:{PORT}/{endpoint.lstrip('/')}"
    for _ in range(n):
        t0 = time.perf_counter()
        subprocess.run(
            ["curl", "-s", "-o", "/dev/null", url],
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


def drain_port(timeout: float = 10.0) -> None:
    """Wait until the guest port is free so the next config can bind."""
    deadline = time.monotonic() + timeout

    def port_busy() -> bool:
        if shutil.which("ss"):
            r = subprocess.run(["ss", "-ltn"], capture_output=True, text=True)
            return f":{PORT}" in r.stdout
        if shutil.which("lsof"):
            r = subprocess.run(
                ["lsof", "-nP", f"-iTCP:{PORT}", "-sTCP:LISTEN"],
                capture_output=True,
                text=True,
            )
            return bool(r.stdout.strip())
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
                s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                s.bind(("127.0.0.1", PORT))
            return False
        except OSError:
            return True

    while time.monotonic() < deadline:
        if not port_busy():
            return
        if shutil.which("fuser"):
            subprocess.run(["fuser", "-k", "-9", f"{PORT}/tcp"], capture_output=True)
        elif shutil.which("lsof"):
            r = subprocess.run(
                ["lsof", "-nP", f"-iTCP:{PORT}", "-sTCP:LISTEN", "-t"],
                capture_output=True,
                text=True,
            )
            for pid in r.stdout.split():
                try:
                    os.kill(int(pid), signal.SIGKILL)
                except OSError:
                    pass
        time.sleep(0.3)
    time.sleep(0.5)


def wait_ready(proc: subprocess.Popen, timeout: float = 60.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(f"guest exited early: {proc.returncode}")
        probe = subprocess.run(
            ["curl", "-s", "-o", "/dev/null", "-m", "2", f"http://127.0.0.1:{PORT}/"],
            capture_output=True,
        )
        if probe.returncode == 0:
            return
        time.sleep(0.3)
    raise RuntimeError("guest did not become ready")


def run_native(elf: Path, n: int) -> dict:
    """Direct execution of the static Linux ELF on bare metal."""
    if platform.system() != "Linux" or platform.machine() not in ("x86_64", "amd64"):
        return stats(
            "Native (direct)",
            0.0,
            [0.0],
            {"skipped": "requires a Linux x86-64 host to exec the guest ELF", "notes": "skipped"},
        )
    proc = subprocess.Popen([str(elf)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        wait_ready(proc, timeout=30.0)
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
        "240",
        "--backend",
        backend,
        *extra_flags,
    ]
    ledger.parent.mkdir(parents=True, exist_ok=True)
    err_file_name = label.replace(" ", "_").replace("(", "").replace(")", "").replace("+", "plus")
    stderr = open(ledger.parent / f"{err_file_name}.err", "w")
    proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=stderr)
    try:
        wait_ready(proc, timeout=90.0)
        curl_n(1)
        total, lat = curl_n(n)
        row = stats(label, total, lat, {"hardening": " ".join(extra_flags) or "default", "backend": backend})
        # ledger counters
        for path in sorted(ledger.glob("*/events.jsonl")):
            for line in path.read_text().splitlines():
                if not line.strip():
                    continue
                try:
                    e = json.loads(line)
                except json.JSONDecodeError:
                    continue
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
    """Clear grouped bar chart with multi-config palette."""
    valid_results = [r for r in results if not r.get("skipped")]
    if not valid_results:
        return

    n_configs = len(valid_results)
    w, h = 1080, 580
    margin = {"top": 80, "right": 40, "bottom": 140, "left": 80}
    chart_w = w - margin["left"] - margin["right"]
    chart_h = h - margin["top"] - margin["bottom"]

    colors = [
        "#2e7d32",  # Native - green
        "#1565c0",  # Baseline - blue
        "#7b1fa2",  # PAC - purple
        "#00796b",  # CFI - teal
        "#c2185b",  # Tags - magenta
        "#ef6c00",  # Full - amber/orange
        "#d32f2f",  # Full+LAM - red
        "#455a64",  # RV64 - grey
    ]

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
        f'<text x="{w/2}" y="32" text-anchor="middle" fill="#111" font-size="19" font-family="system-ui,sans-serif" font-weight="600">'
        "Linux x86-64 Benchmark: Bare Metal vs ISR Mitigations</text>",
        f'<text x="{w/2}" y="56" text-anchor="middle" fill="#555" font-size="12" font-family="system-ui,sans-serif">'
        f"{n_configs} configs · {valid_results[0].get('requests', '')} requests · host {platform.system()} {platform.machine()}</text>",
    ]

    group_w = chart_w / len(plots)
    bar_w = min(36.0, (group_w * 0.75) / n_configs)
    total_bars_w = n_configs * bar_w

    for pi, (key, label) in enumerate(plots):
        gx = margin["left"] + pi * group_w + (group_w - total_bars_w) / 2
        vals = [value(r, key) for r in valid_results]
        max_val = max(vals) * 1.15 or 1.0

        for i in range(5):
            y = margin["top"] + chart_h - (i / 4) * chart_h
            lab = (i / 4) * max_val
            parts.append(
                f'<line x1="{gx - 8}" y1="{y:.1f}" x2="{gx + total_bars_w + 8:.1f}" y2="{y:.1f}" stroke="#e0e0e0"/>'
            )
            parts.append(
                f'<text x="{gx - 12}" y="{y + 4:.1f}" text-anchor="end" fill="#888" font-size="10" font-family="system-ui,sans-serif">{lab:.1f}</text>'
            )

        for ci, r in enumerate(valid_results):
            v = value(r, key)
            bh = (v / max_val) * chart_h
            bx = gx + ci * bar_w
            by = margin["top"] + chart_h - bh
            color = colors[ci % len(colors)]
            parts.append(
                f'<rect x="{bx:.1f}" y="{by:.1f}" width="{bar_w - 2:.1f}" height="{bh:.1f}" fill="{color}" rx="2"/>'
            )
            val_text = f"{v:.1f}" if v >= 10 else f"{v:.2f}"
            parts.append(
                f'<text x="{bx + (bar_w-2)/2:.1f}" y="{by - 5:.1f}" text-anchor="middle" fill="#222" font-size="9" font-family="system-ui,sans-serif" font-weight="600">{val_text}</text>'
            )

        label_y = margin["top"] + chart_h + 30
        parts.append(
            f'<text x="{gx + total_bars_w/2:.1f}" y="{label_y}" text-anchor="middle" fill="#333" font-size="12" font-family="system-ui,sans-serif" font-weight="500">{label}</text>'
        )

    # Multi-column legend at bottom
    cols = 2
    per_col = (n_configs + cols - 1) // cols
    col_w = chart_w / cols
    base_y = margin["top"] + chart_h + 55

    for ci, r in enumerate(valid_results):
        col_idx = ci // per_col
        row_idx = ci % per_col
        lx = margin["left"] + col_idx * col_w
        ly = base_y + row_idx * 18
        color = colors[ci % len(colors)]
        cfg_name = r["config"]
        parts.append(f'<rect x="{lx}" y="{ly - 10}" width="12" height="12" fill="{color}" rx="2"/>')
        parts.append(
            f'<text x="{lx + 18}" y="{ly}" fill="#222" font-size="11" font-family="system-ui,sans-serif">{cfg_name}</text>'
        )

    parts.append("</svg>")
    output.write_text("\n".join(parts))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--requests", type=int, default=50)
    ap.add_argument("--guest", choices=["musl", "heap"], default="musl")
    ap.add_argument("--json", type=Path, default=ROOT / "benchmark_results.json")
    ap.add_argument("--svg", type=Path, default=ROOT / "benchmark_results.svg")
    args = ap.parse_args()

    if not ISR_PROCESS.exists():
        subprocess.run(
            ["cargo", "build", "--locked", "--release", "--bin", "isr-process"],
            cwd=ROOT,
            check=True,
        )

    elf = build_guest(args.guest)
    n = args.requests
    results = []
    print(f"host={platform.system()} {platform.machine()}  requests={n}  guest={elf.name}")

    # List of all options:
    suite = [
        ("1/7 Native (direct)", "Native (direct)", None, []),
        ("2/7 ISR Baseline (no mitigations)", "ISR Baseline (no mitigations)", "x86_64", ["--no-tags", "--no-pac", "--no-cfi", "--no-random-placement", "--no-lam"]),
        ("3/7 ISR + PAC only", "ISR + PAC only", "x86_64", ["--no-tags", "--no-cfi", "--no-random-placement", "--no-lam"]),
        ("4/7 ISR + CFI only", "ISR + CFI only", "x86_64", ["--no-tags", "--no-pac", "--no-random-placement", "--no-lam"]),
        ("5/7 ISR + Shadow Tags only", "ISR + Shadow Tags only", "x86_64", ["--no-pac", "--no-cfi", "--no-random-placement", "--no-lam"]),
        ("6/7 ISR + Full Hardening (software mask)", "ISR + Full Hardening (software mask)", "x86_64", ["--no-lam"]),
        ("7/7 ISR + Full Hardening + LAM requested", "ISR + Full Hardening + LAM requested", "x86_64", []),
    ]

    for step_label, config_label, backend, flags in suite:
        print(f"{step_label} ...")
        if backend is None:
            r = run_native(elf, n)
        else:
            r = run_isr(elf, n, backend, config_label, flags)
        r["requests"] = n
        results.append(r)
        if r.get("skipped"):
            print(f"   skipped: {r['skipped']}")
        else:
            rps = r.get("rps", 0)
            mean_lat = r.get("latency_ms", {}).get("mean", 0)
            tag_scheme = r.get("tag_scheme", "")
            scheme_str = f" scheme={tag_scheme}" if tag_scheme else ""
            print(f"   {rps:.1f} rps, {mean_lat:.2f} ms{scheme_str}")

    args.json.write_text(json.dumps(results, indent=2) + "\n")
    generate_svg(results, args.svg)
    print(f"wrote {args.json}")
    print(f"wrote {args.svg}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
