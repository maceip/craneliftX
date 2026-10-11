#!/usr/bin/env python3
"""Build the real freestanding Rust server offered by the pipeline UI."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def build():
    rustc = subprocess.check_output(["rustup", "which", "--toolchain", "stable", "rustc"], text=True).strip() if shutil.which("rustup") else shutil.which("rustc")
    source = ROOT / "tests/fixtures/rv64_network_server.rs"
    output = ROOT / "target/fixtures/rust-network-server.elf"
    output.parent.mkdir(parents=True, exist_ok=True)
    command = [rustc, "--edition=2021", "--target", "x86_64-unknown-linux-gnu",
               "-C", "opt-level=2", "-C", "panic=abort", "-C", "relocation-model=static",
               "-C", f"linker={shutil.which('ld.lld') or 'rust-lld'}", "-C", "link-arg=-nostdlib",
               "-C", "link-arg=--static", "-C", "no-vectorize-loops", "-C", "no-vectorize-slp",
               str(source), "-o", str(output)]
    subprocess.run(command, check=True, timeout=90)
    receipt = {"source": str(source), "source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
               "binary_sha256": hashlib.sha256(output.read_bytes()).hexdigest(), "command": command}
    output.with_suffix(".json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(output)


if __name__ == "__main__":
    build()
