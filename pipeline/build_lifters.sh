#!/usr/bin/env bash
# Build vendored remill against the pinned system LLVM major, then
# anvill-decompile-spec against that install. Remill stays the instruction
# lifter. Anvill reads the protobuf spec this repo already writes and runs its
# cleanup passes.
#
# Anvill is configured against build/remill-install, not vendor/anvill's
# bundled remill submodule.
#
# The LLVM major is the SINGLE SOURCE OF TRUTH: see .llvm-version at the repo
# root. Every tool name below is derived from it so the version cannot drift
# between this script, lift_drop.py, the CI container, and the release.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LLVM_MAJOR="$(cat "${ROOT}/.llvm-version" 2>/dev/null || echo 20)"
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
existing=("$PREFIX"/bin/remill-lift-"$LLVM_MAJOR")
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

# Make the pinned LLVM discoverable on Linux (apt: suffixed tools under
# /usr/lib/llvm-<n>) and macOS (Homebrew: unsuffixed tools under
# /opt/homebrew/opt/llvm@<n>). We lay down a shim directory that exposes both
# the suffixed names (llvm-config-<n>, clang-<n>, llc-<n>, ...) the rest of
# this script and lift_drop.py expect and the unsuffixed names remill's
# BCCompiler.cmake probes for. Nothing downstream changes.
setup_llvm_shims() {
  local major="$1"
  local real_prefix=""
  if command -v "llvm-config-${major}" >/dev/null; then
    real_prefix="$(llvm-config-"${major}" --prefix)"
  elif command -v "llvm-config" >/dev/null \
       && [[ "$(llvm-config --version 2>/dev/null | cut -d. -f1)" == "$major" ]]; then
    real_prefix="$(llvm-config --prefix)"
  else
    for hp in "/opt/homebrew/opt/llvm@${major}" "/usr/local/opt/llvm@${major}"; do
      if [[ -x "$hp/bin/llvm-config" ]]; then real_prefix="$hp"; break; fi
    done
  fi
  if [[ -z "$real_prefix" ]]; then
    echo "LLVM ${major} not found. Linux: apt install llvm-${major}-dev clang-${major}; macOS: brew install llvm@${major}" >&2
    exit 1
  fi
  local shim_dir="$ROOT/build/llvm-shims-${major}/bin"
  mkdir -p "$shim_dir"
  local bindir="$real_prefix/bin"
  local tool
  for tool in llvm-config clang clang++ llc llvm-link opt wasm-ld ld.lld; do
    local real="$bindir/$tool"
    [[ "$tool" == "wasm-ld" && ! -x "$real" ]] && real="$bindir/ld.lld"
    [[ -x "$real" ]] || continue
    ln -sf "$real" "$shim_dir/$tool"
    ln -sf "$real" "$shim_dir/${tool}-${major}"
  done
  echo "$shim_dir"
}

LLVM_SHIMS="$(setup_llvm_shims "$LLVM_MAJOR")"
export PATH="$LLVM_SHIMS:$PATH"

LLVM_PREFIX="$(llvm-config-"$LLVM_MAJOR" --prefix)"
LLVM_DIR="$(llvm-config-"$LLVM_MAJOR" --cmakedir)"
DEPS_INSTALL="$ROOT/build/remill-deps/install"
JOBS="${JOBS:-2}"
# Host compiler matches the one LLVM the lifter links. On macOS this is the
# Homebrew llvm@<n> clang via the shims above.
export CC="${CC:-clang-"$LLVM_MAJOR"}"
export CXX="${CXX:-clang++-"$LLVM_MAJOR"}"

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
    z3_lib=""
    for cand in \
        "$(pkg-config --variable=libdir z3 2>/dev/null)/libz3.so" \
        "$(pkg-config --variable=libdir z3 2>/dev/null)/libz3.dylib" \
        /usr/lib/x86_64-linux-gnu/libz3.so \
        /opt/homebrew/lib/libz3.dylib ; do
      if [[ -f "$cand" ]]; then z3_lib="$cand"; break; fi
    done
    if [[ -z "$z3_lib" ]]; then
      echo "libz3 is required to configure anvill (apt install libz3-dev / brew install z3)" >&2
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
