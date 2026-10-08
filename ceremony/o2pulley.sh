#!/usr/bin/env bash
#
# o2pulley.sh -- ONE ENTRY POINT for the Cranelift x Pulley ceremony-mode pipeline.
#
#   native .o  --remill-lift-->  LLVM IR  --(+stub runtime)-->  llc(wasm32)
#        --wasm-ld-->  .wasm  --wasmtime/pulley-->  RESULT
#
# Cranelift has no machine-code decoder, so the lift is done by remill (the part
# Cranelift cannot do). The drop is always the same shared core: Cranelift
# compiles the wasm to Pulley bytecode and the Pulley interpreter runs it.
#
# Usage:
#   o2pulley.sh <object.o> <symbol> [a] [b]
#   o2pulley.sh --bytes <hex> <symbol> [a] [b]        # lift raw bytes directly
#
# Example:
#   o2pulley.sh sign_x86_64.o ceremony_op 3 4
#
set -euo pipefail

# ---- toolchain locations (LLVM 16.0.6 standardized for the whole chain) --------
LLVM16_BIN=/opt/homebrew/opt/llvm@16/bin
REMILL_BIN=/tmp/remill/install/bin
CEREMONY_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUNNER="$CEREMONY_DIR/../ceremony-wasm/target/debug/ceremony-wasm"
STUB="$CEREMONY_DIR/remill_runtime_stub.ll"
EXTRACT="$CEREMONY_DIR/extract_bytes.py"

LIFT="$REMILL_BIN/remill-lift-16"
LLINK="$LLVM16_BIN/llvm-link"
LLVM_DIS="$LLVM16_BIN/llvm-dis"
LLC="$LLVM16_BIN/llc"
WLD="$LLVM16_BIN/wasm-ld"

# ---- args --------------------------------------------------------------------
BYTES=""
SYMBOL=""
A=3
B=4
if [ "${1:-}" = "--bytes" ]; then
  BYTES="$2"; SYMBOL="${3:-call_sub_0}"; A="${4:-3}"; B="${5:-4}"
else
  OBJ="$1"; SYMBOL="${2:-ceremony_op}"; A="${3:-3}"; B="${4:-4}"
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "==================================================================="
echo " [LIFT]  native code -> remill -> LLVM IR"
echo "==================================================================="
if [ -z "$BYTES" ]; then
  echo "  object : $OBJ"
  echo "  symbol : $SYMBOL"
  BYTES="$("$EXTRACT" "$OBJ" "$SYMBOL")"
fi
echo "  bytes  : ${BYTES:0:32}$([ -n "$BYTES" ] && [ ${#BYTES} -gt 32 ] && echo '...' || true)"
[ -n "$BYTES" ] || { echo "  ERROR: could not extract bytes for $SYMBOL" >&2; exit 1; }

# remill-lift writes TEXT IR regardless of the .bc extension.
"$LIFT" -arch amd64 -os linux -address 0 -bytes "$BYTES" \
        -ir_out "$WORK/lifted.ll" -signature "RAX(RDI,RSI)"

# Rewrite datalayout + triple to wasm32 so llc can target it, and rename the
# lifted entry function (remill names it call_sub_0) to the source symbol.
python3 - "$WORK/lifted.ll" "$SYMBOL" <<'PY'
import re, sys
p, sym = sys.argv[1], sys.argv[2]
s = open(p).read()
s = re.sub(r'target datalayout = "[^"]*"',
           'target datalayout = "e-m:e-p:32:32-i64:64-n32:64-S128"', s)
s = re.sub(r'target triple = "[^"]*"',
           'target triple = "wasm32-unknown-unknown"', s)
s = s.replace("@call_sub_0", "@" + sym)
open(p, "w").write(s)
PY

echo
echo "==================================================================="
echo " [LINK]  remill runtime intrinsics (stub) + lifted IR"
echo "==================================================================="
"$LLINK" "$WORK/lifted.ll" "$STUB" -o "$WORK/full.bc"

echo
echo "==================================================================="
echo " [DROP]  llc(wasm32) -> wasm-ld -> wasm  (Cranelift target = pulley64)"
echo "==================================================================="
"$LLC" -mtriple=wasm32 -filetype=obj "$WORK/full.bc" -o "$WORK/full.o"
"$WLD" --no-entry --export-all "$WORK/full.o" -o "$WORK/out.wasm"
cp "$WORK/out.wasm" "$CEREMONY_DIR/lifted_out.wasm"
echo "  wrote  : $CEREMONY_DIR/lifted_out.wasm"

echo
echo "==================================================================="
echo " [RUN]   wasmtime (Cranelift -> Pulley interpreter)"
echo "==================================================================="
"$RUNNER" "$CEREMONY_DIR/lifted_out.wasm" "$SYMBOL" "$A" "$B"
