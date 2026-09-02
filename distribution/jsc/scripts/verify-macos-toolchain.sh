#!/usr/bin/env bash
set -euo pipefail

usage() {
    echo "usage: $0 [manifest.json]" >&2
    exit 2
}

script_dir=$(cd "$(dirname "$0")" && pwd)
repository_root=$(cd "$script_dir/../../.." && pwd)
manifest=${1:-$repository_root/distribution/jsc/manifest.json}
[[ $# -le 1 ]] || usage
[[ -f "$manifest" ]] || { echo "error: manifest not found: $manifest" >&2; exit 1; }
command -v jq >/dev/null || { echo "error: jq is required" >&2; exit 1; }

toolchain_id=$(jq -er '.targets[] | select(.triple == "aarch64-apple-darwin") | .toolchain' "$manifest")

expected_version() {
    jq -er --arg id "$toolchain_id" --arg name "$1" \
        '.toolchains[] | select(.id == $id) | .tools[] | select(.name == $name) | .version' \
        "$manifest"
}

assert_version() {
    local name=$1
    local actual=$2
    local expected
    expected=$(expected_version "$name")
    if [[ "$actual" != "$expected" ]]; then
        echo "error: $name version mismatch: expected '$expected', observed '$actual'" >&2
        toolchain_mismatch=1
        return
    fi
    printf 'verified %-12s %s\n' "$name" "$actual"
}

toolchain_mismatch=0
xcode_version=$(xcodebuild -version | sed -n '1s/^Xcode //p')
xcode_build=$(xcodebuild -version | sed -n '2s/^Build version //p')
assert_version xcode "$xcode_version ($xcode_build)"
assert_version apple-clang "$(xcrun clang --version | sed -n '1s/^Apple clang version //p')"
assert_version macos-sdk "$(xcrun --sdk macosx --show-sdk-version)"
assert_version cmake "$(cmake --version | sed -n '1s/^cmake version //p')"
assert_version python "$(/usr/bin/python3 --version | sed 's/^Python //')"
assert_version perl "$(/usr/bin/perl -e 'printf qq{%vd\n}, $^V')"
assert_version ruby "$(/usr/bin/ruby --version | awk '{print $2}')"
assert_version git "$(/usr/bin/git --version | sed 's/^git version //')"

if [[ "$toolchain_mismatch" -ne 0 ]]; then
    echo "error: controlled macOS toolchain does not match $toolchain_id" >&2
    exit 1
fi

echo "verified controlled macOS toolchain: $toolchain_id"
