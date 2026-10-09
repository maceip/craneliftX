#!/usr/bin/env bash
# Single-source-of-truth guard for the LLVM/Clang major version.
#
# Every toolchain reference in this repo must derive from .llvm-version. This
# script fails the build if any non-vendored file names an llvm/clang version
# that is not the pinned major, so a wrong toolchain major cannot silently
# ship again.
#
# Vendored / third-party trees (vendor/, hetersec*/) and internal agent data
# (.workbuddy-ai/) are excluded: their version strings are out of our control
# and never reach the command line we run.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

MAJOR="$(tr -d '[:space:]' < .llvm-version 2>/dev/null || true)"
if [[ -z "$MAJOR" ]]; then
  echo "error: .llvm-version is missing or empty" >&2
  exit 1
fi
echo "pinned LLVM major: ${MAJOR}"

# Version-tagged toolchain tokens outside vendored / build / cache dirs.
# -e patterns match: clang-<n>, llvm-<n>, llvm-<n>-dev, /usr/lib/llvm-<n>
offenders="$(grep -rIn \
  --exclude-dir=vendor \
  --exclude-dir=hetersec\* \
  --exclude-dir=.git \
  --exclude-dir=.workbuddy-ai \
  --exclude-dir=target \
  --exclude-dir=build \
  --exclude-dir=node_modules \
  --exclude=ci/check_llvm_version.sh \
  -E '(clang|llvm)-[0-9]+|/usr/lib/llvm-[0-9]+|llvm-[0-9]+-dev' . || true)"

bad=0
if [[ -n "$offenders" ]]; then
  while IFS= read -r hit; do
    [[ -z "$hit" ]] && continue
    # Pull the digits that belong to the llvm/clang token, not the line number
    # or any other number on the line.
    v="$(printf '%s' "$hit" \
      | grep -oE '(clang-[0-9]+|llvm-[0-9]+|/usr/lib/llvm-[0-9]+)' \
      | grep -oE '[0-9]+' | head -n1)"
    if [[ "$v" != "$MAJOR" ]]; then
      echo "STRAY LLVM VERSION (pinned ${MAJOR}): ${hit}"
      bad=1
    fi
  done <<< "$offenders"
fi

if [[ $bad -ne 0 ]]; then
  echo "error: LLVM version references do not match .llvm-version (${MAJOR})" >&2
  exit 1
fi

echo "OK: all LLVM/Clang version references match .llvm-version (${MAJOR})"
