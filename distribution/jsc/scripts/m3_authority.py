#!/usr/bin/env python3
"""Collect/compare authority slices, not full #50/#53 qualification.

The HTTP Node collector delegates to the real developer executor, never a policy
simulation. Filesystem Node evidence still comes from a separately pinned runner.
"""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import shutil
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
NODE_INSTALL_COMMAND = "pnpm --dir <Core> install --frozen-lockfile --force --ignore-scripts --store-dir <new-store>"
LOADER_ENV = frozenset(("NODE_OPTIONS", "NODE_PATH", "NPM_CONFIG_NODE_OPTIONS",
                       "LD_PRELOAD", "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH",
                       "DYLD_INSERT_LIBRARIES", "DYLD_FRAMEWORK_PATH"))
NODE_MODULES = frozenset(("index.js", "application.js", "authority.js"))
SUCCESS = "request-authority/v1 slice passed; not full #50/#53 qualification"
require = m2.require


HTTP_SUITE = "request-authority-http/v1"
HTTP_FIXTURE = FIXTURE.with_name("request-authority-http.js")
HTTP_PROBE = "shared_http_authority_observations_match_contract"
LIFECYCLE_SUITE = "request-authority-lifecycle/v1"
LIFECYCLE_FIXTURE = FIXTURE.with_name("request-authority-lifecycle.js")
LIFECYCLE_PROBE = "shared_post_headers_lifecycle_matches_contract"


def suite_settings(suite):
    settings = {
        SUITE: (FIXTURE, CONTRACT, PROBE),
        HTTP_SUITE: (HTTP_FIXTURE, HTTP_FIXTURE.with_suffix(".contract.json"), HTTP_PROBE),
        LIFECYCLE_SUITE: (LIFECYCLE_FIXTURE, LIFECYCLE_FIXTURE.with_suffix(".contract.json"), LIFECYCLE_PROBE),
    }
    require(suite in settings, "unknown authority suite")
    return settings[suite]


def suite_execution(suite):
    suite_settings(suite)
    return {
        SUITE: ("request_environment", "KUNLUN_AUTHORITY_OBSERVATIONS"),
        HTTP_SUITE: ("request_authority_http", "KUNLUN_M3_HTTP_OBSERVATIONS"),
        LIFECYCLE_SUITE: ("request_authority_lifecycle", "KUNLUN_M3_LIFECYCLE_OBSERVATIONS"),
    }[suite]


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


def corpus(root, suite=SUITE):
    fixture, contract_path, _ = suite_settings(suite)
    contract = load_json(root / contract_path)
    require(isinstance(contract, dict), "authority contract must be an object")
    require(exact(contract.get("schema_version"), 1) and contract.get("suite") == suite,
            "unsupported authority contract")
    if suite == LIFECYCLE_SUITE:
        observations = contract.get("expected_progress")
        require(isinstance(observations, dict) and observations.keys() ==
                {"application", "request", "survivor"}, "lifecycle progress contract required")
    else:
        observations = contract.get("expected_observations")
        require(isinstance(observations, list) and len(observations) == 2,
                "authority contract must specify exactly two request observations")
    result = {
        "fixture_sha256": m2.sha256(root / fixture),
        "contract_sha256": m2.sha256(root / contract_path),
        "observations": observations,
    }
    if suite == HTTP_SUITE:
        require(contract.get("qualification") is False, "HTTP contract is a slice only")
        require(isinstance(contract.get("expected_lifecycle"), dict),
                "HTTP lifecycle contract required")
        require(isinstance(contract.get("expected_requests"), list),
                "ordered HTTP traffic contract required")
        result.update(
            revocation_sha256=m2.sha256(root / fixture.with_name("request-authority-http-revocation.js")),
            lifecycle=contract["expected_lifecycle"], requests=contract["expected_requests"])
    if suite == LIFECYCLE_SUITE:
        for field in ("lifecycle", "traffic", "concurrency", "concurrent_traffic"):
            value = contract.get(f"expected_{field}")
            require(isinstance(value, dict), f"lifecycle {field} contract required")
            result[field] = value
        require(isinstance(contract.get("expected_native_assertions"), dict),
                "native lifecycle accounting contract required")
    return result


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


def validate_raw(root, raw, identity, suite=SUITE):
    require(isinstance(raw, dict) and "error" not in raw, "successful raw report cannot contain error")
    fields_match(raw, {"schema_version": 1, "suite": suite, **corpus(root, suite)},
                 "native observations")
    if suite != SUITE:
        fields_match(raw, {"adapter": "native", "status": "development",
                          "qualification": False}, "authority development export")
    if suite == LIFECYCLE_SUITE:
        contract = load_json(root / LIFECYCLE_FIXTURE.with_suffix(".contract.json"))
        fields_match(raw, {"native_assertions": contract["expected_native_assertions"]},
                     "native lifecycle accounting")
    validate_backend(raw.get("backend"), identity["target"],
                     identity["engine_revision"], identity["mode"])
    return raw


def validate_probe(output, suite=SUITE):
    _, _, probe = suite_settings(suite)
    passed = re.findall(r"^test (\w+) \.\.\. ok$", output, re.MULTILINE)
    require(passed == [probe], "exact successful authority probe is required")
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


def recheck_source(output, commit, expected, suite=SUITE):
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
    require(exact(corpus(ROOT, suite), expected), "authority corpus changed during native collection")


def collect_native(args):
    suite = getattr(args, "suite", SUITE)
    _, _, probe = suite_settings(suite)
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"schema_version": 1, "suite": suite, "adapter": "native",
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
        expected = corpus(ROOT, suite)
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
        test, variable = suite_execution(suite)
        probe_env = {**child_env, variable: str(raw_path)}
        output = m2.command(
            ["cargo", "test", "-p", "kunlun-runtime", "--test", test,
             *features, "--", "--exact", probe, "--test-threads=1"],
            args.output / "probe.txt", timeout=1200, env=probe_env)
        validate_probe(output, suite)
        raw = validate_raw(ROOT, load_json(raw_path), report, suite)
        report.update({key: raw[key] for key in expected})
        if suite != SUITE:
            report["qualification"] = False
        if suite == LIFECYCLE_SUITE:
            report["native_assertions"] = raw["native_assertions"]
        report["backend"] = raw["backend"]
        recheck_source(args.output, report["commit"], expected, suite)
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


def validate_reports(root, reports, commit, node_commit, node_package_version, suite=SUITE):
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
    expected = {"schema_version": 1, "suite": suite, "status": "passed",
                "commit": commit, **corpus(root, suite)}
    node_modules = None
    dependencies = None
    for report in reports:
        require("error" not in report, "successful report cannot contain error")
        fields_match(report, expected, f"{report['adapter']}/{report['target']}")
        if suite != SUITE:
            fields_match(report, {"qualification": False}, "authority slice qualification")
        validate_runner(report.get("runner"), report["target"])
        if report["adapter"] == "native":
            if suite == LIFECYCLE_SUITE:
                contract = load_json(root / LIFECYCLE_FIXTURE.with_suffix(".contract.json"))
                fields_match(report, {"native_assertions": contract["expected_native_assertions"]},
                             "native lifecycle accounting")
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
            if suite == LIFECYCLE_SUITE:
                require("native_assertions" not in report, "Node cannot invent native accounting")
            node = report.get("node")
            fields_match(node, {"package": NODE_PACKAGE,
                               "package_version": node_package_version,
                               "source_commit": node_commit}, "Node adapter")
            require(isinstance(node.get("version"), str) and re.fullmatch(
                r"v(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)"
                r"(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", node["version"]),
                "exact Node runtime version is required (for example v24.1.0)")
            modules = validate_node_build(node.get("build"))
            if suite != SUITE:
                validate_runtime(node.get("runtime"), report["target"])
                fields_match(node["runtime"], {"version": node["version"]}, "runtime version")
                installation = node.get("installation")
                fields_match(installation, {"command": NODE_INSTALL_COMMAND,
                             "fresh_store": True, "ci": True}, "Node installation")
                m2.digest(installation.get("lockfile_sha256"))
                closure = node.get("dependencies")
                validate_dependencies(closure)
                identity = {"installation": installation, "dependencies": closure}
                if dependencies is None:
                    dependencies = identity
                else:
                    require(exact(identity, dependencies),
                            "Node installation/dependencies differ across platforms")
            if node_modules is None:
                node_modules = modules
            else:
                require(exact(modules, node_modules),
                        "Node emitted module hashes differ across platforms")


def validate_dependencies(closure):
    require(isinstance(closure, dict) and closure and all(
        isinstance(key, str) and re.fullmatch(r"(?:@[^/@]+/)?[^/@]+@[^@]+", key)
        for key in closure) and any(key.startswith("undici@") for key in closure),
        "undici dependency closure required")
    for value in closure.values():
        m2.digest(value)


def validate_runtime(runtime, target):
    fields_match(runtime, {
        "arch": "arm64" if target.startswith("aarch64") else "x64",
        "platform": "darwin" if target.endswith("apple-darwin") else "linux",
    }, "Node runtime target")
    m2.digest(runtime.get("executable_sha256"))
    if target.endswith("unknown-linux-gnu"):
        require(isinstance(runtime.get("libc"), str) and re.fullmatch(
            r"\d+\.\d+(?:\.\d+)?", runtime["libc"]), "GNU target requires glibc runtime")


def collect_node(args):
    """Wrap the real executor; never emulate authority or bless an existing build."""
    suite = getattr(args, "suite", HTTP_SUITE)
    require(suite in (HTTP_SUITE, LIFECYCLE_SUITE), "unsupported Node collection suite")
    args.output = args.output.resolve()
    core = args.core_root.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"schema_version": 1, "suite": suite, "adapter": "node",
              "status": "failed", "qualification": False, "target": args.target}
    try:
        for key, value in os.environ.items():
            require(not (key.upper() in LOADER_ENV and value), f"ambient loader rejected: {key}")
        node_executable = shutil.which("node")
        require(node_executable is not None, "Node executable required")
        node_executable = str(Path(node_executable).resolve())
        child_env = dict(os.environ, CI="true")
        child_env["PATH"] = str(Path(node_executable).parent) + os.pathsep + child_env.get("PATH", "")
        helper = Path(__file__).with_name("node-provenance.mjs").resolve().as_uri()
        full_commit(args.commit)
        full_commit(args.node_commit)
        # Keep evidence outside both source trees: no broad status exclusions.
        for source in (ROOT.resolve(), core):
            require(source not in (args.output, *args.output.parents),
                    "Node evidence directory must be outside Runtime and Core")

        def source_identity(source, name):
            commit = m2.command(["git", "-C", str(source), "rev-parse", "HEAD"],
                                args.output / f"{name}-commit.txt", timeout=30).strip()
            status = m2.command(
                ["git", "-C", str(source), "status", "--porcelain", "--untracked-files=all"],
                args.output / f"{name}-status.txt", timeout=30)
            require(not status.strip(), f"{name} requires clean reviewed sources")
            return full_commit(commit)

        require(source_identity(ROOT, "runtime") == args.commit, "Runtime reviewed commit mismatch")
        require(source_identity(core, "core") == args.node_commit, "Core reviewed commit mismatch")
        report["commit"] = args.commit
        expected = corpus(ROOT, suite)
        report["runner"] = runner_identity(args.output)
        validate_runner(report["runner"], args.target)
        node_runtime = json.loads(m2.command(
            [node_executable, "--input-type=module", "-e",
             f"import {{runtimeIdentity}} from {json.dumps(helper)}; console.log(JSON.stringify(runtimeIdentity()))"],
            args.output / "node-runtime.txt", timeout=30, env=child_env))
        validate_runtime(node_runtime, args.target)
        package_path = core / "packages/runtime-node/package.json"
        package_hash = m2.sha256(package_path)
        metadata = load_json(package_path)
        fields_match(metadata, {"name": NODE_PACKAGE, "version": args.node_package_version},
                     "reviewed Node package")
        lock_path = core / "pnpm-lock.yaml"
        m2.command(["git", "-C", str(core), "ls-files", "--error-unmatch", "pnpm-lock.yaml"],
                   args.output / "reviewed-lockfile.txt", timeout=30)
        lock_hash = m2.sha256(lock_path)
        # The directory is evidence-local and created exclusively for this run.
        store = args.output / "pnpm-store"
        store.mkdir(exist_ok=False)
        m2.command(["pnpm", "--dir", str(core), "install", "--frozen-lockfile",
                    "--force", "--ignore-scripts", "--store-dir", str(store)],
                   args.output / "install.txt", timeout=1200, env=child_env)
        require(m2.sha256(lock_path) == lock_hash, "reviewed lockfile changed during install")
        m2.command(["pnpm", "--dir", str(core), "run", "build", "--force"],
                   args.output / "build.txt", timeout=1200, env=child_env)

        def modules():
            return {name: m2.sha256(core / "packages/runtime-node/dist" / name)
                    for name in sorted(NODE_MODULES)}

        built = modules()
        def closure():
            return json.loads(m2.command(
                [node_executable, "--input-type=module", "-e",
                 f"import {{dependencyClosure}} from {json.dumps(helper)}; console.log(JSON.stringify(dependencyClosure({json.dumps(str(core))})))"],
                args.output / "dependencies.txt", timeout=30, env=child_env))
        dependencies = closure()
        validate_dependencies(dependencies)
        raw_path = args.output / "observations.json"
        m2.command([node_executable, str(Path(__file__).with_name("collect-authority-node.mjs")),
                    str(core), str(raw_path), suite], args.output / "probe.txt", timeout=1200, env=child_env)
        raw = load_json(raw_path)
        require("error" not in raw, "successful raw report cannot contain error")
        fields_match(raw, {"schema_version": 1, "suite": suite, "adapter": "node",
                          "status": "development", "qualification": False, **expected},
                     "real Node authority observations")
        require("native_assertions" not in raw, "Node cannot invent native accounting")
        fields_match(raw.get("node"), {"package": NODE_PACKAGE,
                     "package_version": args.node_package_version,
                     "source_commit": args.node_commit, "modules": built,
                     "version": node_runtime["version"], "runtime": node_runtime,
                     "dependencies": dependencies}, "Node executor")
        require(exact(closure(), dependencies), "runtime dependencies changed during execution")
        require(m2.sha256(lock_path) == lock_hash, "reviewed lockfile changed during execution")
        require(exact(modules(), built), "Node build changed during execution")
        require(m2.sha256(package_path) == package_hash, "Node package changed during execution")
        require(exact(corpus(ROOT, suite), expected), "authority fixtures changed during execution")
        require(source_identity(ROOT, "final-runtime") == args.commit, "Runtime source changed")
        require(source_identity(core, "final-core") == args.node_commit, "Core source changed")
        report.update(expected)
        report["node"] = {key: raw["node"][key] for key in
                          ("package", "package_version", "source_commit", "version", "runtime", "dependencies")}
        report["node"]["installation"] = {"command": NODE_INSTALL_COMMAND,
            "fresh_store": True, "ci": True, "lockfile_sha256": lock_hash}
        report["node"]["build"] = {"command": NODE_BUILD_COMMAND, "modules": built}
        require(isinstance(report["node"]["version"], str) and re.fullmatch(
            r"v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?",
            report["node"]["version"]), "exact Node version required")
        report["status"] = "passed"
    except Exception as error:
        report["error"] = str(error)
        raise
    finally:
        (args.output / "report.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")


def compare(args):
    reports = [load_json(path) for path in sorted(args.evidence.glob("*/report.json"))]
    suite = getattr(args, "suite", SUITE)
    validate_reports(ROOT, reports, args.commit, args.node_commit, args.node_package_version, suite)
    print(f"{suite} slice passed; not full #50/#53 qualification")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(dest="action", required=True)
    collect = actions.add_parser("collect-native")
    collect.add_argument("--suite", choices=(SUITE, HTTP_SUITE, LIFECYCLE_SUITE), default=SUITE)
    collect.add_argument("--target", choices=sorted(m2.TARGETS), required=True)
    collect.add_argument("--output", type=Path, required=True, help="new evidence directory")
    collect.set_defaults(function=collect_native)
    node = actions.add_parser("collect-node", aliases=["collect-node-http"])
    node.add_argument("--suite", choices=(HTTP_SUITE, LIFECYCLE_SUITE), default=HTTP_SUITE)
    node.add_argument("--core-root", type=Path, required=True)
    node.add_argument("--target", choices=sorted(m2.TARGETS), required=True)
    node.add_argument("--output", type=Path, required=True)
    node.add_argument("--commit", required=True)
    node.add_argument("--node-commit", required=True)
    node.add_argument("--node-package-version", required=True)
    node.set_defaults(function=collect_node)
    comparison = actions.add_parser("compare")
    comparison.add_argument("--suite", choices=(SUITE, HTTP_SUITE, LIFECYCLE_SUITE), default=SUITE)
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
