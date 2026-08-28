#!/usr/bin/env python3
"""Tests for the deterministic JSC artifact packager."""

from __future__ import annotations

import argparse
import importlib.util
import io
import json
import os
from pathlib import Path
import stat
import tarfile
import tempfile
import unittest
from unittest import mock


MODULE_PATH = Path(__file__).resolve().parents[1] / "jsc_artifact.py"
SPEC = importlib.util.spec_from_file_location("jsc_artifact", MODULE_PATH)
assert SPEC and SPEC.loader
jsc_artifact = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(jsc_artifact)


class ArtifactFixture:
    """Create the smallest complete source and manifest fixture."""

    def __init__(self, root: Path) -> None:
        self.root = root
        self.repository = root / "runtime"
        self.webkit = root / "webkit"
        self.repository.mkdir()
        self.webkit.mkdir()
        header = self.repository / "crates/kunlun-jsc-sys/include/kunlun_jsc.h"
        header.parent.mkdir(parents=True)
        header.write_text("/* header */\n", encoding="utf-8")
        (self.repository / "LICENSE").write_text("runtime license\n", encoding="utf-8")
        (self.webkit / "Source/JavaScriptCore").mkdir(parents=True)
        (self.webkit / "Source/JavaScriptCore/COPYING.LIB").write_text(
            "webkit license\n", encoding="utf-8"
        )
        self.jsc = root / "libJavaScriptCore.dylib"
        self.shim = root / "libkunlun_jsc.dylib"
        self.jsc.write_bytes(b"jsc-binary\n")
        self.shim.write_bytes(b"shim-binary\n")
        self.manifest_path = self.repository / "manifest.json"
        self.target = "aarch64-apple-darwin"
        self.manifest = {
            "distribution": "kunlun-jsc",
            "source": {
                "repository": "https://github.com/WebKit/WebKit.git",
                "revision": "1" * 40,
                "commit_url": "https://github.com/WebKit/WebKit/commit/" + "1" * 40,
            },
            "build": {
                "configuration": "Release",
                "driver": "Tools/Scripts/build-jsc",
                "arguments": {"macos": ["--release"], "linux": ["--release"]},
                "environment": {"LC_ALL": "C", "SOURCE_DATE_EPOCH": "1700000000", "TZ": "UTC"},
                "feature_flags": {"ENABLE_JIT": True},
            },
            "targets": [
                {
                    "triple": self.target,
                    "arch": "arm64",
                    "toolchain": "test-xcode",
                    "deployment_target": {"kind": "macos", "minimum": "14.0"},
                    "artifact": {
                        "archive_path": "artifacts/kunlun-jsc-test-aarch64-apple-darwin.tar.zst",
                        "library_paths": [
                            "lib/libJavaScriptCore.dylib",
                            "lib/libkunlun_jsc.dylib",
                        ],
                        "sbom": {
                            "format": "SPDX-2.3-json",
                            "path": "artifacts/kunlun-jsc-test-aarch64-apple-darwin.spdx.json",
                            "sha256": None,
                        },
                        "provenance": {
                            "format": "SLSA-provenance-v1",
                            "path": "artifacts/kunlun-jsc-test-aarch64-apple-darwin.intoto.jsonl",
                            "sha256": None,
                        },
                    },
                }
            ],
            "licenses": [
                {
                    "component": "Kunlun JSC shim",
                    "spdx_expression": "MIT",
                    "source": {"kind": "local", "path": "LICENSE"},
                    "sha256": jsc_artifact.sha256_file(self.repository / "LICENSE"),
                },
                {
                    "component": "WebKit JavaScriptCore",
                    "spdx_expression": "LGPL-2.1-or-later",
                    "source": {"kind": "upstream", "path": "Source/JavaScriptCore/COPYING.LIB"},
                    "sha256": jsc_artifact.sha256_file(
                        self.webkit / "Source/JavaScriptCore/COPYING.LIB"
                    ),
                },
            ],
            "abi": {"shim_version": 1, "public_headers": ["include/kunlun_jsc.h"]},
        }
        self.manifest_path.write_text(json.dumps(self.manifest), encoding="utf-8")
        self.zstd = root / "fake-zstd.py"
        self.zstd.write_text(
            """#!/usr/bin/env python3
import pathlib
import shutil
import sys
args = sys.argv[1:]
if '-d' in args and '-c' in args:
    sys.stdout.buffer.write(pathlib.Path(args[-1]).read_bytes())
else:
    output = pathlib.Path(args[args.index('-o') + 1])
    source = pathlib.Path(args[args.index('-o') - 1])
    shutil.copyfile(source, output)
""",
            encoding="utf-8",
        )
        self.zstd.chmod(self.zstd.stat().st_mode | stat.S_IXUSR)

    def assemble(self, name: str) -> tuple[Path, Path, Path]:
        """Assemble one fixture build and return archive, SBOM, and staging paths."""
        output = self.root / name
        staging = output / "staging"
        arguments = argparse.Namespace(
            manifest=self.manifest_path,
            repository_root=self.repository,
            webkit_root=self.webkit,
            target=self.target,
            jsc_library=self.jsc,
            shim_library=self.shim,
            staging=staging,
            output=output,
            zstd=str(self.zstd),
        )
        with mock.patch.object(
            jsc_artifact,
            "collect_tool_versions",
            return_value={"xcode": "test"},
        ):
            jsc_artifact.assemble(arguments)
        artifact = self.manifest["targets"][0]["artifact"]
        return output / artifact["archive_path"], output / artifact["sbom"]["path"], staging


class ArtifactTests(unittest.TestCase):
    """Exercise determinism, integrity, and rebuild reporting."""

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="jsc-artifact-test-")
        self.fixture = ArtifactFixture(Path(self.temporary.name))

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def test_independent_assemblies_are_byte_identical(self) -> None:
        first_archive, first_sbom, _ = self.fixture.assemble("first")
        second_archive, second_sbom, _ = self.fixture.assemble("second")
        self.assertEqual(jsc_artifact.sha256_file(first_archive), jsc_artifact.sha256_file(second_archive))
        self.assertEqual(jsc_artifact.sha256_file(first_sbom), jsc_artifact.sha256_file(second_sbom))

        report = Path(self.temporary.name) / "comparison.json"
        arguments = argparse.Namespace(
            first=first_archive,
            second=second_archive,
            output=report,
            zstd=str(self.fixture.zstd),
            require_identical=True,
        )
        jsc_artifact.compare(arguments)
        self.assertTrue(json.loads(report.read_text(encoding="utf-8"))["byte_identical"])

        second_archive.write_bytes(second_archive.read_bytes() + b"trailing transport bytes")
        with self.assertRaisesRegex(jsc_artifact.ArtifactError, "not byte-identical"):
            jsc_artifact.compare(arguments)

    def test_verify_checks_archive_and_sbom_inventory(self) -> None:
        archive, sbom, _ = self.fixture.assemble("verified")
        arguments = argparse.Namespace(
            manifest=self.fixture.manifest_path,
            target=self.fixture.target,
            archive=archive,
            sbom=sbom,
            zstd=str(self.fixture.zstd),
            skip_macho=True,
        )
        jsc_artifact.verify(arguments)

        value = json.loads(sbom.read_text(encoding="utf-8"))
        value["files"][0]["checksums"][0]["checksumValue"] = "0" * 64
        sbom.write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaisesRegex(jsc_artifact.ArtifactError, "SBOM inventory mismatch"):
            jsc_artifact.verify(arguments)

    def test_spdx_inventory_reports_missing_and_extra_in_the_expected_direction(self) -> None:
        extract_root = Path(self.temporary.name) / "inventory"
        extract_root.mkdir()
        (extract_root / "actual.txt").write_text("actual\n", encoding="utf-8")
        sbom = {
            "spdxVersion": "SPDX-2.3",
            "name": f"kunlun-jsc-{self.fixture.target}",
            "files": [
                {
                    "fileName": "./expected.txt",
                    "checksums": [
                        {"algorithm": "SHA1", "checksumValue": "0" * 40},
                        {"algorithm": "SHA256", "checksumValue": "0" * 64},
                    ],
                }
            ],
        }
        with self.assertRaisesRegex(
            jsc_artifact.ArtifactError,
            r"missing=\['expected\.txt'\], extra=\['actual\.txt'\]",
        ):
            jsc_artifact.verify_spdx(sbom, extract_root, self.fixture.target)

    def test_rejects_license_digest_mismatch(self) -> None:
        self.fixture.manifest["licenses"][0]["sha256"] = "0" * 64
        self.fixture.manifest_path.write_text(json.dumps(self.fixture.manifest), encoding="utf-8")
        with self.assertRaisesRegex(jsc_artifact.ArtifactError, "digest mismatch"):
            self.fixture.assemble("bad-license")

    def test_rejects_path_traversal(self) -> None:
        with self.assertRaisesRegex(jsc_artifact.ArtifactError, "safe relative path"):
            jsc_artifact.checked_relative_path("../escape", "fixture")

    def test_rejects_noncanonical_archive_member(self) -> None:
        archive = Path(self.temporary.name) / "noncanonical.tar.zst"
        payload = b"unexpected\n"
        with tarfile.open(archive, "w", format=tarfile.USTAR_FORMAT) as output:
            member = tarfile.TarInfo("include//unexpected")
            member.size = len(payload)
            output.addfile(member, io.BytesIO(payload))
        destination = Path(self.temporary.name) / "extract"
        destination.mkdir()
        with self.assertRaisesRegex(jsc_artifact.ArtifactError, "unsafe or duplicate"):
            jsc_artifact.decompress_archive(archive, str(self.fixture.zstd), destination)


if __name__ == "__main__":
    unittest.main()
