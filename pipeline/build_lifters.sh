#!/usr/bin/env bash
# Build the vendored remill against the system LLVM and install it under
# build/remill-install. Anvill's C++ frontend (vendor/anvill, 2023-07-05) is
# spec-driven and does not compile against LLVM 18; the pipeline emits Anvill
# specification protobufs and lifts with this remill.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${REMILL_PREFIX:-$ROOT/build/remill-install}"
SRC="$ROOT/vendor/remill"

if [[ ! -f "$SRC/CMakeLists.txt" ]]; then
  echo "vendored remill is missing at $SRC" >&2
  exit 1
fi

shopt -s nullglob
existing=("$PREFIX"/bin/remill-lift-*)
if (( ${#existing[@]} > 0 )); then
  echo "remill-lift already installed: ${existing[0]}"
  exit 0
fi

if ! command -v llvm-config-18 >/dev/null; then
  echo "llvm-config-18 is required (apt install llvm-18-dev)" >&2
  exit 1
fi

LLVM_PREFIX="$(llvm-config-18 --prefix)"
LLVM_DIR="$(llvm-config-18 --cmakedir)"
DEPS_INSTALL="$ROOT/build/remill-deps/install"
JOBS="${JOBS:-2}"

cmake -G Ninja -S "$SRC/dependencies" -B "$ROOT/build/remill-deps" \
  -DUSE_EXTERNAL_LLVM=ON \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_PREFIX_PATH="$LLVM_PREFIX" \
  -DCMAKE_INSTALL_PREFIX="$DEPS_INSTALL" \
  -DLLVM_DIR="$LLVM_DIR"
cmake --build "$ROOT/build/remill-deps"

cmake -G Ninja -S "$SRC" -B "$ROOT/build/remill" \
  -DCMAKE_BUILD_TYPE=Release \
  -DREMILL_ENABLE_TESTING=OFF \
  -DCMAKE_PREFIX_PATH="$DEPS_INSTALL;$LLVM_PREFIX" \
  -DCMAKE_INSTALL_PREFIX="$PREFIX" \
  -DLLVM_DIR="$LLVM_DIR"
cmake --build "$ROOT/build/remill" -j "$JOBS"
cmake --install "$ROOT/build/remill"

echo "installed remill into $PREFIX"
ls -1 "$PREFIX"/bin
