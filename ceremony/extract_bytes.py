#!/usr/bin/env python3
"""Extract the raw machine-code bytes of a function from a native object file.

Used by o2pulley.sh so a native `.o` can be fed straight into remill-lift
without manually copying hex. Parses `objdump -d` (Mach-O / ELF) and collects
the instruction bytes belonging to the requested symbol.

The ingest tracer (liftmap/ingest_tracer.py) is the authority on WHERE a
function's bytes live: it passes --start/--end (the exact address window it
recovered, with trailing alignment NOPs already trimmed). When those are given,
this script restricts the slice to that window so the lift never bleeds into a
neighbouring function or pulls padding bytes the tracer rejected.
"""
import sys
import re
import argparse
import subprocess


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("obj")
    ap.add_argument("symbol")
    ap.add_argument("--start", default=None,
                    help="hex start address; collect only bytes in [start, end)")
    ap.add_argument("--end", default=None,
                    help="hex end address (exclusive)")
    ns = ap.parse_args()
    obj, sym = ns.obj, ns.symbol
    start = int(ns.start, 16) if ns.start else None
    end = int(ns.end, 16) if ns.end else None

    # Mach-O C symbols are prefixed with '_'; ELF are not.
    candidates = {sym, "_" + sym} if not sym.startswith("_") else {sym}

    dis = subprocess.run(["objdump", "-d", obj], capture_output=True, text=True)
    if dis.returncode != 0:
        sys.stderr.write(dis.stderr)
        return dis.returncode

    collecting = False
    out = []
    for line in dis.stdout.splitlines():
        stripped = line.strip()
        if stripped.endswith(">:"):
            name = stripped[stripped.index("<") + 1 : stripped.rindex(">:")]
            if name in candidates:
                collecting = True
                continue
            if collecting:
                break  # reached the next symbol -> stop
            continue
        if not collecting:
            continue
        m = re.match(
            r"^\s*([0-9a-fA-F]+):\s+((?:[0-9a-fA-F]{2}[ \t])*[0-9a-fA-F]{2})\b",
            line,
        )
        if not m:
            continue
        addr = int(m.group(1), 16)
        if start is not None and addr < start:
            continue  # before the tracer's window
        if end is not None and addr >= end:
            if out:
                break  # past the window and we already have bytes
            continue
        out.extend(m.group(2).split())

    print("".join(out))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
