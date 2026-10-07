"""Synthetic HTTP evidence validation, never platform qualification."""
import argparse
import copy
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import test_m3_authority as filesystem
from test_m3_authority import gate, COMMIT, NODE_COMMIT, PACKAGE_VERSION


class HttpGateTests(unittest.TestCase):
    def setUp(self):
        base = filesystem.AuthorityGateTests()
        base.setUp()
        self.base = base
        self.addCleanup(base.doCleanups)
        self.root, self.reports = base.root, base.reports
        contract = {**base.contract, "suite": gate.HTTP_SUITE, "qualification": False,
                    "expected_lifecycle": {"pending_rejected_before_response": True,
                                           "later_admission_denied": True},
                    "expected_requests": ["/ok", "/ok", "/pending"]}
        (self.root / gate.HTTP_FIXTURE).write_text("// HTTP synthetic\n")
        (self.root / gate.HTTP_FIXTURE.with_suffix(".contract.json")).write_text(json.dumps(contract))
        (self.root / gate.HTTP_FIXTURE.with_name("request-authority-http-revocation.js")).write_text("// revoke\n")
        for report in self.reports:
            report.update(suite=gate.HTTP_SUITE, qualification=False,
                          **gate.corpus(self.root, gate.HTTP_SUITE))
            if report["adapter"] == "node":
                report["node"].update(
                    runtime={"version": "v24.1.0", "arch": "arm64" if report["target"].startswith("aarch64") else "x64",
                             "platform": "darwin" if report["target"].endswith("apple-darwin") else "linux",
                             "executable_sha256": "a" * 64, "libc": "2.36"},
                    dependencies={"undici@7.0.0": "b" * 64},
                    installation={"command": gate.NODE_INSTALL_COMMAND, "fresh_store": True,
                                  "ci": True, "lockfile_sha256": "c" * 64})

    def validate(self, reports=None):
        gate.validate_reports(self.root, self.reports if reports is None else reports,
                              COMMIT, NODE_COMMIT, PACKAGE_VERSION, gate.HTTP_SUITE)

    def test_complete_matrix(self):
        self.validate()

    def test_http_contract_and_identity_mutations_fail(self):
        mutations = {
            "suite": gate.SUITE, "status": "development", "qualification": True,
            "commit": "f" * 40, "revocation_sha256": "f" * 64,
            "fixture_sha256": "f" * 64, "contract_sha256": "f" * 64,
            "observations": [], "lifecycle": {}, "requests": ["/pending", "/ok", "/ok"],
            "runner": {"system": "Linux", "machine": "aarch64", "translated": True},
        }
        for adapter_index in (0, 4):
            for key, value in mutations.items():
                with self.subTest(adapter=adapter_index, key=key):
                    reports = copy.deepcopy(self.reports)
                    reports[adapter_index][key] = value
                    with self.assertRaises(ValueError):
                        self.validate(reports)
        for reports in (self.reports[:-1], self.reports + [self.reports[0]], []):
            with self.assertRaises(ValueError):
                self.validate(reports)

    def test_missing_fields_and_failed_reports(self):
        for key in ("requests", "lifecycle", "revocation_sha256", "qualification"):
            reports = copy.deepcopy(self.reports)
            del reports[0][key]
            with self.assertRaises(ValueError):
                self.validate(reports)
        for status in ("failed", "skipped"):
            reports = copy.deepcopy(self.reports)
            reports[0]["status"] = status
            with self.assertRaises(ValueError):
                self.validate(reports)

    def test_raw_requires_real_backend_and_development_label(self):
        identity = self.reports[0]
        raw = {**identity, "status": "development"}
        gate.validate_raw(self.root, raw, identity, gate.HTTP_SUITE)
        for key, value in (("backend", {"backend": "system-jsc"}), ("status", "passed"),
                           ("qualification", True), ("requests", [])):
            with self.subTest(key=key), self.assertRaises(ValueError):
                gate.validate_raw(self.root, {**raw, key: value}, identity, gate.HTTP_SUITE)

    def test_exact_http_probe(self):
        output = f"test {gate.HTTP_PROBE} ... ok\n"
        output += "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out;\n"
        gate.validate_probe(output, gate.HTTP_SUITE)
        for invalid in ("", output.replace("ok\n", "ignored\n"),
                        output.replace(gate.HTTP_PROBE, gate.PROBE)):
            with self.assertRaises(ValueError):
                gate.validate_probe(invalid, gate.HTTP_SUITE)

    def test_native_collection_executes_exact_http_test_and_exports_slice(self):
        report, calls = self.base.collection(suite=gate.HTTP_SUITE)
        self.assertEqual(report["status"], "passed")
        self.assertIs(report["qualification"], False)
        self.assertEqual(report["lifecycle"], self.reports[0]["lifecycle"])
        self.assertEqual(report["requests"], self.reports[0]["requests"])
        self.assertEqual(report["backend"], self.reports[0]["backend"])
        cargo = next(call for call in calls if call[:2] == ["cargo", "test"])
        self.assertIn("request_authority_http", cargo)
        self.assertIn(gate.HTTP_PROBE, cargo)

    def test_native_http_system_backend_preserves_failure(self):
        with self.assertRaises(ValueError):
            self.base.collection(suite=gate.HTTP_SUITE,
                                 raw_change=lambda raw: raw["backend"].update(backend="system-jsc"))
        self.assertEqual(gate.load_json(self.root / "collected/report.json")["status"], "failed")

    def node_collection(self, mutation=None, *, loader_env=None):
        with tempfile.TemporaryDirectory() as temp:
            core = (Path(temp) / "core").resolve()
            package = core / "packages/runtime-node"
            (package / "dist").mkdir(parents=True)
            (package / "package.json").write_text(json.dumps(
                {"name": gate.NODE_PACKAGE, "version": PACKAGE_VERSION}))
            (core / "pnpm-lock.yaml").write_text("synthetic reviewed lock")
            for name in gate.NODE_MODULES:
                (package / "dist" / name).write_text("synthetic build")
            args = argparse.Namespace(core_root=core, output=Path(temp) / "evidence",
                                      target=self.reports[4]["target"], commit=COMMIT,
                                      node_commit=NODE_COMMIT, node_package_version=PACKAGE_VERSION,
                                      suite=self.reports[4]["suite"])
            calls = []

            def command(arguments, log, **kwargs):
                calls.append(arguments)
                if arguments[0] == "git":
                    return (NODE_COMMIT if str(core) in arguments else COMMIT) if "rev-parse" in arguments else ""
                if Path(arguments[0]).name == "node":
                    self.assertEqual(kwargs["env"]["CI"], "true")
                    if "--input-type=module" in arguments:
                        return json.dumps(self.reports[4]["node"][
                            "dependencies" if "dependencyClosure" in arguments[-1] else "runtime"])
                    raw = {**copy.deepcopy(self.reports[4]), "status": "development"}
                    raw["node"]["modules"] = {name: gate.m2.sha256(package / "dist" / name)
                                              for name in gate.NODE_MODULES}
                    if mutation:
                        mutation(raw, package)
                    Path(arguments[3]).write_text(json.dumps(raw))
                return ""

            env = {key: value for key, value in os.environ.items()
                   if key.upper() not in gate.LOADER_ENV}
            env.update(loader_env or {})
            with patch.dict(os.environ, env, clear=True), \
                 patch.object(gate, "ROOT", self.root), patch.object(gate.m2, "command", side_effect=command), \
                 patch.object(gate, "runner_identity", return_value=self.reports[4]["runner"]):
                gate.collect_node(args)
            report = gate.load_json(args.output / "report.json")
            self.assertEqual(report["status"], "passed")
            self.assertIn(["pnpm", "--dir", str(core), "run", "build", "--force"], calls)
            self.assertEqual(report["node"]["build"]["command"], gate.NODE_BUILD_COMMAND)
            install = next(call for call in calls if "install" in call)
            self.assertIn("--frozen-lockfile", install)
            self.assertIn("--ignore-scripts", install)
            self.assertIn("--store-dir", install)

    def test_node_collection_binds_force_build_and_rechecks(self):
        self.node_collection()

    def test_node_collection_rejects_source_build_and_raw_mutations(self):
        changes = [
            lambda raw, package: raw.update(status="failed"),
            lambda raw, package: raw.update(requests=[]),
            lambda raw, package: raw.update(lifecycle={}),
            lambda raw, package: raw["node"].update(source_commit="f" * 40),
            lambda raw, package: raw["node"].update(version="v22.0.0"),
            lambda raw, package: raw["node"].update(modules={}),
            lambda raw, package: (package / "dist/index.js").write_text("changed build"),
            lambda raw, package: (package / "package.json").write_text("{}"),
        ]
        for index, change in enumerate(changes):
            with self.subTest(change=index), self.assertRaises(ValueError):
                self.node_collection(change)

    def test_node_errors_preserve_failed_report_without_build(self):
        with tempfile.TemporaryDirectory() as temp:
            args = argparse.Namespace(core_root=Path(temp) / "core", output=Path(temp) / "evidence",
                                      target=self.reports[4]["target"], commit=COMMIT,
                                      node_commit=NODE_COMMIT, node_package_version=PACKAGE_VERSION)
            with patch.object(gate.m2, "command", side_effect=ValueError("dirty source")), \
                 self.assertRaises(ValueError):
                gate.collect_node(args)
            self.assertEqual(gate.load_json(args.output / "report.json")["status"], "failed")
