#!/usr/bin/env bash
set -euo pipefail

# Compiler/linker arguments are supplied by the controlled builder, or omitted
# for the explicit macOS system-framework developer smoke test. Never downloads.
script_dir=$(cd "$(dirname "$0")" && pwd)
repository_root=$(cd "$script_dir/../../.." && pwd)
output=$(mktemp -d "${TMPDIR:-/tmp}/kunlun-native-ownership.XXXXXX")
trap 'rm -rf "$output"' EXIT
if [[ $# == 0 ]]; then
    [[ "$(uname -s)" == Darwin ]] || { echo 'Supply pinned JSC include/link arguments on Linux' >&2; exit 2; }
    set -- -framework JavaScriptCore
fi
native_cxx=${CXX:-}
if [[ -z "$native_cxx" ]]; then
    if [[ "$(uname -s)" == Darwin ]]; then
        native_cxx=$(xcrun -f clang++)
    else
        native_cxx=clang++
    fi
fi
native_flags=()
if [[ "$(uname -s)" == Darwin ]]; then
    native_flags=(-isysroot "$(xcrun --show-sdk-path)")
fi
"$native_cxx" "${native_flags[@]}" -std=c++17 -g -O1 -fno-omit-frame-pointer \
    -fsanitize=address,undefined -fno-sanitize-recover=all \
    -Wall -Wextra -Werror -pthread -DKUNLUN_JSC_TESTING \
    -I "$repository_root/crates/kunlun-jsc-sys/include" \
    -I "$repository_root/crates/kunlun-jsc-sys/native" \
    "$repository_root/crates/kunlun-jsc-sys/native/kunlun_jsc.cpp" \
    "$repository_root/crates/kunlun-jsc-sys/native/ownership_smoke.cpp" \
    "$@" -o "$output/ownership-smoke"
# LeakSanitizer cannot account for uninstrumented JSC's process-global caches;
# the harness separately asserts that every shim backing allocation is freed.
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 \
    python3 - "$output/ownership-smoke" <<'PYTHON'
import subprocess
import sys
subprocess.run([sys.argv[1]], check=True, timeout=120)
PYTHON
