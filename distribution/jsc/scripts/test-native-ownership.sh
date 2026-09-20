#!/usr/bin/env bash
set -euo pipefail

# Compiler/linker arguments are supplied by the controlled builder. --system
# selects only the macOS development baseline; it is not M2 evidence.
# All other invocations compile the pinned M2 paths and fail to link if the
# supplied engine lacks them. Never downloads or silently falls back.
script_dir=$(cd "$(dirname "$0")" && pwd)
repository_root=$(cd "$script_dir/../../.." && pwd)
output=$(mktemp -d "${TMPDIR:-/tmp}/kunlun-native-ownership.XXXXXX")
trap 'rm -rf "$output"' EXIT
capability_flags=(-DKUNLUN_JSC_BUNDLED)
if [[ "${1:-}" == --system ]]; then
    shift
    [[ $# == 0 && "$(uname -s)" == Darwin ]] || { echo '--system accepts no link arguments and requires macOS' >&2; exit 2; }
    capability_flags=(-UKUNLUN_JSC_BUNDLED)
    set -- -framework JavaScriptCore
    echo 'native ASan/UBSan coverage: system baseline only (not M2)'
else
    if [[ "${1:-}" == --pinned ]]; then shift; fi
    [[ $# != 0 ]] || { echo 'usage: test-native-ownership.sh --system | [--pinned] <pinned JSC include/link arguments>' >&2; exit 2; }
    echo 'native ASan/UBSan coverage: pinned M2 modules, roots, callbacks, rejections, microtasks, resource limits, teardown'
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
    -Wall -Wextra -Werror -pthread \
    -I "$repository_root/crates/kunlun-jsc-sys/include" \
    -I "$repository_root/crates/kunlun-jsc-sys/native" \
    "$repository_root/crates/kunlun-jsc-sys/native/kunlun_jsc.cpp" \
    "$repository_root/crates/kunlun-jsc-sys/native/ownership_smoke.cpp" \
    "$@" "${capability_flags[@]}" -DKUNLUN_JSC_TESTING -UNDEBUG \
    -o "$output/ownership-smoke"
# LeakSanitizer cannot account for uninstrumented JSC's process-global caches;
# the harness separately asserts that every shim backing allocation is freed.
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 \
    python3 - "$output/ownership-smoke" <<'PYTHON'
import subprocess
import sys
subprocess.run([sys.argv[1]], check=True, timeout=120)
PYTHON
