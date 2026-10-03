#!/usr/bin/env python3
"""Collect/compare the request-authority/v1 slice, not full #50/#53 qualification.

Node evidence must be produced by a real, separately pinned runtime-node runner.
This tool deliberately supplies neither a Node runner nor an authority policy.
"""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import sys


# Resolve beside this file even when an importlib caller has not added scripts/
# to sys.path. Share M2 mechanics without changing or inheriting its exit policy.
_spec = importlib.util.spec_from_file_location(
    "_authority_m2_gate", Path(__file__).with_name("m2_gate.py"))
m2 = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(m2)
ROOT = m2.ROOT
SUITE = "request-authority/v1"
FIXTURE = Path("crates/kunlun-runtime/tests/fixtures/request-authority.js")
CONTRACT = FIXTURE.with_suffix(".contract.json")
PROBE = "adapter_neutral_authority_probe_runs_unchanged_across_requests"
NODE_PACKAGE = "@kunlun-js/runtime-node"
NODE_BUILD_COMMAND = "pnpm run build --force"
NODE_MODULES = frozenset(("index.js", "application.js", "authority.js"))
SUCCESS = "request-authority/v1 slice passed; not full #50/#53 qualification"
require = m2.require


def exact(actual, expected):
    """JSON value equality including types (True != 1 and 1.0 != 1)."""
    if type(actual) is not type(expected):
        return False
    if isinstance(expected, dict):
        return actual.keys() == expected.keys() and all(
            exact(actual[key], value) for key, value in expected.items())
    if isinstance(expected, list):
        return len(actual) == len(expected) and all(
            exact(left, right) for left, right in zip(actual, expected))
    return actual == expected


def load_json(path):
    def pairs(items):
        result = {}
        for key, value in items:
            require(key not in result, f"duplicate JSON key: {key}")
            result[key] = value
        return result

    def invalid_constant(value):
        raise ValueError(f"non-finite JSON value: {value}")

    return json.loads(path.read_text(), object_pairs_hook=pairs,
                      parse_constant=invalid_constant)


def full_commit(value):
    require(isinstance(value, str) and re.fullmatch(r"[0-9a-f]{40}", value),
            "a full lowercase Git source commit is required")
    return value


def corpus(root):
    contract = load_json(root / CONTRACT)
    require(isinstance(contract, dict), "authority contract must be an object")
    require(exact(contract.get("schema_version"), 1) and contract.get("suite") == SUITE,
            "unsupported authority contract")
    observations = contract.get("expected_observations")
    require(isinstance(observations, list) and len(observations) == 2,
            "authority contract must specify exactly two request observations")
    return {
        "fixture_sha256": m2.sha256(root / FIXTURE),
        "contract_sha256": m2.sha256(root / CONTRACT),
        "observations": observations,
    }


def fields_match(actual, expected, label):
    require(isinstance(actual, dict), f"{label} must be an object")
    for key, value in expected.items():
        require(exact(actual.get(key), value), f"{label}: missing/mismatched {key}")


def validate_backend(backend, target, revision, mode):
    fields_match(backend, {
        "backend": "bundled-jsc", "target": target,
        "engine_revision": revision, "distribution_mode": mode, "hermetic": True,
    }, "native backend")


def validate_node_build(build):
    fields_match(build, {"command": NODE_BUILD_COMMAND}, "Node adapter build")
    modules = build.get("modules")
    require(isinstance(modules, dict) and modules.keys() == NODE_MODULES,
            "Node build requires the exact emitted module inventory")
    for value in modules.values():
        m2.digest(value)
    return modules


def validate_raw(root, raw, identity):
    fields_match(raw, {"schema_version": 1, "suite": SUITE, **corpus(root)},
                 "native observations")
    validate_backend(raw.get("backend"), identity["target"],
                     identity["engine_revision"], identity["mode"])
    return raw


def validate_probe(output):
    passed = re.findall(r"^test (\w+) \.\.\. ok$", output, re.MULTILINE)
    require(passed == [PROBE], "exact successful authority probe is required")
    summaries = re.findall(
        r"^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; "
        r"\d+ filtered out;(?: finished in .*)?$", output, re.MULTILINE)
    require(len(summaries) == 1, "authority probe was skipped, failed, or not run")


def runner_identity(output):
    system = platform.system()
    translated = False
    if system == "Darwin":
        # -i ignores the absent OID on older/native Intel kernels. Do not infer
        # Rosetta from platform.machine(): it reports x86_64 under translation.
        value = m2.command(["sysctl", "-in", "sysctl.proc_translated"],
                           output / "translation.txt", timeout=10).strip()
        require(value in ("", "0", "1"), "unknown Darwin translation state")
        translated = value == "1"
    return {"system": system, "machine": platform.machine(), "translated": translated}


def validate_runner(runner, target):
    require(isinstance(runner, dict), "physical runner identity is required")
    system = "Darwin" if target.endswith("apple-darwin") else "Linux"
    machines = ({"arm64", "aarch64"} if target.startswith("aarch64-") else {"x86_64"})
    require(runner.get("system") == system and runner.get("machine") in machines,
            "runner system/physical architecture does not match target")
    require(runner.get("translated") is False,
            "translated/unknown runner is not physical platform qualification")


def recheck_source(output, commit, expected):
    """Do not stamp evidence if sources/HEAD changed while the probe ran."""
    current_commit = m2.command(["git", "rev-parse", "HEAD"],
                                output / "final-commit.txt", timeout=30).strip()
    require(current_commit == commit, "reviewed HEAD changed during native collection")
    arguments = ["git", "status", "--porcelain", "--untracked-files=all", "--", "."]
    try:
        relative_output = output.relative_to(ROOT.resolve())
    except ValueError:
        pass
    else:
        # This directory was created exclusively by this collection. Its logs
        # are the only permitted working-tree additions, not source changes.
        arguments.append(f":(top,exclude,literal){relative_output.as_posix()}")
    status = m2.command(arguments, output / "final-git-status.txt", timeout=30)
    require(not status.strip(), "reviewed sources changed during native collection")
    require(exact(corpus(ROOT), expected), "authority corpus changed during native collection")


def collect_native(args):
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"schema_version": 1, "suite": SUITE, "adapter": "native",
              "status": "failed", "target": args.target}
    try:
        # The fresh directory is still empty, so it cannot dirty this check.
        status = m2.command(["git", "status", "--porcelain", "--untracked-files=all"],
                            args.output / "git-status.txt", timeout=30)
        require(not status.strip(), "authority evidence requires a clean reviewed commit")
        report["commit"] = full_commit(m2.command(
            ["git", "rev-parse", "HEAD"], args.output / "commit.txt", timeout=30).strip())
        if os.environ.get("GITHUB_SHA"):
            require(report["commit"] == os.environ["GITHUB_SHA"], "CI checkout is not GITHUB_SHA")
        directory = Path(os.environ["KUNLUN_JSC_DIST_DIR"])
        report.update(m2.identity(
            ROOT, args.target, directory, os.environ["KUNLUN_JSC_RECEIPT_SHA256"]))
        receipt = load_json(directory / ".kunlun-jsc-verification.json")
        require(exact(receipt.get("schema_version"), 1), "invalid receipt schema")
        expected = corpus(ROOT)
        report.update({key: value for key, value in expected.items() if key != "observations"})
        report["runner"] = runner_identity(args.output)
        try:
            validate_runner(report["runner"], args.target)
        except ValueError as error:
            report["diagnostic"] = f"{error}; diagnostic collection only"
        features = ["--locked", "--no-default-features", "--features", "bundled-jsc",
                    "--target", args.target]
        child_env = m2.native_environment(directory, args.target)
        doctor = m2.command(
            ["cargo", "run", "-p", "kunlun-runtime", *features, "--", "doctor"],
            args.output / "doctor.txt", timeout=1200, env=child_env)
        m2.validate_doctor(doctor, args.target, report["engine_revision"], report["mode"])
        raw_path = args.output / "observations.json"
        # Pass env to this child only. Backend checks below fail closed even if
        # inherited Cargo/config/feature settings select a different backend.
        probe_env = {**child_env, "KUNLUN_AUTHORITY_OBSERVATIONS": str(raw_path)}
        output = m2.command(
            ["cargo", "test", "-p", "kunlun-runtime", "--test", "request_environment",
             *features, "--", "--exact", PROBE, "--test-threads=1"],
            args.output / "probe.txt", timeout=1200, env=probe_env)
        validate_probe(output)
        raw = validate_raw(ROOT, load_json(raw_path), report)
        report["observations"] = raw["observations"]
        report["backend"] = raw["backend"]
        recheck_source(args.output, report["commit"], expected)
        fields_match(report, m2.identity(
            ROOT, args.target, directory, os.environ["KUNLUN_JSC_RECEIPT_SHA256"]),
            "artifact identity after execution")
        report["status"] = "passed"
    except Exception as error:
        report["error"] = str(error)
        raise
    finally:
        (args.output / "report.json").write_text(
            json.dumps(report, indent=2, sort_keys=True) + "\n")


def validate_reports(root, reports, commit, node_commit, node_package_version):
    full_commit(commit)
    full_commit(node_commit)
    require(isinstance(node_package_version, str) and re.fullmatch(
        r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?"
        r"(?:\+[0-9A-Za-z.-]+)?", node_package_version), "invalid pinned Node package version")
    require(len(reports) == 8, "authority slice requires exactly eight adapter/platform reports")
    expected_pairs = {(adapter, target) for adapter in ("native", "node") for target in m2.TARGETS}
    pairs = []
    for report in reports:
        require(isinstance(report, dict), "report must be an object")
        require(isinstance(report.get("adapter"), str) and isinstance(report.get("target"), str),
                "report adapter/target must be strings")
        pairs.append((report["adapter"], report["target"]))
    require(len(set(pairs)) == 8 and set(pairs) == expected_pairs,
            "missing, duplicate, or unknown adapter/platform")
    manifest = load_json(root / m2.MANIFEST)
    require({entry["triple"] for entry in manifest["targets"]} == m2.TARGETS,
            "manifest must contain exactly the four M2 platforms")
    expected = {"schema_version": 1, "suite": SUITE, "status": "passed",
                "commit": commit, **corpus(root)}
    node_modules = None
    for report in reports:
        fields_match(report, expected, f"{report['adapter']}/{report['target']}")
        validate_runner(report.get("runner"), report["target"])
        if report["adapter"] == "native":
            fields_match(report, {
                "manifest_sha256": m2.sha256(root / m2.MANIFEST),
                "engine_revision": manifest["source"]["revision"],
            }, "native artifact")
            require(report.get("mode") in ("source-build", "published"),
                    "native artifact must use a verified pinned distribution")
            for key in ("receipt_sha256", "archive_sha256", "sbom_sha256"):
                m2.digest(report.get(key))
            validate_backend(report.get("backend"), report["target"],
                             report["engine_revision"], report["mode"])
        else:
            node = report.get("node")
            fields_match(node, {"package": NODE_PACKAGE,
                               "package_version": node_package_version,
                               "source_commit": node_commit}, "Node adapter")
            require(isinstance(node.get("version"), str) and re.fullmatch(
                r"v(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)"
                r"(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", node["version"]),
                "exact Node runtime version is required (for example v24.1.0)")
            modules = validate_node_build(node.get("build"))
            if node_modules is None:
                node_modules = modules
            else:
                require(exact(modules, node_modules),
                        "Node emitted module hashes differ across platforms")


def compare(args):
    reports = [load_json(path) for path in sorted(args.evidence.glob("*/report.json"))]
    validate_reports(ROOT, reports, args.commit, args.node_commit, args.node_package_version)
    print(SUCCESS)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(dest="action", required=True)
    collect = actions.add_parser("collect-native")
    collect.add_argument("--target", choices=sorted(m2.TARGETS), required=True)
    collect.add_argument("--output", type=Path, required=True, help="new evidence directory")
    collect.set_defaults(function=collect_native)
    comparison = actions.add_parser("compare")
    comparison.add_argument("--evidence", type=Path, required=True)
    comparison.add_argument("--commit", required=True)
    comparison.add_argument("--node-commit", required=True)
    comparison.add_argument("--node-package-version", required=True)
    comparison.set_defaults(function=compare)
    args = parser.parse_args()
    try:
        args.function(args)
    except (ValueError, OSError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print(f"authority slice failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
