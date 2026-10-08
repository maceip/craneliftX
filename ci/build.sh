#!/bin/bash
# Build every project binary for one Linux target, then record static
# libraries the compiler actually archived under target/.
#
# Usage: ci/build.sh <rust-target> <file(1) machine substring>
# Example: ci/build.sh aarch64-unknown-linux-gnu "ARM aarch64"

set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <rust-target> <file-machine-substring>" >&2
  exit 2
fi

TARGET="$1"
FILE_NEEDLE="$2"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

case "$TARGET" in
  x86_64-unknown-linux-gnu)
    CC_BIN=gcc
    AR_BIN=ar
    STRIP_BIN=strip
    ;;
  aarch64-unknown-linux-gnu)
    CC_BIN=aarch64-linux-gnu-gcc
    AR_BIN=aarch64-linux-gnu-ar
    STRIP_BIN=aarch64-linux-gnu-strip
    ;;
  riscv64gc-unknown-linux-gnu)
    CC_BIN=riscv64-linux-gnu-gcc
    AR_BIN=riscv64-linux-gnu-ar
    STRIP_BIN=riscv64-linux-gnu-strip
    ;;
  *)
    echo "unsupported target: $TARGET" >&2
    exit 2
    ;;
esac

for tool in "$CC_BIN" "$AR_BIN" "$STRIP_BIN" cargo rustc python3 file; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing tool: $tool" >&2
    exit 1
  fi
done

# cc-rs compiles C/asm for the *target* triple. Cargo's linker env is not
# enough on its own; the build script reads CC_<triple> and AR_<triple>.
triple_env="${TARGET//-/_}"
triple_upper="$(printf '%s' "$triple_env" | tr '[:lower:]' '[:upper:]')"
export "CC_${triple_env}=${CC_BIN}"
export "AR_${triple_env}=${AR_BIN}"
export "CARGO_TARGET_${triple_upper}_LINKER=${CC_BIN}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${ROOT}/target}"
export CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-always}"
export CARGO_REGISTRIES_CRATES_IO_PROTOCOL="${CARGO_REGISTRIES_CRATES_IO_PROTOCOL:-sparse}"
export PKG_CONFIG_ALLOW_CROSS=1

echo "rustc $(rustc --version)"
echo "building ${TARGET}"

build_crate() {
  local manifest="$1"
  echo "==> cargo build --release --locked --target ${TARGET} (${manifest})"
  cargo build --manifest-path "$manifest" --release --locked --target "$TARGET"
}

build_crate ceremony/Cargo.toml
build_crate ceremony-wasm/Cargo.toml
build_crate probe/Cargo.toml

mkdir -p dist
copy_bin() {
  local name="$1"
  local src="${CARGO_TARGET_DIR}/${TARGET}/release/${name}"
  local dest="${ROOT}/dist/${name}-${TARGET}"
  if [[ ! -f "$src" ]]; then
    echo "missing binary: $src" >&2
    exit 1
  fi
  cp "$src" "$dest"
  "$STRIP_BIN" --strip-unneeded "$dest"
  chmod 0755 "$dest"
  if ! file "$dest" | grep -F "$FILE_NEEDLE" >/dev/null; then
    echo "binary is not ${FILE_NEEDLE}: $(file "$dest")" >&2
    exit 1
  fi
  echo "built ${dest}"
}

copy_bin ceremony
copy_bin ceremony-wasm
copy_bin xisa-probe

sample_dest="${ROOT}/dist/sample_network-${TARGET}"
echo "==> ${CC_BIN} liftmap/sample_network.c"
"$CC_BIN" -O1 -fno-inline -o "$sample_dest" liftmap/sample_network.c
"$STRIP_BIN" --strip-unneeded "$sample_dest"
chmod 0755 "$sample_dest"
if ! file "$sample_dest" | grep -F "$FILE_NEEDLE" >/dev/null; then
  echo "sample_network is not ${FILE_NEEDLE}: $(file "$sample_dest")" >&2
  exit 1
fi

if [[ "$TARGET" == "x86_64-unknown-linux-gnu" ]]; then
  echo "==> smoke x86_64 binaries"
  # TRIALS=0 runs the functional Pulley path and skips the random-payload
  # child processes, which can hang the interpreter.
  timeout 60 env TRIALS=0 "${ROOT}/dist/ceremony-${TARGET}" | tee /tmp/ceremony-smoke.txt
  grep -F "ran to completion" /tmp/ceremony-smoke.txt >/dev/null
  timeout 60 "${ROOT}/dist/xisa-probe-${TARGET}" | tee /tmp/probe-smoke.txt
  for isa in x86_64 aarch64 riscv64 s390x; do
    if ! grep -E "^${isa}[[:space:]]" /tmp/probe-smoke.txt | grep -v SKIP >/dev/null; then
      echo "xisa-probe did not compile ${isa}" >&2
      exit 1
    fi
  done
  timeout 60 "${ROOT}/dist/ceremony-wasm-${TARGET}" \
    "${ROOT}/ceremony/sign.wasm" ceremony_op 3 4 15 | tee /tmp/wasm-smoke.txt
  grep -F "RESULT   : OK" /tmp/wasm-smoke.txt >/dev/null
fi

if [[ "${BUILD_LIFTERS:-0}" == "1" ]]; then
  for tool in cmake ninja llvm-config-18 git; do
    if ! command -v "$tool" >/dev/null 2>&1; then
      echo "missing lifter tool: $tool" >&2
      exit 1
    fi
  done
  echo "==> pipeline/build_lifters.sh"
  JOBS="${LIFTER_JOBS:-$(nproc)}" pipeline/build_lifters.sh
  shopt -s nullglob
  lifters=("${ROOT}/build/remill-install/bin/"remill-lift*)
  shopt -u nullglob
  if [[ ${#lifters[@]} -eq 0 ]]; then
    echo "remill-lift was not installed" >&2
    exit 1
  fi
  for bin in "${lifters[@]}"; do
    dest="${ROOT}/dist/$(basename "$bin")-${TARGET}"
    cp "$bin" "$dest"
    "$STRIP_BIN" --strip-unneeded "$dest" || true
    chmod 0755 "$dest"
    if ! file "$dest" | grep -F "$FILE_NEEDLE" >/dev/null; then
      echo "lifter is not ${FILE_NEEDLE}: $(file "$dest")" >&2
      exit 1
    fi
    echo "built ${dest}"
  done
fi

pin_args=()
for pin in vendor/remill/dependencies/CMakeLists.txt vendor/remill/dependencies/xed.cmake; do
  if [[ -f "${ROOT}/${pin}" ]]; then
    pin_args+=(--pin "${ROOT}/${pin}")
  fi
done
archive_args=(--archive-root "${CARGO_TARGET_DIR}")
if [[ -d "${ROOT}/build" ]]; then
  archive_args+=(--archive-root "${ROOT}/build")
fi

python3 ci/static_libs.py \
  --lock ceremony/Cargo.lock ceremony-wasm/Cargo.lock probe/Cargo.lock \
  --target-dir "${CARGO_TARGET_DIR}" \
  --cargo-home "${CARGO_HOME:-${HOME}/.cargo}" \
  "${pin_args[@]}" \
  "${archive_args[@]}" \
  --supplement-out "${ROOT}/dist/static-libs-${TARGET}.json"

echo "done ${TARGET}"
