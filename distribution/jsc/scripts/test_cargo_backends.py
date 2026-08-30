#!/usr/bin/env python3
"""Check Cargo's actual unified feature graph, offline and without a native engine."""

import os
from pathlib import Path
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[3]


def main() -> None:
    environment = os.environ.copy()
    for name in ("KUNLUN_JSC_DIST_DIR", "KUNLUN_JSC_RECEIPT_SHA256"):
        environment.pop(name, None)
    cases = []
    for package in ("kunlun-jsc-sys", "kunlun-jsc", "kunlun-runtime"):
        cases.extend([
            (package, [], "bundled-jsc requires KUNLUN_JSC_DIST_DIR"),
            (package, ["--no-default-features"], "no JSC backend selected"),
            (package, ["--all-features"], "bundled-jsc and system-jsc are mutually exclusive"),
        ])
    cases.extend([
        ("kunlun-jsc-sys", ["--no-default-features", "--features", "system-jsc",
                            "--target", "x86_64-unknown-linux-gnu"],
         "system-jsc is development-only and supports only macOS"),
        ("kunlun-jsc-sys", ["--target", "x86_64-unknown-linux-musl"],
         "bundled-jsc does not support target"),
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
        settings_cases = [({}, "requires KUNLUN_JSC_RECEIPT_SHA256"),
                          ({"KUNLUN_JSC_RECEIPT_SHA256": "0" * 64}, "verified receipt missing")]
        for settings, expected in settings_cases:
            result = subprocess.run(
                ["cargo", "check", "--locked", "--offline", "-p", "kunlun-jsc-sys"],
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
