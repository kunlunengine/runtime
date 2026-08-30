#!/usr/bin/env bash
set -euo pipefail
export LC_ALL=C

manifest=${1:-distribution/jsc/manifest.json}
target=${2:-}
[[ -f "$manifest" && -n "$target" ]] || {
    echo "usage: verify-linux-toolchain.sh [manifest] <target>" >&2
    exit 2
}

case "$target" in
    aarch64-unknown-linux-gnu) expected_machine=aarch64 ;;
    x86_64-unknown-linux-gnu) expected_machine=x86_64 ;;
    *) echo "error: unsupported Linux target: $target" >&2; exit 1 ;;
esac

actual_machine=$(uname -m)
if [[ "$actual_machine" != "$expected_machine" ]]; then
    echo "error: target $target requires native $expected_machine, observed $actual_machine" >&2
    exit 1
fi

toolchain=$(jq -er --arg target "$target" \
    '.targets[] | select(.triple == $target) | .toolchain' "$manifest")
tool_version() {
    jq -er --arg toolchain "$toolchain" --arg name "$1" \
        '.toolchains[] | select(.id == $toolchain) | .tools[] | select(.name == $name) | .version' \
        "$manifest"
}

require_version() {
    local name=$1
    local actual=$2
    local expected
    expected=$(tool_version "$name")
    if [[ "$actual" != "$expected" ]]; then
        echo "error: $name version mismatch: expected $expected, observed $actual" >&2
        exit 1
    fi
}

for command in clang-18 clang++-18 ld.lld-18 cmake ccache ninja python3 pkg-config perl ruby git \
    ld ldd patchelf zstd readelf nm jq strings; do
    command -v "$command" >/dev/null || {
        echo "error: required command is unavailable: $command" >&2
        exit 1
    }
done

# shellcheck disable=SC1091
source /etc/os-release
expected_sysroot=$(tool_version ubuntu-sysroot)
if [[ "${VERSION:-}" != *"$expected_sysroot"* ]]; then
    echo "error: ubuntu-sysroot version mismatch: expected $expected_sysroot, observed ${VERSION:-unknown}" >&2
    exit 1
fi
expected_glibc=$(jq -er --arg target "$target" \
    '.targets[] | select(.triple == $target) | .deployment_target.minimum' "$manifest")
actual_glibc=$(ldd --version | sed -n '1s/.* \([0-9][0-9.]*\)$/\1/p')
if [[ "$actual_glibc" != "$expected_glibc" ]]; then
    echo "error: glibc baseline mismatch: expected $expected_glibc, observed $actual_glibc" >&2
    exit 1
fi

require_version clang "$(clang-18 --version | sed -n '1s/.*clang version \([0-9][0-9.]*\).*/\1/p')"
require_version lld "$(ld.lld-18 --version | sed -n '1s/.*LLD \([0-9][0-9.]*\).*/\1/p')"
require_version cmake "$(cmake --version | sed -n '1s/^cmake version //p')"
require_version ccache "$(ccache --version | sed -n '1s/^ccache version //p')"
require_version ninja "$(ninja --version)"
require_version python "$(python3 --version | sed 's/^Python //')"
require_version icu "$(pkg-config --modversion icu-uc)"
libstdcxx=$(clang++-18 -print-file-name=libstdc++.so.6)
[[ -f "$libstdcxx" ]] || { echo "error: clang did not resolve libstdc++.so.6" >&2; exit 1; }
max_glibcxx=$(strings "$libstdcxx" | sed -n '/^GLIBCXX_[0-9][0-9.]*$/p' | sort -V | tail -n 1)
max_cxxabi=$(strings "$libstdcxx" | sed -n '/^CXXABI_[0-9][0-9.]*$/p' | sort -V | tail -n 1)
require_version libstdc++-abi "$max_glibcxx,$max_cxxabi"
require_version perl "$(perl -e 'printf qq{%vd\n}, $^V')"
require_version ruby "$(ruby --version | sed -n 's/^ruby \([0-9][0-9.]*\).*/\1/p')"
require_version git "$(git --version | sed 's/^git version //')"
require_version binutils "$(ld --version | sed -n '1s/.* \([0-9][0-9.]*\)$/\1/p')"
require_version patchelf "$(patchelf --version | sed -n '1s/^patchelf //p')"
require_version zstd "$(zstd --version | sed -n '1s/.* v\([0-9][0-9.]*\).*/\1/p')"

expected_snapshot=$(jq -er --arg toolchain "$toolchain" \
    '.toolchains[] | select(.id == $toolchain) | .package_snapshot' "$manifest")
if [[ "${KUNLUN_APT_SNAPSHOT:-}" != "$expected_snapshot" ]]; then
    echo "error: builder package snapshot mismatch: expected $expected_snapshot" >&2
    exit 1
fi
expected_image=$(jq -er --arg toolchain "$toolchain" \
    '.toolchains[] | select(.id == $toolchain) | .container_image' "$manifest")
if [[ "${KUNLUN_CONTAINER_IMAGE:-}" != "$expected_image" ]]; then
    echo "error: builder base image does not match the manifest OCI digest" >&2
    exit 1
fi
expected_trust_store=$(jq -er --arg toolchain "$toolchain" \
    '.toolchains[] | select(.id == $toolchain) | .trust_store_image' "$manifest")
if [[ "${KUNLUN_TRUST_STORE_IMAGE:-}" != "$expected_trust_store" ]]; then
    echo "error: builder trust-store image does not match the manifest OCI digest" >&2
    exit 1
fi

echo "verified Linux JSC toolchain $toolchain for $target"
