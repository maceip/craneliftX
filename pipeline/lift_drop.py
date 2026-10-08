#!/usr/bin/env python3
"""Lift one already-compiled function and drop it onto a new ISA.

Pipeline, per function selected by liftmap/ingest_tracer.py:

  object bytes
      |  anvill spec (protobuf; ABI + bytes + arch)
      v
  remill-lift  (vendored)  -> LLVM IR
      |  remill runtime semantics (real memory + flags)
      v
  Cranelift drop -> Pulley bytecode -> Pulley interpreter
      and, when a riscv64 cross compiler is present,
  the same IR -> riscv64 machine code -> qemu-user

Cranelift has no machine-code decoder. Remill is the lift. Pulley is the
emulator the drop starts on.
"""
from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import anvill_spec  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RUNTIME_LL = os.path.join(os.path.dirname(os.path.abspath(__file__)), "remill_runtime.ll")
EXTRACT = os.path.join(ROOT, "ceremony", "extract_bytes.py")

TRIPLES = {
    "wasm32": (
        'target datalayout = "e-m:e-p:32:32-i64:64-n32:64-S128"',
        'target triple = "wasm32-unknown-unknown"',
    ),
    "riscv64": (
        'target datalayout = "e-m:e-p:64:64-i64:64-i128:128-n32:64-S128"',
        'target triple = "riscv64-unknown-linux-gnu"',
    ),
}


def llvm_tool(base: str) -> str:
    """The drop uses the same LLVM 20 major that remill-lift is linked to."""
    name = f"{base}-20"
    path = shutil.which(name)
    if path:
        return path
    raise SystemExit(f"required LLVM tool not found: {name}")


def find_remill() -> str:
    env = os.environ.get("REMILL_LIFT")
    if env and os.path.isfile(env):
        return env
    roots = [
        os.path.join(ROOT, "build", "remill-install", "bin"),
        "/tmp/remill-install/bin",
    ]
    for root in roots:
        if not os.path.isdir(root):
            continue
        names = sorted(n for n in os.listdir(root) if n.startswith("remill-lift"))
        if names:
            return os.path.join(root, names[0])
    raise SystemExit(
        "remill-lift not found. Run pipeline/build_lifters.sh "
        "(vendored sources are in vendor/remill)."
    )


def extract_bytes(obj: str, symbol: str) -> str:
    out = subprocess.check_output(
        [sys.executable, EXTRACT, obj, symbol], text=True
    )
    hexbytes = "".join(out.split())
    if not hexbytes:
        raise SystemExit(f"no bytes extracted for {symbol} from {obj}")
    return hexbytes


def retarget(ir: str, isa: str) -> str:
    datalayout, triple = TRIPLES[isa]
    if re.search(r'target datalayout = "', ir):
        ir = re.sub(r'target datalayout = "[^"]*"', datalayout, ir, count=1)
    else:
        ir = datalayout + "\n" + ir
    if re.search(r'target triple = "', ir):
        ir = re.sub(r'target triple = "[^"]*"', triple, ir, count=1)
    else:
        ir = triple + "\n" + ir
    return ir


def rename_entry(ir: str, symbol: str) -> str:
    match = re.search(r"@call_sub_[0-9A-Za-z]+", ir)
    if not match:
        raise SystemExit("remill-lift did not produce a call_sub_* entry")
    return ir.replace(match.group(0), "@" + symbol)


def devariadic_flags(ir: str) -> str:
    """Remill's flag helpers are variadic so a symbolic executor can see the
    operands. Wasm's vararg lowering drops the fixed i1, so every conditional
    branch looks taken. The runtime only needs that boolean."""
    names = "zero|sign|carry|overflow"
    ir = re.sub(
        rf"(declare[^@\n]*@__remill_flag_computation_(?:{names}))\([^)]*\)",
        r"\1(i1 zeroext)",
        ir,
    )

    def repl_call(match: re.Match) -> str:
        prefix = match.group(1).replace("(i1, ...)", "(i1)")
        return f"{prefix}{match.group(2)}(i1 {match.group(3)})"

    ir = re.sub(
        rf"(call [^@\n]*?)(@__remill_flag_computation_(?:{names}))"
        r"\(i1(?: noundef)?(?: zeroext)? (%[\w.]+|true|false)(?:, [^)]*)?\)",
        repl_call,
        ir,
    )
    return ir


def run(cmd: list[str], **kwargs) -> subprocess.CompletedProcess:
    proc = subprocess.run(cmd, text=True, **kwargs)
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr or "")
        raise SystemExit(proc.returncode)
    return proc


def remill_lift(hexbytes: str, signature: str, ir_out: str) -> None:
    run(
        [
            find_remill(),
            "-arch", "amd64",
            "-os", "linux",
            "-address", "0",
            "-bytes", hexbytes,
            "-signature", signature,
            "-ir_out", ir_out,
        ],
        capture_output=True,
    )


def link_ir(lifted_path: str, symbol: str, isa: str, work: str) -> str:
    lifted = rename_entry(open(lifted_path).read(), symbol)
    lifted = devariadic_flags(lifted)
    lifted = retarget(lifted, isa)
    runtime = retarget(open(RUNTIME_LL).read(), isa)
    lifted_ll = os.path.join(work, f"lifted_{isa}.ll")
    runtime_ll = os.path.join(work, f"runtime_{isa}.ll")
    linked = os.path.join(work, f"full_{isa}.bc")
    open(lifted_ll, "w").write(lifted)
    open(runtime_ll, "w").write(runtime)
    run(
        [llvm_tool("llvm-link"), lifted_ll, runtime_ll, "-o", linked],
        capture_output=True,
    )
    return linked


def build_wasm(linked_bc: str, work: str) -> str:
    obj = os.path.join(work, "full.wasm.o")
    wasm = os.path.join(work, "lifted.wasm")
    run(
        [
            llvm_tool("llc"),
            "-O0",
            "-mtriple=wasm32-unknown-unknown",
            "-filetype=obj",
            linked_bc,
            "-o", obj,
        ],
        capture_output=True,
    )
    run(
        [
            llvm_tool("wasm-ld"),
            "--no-entry",
            "--export-all",
            "--export-memory",
            "--initial-memory=262144",
            "--max-memory=1048576",
            obj,
            "-o", wasm,
        ],
        capture_output=True,
    )
    return wasm


def runner_path() -> str:
    env = os.environ.get("CEREMONY_WASM")
    if env and os.path.isfile(env):
        return env
    candidate = os.path.join(ROOT, "ceremony-wasm", "target", "debug", "ceremony-wasm")
    if os.path.isfile(candidate):
        return candidate
    raise SystemExit(
        "ceremony-wasm runner is not built. From ceremony-wasm/: cargo build"
    )


def pulley_argv(wasm: str, symbol: str, arguments: list, expected, expected_outs) -> list[str]:
    cmd = [runner_path(), "--wasm", wasm, "--func", symbol]
    out_i = 0
    for arg in arguments:
        if isinstance(arg, int):
            cmd += ["--arg", str(int(arg) & 0xFFFFFFFFFFFFFFFF)]
        elif arg[0] == "mem":
            cmd += ["--mem", arg[1].hex()]
        elif arg[0] == "out":
            cmd += ["--out", arg[1]]
            if expected_outs and out_i < len(expected_outs) and expected_outs[out_i] is not None:
                cmd += ["--expect-out", str(expected_outs[out_i])]
            out_i += 1
        else:
            raise SystemExit(f"bad argument {arg!r}")
    if expected is not None:
        cmd += ["--expect", str(expected)]
    return cmd


def render_harness(symbol: str, arguments: list, expected, expected_outs) -> str:
    decls = []
    params = []
    mem_i = 0
    out_i = 0
    out_names = []
    for arg in arguments:
        if isinstance(arg, int):
            params.append(f"{int(arg) & 0xFFFFFFFFFFFFFFFF}ull")
        elif arg[0] == "mem":
            name = f"mem{mem_i}"
            body = ", ".join(str(b) for b in arg[1])
            decls.append(f"static uint8_t {name}[] = {{{body}}};")
            params.append(f"(uint64_t)(uintptr_t){name}")
            mem_i += 1
        elif arg[0] == "out":
            name = f"out{out_i}"
            ctype = "uint32_t" if arg[1] == "u32" else "uint64_t"
            decls.append(f"static {ctype} {name};")
            params.append(f"(uint64_t)(uintptr_t)&{name}")
            out_names.append((name, arg[1]))
            out_i += 1
    formals = ", ".join(["uint64_t"] * len(params)) or "void"
    prints = ['printf("QEMU %llu", (unsigned long long)r);']
    checks = []
    if expected is not None:
        checks.append(f"if (r != {int(expected)}ull) bad = 1;")
    for i, (name, ty) in enumerate(out_names):
        conv = "u" if ty == "u32" else "llu"
        cast = "(unsigned)" if ty == "u32" else "(unsigned long long)"
        prints.append(f'printf(" {conv_placeholder(conv)}", {cast}{name});')
        if expected_outs and i < len(expected_outs) and expected_outs[i] is not None:
            checks.append(f"if ({name} != {int(expected_outs[i])}u) bad = 1;")
    prints.append(r'printf("\n");')
    return "\n".join(
        [
            "#include <stdint.h>",
            "#include <stdio.h>",
            f"uint64_t {symbol}({formals});",
            "int main(void) {",
            "  int bad = 0;",
            *["  " + d for d in decls],
            f"  uint64_t r = {symbol}({', '.join(params)});",
            *["  " + p for p in prints],
            *["  " + c for c in checks],
            "  return bad;",
            "}",
            "",
        ]
    )


def conv_placeholder(conv: str) -> str:
    return "%u" if conv == "u" else "%llu"


def run_qemu(linked_bc: str, symbol: str, arguments, expected, expected_outs, work: str) -> str:
    gcc = shutil.which("riscv64-linux-gnu-gcc")
    qemu = shutil.which("qemu-riscv64-static") or shutil.which("qemu-riscv64")
    if not gcc or not qemu:
        return "SKIP (riscv64 gcc or qemu-user not installed)"
    obj = os.path.join(work, "full.riscv.o")
    harness = os.path.join(work, "harness.c")
    prog = os.path.join(work, "lifted.riscv")
    open(harness, "w").write(render_harness(symbol, arguments, expected, expected_outs))
    # clang picks the lp64d ABI the riscv64 cross gcc links against.
    run(
        [
            llvm_tool("clang"),
            "--target=riscv64-linux-gnu",
            "--sysroot=/usr/riscv64-linux-gnu",
            "-O0",
            "-c",
            linked_bc,
            "-o", obj,
        ],
        capture_output=True,
    )
    run([gcc, "-O1", harness, obj, "-o", prog], capture_output=True)
    qemu_cmd = [qemu]
    if os.path.isdir("/usr/riscv64-linux-gnu"):
        qemu_cmd += ["-L", "/usr/riscv64-linux-gnu"]
    qemu_cmd.append(prog)
    proc = subprocess.run(qemu_cmd, text=True, capture_output=True)
    sys.stdout.write(proc.stdout)
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        raise SystemExit(f"qemu-riscv64 failed for {symbol} (exit {proc.returncode})")
    return proc.stdout.strip()


def lift_and_drop(
    *,
    obj: str | None,
    hexbytes: str | None,
    symbol: str,
    signature: str,
    arguments: list,
    expected=None,
    expected_outs=None,
    qemu: bool = True,
    keep_wasm: str | None = None,
) -> dict:
    if hexbytes is None:
        if not obj:
            raise SystemExit("need an object file or raw bytes")
        hexbytes = extract_bytes(obj, symbol)
    code = bytes.fromhex(hexbytes)
    spec_blob = anvill_spec.build_spec(symbol, code, signature, address=0)
    spec = anvill_spec.load_spec(spec_blob)
    spec_sig = anvill_spec.signature_of(spec)
    spec_code = anvill_spec.code_of(spec)
    if spec_sig.replace(" ", "") != signature.replace(" ", "").upper():
        raise SystemExit(f"anvill spec signature {spec_sig} != {signature}")
    if spec_code != code:
        raise SystemExit("anvill spec dropped executable bytes")
    if anvill_spec.symbol_of(spec).lstrip("_") != symbol.lstrip("_"):
        raise SystemExit("anvill spec symbol mismatch")

    print("=" * 67)
    print(f" [SPEC]  anvill protobuf  {symbol}  {spec_sig}  {len(spec_code)} bytes")
    print("=" * 67)

    work = tempfile.mkdtemp(prefix="liftdrop-")
    try:
        lifted_ll = os.path.join(work, "lifted.ll")
        print(" [LIFT]  remill-lift -> LLVM IR")
        remill_lift(spec_code.hex(), spec_sig, lifted_ll)

        print(" [DROP]  Cranelift -> Pulley interpreter")
        linked_wasm = link_ir(lifted_ll, symbol, "wasm32", work)
        wasm = build_wasm(linked_wasm, work)
        if keep_wasm:
            shutil.copy(wasm, keep_wasm)
        proc = subprocess.run(
            pulley_argv(wasm, symbol, arguments, expected, expected_outs),
            text=True,
            capture_output=True,
        )
        sys.stdout.write(proc.stdout or "")
        if proc.returncode != 0:
            sys.stderr.write(proc.stderr or "")
            raise SystemExit(proc.returncode)
        if expected is not None and "RESULT   : OK" not in proc.stdout:
            raise SystemExit(f"Pulley validation failed for {symbol}")

        qemu_out = ""
        if qemu:
            print(" [EMU]   same LLVM IR -> riscv64 -> qemu-user")
            linked_rv = link_ir(lifted_ll, symbol, "riscv64", work)
            qemu_out = run_qemu(linked_rv, symbol, arguments, expected, expected_outs, work)
        return {"symbol": symbol, "wasm": wasm, "pulley": proc.stdout, "qemu": qemu_out}
    finally:
        shutil.rmtree(work, ignore_errors=True)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description="Lift a native function onto Pulley and qemu-riscv64"
    )
    parser.add_argument("--object", help="native object or executable")
    parser.add_argument("--bytes", help="hex machine-code bytes instead of an object")
    parser.add_argument("--symbol", required=True)
    parser.add_argument("--signature", default="RAX(RDI,RSI)")
    parser.add_argument("--arg", action="append", type=int, default=[])
    parser.add_argument("--expect", type=int)
    parser.add_argument("--no-qemu", action="store_true")
    ns = parser.parse_args(argv)
    if not ns.object and not ns.bytes:
        parser.error("pass --object or --bytes")
    lift_and_drop(
        obj=ns.object,
        hexbytes=ns.bytes,
        symbol=ns.symbol,
        signature=ns.signature,
        arguments=list(ns.arg),
        expected=ns.expect,
        qemu=not ns.no_qemu,
        keep_wasm=os.path.join(ROOT, "ceremony", "lifted_out.wasm"),
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
