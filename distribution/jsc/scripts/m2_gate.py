#!/usr/bin/env python3
"""Run and aggregate the M2 exit gate. No downloads or system-engine fallback."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import signal
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[3]
MANIFEST = Path("distribution/jsc/manifest.json")
SUITE = Path("crates/kunlun-runtime/tests/m2_conformance.rs")
TARGETS = {
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu",
}
CAPABILITIES = (
    "deferred Promise primitive",
    "native ESM loader",
    "explicit microtask checkpoint",
    "AbortSignal and bounded host streams",
    "execution watchdog",
    "heap telemetry",
    "native Temporal API",
)
SANITIZER_PASS = (
    "PASS pinned M2 ASan/UBSan: modules, roots, loader callbacks, rejection records, "
    "explicit microtasks, resource limits, repeated teardown"
)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def digest(value):
    require(isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value),
            "missing or invalid SHA-256 identity")
    return value


def corpus(root):
    paths = [SUITE, *sorted(path.relative_to(root) for path in
                           (root / SUITE.parent / "fixtures/m2").rglob("*")
                           if path.is_file())]
    require(len(paths) > 1, "M2 fixture corpus is missing")
    inventory = {path.as_posix(): sha256(root / path) for path in paths}
    return hashlib.sha256(json.dumps(inventory, sort_keys=True).encode()).hexdigest()


def expected_tests(root):
    names = re.findall(r"#\[test\]\s+fn (\w+)\(", (root / SUITE).read_text())
    require(names and len(names) == len(set(names)), "M2 test inventory is empty or duplicated")
    return sorted(names)


def validate_tests(output, expected):
    passed = sorted(re.findall(r"^test (\w+) \.\.\. ok$", output, re.MULTILINE))
    require(passed == expected, "M2 corpus missing, ignored, filtered, or failed tests")
    summary = f"test result: ok. {len(expected)} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;"
    require(summary in output, "M2 test summary does not match the reviewed inventory")
    return passed


def validate_doctor(output, target, revision, mode):
    fields = dict(line.split(": ", 1) for line in output.splitlines() if ": " in line)
    expected = {
        "backend": "bundled-jsc",
        "engine revision": revision,
        "target": target,
        "distribution mode": mode,
        "distribution": "pinned Kunlun JSC artifact",
        "hermetic": "true",
        "synchronous smoke test": "ok",
        **{capability: "true" for capability in CAPABILITIES},
    }
    for key, value in expected.items():
        require(fields.get(key) == value, f"required doctor identity/capability missing: {key}={value}")
    return {capability: True for capability in CAPABILITIES}


def validate_sanitizers(output):
    require(SANITIZER_PASS in output.splitlines(), "pinned M2 ASan/UBSan evidence is missing")


def identity(root, target, directory, trusted_digest):
    manifest = json.loads((root / MANIFEST).read_text())
    require({entry["triple"] for entry in manifest["targets"]} == TARGETS,
            "manifest must contain exactly the four M2 platforms")
    require(target in TARGETS, "unsupported M2 platform")
    receipt_path = directory / ".kunlun-jsc-verification.json"
    require(sha256(receipt_path) == digest(trusted_digest), "trusted verification receipt mismatch")
    receipt = json.loads(receipt_path.read_text())
    require(receipt.get("schema_version") == 1 and receipt.get("native_verified") is True,
            "native artifact verification is required")
    require(receipt.get("target") == target, "receipt target mismatch")
    require(receipt.get("manifest_sha256") == sha256(root / MANIFEST), "receipt manifest mismatch")
    require(receipt.get("mode") in ("source-build", "published"), "untrusted verification mode")
    return {
        "manifest_sha256": receipt["manifest_sha256"],
        "engine_revision": manifest["source"]["revision"],
        "target": target,
        "mode": receipt["mode"],
        "receipt_sha256": trusted_digest,
        "archive_sha256": digest(receipt.get("archive_sha256")),
        "sbom_sha256": digest(receipt.get("sbom_sha256")),
    }


def command(arguments, log, timeout=1200):
    """Bound the whole process group, including a stalled native test, not just Cargo."""
    print("+ " + " ".join(map(str, arguments)), flush=True)
    with subprocess.Popen(arguments, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                          text=True, start_new_session=True) as child:
        try:
            output, _ = child.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            output, _ = child.communicate()
            log.write_text(output)
            print(output, end="", flush=True)
            raise ValueError(f"gate watchdog expired: {log.name}") from None
    log.write_text(output)
    print(output, end="", flush=True)
    require(child.returncode == 0, f"gate command failed ({child.returncode}): {log.name}")
    return output


def run(args):
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"schema_version": 1, "status": "failed", "target": args.target}
    try:
        require(not subprocess.check_output(
            ["git", "status", "--porcelain", "--untracked-files=all"], cwd=ROOT, text=True).strip(),
            "M2 evidence requires a clean reviewed commit")
        report["commit"] = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
        if os.environ.get("GITHUB_SHA"):
            require(report["commit"] == os.environ["GITHUB_SHA"], "CI checkout is not GITHUB_SHA")
        directory = Path(os.environ["KUNLUN_JSC_DIST_DIR"])
        report.update(identity(ROOT, args.target, directory, os.environ["KUNLUN_JSC_RECEIPT_SHA256"]))
        report["corpus_sha256"] = corpus(ROOT)
        report["runner"] = {"system": platform.system(), "machine": platform.machine()}
        native_log = args.native_log.read_text()
        (args.output / "native.txt").write_text(native_log)
        validate_sanitizers(native_log)
        report["native_sanitizers"] = "passed"
        print(json.dumps(report, indent=2, sort_keys=True), flush=True)
        command(["rustc", "-vV"], args.output / "rustc.txt")
        features = ["--locked", "--no-default-features", "--features", "bundled-jsc",
                    "--target", args.target]
        doctor = command(["cargo", "run", "-p", "kunlun-runtime", *features, "--", "doctor"],
                         args.output / "doctor.txt")
        report["capabilities"] = validate_doctor(
            doctor, args.target, report["engine_revision"], report["mode"])
        # Cargo's backend validator rehashes the entire installed artifact before these commands.
        command(["cargo", "test", "--workspace", *features], args.output / "workspace.txt")
        output = command(["cargo", "test", "-p", "kunlun-runtime", "--test", "m2_conformance",
                          *features, "--", "--test-threads=1"], args.output / "corpus.txt")
        report["tests"] = validate_tests(output, expected_tests(ROOT))
        report["status"] = "passed"
    except (ValueError, OSError, KeyError, subprocess.SubprocessError) as error:
        report["error"] = str(error)
        raise
    finally:
        (args.output / "report.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")


def validate_reports(root, reports, commit):
    require(len(reports) == len(TARGETS), "M2 requires exactly four platform reports")
    require({report.get("target") for report in reports} == TARGETS,
            "missing, duplicate, or unsupported M2 platform")
    manifest = json.loads((root / MANIFEST).read_text())
    require({entry["triple"] for entry in manifest["targets"]} == TARGETS,
            "manifest must contain exactly the four M2 platforms")
    expected = {
        "schema_version": 1,
        "status": "passed",
        "commit": commit,
        "manifest_sha256": sha256(root / MANIFEST),
        "engine_revision": manifest["source"]["revision"],
        "corpus_sha256": corpus(root),
        "tests": expected_tests(root),
        "capabilities": {capability: True for capability in CAPABILITIES},
        "native_sanitizers": "passed",
    }
    for report in reports:
        for key, value in expected.items():
            require(report.get(key) == value, f"{report['target']}: missing/mismatched {key}")
        require(report.get("mode") in ("source-build", "published"), "untrusted verification mode")
        for key in ("receipt_sha256", "archive_sha256", "sbom_sha256"):
            digest(report.get(key))


def summarize(args):
    reports = [json.loads(path.read_text()) for path in sorted(args.evidence.glob("*/report.json"))]
    validate_reports(ROOT, reports, args.commit)
    lines = [
        "# M2 exit gate",
        "",
        f"Commit: `{args.commit}`",
        f"Manifest SHA-256: `{sha256(ROOT / MANIFEST)}`",
        f"Corpus SHA-256: `{corpus(ROOT)}`",
        "",
        "| Target | Artifact SHA-256 | Receipt SHA-256 | Tests |",
        "| --- | --- | --- | --- |",
    ]
    for report in reports:
        lines.append(f"| {report['target']} | `{report['archive_sha256']}` | "
                     f"`{report['receipt_sha256']}` | {len(report['tests'])} passed |")
    summary = "\n".join(lines) + "\n"
    print(summary)
    args.output.write_text(summary)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(dest="action", required=True)
    runner = actions.add_parser("run")
    runner.add_argument("--target", choices=sorted(TARGETS), required=True)
    runner.add_argument("--output", type=Path, required=True, help="new evidence directory")
    runner.add_argument("--native-log", type=Path, required=True,
                        help="successful pinned ASan/UBSan log from the same source build")
    runner.set_defaults(function=run)
    aggregate = actions.add_parser("summarize")
    aggregate.add_argument("--evidence", type=Path, required=True)
    aggregate.add_argument("--commit", required=True)
    aggregate.add_argument("--output", type=Path, required=True)
    aggregate.set_defaults(function=summarize)
    args = parser.parse_args()
    try:
        args.function(args)
    except (ValueError, OSError, KeyError, subprocess.SubprocessError) as error:
        print(f"M2 gate failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
