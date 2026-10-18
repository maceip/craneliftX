#!/usr/bin/env bash
# static-link.sh — convert a dynamically-linked binary to a static one
# suitable for the ISR hardened runtime.
#
# Usage:
#   ./scripts/static-link.sh /path/to/dynamic-binary /path/to/output-static-binary
#
# This script tries several strategies in order:
# 1. If the binary is already static, copy it verbatim.
# 2. If musl-gcc is available and the binary was built from source,
#    suggest recompiling with -static.
# 3. Use patchelf + static-linker bundling (best effort).
# 4. Use `ldd` to enumerate libraries and create a self-extracting bundle.

set -euo pipefail

INPUT="${1:-}"
OUTPUT="${2:-}"

if [ -z "$INPUT" ] || [ -z "$OUTPUT" ]; then
  echo "Usage: $0 <input-binary> <output-static-binary>"
  exit 1
fi

if [ ! -f "$INPUT" ]; then
  echo "Error: input binary not found: $INPUT"
  exit 1
fi

echo "Analyzing $INPUT ..."
FILE_INFO=$(file "$INPUT")
echo "  $FILE_INFO"

# Check if already static
if echo "$FILE_INFO" | grep -q "statically linked"; then
  echo "Binary is already statically linked. Copying to $OUTPUT."
  cp "$INPUT" "$OUTPUT"
  chmod +x "$OUTPUT"
  exit 0
fi

# Check architecture
if ! echo "$FILE_INFO" | grep -q "x86-64"; then
  echo "Error: ISR runtime requires x86-64 binaries."
  exit 1
fi

echo "Binary is dynamically linked. Attempting conversion strategies ..."

# Strategy 1: patchelf to change interpreter and RPATH
if command -v patchelf >/dev/null 2>&1; then
  echo "  [1] patchelf available — copying with patched interpreter"
  cp "$INPUT" "$OUTPUT"
  # Try to find a static interpreter (musl)
  if [ -f /lib/ld-musl-x86_64.so.1 ]; then
    patchelf --set-interpreter /lib/ld-musl-x86_64.so.1 "$OUTPUT" 2>/dev/null || true
  fi
  # Note: this does NOT create a truly static binary, but it may work
  # if the ISR runtime is extended to handle dynamic libraries.
  echo "  Warning: patchelf produces a dynamic binary. Full static linking"
  echo "  requires recompilation with -static or musl-gcc."
  exit 0
fi

# Strategy 2: ldd + bundle
echo "  [2] Enumerating dependencies with ldd ..."
if command -v ldd >/dev/null 2>&1; then
  ldd "$INPUT" || true
fi

echo ""
echo "=== Summary ==="
echo "The ISR runtime currently requires STATIC x86-64 ELF binaries."
echo "Dynamic linking is not yet supported (shared library lifting is future work)."
echo ""
echo "To make your binary compatible, rebuild it with static linking:"
echo ""
echo "  C/C++ with gcc/clang:"
echo "    gcc -static -O2 -o output input.c"
echo ""
echo "  C/C++ with musl (smaller binaries):"
echo "    musl-gcc -static -O2 -o output input.c"
echo ""
echo "  Rust:"
echo "    RUSTFLAGS='-C target-feature=+crt-static' cargo build --release --target x86_64-unknown-linux-gnu"
echo ""
echo "  Go:"
echo "    CGO_ENABLED=0 go build -ldflags '-extldflags -static'"
echo ""
echo "If you cannot rebuild from source, consider using a container"
echo "with all dependencies pre-installed, or wait for dynamic-link support."

exit 1
