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

    def install_arguments(self, archive: Path, sbom: Path) -> argparse.Namespace:
        return argparse.Namespace(
            manifest=self.fixture.manifest_path, target=self.fixture.target,
            archive=archive, sbom=sbom, zstd=str(self.fixture.zstd),
            skip_macho=False, source_build=True, provenance=None,
            install_dir=Path(self.temporary.name) / "installed",
        )

    def test_install_requires_native_verification_and_explicit_source_trust(self) -> None:
        archive, sbom, _ = self.fixture.assemble("install")
        args = self.install_arguments(archive, sbom)
        args.skip_macho = True
        with self.assertRaisesRegex(jsc_artifact.ArtifactError, "requires native verification"):
            jsc_artifact.verify(args)
        args.skip_macho = False
        args.source_build = False
        with self.assertRaisesRegex(jsc_artifact.ArtifactError, "explicit --source-build"):
            jsc_artifact.verify(args)
        self.assertFalse(args.install_dir.exists())

    def test_install_records_exact_extracted_files_and_does_not_overwrite(self) -> None:
        archive, sbom, staging = self.fixture.assemble("install")
        args = self.install_arguments(archive, sbom)
        self.fixture.manifest["targets"][0]["artifact"]["status"] = "planned"
        self.fixture.manifest_path.write_text(json.dumps(self.fixture.manifest), encoding="utf-8")
        # Fixture bytes are not Mach-O; only this unit test mocks native validation.
        with mock.patch.object(jsc_artifact, "verify_macho") as native:
            jsc_artifact.verify(args)
        native.assert_called_once()
        receipt = jsc_artifact.read_json(args.install_dir / jsc_artifact.VERIFICATION_RECEIPT)
        self.assertTrue(receipt["native_verified"])
        self.assertEqual(receipt["mode"], "source-build")
        self.assertEqual(receipt["manifest_sha256"], jsc_artifact.sha256_file(self.fixture.manifest_path))
        self.assertEqual(receipt["files"], {
            path.relative_to(staging).as_posix(): jsc_artifact.sha256_file(path)
            for path in jsc_artifact.iter_regular_files(staging)
        })
        (args.install_dir / "keep.txt").write_text("keep", encoding="utf-8")
        with self.assertRaisesRegex(jsc_artifact.ArtifactError, "must be a new directory"):
            jsc_artifact.verify(args)
        self.assertEqual((args.install_dir / "keep.txt").read_text(), "keep")

    def test_published_artifacts_pin_archive_sbom_and_provenance(self) -> None:
        archive, sbom, _ = self.fixture.assemble("published")
        args = self.install_arguments(archive, sbom)
        args.source_build = False
        args.provenance = Path(self.temporary.name) / "provenance.jsonl"
        args.provenance.write_text("reviewed provenance bundle", encoding="utf-8")
        artifact = self.fixture.manifest["targets"][0]["artifact"]
        artifact["status"] = "published"
        artifact["sha256"] = jsc_artifact.sha256_file(archive)
        artifact["sbom"]["sha256"] = jsc_artifact.sha256_file(sbom)
        artifact["provenance"]["sha256"] = jsc_artifact.sha256_file(args.provenance)
        self.fixture.manifest_path.write_text(json.dumps(self.fixture.manifest), encoding="utf-8")
        with mock.patch.object(jsc_artifact, "verify_macho"):
            jsc_artifact.verify(args)
        receipt = jsc_artifact.read_json(args.install_dir / jsc_artifact.VERIFICATION_RECEIPT)
        self.assertEqual(receipt["mode"], "published")
        args.install_dir = None
        for path in (archive, sbom, args.provenance):
            original = path.read_bytes()
            path.write_bytes(original + b"corruption")
            with self.assertRaisesRegex(jsc_artifact.ArtifactError, "SHA-256 mismatch"):
                jsc_artifact.verify(args)
            path.write_bytes(original)

    def test_release_evidence_verifies_signatures_and_exact_sbom(self) -> None:
        archive, sbom, _ = self.fixture.assemble("evidence")
        artifact = self.fixture.manifest["targets"][0]["artifact"]
        evidence_dir = archive.parent
        provenance = evidence_dir / Path(artifact["provenance"]["path"]).name
        provenance.write_text("signed provenance bundle\n", encoding="utf-8")
        evidence_files = (archive, sbom, provenance)
        (evidence_dir / "SHA256SUMS").write_text(
            "".join(
                f"{jsc_artifact.sha256_file(path)}  {path.name}\n"
                for path in evidence_files
            ),
            encoding="utf-8",
        )
        arguments = argparse.Namespace(
            manifest=self.fixture.manifest_path,
            target=self.fixture.target,
            evidence_dir=evidence_dir,
            repository="kunlunengine/runtime",
            signer_workflow="kunlunengine/runtime/.github/workflows/jsc-macos.yml",
            source_digest="a" * 40,
            zstd=str(self.fixture.zstd),
        )
        verified_sbom = json.loads(sbom.read_text(encoding="utf-8"))
        command_results = [
            json.dumps([{"verificationResult": {"statement": {"predicate": {}}}}]),
            json.dumps(
                [
                    {
                        "verificationResult": {
                            "statement": {"predicate": verified_sbom}
                        }
                    }
                ]
            ),
        ]
        with (
            mock.patch.object(jsc_artifact, "verify") as verify_artifact,
            mock.patch.object(
                jsc_artifact, "command_output", side_effect=command_results
            ) as verify_attestation,
        ):
            jsc_artifact.verify_evidence(arguments)

        verify_artifact.assert_called_once()
        self.assertEqual(verify_attestation.call_count, 2)
        provenance_command = verify_attestation.call_args_list[0].args[0]
        self.assertIn("--bundle", provenance_command)
        self.assertIn("--source-digest", provenance_command)
        sbom_command = verify_attestation.call_args_list[1].args[0]
        self.assertIn("https://spdx.dev/Document/v2.3", sbom_command)

    def test_release_evidence_rejects_unlisted_or_tampered_files(self) -> None:
        archive, sbom, _ = self.fixture.assemble("bad-evidence")
        artifact = self.fixture.manifest["targets"][0]["artifact"]
        evidence_dir = archive.parent
        provenance = evidence_dir / Path(artifact["provenance"]["path"]).name
        provenance.write_text("signed provenance bundle\n", encoding="utf-8")
        evidence_files = (archive, sbom, provenance)
        checksums = evidence_dir / "SHA256SUMS"
        checksums.write_text(
            "".join(
                f"{jsc_artifact.sha256_file(path)}  {path.name}\n"
                for path in evidence_files
            ),
            encoding="utf-8",
        )
        manifest = jsc_artifact.read_json(self.fixture.manifest_path)
        archive.write_bytes(archive.read_bytes() + b"corruption")
        with self.assertRaisesRegex(jsc_artifact.ArtifactError, "SHA-256 mismatch"):
            jsc_artifact.verify_evidence_checksums(
                evidence_dir, manifest, self.fixture.target
            )

        archive.write_bytes(archive.read_bytes()[: -len(b"corruption")])
        (evidence_dir / "unexpected.txt").write_text("unexpected\n", encoding="utf-8")
        with self.assertRaisesRegex(jsc_artifact.ArtifactError, "inventory mismatch"):
            jsc_artifact.verify_evidence_checksums(
                evidence_dir, manifest, self.fixture.target
            )

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

    def test_parses_elf_dependency_and_symbol_version_metadata(self) -> None:
        outputs = {
            "-hW": "  Machine:                           AArch64\n",
            "-dW": """
 0x0000000000000001 (NEEDED) Shared library: [libc.so.6]
 0x000000000000000e (SONAME) Library soname: [libkunlun_jsc.so]
 0x000000000000000f (RPATH) Library rpath: [$ORIGIN]
 0x000000000000001d (RUNPATH) Library runpath: [$ORIGIN]
""",
            "--version-info": "  0x0010:   Name: GLIBC_2.17  Flags: none  Version: 2\n",
            "nm": "kunlun_jsc_context_create T 100 20\n",
        }

        def output(command: list[str]) -> str:
            if command[0] == "nm":
                return outputs["nm"]
            return outputs[command[1]]

        with mock.patch.object(jsc_artifact, "command_output", side_effect=output):
            identity = jsc_artifact.inspect_elf(Path("libkunlun_jsc.so"), include_exports=True)

        self.assertEqual(identity["machine"], "AArch64")
        self.assertEqual(identity["soname"], "libkunlun_jsc.so")
        self.assertEqual(identity["needed"], ["libc.so.6"])
        self.assertEqual(identity["runpaths"], ["$ORIGIN"])
        self.assertEqual(identity["required_versions"], ["GLIBC_2.17"])
        self.assertEqual(identity["exports"], ["kunlun_jsc_context_create"])

    def test_linux_policy_rejects_dependency_and_symbol_baseline_drift(self) -> None:
        root = Path(self.temporary.name) / "elf"
        (root / "metadata").mkdir(parents=True)
        metadata = {
            "schema_version": 1,
            "target": "aarch64-unknown-linux-gnu",
            "libraries": {
                "lib/libJavaScriptCore.so": {
                    "machine": "AArch64",
                    "soname": "libJavaScriptCore.so",
                    "needed": ["libc.so.6", "libicuuc.so.74"],
                    "runpaths": ["$ORIGIN"],
                    "required_versions": ["GLIBC_2.39", "GLIBCXX_3.4.32"],
                },
                "lib/libkunlun_jsc.so": {
                    "machine": "AArch64",
                    "soname": "libkunlun_jsc.so",
                    "needed": ["libJavaScriptCore.so", "libc.so.6"],
                    "runpaths": ["$ORIGIN"],
                    "required_versions": ["GLIBC_2.17"],
                    "exports": ["kunlun_jsc_context_create"],
                },
            },
        }
        runtime_metadata = root / "metadata/runtime-dependencies.json"
        runtime_metadata.write_text(json.dumps(metadata), encoding="utf-8")

        with mock.patch.object(jsc_artifact, "generate_elf_metadata", return_value=metadata):
            jsc_artifact.verify_elf(
                root,
                "aarch64-unknown-linux-gnu",
                "2.39",
                "GLIBCXX_3.4.32,CXXABI_1.3.14",
            )

        x86_metadata = json.loads(json.dumps(metadata))
        x86_metadata["target"] = "x86_64-unknown-linux-gnu"
        for identity in x86_metadata["libraries"].values():
            identity["machine"] = "Advanced Micro Devices X86-64"
        x86_metadata["libraries"]["lib/libJavaScriptCore.so"]["needed"].append(
            "ld-linux-x86-64.so.2"
        )
        runtime_metadata.write_text(json.dumps(x86_metadata), encoding="utf-8")
        with mock.patch.object(
            jsc_artifact, "generate_elf_metadata", return_value=x86_metadata
        ):
            jsc_artifact.verify_elf(
                root,
                "x86_64-unknown-linux-gnu",
                "2.39",
                "GLIBCXX_3.4.32,CXXABI_1.3.14",
            )

        drifted = json.loads(json.dumps(metadata))
        drifted["libraries"]["lib/libJavaScriptCore.so"]["needed"].append("libcurl.so.4")
        runtime_metadata.write_text(json.dumps(drifted), encoding="utf-8")
        with mock.patch.object(jsc_artifact, "generate_elf_metadata", return_value=drifted):
            with self.assertRaisesRegex(jsc_artifact.ArtifactError, "unsupported runtime"):
                jsc_artifact.verify_elf(
                    root,
                    "aarch64-unknown-linux-gnu",
                    "2.39",
                    "GLIBCXX_3.4.32,CXXABI_1.3.14",
                )

        drifted["libraries"]["lib/libJavaScriptCore.so"]["needed"].remove("libcurl.so.4")
        drifted["libraries"]["lib/libJavaScriptCore.so"]["needed"].append(
            "ld-linux-x86-64.so.2"
        )
        runtime_metadata.write_text(json.dumps(drifted), encoding="utf-8")
        with mock.patch.object(jsc_artifact, "generate_elf_metadata", return_value=drifted):
            with self.assertRaisesRegex(jsc_artifact.ArtifactError, "ld-linux-x86-64"):
                jsc_artifact.verify_elf(
                    root,
                    "aarch64-unknown-linux-gnu",
                    "2.39",
                    "GLIBCXX_3.4.32,CXXABI_1.3.14",
                )

        drifted["libraries"]["lib/libJavaScriptCore.so"]["needed"].remove(
            "ld-linux-x86-64.so.2"
        )
        drifted["libraries"]["lib/libJavaScriptCore.so"]["required_versions"].append(
            "GLIBC_2.40"
        )
        runtime_metadata.write_text(json.dumps(drifted), encoding="utf-8")
        with mock.patch.object(jsc_artifact, "generate_elf_metadata", return_value=drifted):
            with self.assertRaisesRegex(jsc_artifact.ArtifactError, "beyond the recorded"):
                jsc_artifact.verify_elf(
                    root,
                    "aarch64-unknown-linux-gnu",
                    "2.39",
                    "GLIBCXX_3.4.32,CXXABI_1.3.14",
                )

        drifted = json.loads(json.dumps(metadata))
        drifted["libraries"]["lib/libkunlun_jsc.so"]["exports"].append("_init")
        runtime_metadata.write_text(json.dumps(drifted), encoding="utf-8")
        with mock.patch.object(jsc_artifact, "generate_elf_metadata", return_value=drifted):
            with self.assertRaisesRegex(jsc_artifact.ArtifactError, "_init"):
                jsc_artifact.verify_elf(
                    root,
                    "aarch64-unknown-linux-gnu",
                    "2.39",
                    "GLIBCXX_3.4.32,CXXABI_1.3.14",
                )

    def test_assembles_and_verifies_linux_inventory(self) -> None:
        target = "aarch64-unknown-linux-gnu"
        self.fixture.target = target
        self.fixture.jsc = self.fixture.root / "libJavaScriptCore.so"
        self.fixture.shim = self.fixture.root / "libkunlun_jsc.so"
        self.fixture.jsc.write_bytes(b"linux-jsc\n")
        self.fixture.shim.write_bytes(b"linux-shim\n")
        entry = self.fixture.manifest["targets"][0]
        entry.update(
            {
                "triple": target,
                "os": "linux",
                "arch": "arm64",
                "libc": "glibc",
                "deployment_target": {"kind": "glibc", "minimum": "2.39"},
            }
        )
        entry["artifact"].update(
            {
                "archive_path": f"artifacts/kunlun-jsc-test-{target}.tar.zst",
                "library_paths": ["lib/libJavaScriptCore.so", "lib/libkunlun_jsc.so"],
            }
        )
        entry["artifact"]["sbom"]["path"] = f"artifacts/kunlun-jsc-test-{target}.spdx.json"
        entry["artifact"]["provenance"]["path"] = (
            f"artifacts/kunlun-jsc-test-{target}.intoto.jsonl"
        )
        self.fixture.manifest_path.write_text(
            json.dumps(self.fixture.manifest), encoding="utf-8"
        )
        elf_metadata = {
            "schema_version": 1,
            "target": target,
            "libraries": {},
        }
        with mock.patch.object(
            jsc_artifact, "generate_elf_metadata", return_value=elf_metadata
        ):
            archive, sbom, staging = self.fixture.assemble("linux")
            arguments = argparse.Namespace(
                manifest=self.fixture.manifest_path,
                target=target,
                archive=archive,
                sbom=sbom,
                zstd=str(self.fixture.zstd),
                skip_macho=True,
            )
            jsc_artifact.verify(arguments)

        self.assertTrue((staging / "lib/libJavaScriptCore.so").is_file())
        self.assertTrue((staging / "lib/libkunlun_jsc.so").is_file())
        self.assertTrue((staging / "metadata/runtime-dependencies.json").is_file())


if __name__ == "__main__":
    unittest.main()
