"""Fail-closed gate checks without native JSC, network access, or Cargo."""

import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


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


if __name__ == "__main__":
    unittest.main()
