#!/usr/bin/env bash
#
# o2pulley.sh -- one entry point for the lift/drop pipeline.
#
#   native .o -> anvill spec -> anvill-decompile-spec -> LLVM IR
#             -> Cranelift (wasm) -> Pulley interpreter
#             -> riscv64 -> qemu-user
#
# Usage:
#   o2pulley.sh <object.o> <symbol> [a] [b] [expected]
#   o2pulley.sh --bytes <hex> <symbol> [a] [b] [expected]
#   o2pulley.sh --signature 'RAX(RDI,RSI,RDX,RCX)' <object.o> <symbol> [args...] [expected]
#
set -euo pipefail

CEREMONY_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PY="$CEREMONY_DIR/../pipeline/lift_drop.py"

SIGNATURE="RAX(RDI,RSI)"
BYTES=""
if [ "${1:-}" = "--signature" ]; then
  SIGNATURE="$2"
  shift 2
fi
if [ "${1:-}" = "--bytes" ]; then
  BYTES="$2"
  SYMBOL="${3:-call_sub_0}"
  shift 3 || true
else
  OBJ="${1:?object file required}"
  SYMBOL="${2:?symbol required}"
  shift 2
fi

CMD=(python3 "$PY" --symbol "$SYMBOL" --signature "$SIGNATURE")
if [ -n "$BYTES" ]; then
  CMD+=(--bytes "$BYTES")
else
  CMD+=(--object "$OBJ")
fi

# Every number except a final expected value is an argument. The historical
# ceremony call passes exactly two inputs plus an expected result. When more
# numbers are present, the caller is using the signature's arity: if one extra
# number was given, it is the expected result.
NARGS="$(python3 - "$SIGNATURE" <<'PY'
import sys
sig = sys.argv[1]
inner = sig[sig.find("(") + 1 : -1]
print(0 if not inner.strip() else len(inner.split(",")))
PY
)"
NUMS=("$@")
if [ "${#NUMS[@]}" -eq $((NARGS + 1)) ]; then
  EXPECT="${NUMS[-1]}"
  unset 'NUMS[-1]'
  for n in "${NUMS[@]}"; do
    CMD+=(--arg "$n")
  done
  CMD+=(--expect "$EXPECT")
else
  for n in "${NUMS[@]}"; do
    CMD+=(--arg "$n")
  done
fi

exec "${CMD[@]}"
