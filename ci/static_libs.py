#!/usr/bin/env python3
"""Track third-party library versions Syft and Grype miss.

Syft's cargo cataloger records the Rust crate that *builds* a native library
(for example ``zstd-sys`` 2.1.0+zstd.1.5.7) and Grype then matches that crate.
The C library actually linked into the binary is a static archive produced by
the ``cc`` crate (zstd 1.5.7). Advisories filed against that C library never
meet the SBOM, so a later critical-bug triage has nothing to search.

This tool:

* reads every Cargo.lock
* recovers bundled C/C++ versions from ``+lib.X.Y.Z`` crate versions, from
  version macros in the crate sources, and from ``.a`` archives under
  ``target/**/build/``
* appends those libraries to a Syft CycloneDX document when Syft did not
  already record the same name and version
* writes a flat version index meant to be searched when an advisory lands

Rust ``.rlib`` files are skipped. Their crate versions are already in the
lockfile, which Syft catalogs.
"""

from __future__ import annotations

import argparse
import copy
import json
import re
import sys
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

# CPE vendors we are willing to assert. Unknown libraries stay version-tracked
# without a guessed CPE, so a later Grype run is not fed a wrong identifier.
CPE_VENDORS = {
    "zstd": "facebook",
    "glog": "google",
    "googletest": "google",
}

# Archive filenames that do not match the upstream project name.
ARCHIVE_NAMES = {
    "gtest": "googletest",
    "gtest_main": "googletest",
    "gmock": "googletest",
    "gmock_main": "googletest",
}

_SIMPLE_GIT = re.compile(
    r"simple_git\(\s*(https://github\.com/[A-Za-z0-9_.-]+/([A-Za-z0-9_.-]+))\s+(\S+)"
)
_EXTERNAL_PROJECT = re.compile(
    r'ExternalProject_Add\(\s*(\w+).*?GIT_REPOSITORY\s+"(https://github\.com/[^"]+)".*?GIT_TAG\s+"([^"]+)"',
    re.S,
)

# Filename / macro prefixes that do not match the registry crate name.
NAME_ALIASES = {
    "libzstd": "zstd",
    "libz": "zlib",
}

_SUFFIX_NAMED = re.compile(
    r"^(?P<name>[A-Za-z][A-Za-z0-9_+-]*?)[.-](?P<ver>\d+\.\d+\.\d+[A-Za-z0-9]*)$"
)
_SUFFIX_BARE = re.compile(r"^(?P<ver>\d+\.\d+\.\d+[A-Za-z0-9]*)$")
_DEFINE = re.compile(r"^\s*#\s*define\s+([A-Z0-9_]+)\s+(\d+)\b", re.M)
_ARCHIVE_CRATE = re.compile(r"[/\\]build[/\\](.+)-([0-9a-f]{16})[/\\]")

MAX_ARCHIVE_BYTES = 20 * 1024 * 1024
MAX_HEADER_BYTES = 1024 * 1024
MAX_HEADER_FILES = 200


@dataclass
class Package:
    name: str
    version: str
    deps: list[str]
    source: str
    lock_path: str


@dataclass
class StaticLib:
    name: str
    version: str
    bundled_by: str
    evidence: list[str] = field(default_factory=list)
    cpe: str | None = None
    # static-library: linked from a .a that Syft/Grype do not attribute.
    # cmake-pin: third-party source pin that is not itself linked (build tool).
    origin: str = "static-library"

    def add_evidence(self, item: str) -> None:
        if item not in self.evidence:
            self.evidence.append(item)


@dataclass
class Discovery:
    libraries: list[StaticLib] = field(default_factory=list)
    unversioned_static_archives: list[dict[str, str]] = field(default_factory=list)

    def by_key(self) -> dict[tuple[str, str], StaticLib]:
        return {(lib.name, lib.version): lib for lib in self.libraries}


def parse_lock(path: Path) -> list[Package]:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    packages: list[Package] = []
    for raw in data.get("package", []):
        deps: list[str] = []
        for dep in raw.get("dependencies", []):
            if isinstance(dep, str):
                deps.append(dep.split()[0])
            elif isinstance(dep, dict) and "name" in dep:
                deps.append(str(dep["name"]))
        packages.append(
            Package(
                name=raw["name"],
                version=str(raw["version"]),
                deps=deps,
                source=str(raw.get("source") or ""),
                lock_path=str(path),
            )
        )
    return packages


def library_name_from_crate(crate: str) -> str:
    name = crate
    for suffix in ("-sys", "-src"):
        if name.endswith(suffix) and len(name) > len(suffix):
            name = name[: -len(suffix)]
            break
    return NAME_ALIASES.get(name, name)


def version_from_suffix(package: Package) -> tuple[str, str] | None:
    if "+" not in package.version:
        return None
    _crate_ver, meta = package.version.split("+", 1)
    named = _SUFFIX_NAMED.match(meta)
    if named:
        name = NAME_ALIASES.get(named.group("name"), named.group("name"))
        return name, named.group("ver")
    bare = _SUFFIX_BARE.match(meta)
    if bare:
        return library_name_from_crate(package.name), bare.group("ver")
    return None


def _normalize(token: str) -> str:
    return token.lower().replace("_", "").replace("-", "")


def version_from_headers(crate_dir: Path, package: Package) -> tuple[str, str] | None:
    """Read MAJOR/MINOR/RELEASE macros from a crate's bundled headers."""
    expected = version_from_suffix(package)
    expected_name = expected[0] if expected else library_name_from_crate(package.name)
    triples: dict[str, dict[str, int]] = {}
    seen = 0
    for path in sorted(crate_dir.rglob("*")):
        if seen >= MAX_HEADER_FILES:
            break
        if not path.is_file() or path.suffix.lower() not in {".h", ".hpp", ".c", ".cc", ".hh"}:
            continue
        if path.stat().st_size > MAX_HEADER_BYTES:
            continue
        seen += 1
        text = path.read_text(encoding="utf-8", errors="ignore")
        for macro, value in _DEFINE.findall(text):
            kind = None
            prefix = None
            for tail, label in (
                ("_VERSION_MAJOR", "major"),
                ("_VERSION_MINOR", "minor"),
                ("_VERSION_RELEASE", "release"),
                ("_VERSION_PATCH", "release"),
                ("_VERSION_MICRO", "release"),
            ):
                if macro.endswith(tail):
                    kind = label
                    prefix = macro[: -len(tail)]
                    break
            if kind is None or prefix is None:
                continue
            number = int(value)
            if number > 10_000:
                continue
            triples.setdefault(prefix, {})[kind] = number

    candidates: list[tuple[str, str]] = []
    for prefix, parts in triples.items():
        if not {"major", "minor", "release"} <= parts.keys():
            continue
        version = f"{parts['major']}.{parts['minor']}.{parts['release']}"
        name = NAME_ALIASES.get(prefix.lower(), prefix.lower())
        candidates.append((name, version))

    if not candidates:
        return None
    wanted = _normalize(expected_name)
    for name, version in candidates:
        if _normalize(name) == wanted or wanted in _normalize(name) or _normalize(name) in wanted:
            return name, version
    return None


def cpe_for(name: str, version: str) -> str | None:
    vendor = CPE_VENDORS.get(name)
    if vendor is None:
        return None
    return f"cpe:2.3:a:{vendor}:{name}:{version}:*:*:*:*:*:*:*"


def _bundled_by(package: Package) -> str:
    return f"{package.name}@{package.version}"


def clean_tag(tag: str) -> str:
    if tag.startswith("v") and len(tag) > 1 and tag[1].isdigit():
        return tag[1:]
    return tag


def _inside_disabled_option(text: str, pos: int) -> bool:
    """True when `pos` sits in an ``if(ENABLE_...)`` block.

    Those dependencies are not built unless the option is turned on. The
    lifter build leaves them off, so recording them would make later triage
    treat an unused pin as a shipped library.
    """
    depth = 0
    enable_depth: int | None = None
    index = 0
    while index < pos:
        if text.startswith("endif", index) and re.match(r"endif\s*\(", text[index:]):
            if enable_depth is not None and depth == enable_depth:
                enable_depth = None
            depth = max(0, depth - 1)
            index += 5
            continue
        if text.startswith("if", index) and re.match(r"if\s*\(", text[index:]):
            depth += 1
            if "ENABLE_" in text[index : index + 48] and enable_depth is None:
                enable_depth = depth
            index += 2
            continue
        index += 1
    return enable_depth is not None


def parse_cmake_pins(path: Path) -> list[StaticLib]:
    """Recover third-party versions pinned in a CMake superbuild.

    Remill builds glog, gflags, googletest, and XED as static archives. Syft
    does not read ``simple_git`` / ``ExternalProject_Add`` tags, so those
    versions never reach Grype. Pins inside ``if(ENABLE_...)`` are skipped
    because that option is off in the lifter build.
    """
    text = path.read_text(encoding="utf-8", errors="ignore")
    found: list[StaticLib] = []
    for match in _SIMPLE_GIT.finditer(text):
        if _inside_disabled_option(text, match.start()):
            continue
        url, name, tag = match.group(1), match.group(2), match.group(3)
        version = clean_tag(tag)
        found.append(
            StaticLib(
                name=name,
                version=version,
                bundled_by=f"{url}@{tag}",
                evidence=[f"cmake-pin:{path}:{url}@{tag}"],
                cpe=cpe_for(name, version),
                origin="static-library",
            )
        )
    for match in _EXTERNAL_PROJECT.finditer(text):
        if _inside_disabled_option(text, match.start()):
            continue
        name, url, tag = match.group(1), match.group(2), match.group(3)
        # mbuild is the XED build driver. It is not linked into the lifter.
        origin = "cmake-pin" if name == "mbuild" else "static-library"
        version = clean_tag(tag)
        found.append(
            StaticLib(
                name=name,
                version=version,
                bundled_by=f"{url}@{tag}",
                evidence=[f"cmake-pin:{path}:{url}@{tag}"],
                cpe=cpe_for(name, version),
                origin=origin,
            )
        )
    return found


def attach_named_archives(root: Path, libraries: dict[tuple[str, str], StaticLib]) -> None:
    """Attach ``lib<name>.a`` files to libraries we already identified."""
    if not root.exists():
        return
    by_name: dict[str, list[StaticLib]] = {}
    for lib in libraries.values():
        by_name.setdefault(lib.name, []).append(lib)
    for path in sorted(root.rglob("*.a")):
        if not path.is_file():
            continue
        stem = path.name
        if not (stem.startswith("lib") and stem.endswith(".a")):
            continue
        libname = ARCHIVE_NAMES.get(stem[3:-2], stem[3:-2])
        for lib in by_name.get(libname, []):
            lib.add_evidence(f"static-archive:{path}")


def discover(
    locks: list[Path],
    target_dirs: list[Path] | None = None,
    cargo_home: Path | None = None,
    pin_files: list[Path] | None = None,
    archive_roots: list[Path] | None = None,
) -> Discovery:
    packages: list[Package] = []
    for lock in locks:
        packages.extend(parse_lock(lock))

    found: dict[tuple[str, str], StaticLib] = {}

    def add(name: str, version: str, package: Package, evidence: str) -> StaticLib:
        key = (name, version)
        lib = found.get(key)
        if lib is None:
            lib = StaticLib(
                name=name,
                version=version,
                bundled_by=_bundled_by(package),
                cpe=cpe_for(name, version),
            )
            found[key] = lib
        lib.add_evidence(evidence)
        return lib

    third_party = [pkg for pkg in packages if pkg.source.startswith("registry+")]
    cc_packages = [pkg for pkg in third_party if "cc" in pkg.deps or "+" in pkg.version]

    for pkg in cc_packages:
        suffix = version_from_suffix(pkg)
        header = None
        crate_dir = _crate_source_dir(cargo_home, pkg) if cargo_home else None
        if crate_dir is not None:
            header = version_from_headers(crate_dir, pkg)
        chosen = suffix or header
        if chosen is None:
            continue
        name, version = chosen
        if suffix is not None:
            evidence = f"cargo-lock-version-suffix:{pkg.lock_path}:{pkg.version}"
        else:
            evidence = f"cargo-lock:{pkg.lock_path}:{_bundled_by(pkg)}"
        lib = add(name, version, pkg, evidence)
        if header is not None and (header[0] != name or header[1] != version):
            lib.add_evidence(f"header-disagrees:{header[0]}@{header[1]}")
        elif header is not None:
            lib.add_evidence(f"header-confirms:{crate_dir}")

    unversioned: list[dict[str, str]] = []
    for archive in _static_archives(target_dirs or []):
        crate_name = _crate_from_archive(archive)
        owners = [pkg for pkg in cc_packages if pkg.name == crate_name]
        blob = _read_archive(archive)
        attached = False
        for pkg in owners or []:
            for lib in found.values():
                if not lib.bundled_by.startswith(f"{pkg.name}@"):
                    continue
                lib.add_evidence(f"static-archive:{archive}")
                if lib.version.encode() in blob:
                    lib.add_evidence(f"archive-contains-version:{lib.version}")
                attached = True
        if attached:
            continue
        sniffed = _sniff_archive_version(blob, crate_name) if crate_name else None
        if sniffed is not None and owners:
            name, version = sniffed
            add(name, version, owners[0], f"static-archive:{archive}")
            continue
        unversioned.append(
            {
                "path": str(archive),
                "crate": crate_name or "",
                "reason": "static archive has no recoverable upstream version",
            }
        )

    for pin in pin_files or []:
        if not pin.is_file():
            continue
        for lib in parse_cmake_pins(pin):
            key = (lib.name, lib.version)
            existing = found.get(key)
            if existing is None:
                found[key] = lib
            else:
                for item in lib.evidence:
                    existing.add_evidence(item)
                if existing.cpe is None:
                    existing.cpe = lib.cpe

    for root in archive_roots or []:
        attach_named_archives(root, found)

    libraries = sorted(found.values(), key=lambda lib: (lib.name, lib.version, lib.bundled_by))
    return Discovery(libraries=libraries, unversioned_static_archives=unversioned)


def _crate_source_dir(cargo_home: Path, package: Package) -> Path | None:
    root = cargo_home / "registry" / "src"
    if not root.is_dir():
        return None
    dirname = f"{package.name}-{package.version}"
    matches = [path for path in root.glob(f"*/{dirname}") if path.is_dir()]
    if not matches:
        return None
    return matches[0]


def _static_archives(target_dirs: list[Path]) -> list[Path]:
    archives: list[Path] = []
    for root in target_dirs:
        if not root.exists():
            continue
        for path in root.rglob("*.a"):
            if not path.is_file():
                continue
            # Only build-script products. Rust rlibs and our own staticlibs
            # live under deps/ or the release root, and Syft already has those
            # crate versions from Cargo.lock.
            if "/build/" not in path.as_posix() and "\\build\\" not in str(path):
                continue
            archives.append(path)
    return sorted(archives)


def _crate_from_archive(path: Path) -> str | None:
    match = _ARCHIVE_CRATE.search(str(path))
    if match is None:
        return None
    return match.group(1)


def _read_archive(path: Path) -> bytes:
    size = path.stat().st_size
    with path.open("rb") as handle:
        return handle.read(min(size, MAX_ARCHIVE_BYTES))


def _sniff_archive_version(blob: bytes, crate_name: str) -> tuple[str, str] | None:
    """Pull a version that sits next to the library name inside an archive."""
    name = library_name_from_crate(crate_name)
    text = blob.decode("latin1", errors="ignore")
    if name.lower() not in text.lower() and crate_name.lower() not in text.lower():
        return None
    pattern = re.compile(
        rf"(?i)(?:{re.escape(name)}|{re.escape(crate_name)})[^0-9]{{0,32}}(\d+\.\d+\.\d+)"
    )
    match = pattern.search(text)
    if match is None:
        return None
    return NAME_ALIASES.get(name, name), match.group(1)


def merge_supplements(discovery: Discovery, supplements: list[Path]) -> Discovery:
    found = discovery.by_key()
    unversioned = list(discovery.unversioned_static_archives)
    seen_unversioned = {item["path"] for item in unversioned}
    for path in supplements:
        payload = json.loads(path.read_text(encoding="utf-8"))
        for raw in payload.get("static_libraries", []):
            key = (raw["name"], raw["version"])
            lib = found.get(key)
            if lib is None:
                lib = StaticLib(
                    name=raw["name"],
                    version=raw["version"],
                    bundled_by=raw.get("bundled_by", ""),
                    evidence=list(raw.get("evidence") or []),
                    cpe=raw.get("cpe"),
                    origin=raw.get("origin") or "static-library",
                )
                found[key] = lib
            else:
                for item in raw.get("evidence") or []:
                    lib.add_evidence(item)
                if lib.cpe is None and raw.get("cpe"):
                    lib.cpe = raw["cpe"]
        for item in payload.get("unversioned_static_archives", []):
            if item.get("path") not in seen_unversioned:
                unversioned.append(item)
                seen_unversioned.add(item.get("path", ""))
    libraries = sorted(found.values(), key=lambda lib: (lib.name, lib.version, lib.bundled_by))
    return Discovery(libraries=libraries, unversioned_static_archives=unversioned)


def supplement_document(discovery: Discovery) -> dict:
    return {
        "static_libraries": [
            {
                "name": lib.name,
                "version": lib.version,
                "bundled_by": lib.bundled_by,
                "evidence": lib.evidence,
                "cpe": lib.cpe,
                "origin": lib.origin,
            }
            for lib in discovery.libraries
        ],
        "unversioned_static_archives": discovery.unversioned_static_archives,
    }


def _component_identity(component: dict) -> tuple[str, str]:
    return (str(component.get("name") or ""), str(component.get("version") or ""))


def merge_cyclonedx(syft_document: dict, discovery: Discovery) -> dict:
    document = copy.deepcopy(syft_document)
    components = document.setdefault("components", [])
    present = {_component_identity(component) for component in components}
    dependencies = document.setdefault("dependencies", [])
    dep_by_ref = {entry.get("ref"): entry for entry in dependencies}

    for lib in discovery.libraries:
        if (lib.name, lib.version) in present:
            continue
        purl = f"pkg:generic/{lib.name}@{lib.version}"
        component: dict = {
            "type": "library",
            "bom-ref": purl,
            "name": lib.name,
            "version": lib.version,
            "purl": purl,
            "description": (
                "Statically linked library missed by Syft/Grype cargo cataloging. "
                f"Bundled by {lib.bundled_by}."
            ),
            "properties": [
                {"name": "ci:origin", "value": lib.origin},
                {"name": "ci:missed-by", "value": "syft,grype"},
                {"name": "ci:bundled-by", "value": lib.bundled_by},
            ],
        }
        for item in lib.evidence:
            component["properties"].append({"name": "ci:evidence", "value": item})
        if lib.cpe:
            component["cpe"] = lib.cpe
        components.append(component)
        present.add((lib.name, lib.version))

        crate_name, _, crate_version = lib.bundled_by.partition("@")
        for existing in components:
            if existing.get("name") != crate_name or existing.get("version") != crate_version:
                continue
            ref = existing.get("bom-ref")
            if not ref:
                continue
            entry = dep_by_ref.get(ref)
            if entry is None:
                entry = {"ref": ref, "dependsOn": []}
                dependencies.append(entry)
                dep_by_ref[ref] = entry
            depends = entry.setdefault("dependsOn", [])
            if purl not in depends:
                depends.append(purl)
    return document


def _syft_origin(component: dict) -> str:
    for prop in component.get("properties") or []:
        if prop.get("name") == "syft:package:type" and prop.get("value"):
            return str(prop["value"])
        if prop.get("name") == "ci:origin" and prop.get("value"):
            return str(prop["value"])
    purl = str(component.get("purl") or "")
    if purl.startswith("pkg:cargo/"):
        return "rust-crate"
    if purl.startswith("pkg:generic/"):
        return "static-library"
    return component.get("type") or "library"


def _missed(component: dict) -> bool:
    for prop in component.get("properties") or []:
        if prop.get("name") == "ci:missed-by":
            return True
    return False


def _bundled(component: dict) -> str:
    for prop in component.get("properties") or []:
        if prop.get("name") == "ci:bundled-by":
            return str(prop["value"])
    return ""


def _evidence(component: dict) -> list[str]:
    return [
        str(prop.get("value"))
        for prop in component.get("properties") or []
        if prop.get("name") == "ci:evidence" and prop.get("value")
    ]


def version_index(document: dict, discovery: Discovery) -> dict:
    libraries = []
    seen: set[tuple[str, str, str]] = set()
    for component in document.get("components") or []:
        if component.get("type") != "library":
            continue
        name = str(component.get("name") or "")
        version = str(component.get("version") or "")
        purl = str(component.get("purl") or "")
        key = (name, version, purl)
        if not name or not version or key in seen:
            continue
        seen.add(key)
        libraries.append(
            {
                "name": name,
                "version": version,
                "purl": purl,
                "origin": _syft_origin(component),
                "missed_by_syft": _missed(component),
                "bundled_by": _bundled(component),
                "cpe": component.get("cpe") or "",
                "evidence": _evidence(component),
            }
        )
    libraries.sort(key=lambda item: (not item["missed_by_syft"], item["name"], item["version"], item["purl"]))
    return {
        "description": (
            "Third-party library versions for critical-bug triage. "
            "Entries with missed_by_syft true are static libraries Syft and Grype "
            "do not attribute to the upstream project; match an advisory against "
            "name + version here before deciding the build is affected."
        ),
        "libraries": libraries,
        "unversioned_static_archives": discovery.unversioned_static_archives,
    }


def version_text(index: dict) -> str:
    lines = [
        "# Third-party library versions for critical-bug triage.",
        "# STATIC-MISSED rows are static libraries Syft/Grype did not record as their own component.",
        "# Columns: name version origin triage purl bundled_by",
        "",
    ]
    for lib in index["libraries"]:
        if lib["missed_by_syft"] and lib["origin"] == "static-library":
            triage = "STATIC-MISSED"
        elif lib["missed_by_syft"]:
            triage = "PINNED-MISSED"
        else:
            triage = "syft"
        lines.append(
            "\t".join(
                [
                    lib["name"],
                    lib["version"],
                    lib["origin"],
                    triage,
                    lib["purl"],
                    lib["bundled_by"],
                ]
            )
        )
    if index["unversioned_static_archives"]:
        lines.append("")
        lines.append("# Static archives whose upstream version could not be recovered.")
        for item in index["unversioned_static_archives"]:
            lines.append(
                "\t".join(["UNKNOWN", "", "static-library", "UNVERSIONED", item.get("crate", ""), item.get("path", "")])
            )
    lines.append("")
    return "\n".join(lines)


def _load_syft(path: Path) -> dict:
    document = json.loads(path.read_text(encoding="utf-8"))
    if document.get("bomFormat") != "CycloneDX":
        raise SystemExit(f"{path} is not a CycloneDX document")
    return document


def build_discovery(args: argparse.Namespace) -> Discovery:
    locks = [Path(path) for path in args.lock]
    target_dirs = [Path(path) for path in (args.target_dir or [])]
    cargo_home = Path(args.cargo_home) if args.cargo_home else None
    discovery = discover(
        locks,
        target_dirs,
        cargo_home,
        pin_files=[Path(path) for path in (args.pin or [])],
        archive_roots=[Path(path) for path in (args.archive_root or [])],
    )
    if args.supplement:
        discovery = merge_supplements(discovery, [Path(path) for path in args.supplement])
    return discovery


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lock", nargs="+", required=True, help="Cargo.lock files")
    parser.add_argument("--syft", help="CycloneDX JSON produced by Syft")
    parser.add_argument("--supplement", nargs="*", default=[], help="static-lib JSON from earlier steps")
    parser.add_argument("--target-dir", action="append", default=[], help="Cargo target directory to scan for .a archives")
    parser.add_argument("--cargo-home", help="Cargo home, used to read bundled headers")
    parser.add_argument("--pin", action="append", default=[], help="CMake file with third-party version pins")
    parser.add_argument("--archive-root", action="append", default=[], help="Directory to scan for named static archives")
    parser.add_argument("--out", help="write the merged CycloneDX document here")
    parser.add_argument("--supplement-out", help="write only the static-library supplement JSON")
    parser.add_argument("--versions-json", help="write the triage version index JSON")
    parser.add_argument("--versions-txt", help="write the triage version index text")
    args = parser.parse_args(argv)

    discovery = build_discovery(args)
    if args.supplement_out:
        Path(args.supplement_out).write_text(
            json.dumps(supplement_document(discovery), indent=2) + "\n",
            encoding="utf-8",
        )

    document = None
    if args.out or args.versions_json or args.versions_txt:
        if not args.syft:
            raise SystemExit("--syft is required when writing a merged SBOM or version index")
        document = merge_cyclonedx(_load_syft(Path(args.syft)), discovery)
    if args.out:
        assert document is not None
        Path(args.out).write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    if args.versions_json or args.versions_txt:
        assert document is not None
        index = version_index(document, discovery)
        if args.versions_json:
            Path(args.versions_json).write_text(json.dumps(index, indent=2) + "\n", encoding="utf-8")
        if args.versions_txt:
            Path(args.versions_txt).write_text(version_text(index), encoding="utf-8")

    missed = [f"{lib.name} {lib.version}" for lib in discovery.libraries]
    print(f"static libraries tracked: {', '.join(missed) if missed else '(none)'}")
    if discovery.unversioned_static_archives:
        print(f"unversioned static archives: {len(discovery.unversioned_static_archives)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
