#!/bin/bash
# Attach the attestation bundle, write release notes, and hash the assets.
#
# Usage: ci/finalize_release.sh <release-dir>
# Reads ATTESTATION_URL and ATTESTATION_BUNDLE from the environment.

set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <release-dir>" >&2
  exit 2
fi

RELEASE="$1"
mkdir -p "${RELEASE}/meta"

if [[ -n "${ATTESTATION_BUNDLE:-}" && -f "${ATTESTATION_BUNDLE}" ]]; then
  cp "${ATTESTATION_BUNDLE}" "${RELEASE}/meta/attestation.sigstore.json"
fi

python3 - "${RELEASE}" <<'PY'
import json
import os
import sys

release = sys.argv[1]
index = json.load(open(f"{release}/meta/dependency-versions.json", encoding="utf-8"))
missed = [
    row for row in index["libraries"]
    if row.get("missed_by_syft") and row.get("origin") == "static-library"
]
lines = [
    "Linux binaries for x86-64, aarch64, and riscv64.",
    "",
    "GitHub artifact attestation signs every file in `bin/`.",
    f"Attestation: {os.environ.get('ATTESTATION_URL', '')}",
    "",
    "Third-party versions for critical-bug triage are in",
    "`meta/dependency-versions.json` and `meta/DEPENDENCY_VERSIONS.txt`.",
    "Rows tagged STATIC-MISSED are static libraries Syft/Grype do not",
    "record as their own component (bundled C code linked from `.a`",
    "archives). Match an advisory on library name and version.",
    "",
    f"Libraries indexed: {len(index['libraries'])}.",
    f"Static libraries missed by Syft: {len(missed)}.",
]
if missed:
    lines.append("")
    lines.append("Static libraries missed by Syft:")
    for row in missed:
        lines.append(f"- {row['name']} {row['version']} (bundled by {row['bundled_by']})")
open(f"{release}/NOTES.md", "w", encoding="utf-8").write("\n".join(lines) + "\n")
print(f"wrote {release}/NOTES.md ({len(missed)} static libs missed by Syft)")
PY

(
  cd "${RELEASE}"
  find bin meta -type f ! -name SHA256SUMS | sort | xargs sha256sum
) > "${RELEASE}/meta/SHA256SUMS"

echo "checksums: $(wc -l < "${RELEASE}/meta/SHA256SUMS") files"
