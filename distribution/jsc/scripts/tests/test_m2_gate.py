"""Fail-closed gate checks without native JSC, network access, or Cargo."""

import argparse
import copy
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import MagicMock, patch


SCRIPT = Path(__file__).resolve().parents[1] / "m2_gate.py"
SPEC = importlib.util.spec_from_file_location("m2_gate", SCRIPT)
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)


class M2GateTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        manifest = self.root / gate.MANIFEST
        manifest.parent.mkdir(parents=True)
        manifest.write_text(json.dumps({
            "source": {"revision": "a" * 40},
            "targets": [{"triple": target} for target in sorted(gate.TARGETS)],
        }))
        suite = self.root / gate.SUITE
        suite.parent.mkdir(parents=True)
        suite.write_text("#[test]\nfn required_semantics() {}\n")
        fixture = suite.parent / "fixtures/m2/entry.mjs"
        fixture.parent.mkdir(parents=True)
        fixture.write_text("await Promise.resolve();")
        self.reports = [{
            "schema_version": 1, "status": "passed", "commit": "b" * 40,
            "manifest_sha256": gate.sha256(manifest), "engine_revision": "a" * 40,
            "corpus_sha256": gate.corpus(self.root), "target": target,
            "tests": ["required_semantics"], "mode": "source-build",
            "capabilities": {key: True for key in gate.CAPABILITIES},
            "native_sanitizers": "passed",
            "receipt_sha256": "c" * 64, "archive_sha256": "d" * 64, "sbom_sha256": "e" * 64,
        } for target in sorted(gate.TARGETS)]

    def validate(self, reports):
        gate.validate_reports(self.root, reports, "b" * 40)

    def test_complete_matrix_passes(self):
        self.validate(self.reports)

    def test_missing_duplicate_and_extra_platforms_fail(self):
        for reports in [self.reports[:-1], self.reports + [self.reports[0]],
                        [self.reports[0]] * 4]:
            with self.subTest(reports=reports), self.assertRaises(ValueError):
                self.validate(reports)

    def test_mismatched_or_incomplete_evidence_fails(self):
        for key, value in {
            "status": "skipped", "commit": "c" * 40, "manifest_sha256": "f" * 64,
            "engine_revision": "d" * 40, "corpus_sha256": "a" * 64,
            "tests": [], "capabilities": {}, "receipt_sha256": None,
            "archive_sha256": None, "sbom_sha256": None, "mode": "system-jsc",
            "native_sanitizers": "skipped",
        }.items():
            reports = copy.deepcopy(self.reports)
            reports[0][key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.validate(reports)

    def test_fixture_change_invalidates_reports(self):
        fixture = self.root / gate.SUITE.parent / "fixtures/m2/entry.mjs"
        fixture.write_text("throw new Error('changed');")
        with self.assertRaisesRegex(ValueError, "corpus_sha256"):
            self.validate(self.reports)

    def test_missing_capabilities_fail(self):
        fields = {
            "backend": "bundled-jsc", "engine revision": "a" * 40,
            "target": sorted(gate.TARGETS)[0], "distribution mode": "source-build",
            "distribution": "pinned Kunlun JSC artifact", "hermetic": "true",
            "synchronous smoke test": "ok",
            **{key: "true" for key in gate.CAPABILITIES},
        }
        output = lambda data: "\n".join(f"{key}: {value}" for key, value in data.items())
        gate.validate_doctor(output(fields), fields["target"], "a" * 40, "source-build")
        for capability in gate.CAPABILITIES:
            with self.subTest(capability=capability), self.assertRaises(ValueError):
                gate.validate_doctor(output({**fields, capability: "false"}),
                                     fields["target"], "a" * 40, "source-build")

    def test_empty_skipped_or_filtered_corpus_fails(self):
        valid = ("test required_semantics ... ok\n"
                 "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;\n")
        self.assertEqual(gate.validate_tests(valid, ["required_semantics"]), ["required_semantics"])
        for output in ["", valid.replace(" ... ok", " ... ignored"),
                       valid.replace("0 filtered", "1 filtered"),
                       valid.replace("1 passed", "0 passed")]:
            with self.subTest(output=output), self.assertRaises(ValueError):
                gate.validate_tests(output, ["required_semantics"])

    def test_system_baseline_does_not_satisfy_native_m2_gate(self):
        gate.validate_sanitizers(gate.SANITIZER_PASS + "\n")
        for output in ["", "PASS system baseline ASan/UBSan", "prefix " + gate.SANITIZER_PASS]:
            with self.subTest(output=output), self.assertRaises(ValueError):
                gate.validate_sanitizers(output)

    def test_receipt_requires_trusted_digest_native_verification_and_identity(self):
        receipt = {
            "schema_version": 1, "native_verified": True,
            "target": sorted(gate.TARGETS)[0], "mode": "source-build",
            "manifest_sha256": gate.sha256(self.root / gate.MANIFEST),
            "archive_sha256": "c" * 64, "sbom_sha256": "d" * 64,
        }
        path = self.root / ".kunlun-jsc-verification.json"
        path.write_text(json.dumps(receipt))
        gate.identity(self.root, receipt["target"], self.root, gate.sha256(path))
        with self.assertRaises(ValueError):
            gate.identity(self.root, receipt["target"], self.root, "f" * 64)
        for key, value in {"native_verified": False, "target": "wrong",
                           "manifest_sha256": "f" * 64, "mode": "system-jsc"}.items():
            path.write_text(json.dumps({**receipt, key: value}))
            with self.subTest(key=key), self.assertRaises(ValueError):
                gate.identity(self.root, receipt["target"], self.root, gate.sha256(path))

    def test_pinned_m2_cargo_processes_receive_verified_loader_environment(self):
        for target in ("aarch64-apple-darwin", "aarch64-unknown-linux-gnu"):
            variable = "DYLD_LIBRARY_PATH" if target.endswith("apple-darwin") else "LD_LIBRARY_PATH"
            for inherited in (None, "/inherited/lib"):
                with self.subTest(target=target, inherited=inherited), \
                        tempfile.TemporaryDirectory(dir=self.root) as temporary:
                    directory = Path(temporary)
                    dist = directory / "dist"
                    dist.mkdir()
                    receipt = dist / ".kunlun-jsc-verification.json"
                    receipt.write_text(json.dumps({
                        "schema_version": 1, "native_verified": True, "target": target,
                        "mode": "source-build", "engine_revision": "a" * 40,
                        "manifest_sha256": gate.sha256(self.root / gate.MANIFEST),
                        "archive_sha256": "c" * 64, "sbom_sha256": "d" * 64,
                    }))
                    native_log = directory / "native.txt"
                    native_log.write_text(gate.SANITIZER_PASS + "\n")
                    env = {
                        "PATH": os.environ["PATH"], "CARGO_HOME": "/test/cargo",
                        "KUNLUN_JSC_DIST_DIR": str(dist),
                        "KUNLUN_JSC_RECEIPT_SHA256": gate.sha256(receipt),
                    }
                    if inherited is not None:
                        env[variable] = inherited
                    cargo_calls = []

                    def spawn(arguments, **kwargs):
                        self.assertEqual(kwargs["cwd"], self.root)
                        self.assertTrue(kwargs["start_new_session"])
                        if arguments[0] == "cargo":
                            cargo_calls.append(arguments)
                            expected = {
                                **env, variable: str(dist.resolve() / "lib")
                                + (os.pathsep + inherited if inherited else ""),
                            }
                            self.assertEqual(kwargs["env"], expected)
                            self.assertIn("--locked", arguments)
                            self.assertIn("bundled-jsc", arguments)
                            self.assertEqual(arguments[arguments.index("--target") + 1], target)
                        if arguments[:2] == ["cargo", "run"]:
                            fields = {
                                "backend": "bundled-jsc", "engine revision": "a" * 40,
                                "target": target, "distribution mode": "source-build",
                                "distribution": "pinned Kunlun JSC artifact", "hermetic": "true",
                                "synchronous smoke test": "ok",
                                **{key: "true" for key in gate.CAPABILITIES},
                            }
                            text = "\n".join(f"{key}: {value}" for key, value in fields.items())
                        elif "--test" in arguments:
                            text = ("test required_semantics ... ok\n"
                                    "test result: ok. 1 passed; 0 failed; 0 ignored; "
                                    "0 measured; 0 filtered out;\n")
                        else:
                            text = "ok\n"
                        child = MagicMock()
                        child.__enter__.return_value = child
                        child.returncode = 0
                        child.communicate.return_value = (text, None)
                        return child

                    output = directory / "evidence"
                    with patch.object(gate, "ROOT", self.root), \
                            patch.dict(os.environ, env, clear=True), \
                            patch.object(gate.subprocess, "check_output",
                                         side_effect=["", "b" * 40 + "\n"]), \
                            patch.object(gate.subprocess, "Popen", side_effect=spawn):
                        before = dict(os.environ)
                        gate.run(argparse.Namespace(target=target, output=output,
                                                    native_log=native_log))
                        self.assertEqual(dict(os.environ), before)
                    self.assertEqual(len(cargo_calls), 3)
                    self.assertIn("--workspace", cargo_calls[1])
                    self.assertIn("m2_conformance", cargo_calls[2])
                    report = json.loads((output / "report.json").read_text())
                    self.assertEqual(report["status"], "passed")
                    self.assertEqual(report["receipt_sha256"], gate.sha256(receipt))


class CommandTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.log = Path(temporary.name) / "command.txt"

    def test_real_child_inherits_environment_by_default(self):
        with patch.dict(os.environ, {"KUNLUN_COMMAND_TEST": "inherited"}):
            output = gate.command(
                [sys.executable, "-c", "import os; print(os.environ['KUNLUN_COMMAND_TEST'])"],
                self.log, timeout=10)
        self.assertEqual(output, "inherited\n")
        self.assertEqual(self.log.read_text(), output)

    def test_real_child_receives_overrides_without_mutating_parent(self):
        with patch.dict(os.environ, {"KUNLUN_COMMAND_TEST": "parent"}):
            before = dict(os.environ)
            env = {**before, "KUNLUN_COMMAND_TEST": "child"}
            output = gate.command(
                [sys.executable, "-c",
                 "import os; print(os.environ['KUNLUN_COMMAND_TEST']); print(os.environ['PATH'])"],
                self.log, timeout=10, env=env)
            self.assertEqual(dict(os.environ), before)
            self.assertEqual(env["KUNLUN_COMMAND_TEST"], "child")
        self.assertEqual(output, "child\n" + before["PATH"] + "\n")
        self.assertEqual(self.log.read_text(), output)

    def test_child_failure_retains_log_with_explicit_environment(self):
        with self.assertRaisesRegex(ValueError, "gate command failed"):
            gate.command([sys.executable, "-c", "print('failure'); raise SystemExit(7)"],
                         self.log, timeout=10, env=os.environ.copy())
        self.assertEqual(self.log.read_text(), "failure\n")

    def test_timeout_kills_process_group_and_retains_log_with_environment(self):
        child = MagicMock()
        child.__enter__.return_value = child
        child.pid = 12345
        child.communicate.side_effect = [subprocess.TimeoutExpired(["cargo"], 1),
                                        ("partial output\n", None)]
        env = {**os.environ, "KUNLUN_COMMAND_TEST": "child"}
        with patch.object(gate.subprocess, "Popen", return_value=child) as spawn, \
                patch.object(gate.os, "killpg") as kill:
            with self.assertRaisesRegex(ValueError, "watchdog expired"):
                gate.command(["cargo"], self.log, timeout=1, env=env)
        self.assertIs(spawn.call_args.kwargs["env"], env)
        self.assertTrue(spawn.call_args.kwargs["start_new_session"])
        kill.assert_called_once_with(child.pid, gate.signal.SIGKILL)
        self.assertEqual(child.communicate.call_args_list[0].kwargs, {"timeout": 1})
        self.assertEqual(child.communicate.call_count, 2)
        self.assertEqual(self.log.read_text(), "partial output\n")


if __name__ == "__main__":
    unittest.main()
