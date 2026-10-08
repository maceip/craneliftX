#!/usr/bin/env bash
# Build the vendored remill against system LLVM 20 and install it under
# build/remill-install. The Anvill decompiler stays unbuilt on purpose:
# vendor/anvill is only the protobuf schema (specification.proto does not
# depend on LLVM). This script does not configure that tree. The pipeline
# emits Anvill specification protobufs and lifts with this remill.
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

if ! command -v llvm-config-20 >/dev/null; then
  echo "llvm-config-20 is required (system LLVM 20: apt install llvm-20-dev clang-20)" >&2
  exit 1
fi

LLVM_PREFIX="$(llvm-config-20 --prefix)"
LLVM_DIR="$(llvm-config-20 --cmakedir)"
DEPS_INSTALL="$ROOT/build/remill-deps/install"
JOBS="${JOBS:-2}"
# Host compiler matches the one LLVM the lifter links. The default c++ on
# this image is clang 18, which is a different major.
export CC="${CC:-clang-20}"
export CXX="${CXX:-clang++-20}"

# Sleigh is linked into remill-lift. Build it in the dependency superbuild
# (ENABLE_SLEIGH) and point remill at that install (REMILL_FETCH_SLEIGH=OFF),
# which is the configuration remill's own CI uses. A previous configure can
# cache CLANG_PATH=NOTFOUND; -UCLANG_PATH makes cmake search again once
# clang-18 is installed next to llvm-link.
cmake -G Ninja -S "$SRC/dependencies" -B "$ROOT/build/remill-deps" \
  -DUSE_EXTERNAL_LLVM=ON \
  -DENABLE_SLEIGH=ON \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_PREFIX_PATH="$LLVM_PREFIX" \
  -DCMAKE_INSTALL_PREFIX="$DEPS_INSTALL" \
  -DLLVM_DIR="$LLVM_DIR"
cmake --build "$ROOT/build/remill-deps"

cmake -G Ninja -S "$SRC" -B "$ROOT/build/remill" \
  -DCMAKE_BUILD_TYPE=Release \
  -DREMILL_ENABLE_TESTING=OFF \
  -DREMILL_FETCH_SLEIGH=OFF \
  -UCLANG_PATH \
  -DCMAKE_PREFIX_PATH="$DEPS_INSTALL;$LLVM_PREFIX" \
  -DCMAKE_INSTALL_PREFIX="$PREFIX" \
  -DLLVM_DIR="$LLVM_DIR"
cmake --build "$ROOT/build/remill" -j "$JOBS"
cmake --install "$ROOT/build/remill"

echo "installed remill into $PREFIX"
ls -1 "$PREFIX"/bin
