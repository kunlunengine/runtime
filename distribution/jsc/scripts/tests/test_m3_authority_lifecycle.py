"""Synthetic lifecycle collection/validation; no permission-policy emulation."""
import copy
import json
import unittest

from test_m3_authority_http import HttpGateTests
from test_m3_authority import gate, COMMIT, NODE_COMMIT, PACKAGE_VERSION


class LifecycleGateTests(HttpGateTests):
    def setUp(self):
        super().setUp()
        contract = {
            "schema_version": 1, "suite": gate.LIFECYCLE_SUITE,
            "expected_progress": {"application": {}, "request": {}, "survivor": {}},
            "expected_lifecycle": {"closed": True}, "expected_traffic": {"paths": []},
            "expected_concurrency": {"isolated": True},
            "expected_concurrent_traffic": {"paths": []},
            "expected_native_assertions": {"settled": True},
        }
        (self.root / gate.LIFECYCLE_FIXTURE).write_text("// synthetic lifecycle")
        (self.root / gate.LIFECYCLE_FIXTURE.with_suffix(".contract.json")).write_text(json.dumps(contract))
        for report in self.reports:
            for field in ("requests", "revocation_sha256"):
                report.pop(field, None)
            report.update(suite=gate.LIFECYCLE_SUITE, **gate.corpus(self.root, gate.LIFECYCLE_SUITE))
            if report["adapter"] == "native":
                report["native_assertions"] = contract["expected_native_assertions"]

    def validate(self, reports=None):
        gate.validate_reports(self.root, self.reports if reports is None else reports,
                              COMMIT, NODE_COMMIT, PACKAGE_VERSION, gate.LIFECYCLE_SUITE)

    # HTTP-specific assertions are intentionally replaced, not interpreted as
    # lifecycle behavior. Shared collection/matrix tests still use real schema.
    def test_native_collection_executes_exact_http_test_and_exports_slice(self):
        report, calls = self.base.collection(suite=gate.LIFECYCLE_SUITE)
        self.assertEqual(report["native_assertions"], {"settled": True})
        self.assertEqual(report["status"], "passed")
        cargo = next(call for call in calls if call[:2] == ["cargo", "test"])
        self.assertIn(gate.LIFECYCLE_PROBE, cargo)
        self.assertIn("request_authority_lifecycle", cargo)

    def test_missing_fields_and_failed_reports(self):
        for field in ("lifecycle", "traffic", "concurrency", "concurrent_traffic", "native_assertions"):
            reports = copy.deepcopy(self.reports)
            del reports[0][field]
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.validate(reports)
        reports = copy.deepcopy(self.reports)
        reports[4]["native_assertions"] = {"settled": True}
        with self.assertRaises(ValueError):
            self.validate(reports)

    def test_raw_requires_real_backend_and_development_label(self):
        raw = {**self.reports[0], "status": "development"}
        gate.validate_raw(self.root, raw, self.reports[0], gate.LIFECYCLE_SUITE)
        for field in ("native_assertions", "traffic", "concurrency"):
            with self.subTest(field=field), self.assertRaises(ValueError):
                gate.validate_raw(self.root, {**raw, field: {}}, self.reports[0], gate.LIFECYCLE_SUITE)

    def test_provenance_mutations(self):
        changes = [
            lambda n: n.pop("installation"),
            lambda n: n["installation"].update(fresh_store=False),
            lambda n: n["installation"].update(lockfile_sha256="f" * 64),
            lambda n: n.update(dependencies={}),
            lambda n: n["dependencies"].update({"undici@7.0.0": "f" * 64}),
            lambda n: n["runtime"].update(arch="x64"),
            lambda n: n["runtime"].update(executable_sha256="bad"),
        ]
        for change in changes:
            reports = copy.deepcopy(self.reports)
            change(reports[4]["node"])
            with self.assertRaises(ValueError):
                self.validate(reports)
        for value in (None, "", "musl", True):
            reports = copy.deepcopy(self.reports)
            linux = next(r for r in reports if r["adapter"] == "node" and r["target"].endswith("unknown-linux-gnu"))
            linux["node"]["runtime"]["libc"] = value
            with self.assertRaises(ValueError):
                self.validate(reports)

    def test_http_contract_and_identity_mutations_fail(self):
        for index in (0, 4):
            for field in ("observations", "lifecycle", "traffic", "concurrency", "concurrent_traffic"):
                reports = copy.deepcopy(self.reports)
                reports[index][field] = {}
                with self.subTest(field=field), self.assertRaises(ValueError):
                    self.validate(reports)

    def test_node_collection_rejects_source_build_and_raw_mutations(self):
        for field in ("observations", "traffic", "lifecycle", "concurrency", "concurrent_traffic"):
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.node_collection(lambda raw, package: raw.update({field: {}}))
        for change in (
            lambda raw, package: raw.update(error=None),
            lambda raw, package: raw["node"]["runtime"].update(executable_sha256="f" * 64),
            lambda raw, package: raw["node"].update(dependencies={}),
            lambda raw, package: (package.parents[1] / "pnpm-lock.yaml").write_text("changed"),
        ):
            with self.assertRaises(ValueError):
                self.node_collection(change)

    def test_ambient_loaders_fail_before_child(self):
        for key in gate.LOADER_ENV:
            with self.assertRaisesRegex(ValueError, f"ambient loader rejected: {key.lower()}"):
                self.node_collection(loader_env={key.lower(): "injected"})

    def test_success_error_key_rejected(self):
        for index in (0, 4):
            reports = copy.deepcopy(self.reports)
            reports[index]["error"] = None
            with self.assertRaises(ValueError):
                self.validate(reports)
