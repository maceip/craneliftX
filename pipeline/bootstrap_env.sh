#!/usr/bin/env bash
# Provision the dev environment the native -> Pulley pipeline needs:
#
#   1. the pinned LLVM major (from .llvm-version), with dev headers/libs
#   2. a python venv with capstone (tracer) + protobuf (Anvill spec writer)
#
# Idempotent: safe to re-run; it verifies first and only fills what is missing.
# See ENVIRONMENT.md for the full dependency scope and known failure modes.
#
# Usage: pipeline/bootstrap_env.sh [--llvm-dir DIR]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LLVM_MAJOR="$(cat "${ROOT}/.llvm-version" 2>/dev/null || echo 20)"
VENV="${ROOT}/.venv"
LLVM_DIR_DEFAULT="${HOME}/llvm${LLVM_MAJOR}"
LLVM_DIR_CUSTOM=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --llvm-dir) LLVM_DIR_CUSTOM="${2:-}"; shift 2 ;;
    *) echo "usage: $0 [--llvm-dir DIR]" >&2; exit 2 ;;
  esac
done

log()  { printf '[bootstrap] %s\n' "$*"; }
warn() { printf '[bootstrap] WARN %s\n' "$*" >&2; }

# --- locate an existing LLVM ${LLVM_MAJOR} -----------------------------------
find_llvm() {
  if command -v "llvm-config-${LLVM_MAJOR}" >/dev/null 2>&1; then
    "llvm-config-${LLVM_MAJOR}" --prefix; return 0
  fi
  if command -v llvm-config >/dev/null 2>&1; then
    local v
    v="$(llvm-config --version 2>/dev/null | cut -d. -f1 || true)"
    if [[ "$v" == "$LLVM_MAJOR" ]]; then llvm-config --prefix; return 0; fi
  fi
  local p
  for p in "${LLVM_DIR_CUSTOM}" \
           "/opt/homebrew/opt/llvm@${LLVM_MAJOR}" \
           "/usr/local/opt/llvm@${LLVM_MAJOR}" \
           "${LLVM_DIR_DEFAULT}"; do
    [[ -n "$p" ]] || continue
    if [[ -x "$p/bin/llvm-config" ]]; then printf '%s\n' "$p"; return 0; fi
  done
  return 1
}

# --- verify a candidate really is the pinned major and has dev files ---------
llvm_ok() {
  local prefix="$1"
  [[ -x "$prefix/bin/llvm-config" ]] || return 1
  local v
  v="$("$prefix/bin/llvm-config" --version 2>/dev/null | cut -d. -f1 || true)"
  [[ "$v" == "$LLVM_MAJOR" ]] || return 1
  # remill needs LLVM as a library: cmake package + headers.
  [[ -d "$prefix/include/llvm" ]] || return 1
  "$prefix/bin/llvm-config" --cmakedir >/dev/null 2>&1 || return 1
  return 0
}

llvm_prefix=""
if candidate="$(find_llvm)"; then
  if llvm_ok "$candidate"; then llvm_prefix="$candidate"; fi
fi

if [[ -z "$llvm_prefix" && -n "$LLVM_DIR_CUSTOM" ]]; then
  if llvm_ok "$LLVM_DIR_CUSTOM"; then llvm_prefix="$LLVM_DIR_CUSTOM"; fi
fi

# --- install LLVM ${LLVM_MAJOR} if still missing -----------------------------
if [[ -z "$llvm_prefix" ]]; then
  log "LLVM ${LLVM_MAJOR} not found; installing."
  if [[ "$(uname)" == "Darwin" ]] && command -v brew >/dev/null 2>&1; then
    log "trying Homebrew (brew install llvm@${LLVM_MAJOR})"
    # brew is unusable in some sandboxes (sandbox_exec denied); tolerate failure.
    brew install "llvm@${LLVM_MAJOR}" >/tmp/brew-llvm.log 2>&1 || \
      warn "brew install failed (see /tmp/brew-llvm.log); falling back to prebuilt"
  fi
  if candidate="$(find_llvm)"; then
    if llvm_ok "$candidate"; then llvm_prefix="$candidate"; fi
  fi
fi

if [[ -z "$llvm_prefix" ]]; then
  # Official prebuilt: no brew, no sandbox, and includes dev headers/libs/cmake.
  if [[ "$(uname)" != "Darwin" ]]; then
    warn "on Linux install with: apt-get install llvm-${LLVM_MAJOR}-dev clang-${LLVM_MAJOR} lld-${LLVM_MAJOR}"
    exit 1
  fi
  arch="$(uname -m)"
  case "$arch" in
    arm64|aarch64) asset="LLVM-${LLVM_MAJOR}.1.8-macOS-ARM64.tar.xz" ;;
    x86_64)        asset="LLVM-${LLVM_MAJOR}.1.8-macOS-X64.tar.xz" ;;
    *) echo "unsupported arch: $arch" >&2; exit 1 ;;
  esac
  url="https://github.com/llvm/llvm-project/releases/download/llvmorg-${LLVM_MAJOR}.1.8/${asset}"
  log "downloading prebuilt ${asset} (~1.4 GB) -> ${LLVM_DIR_DEFAULT}"
  mkdir -p "$LLVM_DIR_DEFAULT"
  curl -fL --progress-bar -o /tmp/llvm-prebuilt.tar.xz "$url"
  tar -xf /tmp/llvm-prebuilt.tar.xz -C "$LLVM_DIR_DEFAULT" --strip-components=1
  rm -f /tmp/llvm-prebuilt.tar.xz
  if llvm_ok "$LLVM_DIR_DEFAULT"; then
    llvm_prefix="$LLVM_DIR_DEFAULT"
  else
    echo "prebuilt LLVM did not validate at ${LLVM_DIR_DEFAULT}" >&2; exit 1
  fi
fi

log "LLVM ${LLVM_MAJOR}: ${llvm_prefix} ($("${llvm_prefix}/bin/llvm-config" --version))"

# --- python venv with capstone + protobuf ------------------------------------
if [[ ! -x "${VENV}/bin/python" ]]; then
  if command -v uv >/dev/null 2>&1; then
    log "creating venv with uv: ${VENV}"
    (cd "$ROOT" && uv venv "$VENV")
  else
    log "uv not found; using python3 -m venv"
    python3 -m venv "$VENV"
  fi
fi

PY="${VENV}/bin/python"
if command -v uv >/dev/null 2>&1; then
  uv pip install --python "$PY" capstone protobuf
else
  "$PY" -m pip install --upgrade pip >/dev/null
  "$PY" -m pip install capstone protobuf
fi

# --- verify -------------------------------------------------------------------
cd "$ROOT"
log "verifying python deps"
"$PY" - <<'PYEOF'
import sys
sys.path.insert(0, "pipeline")
import capstone, google.protobuf
import specification_pb2, anvill_spec
print(f"  python    {sys.version.split()[0]}")
print(f"  capstone  {capstone.__version__}")
print(f"  protobuf  {google.protobuf.__version__}")
print("  specification_pb2 + anvill_spec import OK")
PYEOF

cat <<EOF

[bootstrap] environment ready.

  LLVM ${LLVM_MAJOR}      : ${llvm_prefix}
  python venv       : ${VENV}

Next:
  PATH=${llvm_prefix}/bin:\$PATH SKIP_DEPS=1 JOBS=$(sysctl -n hw.ncpu 2>/dev/null || echo 4) make lifters
  make demo

(SKIP_DEPS=1 only works around XED's spurious exit code under the cmake graph;
 drop it on a clean machine and the dependency superbuild runs serially.)
EOF
