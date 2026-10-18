#!/usr/bin/env python3
"""Build real x86-64 exploit guests and require the hardened heap to stop them.

Each fixture is a freestanding Linux x86-64 ELF that performs a genuine
exploitation primitive -- linear overflow, use-after-free, pointer substitution,
heap grooming -- through the runtime's allocator. Every one is executed under
`isr-process`, which lifts it to RV64 and interprets it; nothing runs natively.

The CVE-named fixtures replicate the *shape* of public heap bugs (unchecked
fixed-buffer copy, dangling cached pointer, slab neighbour overflow). They are
generic primitives, not reduced copies of the original vulnerable products.

For each exploit there are two runs:
  hardened  : all mitigations armed; the run must be rejected with the specific
              mitigation fault, and the primitive must not complete.
  control   : the matching mitigation disarmed; the primitive must complete and
              observe its own effect, proving the fixture really is an exploit
              and not a program that merely fails to run.

A pass therefore requires both directions. A green run that only shows "it
crashed" would not distinguish a mitigation from a bug.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "tests" / "fixtures"
OUT = ROOT / "target" / "fixtures"
LEDGERS = ROOT / ".isr-state" / "mitigations"

# Heap syscalls exposed by the runtime for guests with no allocator of their own.
ISR_MALLOC = 0x4953_5200

EXPECTED = {
    # name: (acceptable control exit codes, control flags, required hardened fault kind)
    #
    # Control runs must demonstrate the primitive, not merely finish. For an
    # overflow that means the neighbour was corrupted (exit 3). For a UAF it
    # means the dangling access completed: exit 3 if the slot was reused, or
    # exit 4 if the freed slot still held its old bytes (still a UAF).
    # Original synthetic primitives
    "heap_overflow": ((3,), ["--no-tags", "--no-random-placement"], ["TagMismatch", "UseAfterFree"]),
    "use_after_free": ((0,), ["--no-tags"], ["UseAfterFree"]),
    "pointer_substitution": ((20,), ["--no-pac"], ["PointerAuth"]),
    # CVE-shaped primitives (generic overflow / UAF named after public bugs)
    "cve_2026_42533_heap_overflow": ((3,), ["--no-tags", "--no-random-placement"], ["TagMismatch", "UseAfterFree"]),
    "cve_2026_42530_use_after_free": ((3, 4), ["--no-tags"], ["UseAfterFree", "TagMismatch"]),
    "cve_2026_14432_v8_uaf": ((3, 4), ["--no-tags"], ["UseAfterFree", "TagMismatch"]),
    "cve_2026_53362_ipv6_frag": ((3,), ["--no-tags", "--no-random-placement"], ["TagMismatch", "UseAfterFree"]),
}


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def build(name: str) -> Path:
    source = FIXTURES / f"{name}.rs"
    output = OUT / f"{name}.elf"
    OUT.mkdir(parents=True, exist_ok=True)
    rustc = (
        subprocess.check_output(
            ["rustup", "which", "--toolchain", "stable", "rustc"], text=True
        ).strip()
        if shutil.which("rustup")
        else shutil.which("rustc")
    )
    command = [
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
        str(source),
        "-o",
        str(output),
    ]
    subprocess.run(command, check=True, timeout=120)
    assert output.read_bytes()[:4] == b"\x7fELF", f"{name} is not an ELF"
    return output


def run(binary: Path, tag: str, flags: list[str], backend: str) -> dict:
    ledger = LEDGERS / tag
    if ledger.exists():
        shutil.rmtree(ledger)
    command = [
        str(ROOT / "target" / "release" / "isr-process"),
        str(binary),
        "--ledger-dir",
        str(ledger),
        "--max-seconds",
        "60",
        "--backend",
        backend,
        *flags,
    ]
    done = subprocess.run(
        command, capture_output=True, text=True, errors="replace", timeout=180
    )
    events_path = None
    for line in done.stderr.splitlines():
        if line.startswith("ledger: "):
            events_path = Path(line[len("ledger: ") :])
    events = []
    if events_path and events_path.is_file():
        events = [json.loads(x) for x in events_path.read_text().splitlines() if x.strip()]
    report = None
    mitigations = None
    for event in events:
        if event.get("type") == "result":
            report = event.get("report")
        if event.get("type") == "mitigations":
            mitigations = event
    return {
        "command": command,
        "returncode": done.returncode,
        "stdout": done.stdout,
        "stderr": done.stderr.strip(),
        "report": report,
        "mitigations": mitigations,
        "ledger": str(events_path) if events_path else None,
    }


def binary_sha(binary: Path) -> str:
    return sha(binary.read_bytes())


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--keep-ledgers", action="store_true")
    parser.add_argument(
        "--backend",
        default="x86_64",
        choices=["x86_64", "rv64"],
        help="compilation backend under test (default: x86_64 host-native)",
    )
    args = parser.parse_args()
    backend = args.backend

    subprocess.run(
        ["cargo", "build", "--locked", "--release", "--bin", "isr-process"],
        cwd=ROOT,
        check=True,
    )
    LEDGERS.mkdir(parents=True, exist_ok=True)
    results = {}
    failures = []

    for name, (control_exits, control_flags, fault_kinds) in EXPECTED.items():
        binary = build(name)
        hardened = run(binary, f"{name}-hardened", [], backend)
        control = run(binary, f"{name}-control", control_flags, backend)
        error = hardened["stderr"] or ""
        faulted = hardened["returncode"] != 0 and "Mitigation {" in error
        results[name] = {
            "binary_sha256": binary_sha(binary),
            "hardened": {
                "returncode": hardened["returncode"],
                "rejected": faulted,
                "error": error,
            },
            "control": {
                "flags": control_flags,
                "returncode": control["returncode"],
                "exit_code": (control["report"] or {}).get("exit_code"),
                "stdout": control["stdout"].strip(),
                "error": control["stderr"],
            },
        }
        if not faulted:
            failures.append(f"{name}: hardened run completed; the exploit was not stopped")
        elif not any(f"kind: {kind}" in error for kind in fault_kinds):
            # A nonzero exit alone proves nothing: the fixture exits nonzero on
            # its own. Only the specific mitigation fault counts.
            failures.append(
                f"{name}: rejected for the wrong reason, expected one of {fault_kinds}: {error}"
            )
        control_exit = (control["report"] or {}).get("exit_code")
        if control_exit not in control_exits:
            failures.append(
                f"{name}: control exit {control_exit} not in {control_exits}; "
                f"the fixture is not demonstrating the primitive"
            )

    # Grooming is measured, not faulted: adjacency must collapse when placement
    # is randomized, and be total when it is not.
    grooming = build("heap_grooming")
    randomized = run(grooming, "grooming-randomized", [], backend)
    ordered = run(grooming, "grooming-ordered", ["--no-random-placement"], backend)
    random_adjacent = (randomized["report"] or {}).get("exit_code")
    ordered_adjacent = (ordered["report"] or {}).get("exit_code")
    results["heap_grooming"] = {
        "binary_sha256": binary_sha(grooming),
        "adjacent_randomized": random_adjacent,
        "adjacent_ordered": ordered_adjacent,
        "hardened": {"returncode": randomized["returncode"]},
        "control": {"returncode": ordered["returncode"], "flags": ["--no-random-placement"]},
    }
    if not isinstance(random_adjacent, int) or not isinstance(ordered_adjacent, int):
        failures.append("heap_grooming: both runs must complete and report a count")
    else:
        if ordered_adjacent < 16:
            failures.append(
                f"heap_grooming: ordered placement should always be adjacent, got {ordered_adjacent}/16"
            )
        if random_adjacent > 4:
            failures.append(
                f"heap_grooming: randomized placement should rarely be adjacent, got {random_adjacent}/16"
            )

    # Regression: the real network server must still serve a real client with
    # every mitigation armed. It is driven end to end because it never exits on
    # its own -- running it standalone would only prove the deadline fires.
    network = subprocess.run(
        [sys.executable, str(ROOT / "scripts" / "verify_rv64_network_server.py"), "--backend", backend],
        cwd=ROOT,
        capture_output=True,
        text=True,
        errors="replace",
        timeout=600,
    )
    results["network_server_regression"] = {
        "returncode": network.returncode,
        "tail": network.stdout.strip()[-400:],
    }
    if network.returncode != 0:
        failures.append(
            f"network server regression failed: {network.stdout[-400:]} {network.stderr[-400:]}"
        )

    receipt = {
        "expectations": "each hardened run must be rejected; each control run must complete",
        "backend": backend,
        "results": results,
        "failures": failures,
        "passed": not failures,
    }
    path = ROOT / ".isr-state" / "mitigation_verification.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(receipt, indent=2))
    if not args.keep_ledgers and LEDGERS.exists():
        shutil.rmtree(LEDGERS)
    if failures:
        print("\nFAILED:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1
    print("\nAll heap mitigations verified against real guest binaries.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
