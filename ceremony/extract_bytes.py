#!/usr/bin/env python3
"""Extract the raw machine-code bytes of a function from a native object file.

Used by o2pulley.sh so a native `.o` can be fed straight into remill-lift
without manually copying hex. Parses `objdump -d` (Mach-O / ELF) and collects
the instruction bytes belonging to the requested symbol.
"""
import sys
import re
import subprocess


def main() -> int:
    if len(sys.argv) < 3:
        print("usage: extract_bytes.py <object.o> <symbol>", file=sys.stderr)
        return 2
    obj, sym = sys.argv[1], sys.argv[2]

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
        if collecting:
            # Stop at the mnemonic. A trailing `\s*` would swallow hex letters
            # from the text, e.g. the "ad" in "add %ebx,%eax", and remill would
            # lift those letters as real opcodes.
            m = re.match(
                r"^\s*[0-9a-fA-F]+:\s+((?:[0-9a-fA-F]{2}[ \t])*[0-9a-fA-F]{2})\b",
                line,
            )
            if m:
                out.extend(m.group(1).split())

    print("".join(out))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
