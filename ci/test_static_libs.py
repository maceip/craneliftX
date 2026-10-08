#!/usr/bin/env python3
"""Regression tests for static-library version recovery."""

from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from ci.static_libs import (
    Package,
    discover,
    merge_cyclonedx,
    merge_supplements,
    supplement_document,
    version_from_suffix,
    version_index,
)


ROOT = Path(__file__).resolve().parents[1]


def _package(name: str, version: str, deps: list[str] | None = None) -> Package:
    return Package(
        name=name,
        version=version,
        deps=deps or ["cc"],
        source="registry+https://github.com/rust-lang/crates.io-index",
        lock_path="Cargo.lock",
    )


class SuffixTests(unittest.TestCase):
    def test_zstd_sys_suffix(self) -> None:
        self.assertEqual(
            version_from_suffix(_package("zstd-sys", "2.1.0+zstd.1.5.7")),
            ("zstd", "1.5.7"),
        )

    def test_dash_separated_suffix(self) -> None:
        self.assertEqual(
            version_from_suffix(_package("curl-sys", "0.1.0+curl-8.6.0")),
            ("curl", "8.6.0"),
        )

    def test_libssh2_suffix_keeps_full_semver(self) -> None:
        self.assertEqual(
            version_from_suffix(_package("libssh2-sys", "0.2.0+libssh2.1.11.0")),
            ("libssh2", "1.11.0"),
        )

    def test_bare_suffix_uses_crate_name(self) -> None:
        self.assertEqual(
            version_from_suffix(_package("openssl-src", "300.0.0+3.2.1")),
            ("openssl", "3.2.1"),
        )

    def test_no_suffix(self) -> None:
        self.assertIsNone(version_from_suffix(_package("ittapi-sys", "0.4.0")))


class LockAndMergeTests(unittest.TestCase):
    def test_real_lockfile_recovers_bundled_zstd(self) -> None:
        discovery = discover([ROOT / "ceremony-wasm" / "Cargo.lock"])
        keys = {(lib.name, lib.version) for lib in discovery.libraries}
        self.assertIn(("zstd", "1.5.7"), keys)
        zstd = next(lib for lib in discovery.libraries if lib.name == "zstd")
        self.assertEqual(zstd.bundled_by, "zstd-sys@2.1.0+zstd.1.5.7")
        self.assertTrue(zstd.cpe)
        self.assertIn("facebook:zstd:1.5.7", zstd.cpe or "")
        # The Rust wrapper crate version is not the C library version.
        self.assertNotIn(("zstd", "0.13.3"), keys)
        self.assertNotIn(("zstd-sys", "2.1.0"), keys)

    def test_merge_adds_static_lib_without_dropping_syft_crates(self) -> None:
        syft = {
            "bomFormat": "CycloneDX",
            "specVersion": "1.7",
            "components": [
                {
                    "bom-ref": "pkg:cargo/zstd@0.13.3?package-id=abc",
                    "type": "library",
                    "name": "zstd",
                    "version": "0.13.3",
                    "purl": "pkg:cargo/zstd@0.13.3",
                },
                {
                    "bom-ref": "pkg:cargo/zstd-sys@2.1.0+zstd.1.5.7?package-id=def",
                    "type": "library",
                    "name": "zstd-sys",
                    "version": "2.1.0+zstd.1.5.7",
                    "purl": "pkg:cargo/zstd-sys@2.1.0%2Bzstd.1.5.7",
                    "properties": [
                        {"name": "syft:package:foundBy", "value": "rust-cargo-lock-cataloger"}
                    ],
                },
            ],
            "dependencies": [
                {
                    "ref": "pkg:cargo/zstd-sys@2.1.0+zstd.1.5.7?package-id=def",
                    "dependsOn": ["pkg:cargo/cc@1.6.0"],
                }
            ],
        }
        discovery = discover([ROOT / "ceremony-wasm" / "Cargo.lock"])
        merged = merge_cyclonedx(syft, discovery)
        identities = {(c["name"], c["version"]) for c in merged["components"]}
        self.assertIn(("zstd", "0.13.3"), identities)
        self.assertIn(("zstd", "1.5.7"), identities)
        self.assertIn(("zstd-sys", "2.1.0+zstd.1.5.7"), identities)
        static = next(c for c in merged["components"] if c["name"] == "zstd" and c["version"] == "1.5.7")
        self.assertEqual(static["purl"], "pkg:generic/zstd@1.5.7")
        props = {p["name"]: p["value"] for p in static["properties"] if p["name"] != "ci:evidence"}
        self.assertEqual(props["ci:missed-by"], "syft,grype")
        dep = next(
            d for d in merged["dependencies"] if d["ref"].startswith("pkg:cargo/zstd-sys@")
        )
        self.assertIn("pkg:generic/zstd@1.5.7", dep["dependsOn"])

        index = version_index(merged, discovery)
        missed = [lib for lib in index["libraries"] if lib["missed_by_syft"]]
        self.assertTrue(any(lib["name"] == "zstd" and lib["version"] == "1.5.7" for lib in missed))
        # Deduped triage rows: the Rust zstd crate and the C library are distinct.
        zstd_rows = [lib for lib in index["libraries"] if lib["name"] == "zstd"]
        self.assertEqual({row["version"] for row in zstd_rows}, {"0.13.3", "1.5.7"})

    def test_does_not_duplicate_a_static_lib_syft_already_has(self) -> None:
        syft = {
            "bomFormat": "CycloneDX",
            "specVersion": "1.7",
            "components": [
                {
                    "type": "library",
                    "name": "zstd",
                    "version": "1.5.7",
                    "purl": "pkg:generic/zstd@1.5.7",
                }
            ],
            "dependencies": [],
        }
        discovery = discover([ROOT / "ceremony-wasm" / "Cargo.lock"])
        merged = merge_cyclonedx(syft, discovery)
        matches = [c for c in merged["components"] if c["name"] == "zstd" and c["version"] == "1.5.7"]
        self.assertEqual(len(matches), 1)

    def test_archive_and_header_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            lock = root / "Cargo.lock"
            lock.write_text(
                "\n".join(
                    [
                        "version = 4",
                        "[[package]]",
                        'name = "demo-sys"',
                        'version = "1.0.0"',
                        'source = "registry+https://github.com/rust-lang/crates.io-index"',
                        "dependencies = [",
                        ' "cc",',
                        "]",
                        "[[package]]",
                        'name = "cc"',
                        'version = "1.0.0"',
                        'source = "registry+https://github.com/rust-lang/crates.io-index"',
                    ]
                ),
                encoding="utf-8",
            )
            header_dir = root / "cargo" / "registry" / "src" / "index" / "demo-sys-1.0.0"
            header_dir.mkdir(parents=True)
            (header_dir / "demo.h").write_text(
                "\n".join(
                    [
                        "#define DEMO_VERSION_MAJOR 4",
                        "#define DEMO_VERSION_MINOR 2",
                        "#define DEMO_VERSION_RELEASE 9",
                    ]
                ),
                encoding="utf-8",
            )
            archive_dir = (
                root / "target" / "x86_64-unknown-linux-gnu" / "release" / "build"
                / "demo-sys-0123456789abcdef" / "out"
            )
            archive_dir.mkdir(parents=True)
            payload = b"!<arch>\n" + b"demo version 4.2.9\x00" + b"DEMO_VERSION"
            (archive_dir / "libdemo.a").write_bytes(payload)

            discovery = discover([lock], target_dirs=[root / "target"], cargo_home=root / "cargo")
            self.assertEqual([(lib.name, lib.version) for lib in discovery.libraries], [("demo", "4.2.9")])
            evidence = " ".join(discovery.libraries[0].evidence)
            self.assertIn("static-archive:", evidence)
            self.assertIn("libdemo.a", evidence)
            self.assertIn("header-confirms:", evidence)

            # A second archive with no version and no header stays visible.
            other = (
                root / "target" / "release" / "build" / "mystery-sys-fedcba9876543210" / "out"
            )
            other.mkdir(parents=True)
            (other / "libmystery.a").write_bytes(b"!<arch>\nno version here")
            deps = root / "target" / "release" / "deps"
            deps.mkdir(parents=True)
            (deps / "libnot_native.a").write_bytes(b"rust staticlib version 9.9.9")
            discovery = discover([lock], target_dirs=[root / "target"], cargo_home=root / "cargo")
            self.assertTrue(any(item["crate"] == "mystery-sys" for item in discovery.unversioned_static_archives))
            self.assertFalse(
                any("libnot_native.a" in item["path"] for item in discovery.unversioned_static_archives)
            )

            doc = supplement_document(discovery)
            blob = root / "supplement.json"
            blob.write_text(json.dumps(doc), encoding="utf-8")
            merged = merge_supplements(discover([lock]), [blob])
            self.assertEqual(merged.libraries[0].version, "4.2.9")
            self.assertTrue(any("libdemo.a" in item for item in merged.libraries[0].evidence))

    def test_remill_cmake_pins_and_static_archives(self) -> None:
        pins = [
            ROOT / "vendor/remill/dependencies/CMakeLists.txt",
            ROOT / "vendor/remill/dependencies/xed.cmake",
        ]
        discovery = discover([ROOT / "ceremony-wasm" / "Cargo.lock"], pin_files=pins)
        keyed = {(lib.name, lib.version, lib.origin) for lib in discovery.libraries}
        self.assertIn(("zstd", "1.5.7", "static-library"), keyed)
        self.assertIn(("glog", "0.7.1", "static-library"), keyed)
        self.assertIn(("googletest", "1.17.0", "static-library"), keyed)
        self.assertIn(("xed", "2025.06.08", "static-library"), keyed)
        self.assertIn(("mbuild", "2024.11.04", "cmake-pin"), keyed)
        self.assertNotIn("sleigh", {name for name, _version, _origin in keyed})
        glog = next(lib for lib in discovery.libraries if lib.name == "glog")
        self.assertIn("google:glog:0.7.1", glog.cpe or "")

        with tempfile.TemporaryDirectory() as tmp:
            libdir = Path(tmp) / "install" / "lib"
            libdir.mkdir(parents=True)
            (libdir / "libglog.a").write_bytes(b"!<arch>\nglog static")
            (libdir / "libgtest.a").write_bytes(b"!<arch>\ngtest static")
            (libdir / "libLLVMCore.a").write_bytes(b"!<arch>\nnot a third party pin")
            attached = discover(
                [ROOT / "ceremony-wasm" / "Cargo.lock"],
                pin_files=pins,
                archive_roots=[Path(tmp)],
            )
            glog = next(lib for lib in attached.libraries if lib.name == "glog")
            gtest = next(lib for lib in attached.libraries if lib.name == "googletest")
            self.assertTrue(any(item.endswith("libglog.a") for item in glog.evidence))
            self.assertTrue(any(item.endswith("libgtest.a") for item in gtest.evidence))
            self.assertFalse(
                any("libLLVMCore.a" in item for lib in attached.libraries for item in lib.evidence)
            )

    def test_api_version_num_in_bundled_headers(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            lock = root / "Cargo.lock"
            lock.write_text(
                "\n".join(
                    [
                        "version = 4",
                        "[[package]]",
                        'name = "ittapi-sys"',
                        'version = "0.4.0"',
                        'source = "registry+https://github.com/rust-lang/crates.io-index"',
                        "dependencies = [",
                        ' "cc",',
                        "]",
                    ]
                ),
                encoding="utf-8",
            )
            header = root / "cargo" / "registry" / "src" / "index" / "ittapi-sys-0.4.0" / "ittnotify_config.h"
            header.parent.mkdir(parents=True)
            header.write_text("#define API_VERSION_NUM 3.24.2\n", encoding="utf-8")
            archive = (
                root / "target" / "release" / "build" / "ittapi-sys-0123456789abcdef" / "out" / "libittnotify.a"
            )
            archive.parent.mkdir(parents=True)
            archive.write_bytes(b"!<arch>\nITT-API-Version 3.24.2")
            owned = root / "target" / "release" / "build" / "wasmtime-aaaaaaaaaaaaaaaa" / "out" / "libhelpers.a"
            owned.parent.mkdir(parents=True)
            owned.write_bytes(b"!<arch>\nwasmtime helper")
            # wasmtime itself is not in this tiny lock, so this archive stays unversioned.
            discovery = discover([lock], target_dirs=[root / "target"], cargo_home=root / "cargo")
            itt = next(lib for lib in discovery.libraries if lib.name == "ittapi")
            self.assertEqual(itt.version, "3.24.2")
            self.assertTrue(any("libittnotify.a" in item for item in itt.evidence))


if __name__ == "__main__":
    unittest.main()
