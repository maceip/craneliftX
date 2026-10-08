#!/bin/bash
# Syft SBOM of declared dependencies, plus the static-library supplement.
# Runs inside the toolchain container (syft and python3 are on PATH).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
mkdir -p dist

syft scan dir:. \
  -o "cyclonedx-json=${ROOT}/dist/sbom.syft.cdx.json" \
  --exclude './.git/**' \
  --exclude './target/**' \
  --exclude './.ci-cargo/**' \
  --exclude './dist/**' \
  --exclude './build/**' \
  --source-name ceremony \
  --source-version "${GITHUB_SHA:-local}"

pin_args=()
for pin in vendor/remill/dependencies/CMakeLists.txt vendor/remill/dependencies/xed.cmake; do
  if [[ -f "${ROOT}/${pin}" ]]; then
    pin_args+=(--pin "${ROOT}/${pin}")
  fi
done

python3 ci/static_libs.py \
  --lock ceremony/Cargo.lock ceremony-wasm/Cargo.lock probe/Cargo.lock \
  "${pin_args[@]}" \
  --supplement-out "${ROOT}/dist/static-libs-lockfile.json"

python3 - <<'PY'
import json
doc = json.load(open("dist/static-libs-lockfile.json", encoding="utf-8"))
libs = {(item["name"], item["version"]) for item in doc["static_libraries"]}
if ("zstd", "1.5.7") not in libs:
    raise SystemExit(f"bundled zstd 1.5.7 was not tracked: {sorted(libs)}")
if ("glog", "0.7.1") not in libs:
    raise SystemExit(f"pinned glog 0.7.1 was not tracked: {sorted(libs)}")
if ("sleigh", "7c6b742") not in libs:
    raise SystemExit(f"pinned sleigh 7c6b742 was not tracked: {sorted(libs)}")
print("lockfile static libs:", ", ".join(f"{name} {version}" for name, version in sorted(libs)))
PY
