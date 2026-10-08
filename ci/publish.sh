#!/bin/bash
# Stage release files from downloaded CI artifacts and merge the Syft SBOM
# with every static-library supplement.
#
# Usage: ci/publish.sh <collected-dir> <release-dir>

set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <collected-dir> <release-dir>" >&2
  exit 2
fi

COLLECTED="$1"
RELEASE="$2"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

TARGETS=(
  x86_64-unknown-linux-gnu
  aarch64-unknown-linux-gnu
  riscv64gc-unknown-linux-gnu
)
BINS=(
  ceremony
  ceremony-wasm
  xisa-probe
  sample_network
)

mkdir -p "${RELEASE}/bin" "${RELEASE}/meta"
for target in "${TARGETS[@]}"; do
  for bin in "${BINS[@]}"; do
    src="${COLLECTED}/${bin}-${target}"
    if [[ ! -f "$src" ]]; then
      echo "missing release binary: $src" >&2
      exit 1
    fi
    cp "$src" "${RELEASE}/bin/${bin}-${target}"
    chmod 0755 "${RELEASE}/bin/${bin}-${target}"
  done
done

if [[ ! -f "${COLLECTED}/sbom.syft.cdx.json" ]]; then
  echo "missing Syft SBOM" >&2
  exit 1
fi
cp "${COLLECTED}/sbom.syft.cdx.json" "${RELEASE}/meta/sbom.syft.cdx.json"

shopt -s nullglob
supplements=("${COLLECTED}"/static-libs-*.json)
shopt -u nullglob
if [[ ${#supplements[@]} -eq 0 ]]; then
  echo "missing static-library supplements" >&2
  exit 1
fi

pin_args=()
for pin in vendor/remill/dependencies/CMakeLists.txt vendor/remill/dependencies/xed.cmake; do
  if [[ -f "${ROOT}/${pin}" ]]; then
    pin_args+=(--pin "${ROOT}/${pin}")
  fi
done

shopt -s nullglob
extras=("${COLLECTED}"/remill-lift*)
shopt -u nullglob
for extra in "${extras[@]}"; do
  cp "$extra" "${RELEASE}/bin/$(basename "$extra")"
  chmod 0755 "${RELEASE}/bin/$(basename "$extra")"
done

python3 ci/static_libs.py \
  --lock ceremony/Cargo.lock ceremony-wasm/Cargo.lock probe/Cargo.lock \
  --syft "${RELEASE}/meta/sbom.syft.cdx.json" \
  --supplement "${supplements[@]}" \
  "${pin_args[@]}" \
  --out "${RELEASE}/meta/sbom.cdx.json" \
  --versions-json "${RELEASE}/meta/dependency-versions.json" \
  --versions-txt "${RELEASE}/meta/DEPENDENCY_VERSIONS.txt"

python3 - <<PY
import json
doc = json.load(open("${RELEASE}/meta/sbom.cdx.json", encoding="utf-8"))
have = {(c.get("name"), c.get("version")) for c in doc.get("components", [])}
if ("zstd", "1.5.7") not in have or ("glog", "0.7.1") not in have or ("xed", "2025.06.08") not in have:
    raise SystemExit(f"merged SBOM is missing a pinned static library: {sorted(have)}")
index = json.load(open("${RELEASE}/meta/dependency-versions.json", encoding="utf-8"))
missed = [row for row in index["libraries"] if row.get("missed_by_syft")]
print(f"triage index: {len(index['libraries'])} libraries, {len(missed)} static libs Syft missed")
PY

echo "staged $(find "${RELEASE}/bin" -type f | wc -l) binaries"
