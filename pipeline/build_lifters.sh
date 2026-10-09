#!/usr/bin/env bash
# Build vendored remill against system LLVM 20, then anvill-decompile-spec
# against that install. Remill stays the instruction lifter. Anvill reads the
# protobuf spec this repo already writes and runs its cleanup passes.
#
# Anvill is configured against build/remill-install, not vendor/anvill's
# bundled remill submodule. One LLVM major: LLVM 20, the one remill links.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${REMILL_PREFIX:-$ROOT/build/remill-install}"
SRC="$ROOT/vendor/remill"
ANVILL_SRC="$ROOT/vendor/anvill"

if [[ ! -f "$SRC/CMakeLists.txt" ]]; then
  echo "vendored remill is missing at $SRC" >&2
  exit 1
fi
if [[ ! -f "$ANVILL_SRC/CMakeLists.txt" ]]; then
  echo "vendored anvill is missing at $ANVILL_SRC" >&2
  exit 1
fi

shopt -s nullglob
existing=("$PREFIX"/bin/remill-lift-20)
anvill_bin="$PREFIX/bin/anvill-decompile-spec"
need_remill=0
need_anvill=0
if [[ ! -x "${existing[0]:-}" ]]; then
  need_remill=1
fi
if [[ ! -x "$anvill_bin" ]]; then
  need_anvill=1
fi
if (( need_remill == 0 && need_anvill == 0 )); then
  echo "remill-lift already installed: ${existing[0]}"
  echo "anvill-decompile-spec already installed: $anvill_bin"
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

if (( need_remill )); then
  # Sleigh is linked into remill-lift. Build it in the dependency superbuild
  # (ENABLE_SLEIGH) and point remill at that install (REMILL_FETCH_SLEIGH=OFF),
  # which is the configuration remill's own CI uses. A previous configure can
  # cache CLANG_PATH=NOTFOUND; -UCLANG_PATH makes cmake search again once
  # clang-20 is installed next to llvm-link.
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
fi

if (( need_anvill )); then
  # Ubuntu's libz3-dev has headers and libz3.so, not a CMake package. Anvill's
  # configure requires find_package(Z3 CONFIG).
  Z3_CMAKE="$ROOT/build/cmake"
  if [[ ! -f "$Z3_CMAKE/lib/cmake/Z3/Z3Config.cmake" ]]; then
    z3_lib="$(pkg-config --variable=libdir z3 2>/dev/null || true)/libz3.so"
    if [[ ! -f "$z3_lib" ]]; then
      z3_lib="/usr/lib/x86_64-linux-gnu/libz3.so"
    fi
    if [[ ! -f "$z3_lib" ]]; then
      echo "libz3 is required to configure anvill (apt install libz3-dev)" >&2
      exit 1
    fi
    z3_inc="$(pkg-config --variable=includedir z3 2>/dev/null || echo /usr/include)"
    mkdir -p "$Z3_CMAKE/lib/cmake/Z3"
    cat > "$Z3_CMAKE/lib/cmake/Z3/Z3Config.cmake" <<EOF
add_library(z3::libz3 SHARED IMPORTED)
set_target_properties(z3::libz3 PROPERTIES
  IMPORTED_LOCATION "${z3_lib}"
  INTERFACE_INCLUDE_DIRECTORIES "${z3_inc}"
)
set(Z3_FOUND TRUE)
EOF
  fi

  cmake -G Ninja -S "$ANVILL_SRC" -B "$ROOT/build/anvill" \
    -DCMAKE_BUILD_TYPE=Release \
    -DANVILL_ENABLE_TESTS=OFF \
    -DANVILL_ENABLE_PYTHON3_LIBS=OFF \
    -DANVILL_ENABLE_INSTALL=ON \
    -DCMAKE_PREFIX_PATH="$PREFIX;$DEPS_INSTALL;$LLVM_PREFIX;$Z3_CMAKE" \
    -DCMAKE_INSTALL_PREFIX="$PREFIX" \
    -DLLVM_DIR="$LLVM_DIR" \
    -Dremill_DIR="$PREFIX/lib/cmake/remill"
  cmake --build "$ROOT/build/anvill" -j "$JOBS"
  cmake --install "$ROOT/build/anvill"
  echo "installed anvill-decompile-spec into $PREFIX"
fi

ls -1 "$PREFIX"/bin
