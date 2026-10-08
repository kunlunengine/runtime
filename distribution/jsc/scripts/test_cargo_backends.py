#!/usr/bin/env python3
"""Check Cargo's actual unified feature graph, offline and without a native engine."""

import json
import os
from pathlib import Path
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[3]


def main() -> None:
    environment = os.environ.copy()
    for name in ("KUNLUN_JSC_DIST_DIR", "KUNLUN_JSC_RECEIPT_SHA256"):
        environment.pop(name, None)
    version = subprocess.run(["rustc", "-vV"], check=True, text=True, capture_output=True).stdout
    host = next(line.removeprefix("host: ") for line in version.splitlines() if line.startswith("host: "))
    manifest = json.loads((ROOT / "distribution/jsc/manifest.json").read_text(encoding="utf-8"))
    supported = {entry["triple"] for entry in manifest["targets"]}
    missing_backend = (
        "bundled-jsc requires KUNLUN_JSC_DIST_DIR"
        if host in supported else "bundled-jsc does not support target"
    )
    cases = []
    for package in ("kunlun-jsc-sys", "kunlun-jsc", "kunlun-runtime"):
        cases.extend([
            (package, [], missing_backend),
            (package, ["--no-default-features"], "no JSC backend selected"),
            (package, ["--all-features"], "bundled-jsc and system-jsc are mutually exclusive"),
        ])
    cases.extend([
        ("kunlun-jsc-sys", ["--no-default-features", "--features", "system-jsc",
                            "--target", "x86_64-unknown-linux-gnu"],
         "system-jsc is development-only and supports only macOS"),
        ("kunlun-jsc-sys", ["--target", "x86_64-unknown-linux-musl"],
         "bundled-jsc does not support target"),
        ("kunlun-jsc-sys", ["--target", "x86_64-pc-windows-msvc"],
         "bundled-jsc does not support target"),
        ("kunlun-jsc-sys", ["--no-default-features", "--features", "system-jsc",
                            "--target", "x86_64-pc-windows-msvc"],
         "system-jsc is development-only and supports only macOS"),
    ])
    for package, flags, expected in cases:
        command = ["cargo", "check", "--locked", "--offline", "-p", package, *flags]
        result = subprocess.run(command, cwd=ROOT, env=environment,
                                text=True, capture_output=True, check=False)
        if result.returncode == 0 or expected not in result.stderr:
            raise AssertionError(f"{' '.join(command)}\n{result.stdout}\n{result.stderr}")
        print(f"ok: {package} {flags}: {expected}", flush=True)

    # Exercise the actual build script's artifact settings, not just pure policy.
    with tempfile.TemporaryDirectory(prefix="kunlun-cargo-backend-") as temporary:
        root = Path(temporary)
        environment["KUNLUN_JSC_DIST_DIR"] = str(root)
        # Probe a supported target even when the host itself is unqualified.
        # sys has no target dependencies; its policy fails before native compilation.
        artifact_target = host if host in supported else "aarch64-apple-darwin"
        settings_cases = [({}, "requires KUNLUN_JSC_RECEIPT_SHA256"),
                          ({"KUNLUN_JSC_RECEIPT_SHA256": "0" * 64}, "verified receipt missing")]
        for settings, expected in settings_cases:
            result = subprocess.run(
                ["cargo", "check", "--locked", "--offline", "-p", "kunlun-jsc-sys",
                 "--target", artifact_target],
                cwd=ROOT, env={**environment, **settings}, text=True, capture_output=True, check=False,
            )
            if result.returncode == 0 or expected not in result.stderr:
                raise AssertionError(result.stderr)
            print(f"ok: artifact settings: {expected}", flush=True)
        result = subprocess.run(
            ["cargo", "check", "--locked", "--offline", "-p", "kunlun-jsc-sys",
             "--no-default-features", "--features", "system-jsc", "--target", "aarch64-apple-darwin"],
            cwd=ROOT, env=environment, text=True, capture_output=True, check=False,
        )
        if result.returncode == 0 or "system-jsc cannot consume distribution settings" not in result.stderr:
            raise AssertionError(result.stderr)
        print("ok: system-jsc rejects distribution environment", flush=True)


if __name__ == "__main__":
    main()
