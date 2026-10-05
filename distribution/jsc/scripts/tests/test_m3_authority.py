"""Synthetic gate-validation unit tests, NOT native/Node compatibility evidence."""

import argparse
import contextlib
import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import MagicMock, patch


SCRIPT = Path(__file__).resolve().parents[1] / "m3_authority.py"
spec = importlib.util.spec_from_file_location("m3_authority_under_test", SCRIPT)
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)
COMMIT = "a" * 40
NODE_COMMIT = "b" * 40
REVISION = "c" * 40
PACKAGE_VERSION = "0.1.0"


class AuthorityGateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / gate.FIXTURE).parent.mkdir(parents=True)
        (self.root / gate.FIXTURE).write_text("// synthetic gate test, never executed\n")
        self.contract = {
            "schema_version": 1, "suite": gate.SUITE, "setup": {"synthetic": True},
            "expected_observations": [
                {"request": 1, "allowed": True, "body": "Exact BODY", "items": [None, 1]},
                {"request": 2, "allowed": False, "body": "Denied", "items": []},
            ],
        }
        (self.root / gate.CONTRACT).write_text(json.dumps(self.contract))
        (self.root / gate.m2.MANIFEST).parent.mkdir(parents=True)
        (self.root / gate.m2.MANIFEST).write_text(json.dumps({
            "source": {"revision": REVISION},
            "targets": [{"triple": target} for target in sorted(gate.m2.TARGETS)],
        }))
        self.reports = []
        for adapter in ("native", "node"):
            for target in sorted(gate.m2.TARGETS):
                report = {
                    "schema_version": 1, "suite": gate.SUITE, "status": "passed",
                    "adapter": adapter, "target": target, "commit": COMMIT,
                    **gate.corpus(self.root),
                    "runner": {
                        "system": "Darwin" if "darwin" in target else "Linux",
                        "machine": "aarch64" if target.startswith("aarch64") else "x86_64",
                        "translated": False,
                    },
                }
                if adapter == "native":
                    report.update({
                        "manifest_sha256": gate.m2.sha256(self.root / gate.m2.MANIFEST),
                        "engine_revision": REVISION, "mode": "source-build",
                        "receipt_sha256": "d" * 64, "archive_sha256": "e" * 64,
                        "sbom_sha256": "f" * 64,
                        "backend": {
                            "backend": "bundled-jsc", "target": target,
                            "engine_revision": REVISION, "distribution_mode": "source-build",
                            "hermetic": True,
                        },
                    })
                else:
                    report["node"] = {
                        "package": gate.NODE_PACKAGE, "package_version": PACKAGE_VERSION,
                        "source_commit": NODE_COMMIT, "version": "v24.1.0",
                        "build": {
                            "command": "pnpm run build --force",
                            "modules": {"index.js": "1" * 64, "application.js": "2" * 64,
                                        "authority.js": "3" * 64},
                        },
                    }
                self.reports.append(report)

    def validate(self, reports=None, **kwargs):
        gate.validate_reports(self.root, self.reports if reports is None else reports,
                              kwargs.get("commit", COMMIT),
                              kwargs.get("node_commit", NODE_COMMIT),
                              kwargs.get("node_package_version", PACKAGE_VERSION))

    def test_complete_synthetic_matrix_validates(self):
        self.validate()

    def test_missing_either_adapter_empty_duplicate_unknown_and_extra(self):
        for reports in ([], self.reports[:4], self.reports[4:], self.reports[:-1],
                        self.reports + [self.reports[0]],
                        self.reports[:-1] + [self.reports[0]]):
            with self.subTest(reports=len(reports)), self.assertRaises(ValueError):
                self.validate(reports)
        for key, value in (("adapter", "mock"), ("target", "unknown"), ("adapter", [])):
            reports = copy.deepcopy(self.reports)
            reports[0][key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.validate(reports)

    def test_common_identity_and_failed_partial_reports(self):
        for adapter_index in (0, 4):
            for key, value in (
                ("schema_version", True), ("schema_version", 1.0),
                ("suite", "other"), ("status", "failed"), ("status", "skipped"),
                ("status", "development"),
                ("commit", NODE_COMMIT), ("fixture_sha256", "0" * 64),
                ("contract_sha256", "0" * 64), ("observations", []),
            ):
                reports = copy.deepcopy(self.reports)
                reports[adapter_index][key] = value
                with self.subTest(adapter=adapter_index, key=key, value=value):
                    with self.assertRaises(ValueError):
                        self.validate(reports)

    def test_exact_observation_shape_values_and_types(self):
        for key, value in (
            ("request", True), ("request", 1.0), ("request", "1"),
            ("allowed", 1), ("body", "exact body"), ("body", "Exact BODY\n"),
            ("items", [None, True]), ("extra", "not in contract"),
        ):
            reports = copy.deepcopy(self.reports)
            reports[4]["observations"][0][key] = value
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                self.validate(reports)
        reports = copy.deepcopy(self.reports)
        reports[4]["observations"].reverse()
        with self.assertRaises(ValueError):
            self.validate(reports)

    def test_stale_corpus_and_manifest(self):
        for relative in (gate.FIXTURE, gate.CONTRACT, gate.m2.MANIFEST):
            path = self.root / relative
            original = path.read_text()
            path.write_text(original + "\n")
            with self.subTest(path=relative), self.assertRaises(ValueError):
                self.validate()
            path.write_text(original)

    def test_native_backend_artifact_and_receipt_digests(self):
        changes = [
            (("mode",), "system"), (("engine_revision",), "0" * 40),
            (("backend", "backend"), "system-jsc"),
            (("backend", "target"), "wrong"),
            (("backend", "engine_revision"), "wrong"),
            (("backend", "distribution_mode"), "system"),
            (("backend", "hermetic"), False), (("backend", "hermetic"), 1),
        ]
        for key in ("receipt_sha256", "archive_sha256", "sbom_sha256"):
            changes.extend([((key,), None), ((key,), "bad"), ((key,), "A" * 64)])
        for keys, value in changes:
            reports = copy.deepcopy(self.reports)
            item = reports[0]
            for key in keys[:-1]:
                item = item[key]
            item[keys[-1]] = value
            with self.subTest(keys=keys, value=value), self.assertRaises(ValueError):
                self.validate(reports)

    def test_node_pinned_package_source_and_exact_runtime_version(self):
        for key, value in (
            ("package", "@kunlun-engine/runtime-node"), ("package_version", "0.2.0"),
            ("source_commit", COMMIT), ("source_commit", "main"),
            ("version", "24"), ("version", "^24.1.0"), ("version", True),
        ):
            reports = copy.deepcopy(self.reports)
            reports[4]["node"][key] = value
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                self.validate(reports)
        for kwargs in ({"commit": "main"}, {"node_commit": "b" * 7},
                       {"node_package_version": "*"}, {"node_package_version": "0.2.0"}):
            with self.subTest(kwargs=kwargs), self.assertRaises(ValueError):
                self.validate(**kwargs)

    def test_node_requires_force_build_and_exact_emitted_module_inventory(self):
        build = self.reports[4]["node"]["build"]
        modules = build["modules"]
        invalid = [
            None, {}, [], {"modules": modules},
            {**build, "command": "pnpm run build"},
            {**build, "command": True},
            {"command": build["command"]},
            {**build, "modules": None},
            {**build, "modules": []},
            {**build, "modules": {}},
            {**build, "modules": {key: value for key, value in modules.items()
                                 if key != "authority.js"}},
            {**build, "modules": {**modules, "extra.js": "4" * 64}},
        ]
        reports = copy.deepcopy(self.reports)
        del reports[4]["node"]["build"]
        with self.assertRaisesRegex(ValueError, "Node adapter build"):
            self.validate(reports)
        for value in invalid:
            reports = copy.deepcopy(self.reports)
            reports[4]["node"]["build"] = value
            with self.subTest(build=value), self.assertRaises(ValueError):
                self.validate(reports)

    def test_node_emitted_hashes_require_full_lowercase_sha256(self):
        for module in ("index.js", "application.js", "authority.js"):
            for value in (None, True, 123, "", "bad", "1" * 63, "A" * 64):
                reports = copy.deepcopy(self.reports)
                reports[4]["node"]["build"]["modules"][module] = value
                with self.subTest(module=module, value=value), self.assertRaises(ValueError):
                    self.validate(reports)

    def test_node_emitted_hashes_must_match_across_all_platforms(self):
        for index in range(4, 8):
            reports = copy.deepcopy(self.reports)
            reports[index]["node"]["build"]["modules"]["authority.js"] = "4" * 64
            with self.subTest(index=index), self.assertRaisesRegex(ValueError, "across platforms"):
                self.validate(reports)
        reports = copy.deepcopy(self.reports)
        for report in reports[4:]:
            report["node"]["build"]["modules"] = {
                key: value for key, value in reversed(
                    list(report["node"]["build"]["modules"].items()))
            }
        self.validate(reports)

    def test_physical_runner_required_for_both_adapters(self):
        for index in (0, 4):
            for key, value in (
                ("translated", True), ("translated", None), ("translated", 0),
                ("system", "Windows"), ("machine", "x86_64"),
            ):
                reports = copy.deepcopy(self.reports)
                reports[index]["runner"][key] = value
                with self.subTest(index=index, key=key), self.assertRaises(ValueError):
                    self.validate(reports)
        reports = copy.deepcopy(self.reports)
        reports[2]["runner"]["machine"] = "arm64"
        with self.assertRaises(ValueError):
            self.validate(reports)

    def test_rosetta_detected_using_sysctl_not_machine(self):
        for value, expected in (("1\n", True), ("0\n", False), ("", False)):
            with patch.object(gate.platform, "system", return_value="Darwin"), \
                    patch.object(gate.platform, "machine", return_value="x86_64"), \
                    patch.object(gate.m2, "command", return_value=value) as command:
                self.assertIs(gate.runner_identity(self.root)["translated"], expected)
                self.assertEqual(command.call_args.args[0], ["sysctl", "-in", "sysctl.proc_translated"])
                self.assertEqual(command.call_args.kwargs["timeout"], 10)
        with patch.object(gate.platform, "system", return_value="Darwin"), \
                patch.object(gate.m2, "command", return_value="unknown"):
            with self.assertRaises(ValueError):
                gate.runner_identity(self.root)

    def test_probe_requires_exact_pass_not_skipped_or_empty(self):
        good = (f"test {gate.PROBE} ... ok\n"
                "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out; "
                "finished in 0.01s\n")
        gate.validate_probe(good)
        for output in ("", good.replace(" ... ok", " ... ignored"),
                       good.replace(gate.PROBE, "other"),
                       good.replace("1 passed", "0 passed"),
                       good + good):
            with self.subTest(output=output), self.assertRaises(ValueError):
                gate.validate_probe(output)

    def test_json_duplicate_keys_and_nonfinite_values_rejected(self):
        path = self.root / "bad.json"
        for content in ('{"a":1,"a":2}', '{"a":NaN}', '{"a":Infinity}'):
            path.write_text(content)
            with self.assertRaises(ValueError):
                gate.load_json(path)

    def test_contract_schema_is_not_boolean_and_requires_two_requests(self):
        for key, value in (("schema_version", True), ("expected_observations", [{}])):
            contract = copy.deepcopy(self.contract)
            contract[key] = value
            (self.root / gate.CONTRACT).write_text(json.dumps(contract))
            with self.assertRaises(ValueError):
                gate.corpus(self.root)

    def test_compare_loads_evidence_and_prints_only_slice_success(self):
        evidence = self.root / "evidence"
        for index, report in enumerate(self.reports):
            path = evidence / str(index) / "report.json"
            path.parent.mkdir(parents=True)
            path.write_text(json.dumps(report))
        args = argparse.Namespace(evidence=evidence, commit=COMMIT,
                                  node_commit=NODE_COMMIT, node_package_version=PACKAGE_VERSION)
        output = io.StringIO()
        with patch.object(gate, "ROOT", self.root), contextlib.redirect_stdout(output):
            gate.compare(args)
        self.assertEqual(output.getvalue(), gate.SUCCESS + "\n")

    def collection(self, *, dirty=False, missing_raw=False, raw_change=None,
                   command_error=None, github_sha=COMMIT, translated=False,
                   receipt_change=None, trusted_digest=None, probe_output=None,
                   final_commit=COMMIT, final_dirty=False, subprocess_transport=False,
                   inherited_loader=None, target_index=0, doctor_change=None, suite=gate.SUITE):
        """Mock command transport only to exercise gate failure persistence."""
        _, _, probe = gate.suite_settings(suite)
        execution, variable = gate.suite_execution(suite)
        target = self.reports[target_index]["target"]
        output = self.root / "collected"
        dist = self.root / "dist"
        dist.mkdir()
        receipt = {
            "schema_version": 1, "native_verified": True, "target": target,
            "manifest_sha256": gate.m2.sha256(self.root / gate.m2.MANIFEST),
            "mode": "source-build", "archive_sha256": "e" * 64, "sbom_sha256": "f" * 64,
        }
        if receipt_change:
            receipt_change(receipt)
        receipt_path = dist / ".kunlun-jsc-verification.json"
        receipt_path.write_text(json.dumps(receipt))
        env = {"KUNLUN_JSC_DIST_DIR": str(dist),
               "KUNLUN_JSC_RECEIPT_SHA256": trusted_digest or gate.m2.sha256(receipt_path),
               "KUNLUN_AUTHORITY_OBSERVATIONS": "inherited-must-not-change",
               "PATH": os.environ.get("PATH", ""),
               "CARGO_HOME": "/synthetic/cargo-home",
               "GITHUB_SHA": github_sha}
        loader_variable = ("DYLD_LIBRARY_PATH" if target.endswith("apple-darwin")
                           else "LD_LIBRARY_PATH")
        if inherited_loader is not None:
            env[loader_variable] = inherited_loader
        calls = []
        real_command = gate.m2.command

        def command(arguments, log, timeout, *, env=None, at_boundary=False):
            if subprocess_transport and not at_boundary:
                def spawn(child_arguments, **kwargs):
                    self.assertEqual(child_arguments, arguments)
                    self.assertEqual(kwargs["cwd"], self.root)
                    self.assertTrue(kwargs["start_new_session"])
                    child = MagicMock()
                    child.__enter__.return_value = child
                    child.returncode = 0
                    text = command(child_arguments, log, timeout,
                                   env=kwargs["env"], at_boundary=True)
                    child.communicate.return_value = (text, None)
                    return child

                with patch.object(gate.m2.subprocess, "Popen", side_effect=spawn):
                    result = real_command(arguments, log, timeout, env=env)
                return result
            calls.append(arguments)
            self.assertGreater(timeout, 0)
            if arguments[0] == "cargo":
                # Assert the environment at the process boundary, not just an
                # argv string. The synthetic probe exports through this env.
                self.assertEqual(env["PATH"], os.environ["PATH"])
                self.assertEqual(env["CARGO_HOME"], "/synthetic/cargo-home")
                self.assertEqual(env["KUNLUN_JSC_DIST_DIR"], str(dist))
                expected_loader = str(dist.resolve() / "lib")
                if inherited_loader:
                    expected_loader += os.pathsep + inherited_loader
                self.assertEqual(env[loader_variable], expected_loader)
            if arguments[:2] == ["git", "status"]:
                changed = final_dirty if log.name.startswith("final-") else dirty
                text = " M dirty\n" if changed else ""
                if log.name.startswith("final-"):
                    self.assertIn(":(top,exclude,literal)collected", arguments)
            elif arguments[:2] == ["git", "rev-parse"]:
                text = (final_commit if log.name.startswith("final-") else COMMIT) + "\n"
            elif arguments[:2] == ["cargo", "run"]:
                if command_error:
                    raise command_error
                self.assertEqual(env["KUNLUN_AUTHORITY_OBSERVATIONS"],
                                 "inherited-must-not-change")
                fields = {"backend": "bundled-jsc", "engine revision": REVISION,
                          "target": target, "distribution mode": "source-build",
                          "distribution": "pinned Kunlun JSC artifact", "hermetic": "true",
                          "synchronous smoke test": "ok",
                          **{capability: "true" for capability in gate.m2.CAPABILITIES}}
                if doctor_change:
                    doctor_change(fields)
                text = "\n".join(f"{key}: {value}" for key, value in fields.items())
            else:
                self.assertEqual(arguments[:2], ["cargo", "test"])
                self.assertIn("--exact", arguments)
                self.assertIn("--no-default-features", arguments)
                self.assertIn("bundled-jsc", arguments)
                self.assertIn(probe, arguments)
                self.assertIn(execution, arguments)
                raw = {"schema_version": 1, "suite": suite, **gate.corpus(self.root, suite),
                       "backend": copy.deepcopy(self.reports[target_index]["backend"])}
                if suite != gate.SUITE:
                    raw.update(adapter="native", status="development", qualification=False)
                if suite == gate.LIFECYCLE_SUITE:
                    raw["native_assertions"] = gate.load_json(
                        self.root / gate.LIFECYCLE_FIXTURE.with_suffix(".contract.json"))["expected_native_assertions"]
                if raw_change:
                    raw_change(raw)
                if not missing_raw:
                    Path(env[variable]).write_text(json.dumps(raw))
                text = probe_output if probe_output is not None else (
                    f"test {probe} ... ok\n"
                    "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out;\n")
            log.write_text(text)
            return text

        runner = {**self.reports[target_index]["runner"], "translated": translated}
        with patch.object(gate, "ROOT", self.root), patch.object(gate.m2, "ROOT", self.root), \
                patch.dict(os.environ, env, clear=True), \
                patch.object(gate, "runner_identity", return_value=runner), \
                patch.object(gate.m2, "command", side_effect=command):
            before = dict(os.environ)
            try:
                gate.collect_native(argparse.Namespace(target=target, output=output, suite=suite))
            finally:
                self.assertEqual(dict(os.environ), before)
        return gate.load_json(output / "report.json"), calls

    def test_collection_copies_synthetic_export_payload_and_bounds_commands(self):
        report, calls = self.collection()
        self.assertEqual(report["status"], "passed")
        self.assertTrue(gate.exact(report["observations"], self.contract["expected_observations"]))
        self.assertEqual(report["backend"], self.reports[0]["backend"])
        self.assertEqual(len(calls), 6)

    def test_cargo_process_environment_restores_sip_stripped_darwin_loader(self):
        report, calls = self.collection(subprocess_transport=True)
        self.assertEqual(report["status"], "passed")
        self.assertEqual([args[1] for args in calls if args[0] == "cargo"], ["run", "test"])

    def test_cargo_process_environment_preserves_inherited_darwin_loader(self):
        report, _ = self.collection(subprocess_transport=True,
                                    inherited_loader="/inherited/lib:/other/lib")
        self.assertEqual(report["status"], "passed")

    def test_cargo_process_environment_restores_linux_loader(self):
        report, _ = self.collection(subprocess_transport=True, target_index=1)
        self.assertEqual(report["status"], "passed")

    def test_cargo_process_environment_preserves_inherited_linux_loader(self):
        report, _ = self.collection(subprocess_transport=True, target_index=1,
                                    inherited_loader="/inherited/lib")
        self.assertEqual(report["status"], "passed")

    def test_rosetta_collection_is_diagnostic_but_not_matrix_evidence(self):
        report, _ = self.collection(translated=True)
        self.assertEqual(report["status"], "passed")
        self.assertIn("diagnostic collection only", report["diagnostic"])
        self.assertTrue(report["runner"]["translated"])
        reports = copy.deepcopy(self.reports)
        reports[0] = report
        with self.assertRaises(ValueError):
            self.validate(reports)

    def test_dirty_checkout_preserves_failed_report(self):
        with self.assertRaisesRegex(ValueError, "clean reviewed"):
            self.collection(dirty=True)
        self.assert_failed_report()

    def test_ci_commit_mismatch_preserves_failed_report(self):
        with self.assertRaisesRegex(ValueError, "GITHUB_SHA"):
            self.collection(github_sha=NODE_COMMIT)
        self.assert_failed_report()

    def test_head_changed_during_collection_preserves_failed_report(self):
        with self.assertRaisesRegex(ValueError, "HEAD changed"):
            self.collection(final_commit=NODE_COMMIT)
        self.assert_failed_report()

    def test_sources_changed_during_collection_preserves_failed_report(self):
        with self.assertRaisesRegex(ValueError, "sources changed"):
            self.collection(final_dirty=True)
        self.assert_failed_report()

    def test_corpus_rechecked_after_execution(self):
        def change(raw):
            (self.root / gate.FIXTURE).write_text("// changed concurrently\n")
            raw["fixture_sha256"] = gate.m2.sha256(self.root / gate.FIXTURE)

        with self.assertRaisesRegex(ValueError, "corpus changed"):
            self.collection(raw_change=change)
        self.assert_failed_report()

    def test_missing_raw_export_preserves_failed_report(self):
        with self.assertRaises(FileNotFoundError):
            self.collection(missing_raw=True)
        self.assert_failed_report()

    def test_skipped_probe_cannot_pass_even_with_raw_file(self):
        with self.assertRaisesRegex(ValueError, "exact successful"):
            self.collection(probe_output="running 0 tests\n")
        self.assert_failed_report()

    def test_receipt_digest_mismatch_preserves_failed_report(self):
        with self.assertRaisesRegex(ValueError, "receipt mismatch"):
            self.collection(trusted_digest="0" * 64)
        self.assert_failed_report()

    def test_receipt_boolean_schema_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "receipt schema"):
            self.collection(receipt_change=lambda receipt: receipt.update(schema_version=True))
        self.assert_failed_report()

    def test_doctor_system_backend_fails_before_probe_execution(self):
        with self.assertRaisesRegex(ValueError, "doctor identity"):
            self.collection(doctor_change=lambda fields: fields.update(backend="system-jsc"))
        self.assert_failed_report()
        self.assertFalse((self.root / "collected/observations.json").exists())

    def test_doctor_unpinned_engine_fails_before_probe_execution(self):
        with self.assertRaisesRegex(ValueError, "doctor identity"):
            self.collection(doctor_change=lambda fields: fields.update(
                {"engine revision": "0" * 40}))
        self.assert_failed_report()
        self.assertFalse((self.root / "collected/observations.json").exists())

    def test_receipt_changed_during_execution_cannot_stamp_passed_report(self):
        def change(raw):
            receipt = self.root / "dist/.kunlun-jsc-verification.json"
            receipt.write_text(receipt.read_text() + "\n")

        with self.assertRaisesRegex(ValueError, "receipt mismatch"):
            self.collection(raw_change=change)
        self.assert_failed_report()

    def test_actual_system_backend_cannot_pass_feature_flags(self):
        with self.assertRaisesRegex(ValueError, "backend"):
            self.collection(raw_change=lambda raw: raw["backend"].update(backend="system-jsc"))
        self.assert_failed_report()
        self.assertTrue((self.root / "collected/observations.json").is_file())

    def test_unexpected_exception_preserves_failed_report(self):
        with self.assertRaises(RuntimeError):
            self.collection(command_error=RuntimeError("synthetic unexpected failure"))
        self.assert_failed_report()

    def test_raw_hash_and_observation_validation(self):
        raw = {"schema_version": 1, "suite": gate.SUITE, **gate.corpus(self.root),
               "backend": self.reports[0]["backend"]}
        for key, value in (("schema_version", True), ("fixture_sha256", "0" * 64),
                           ("contract_sha256", "0" * 64), ("observations", []),
                           ("error", None), ("error", "")):
            with self.subTest(key=key), self.assertRaises(ValueError):
                gate.validate_raw(self.root, {**raw, key: value}, self.reports[0])

    def test_successful_reports_reject_error_key_but_allow_metadata(self):
        for index in (0, 4):
            for value in (None, "", False, "failed"):
                reports = copy.deepcopy(self.reports)
                reports[index]["error"] = value
                with self.subTest(index=index, value=value), self.assertRaises(ValueError):
                    self.validate(reports)
        reports = copy.deepcopy(self.reports)
        reports[0]["review_metadata"] = {"reviewed": True}
        self.validate(reports)

    def test_existing_output_not_overwritten(self):
        output = self.root / "existing"
        output.mkdir()
        report = output / "report.json"
        report.write_text("retain me")
        with self.assertRaises(FileExistsError):
            gate.collect_native(argparse.Namespace(target=self.reports[0]["target"], output=output))
        self.assertEqual(report.read_text(), "retain me")

    def assert_failed_report(self):
        report = gate.load_json(self.root / "collected/report.json")
        self.assertEqual(report["status"], "failed")
        self.assertTrue(report["error"])


if __name__ == "__main__":
    unittest.main()
