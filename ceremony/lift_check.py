#!/usr/bin/env python3
"""
lift_check.py — make the "already-compiled code" boundary concrete.

The original brief was to LIFT already-compiled blocks and DROP them onto a
different backend. Cranelift is DROP-only: it has no machine-code decoder. So:

  - sign_x86_64.o : native Mach-O machine code. Cranelift CANNOT consume this.
                    A lifter (remill/anvill) is required to raise it to IR.
  - sign.wasm     : a genuine ALREADY-COMPILED module (\0asm). Cranelift's Wasm
                    frontend (Wasmtime) CAN drop this onto any backend,
                    including Pulley. This is the Cranelift-consumable artifact.

This script parses sign.wasm's sections to prove it is real compiled code and
to show what a lifter/frontend would hand to the DROP stage.
"""

import struct
import sys

SECTION_NAMES = {
    0: "custom", 1: "type", 2: "import", 3: "function", 4: "table",
    5: "memory", 6: "global", 7: "export", 8: "start", 9: "element",
    10: "code", 11: "data", 12: "data count",
}


def read_uleb(data, pos):
    """Read an unsigned LEB128 integer. Returns (value, new_pos)."""
    result = 0
    shift = 0
    while True:
        if pos >= len(data):
            raise ValueError("truncated LEB128")
        b = data[pos]
        result |= (b & 0x7F) << shift
        pos += 1
        if not (b & 0x80):
            break
        shift += 7
    return result, pos


def read_name(data, pos):
    """Read a Wasm length-prefixed name string. Returns (str, new_pos)."""
    n, pos = read_uleb(data, pos)
    s = data[pos:pos + n].decode("utf-8", errors="replace")
    return s, pos + n


def parse_wasm(path):
    with open(path, "rb") as f:
        data = f.read()
    if data[:4] != b"\x00asm":
        raise ValueError(f"{path}: not a wasm module (bad magic)")
    version = struct.unpack_from("<I", data, 4)[0]
    pos = 8
    sections = {}
    # track exports and function/code counts for the summary
    exports = []
    n_functions = None
    n_code = None
    while pos < len(data):
        sid, pos = read_uleb(data, pos)
        size, pos = read_uleb(data, pos)
        payload = data[pos:pos + size]
        name = SECTION_NAMES.get(sid, f"unknown({sid})")
        sections[name] = size
        # pull out a couple of human-readable facts
        if sid == 3:  # function section: count of function indices
            p = 0
            n_functions, p = read_uleb(payload, p)
        elif sid == 7:  # export section
            p = 0
            cnt, p = read_uleb(payload, p)
            for _ in range(cnt):
                nm, p = read_name(payload, p)
                kind, p = read_uleb(payload, p)
                idx, p = read_uleb(payload, p)
                exports.append((nm, kind, idx))
        elif sid == 10:  # code section: count of function bodies
            p = 0
            n_code, p = read_uleb(payload, p)
        pos += size
    return version, sections, exports, n_functions, n_code


def main():
    wasm = "sign.wasm"
    obj = "sign_x86_64.o"
    print("=" * 64)
    print("LIFT BOUNDARY CHECK — already-compiled artifacts")
    print("=" * 64)

    print("\n[1] sign_x86_64.o  (native Mach-O machine code)")
    try:
        with open(obj, "rb") as f:
            head = f.read(4)
        print(f"    magic bytes : {head.hex()}  (Mach-O 0xcffaedfe)")
        print("    Cranelift   : CANNOT consume — no x86 decoder in tree.")
        print("    Needs       : a binary lifter (remill/anvill) -> LLVM IR.")
    except FileNotFoundError:
        print("    (missing — run clang step first)")

    print("\n[2] sign.wasm  (genuine already-compiled module)")
    ver, sections, exports, nfunc, ncode = parse_wasm(wasm)
    print(f"    magic       : 0061 736d  ('\\0asm')  version {ver}")
    print(f"    sections    : {{{', '.join(f'{k}={v}B' for k, v in sections.items())}}}")
    print(f"    functions   : {nfunc} declared, {ncode} bodies")
    print(f"    exports     : {', '.join(nm for nm, _, _ in exports)}")
    print("    Cranelift   : CAN consume — Wasmtime's Cranelift frontend")
    print("                  drops this onto native OR Pulley + opcode permute.")
    print("\n    => This .wasm is the 'already-compiled' artifact the LIFT")
    print("       stage would produce / the DROP stage would consume.")
    print("=" * 64)


if __name__ == "__main__":
    sys.exit(main())
