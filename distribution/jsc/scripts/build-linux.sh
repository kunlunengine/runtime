#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat >&2 <<'EOF'
usage: build-linux.sh --target <triple> --webkit-root <path> --output <path> [--repository-root <path>]

Runs inside the controlled Linux builder image. It builds pinned JavaScriptCore and the Kunlun
shim, normalizes their ELF identities, assembles the archive and SPDX SBOM, and verifies both.
EOF
    exit 2
}

target=
webkit_root=
output=
repository_root=
while [[ $# -gt 0 ]]; do
    case "$1" in
        --target) target=${2:-}; shift 2 ;;
        --webkit-root) webkit_root=${2:-}; shift 2 ;;
        --output) output=${2:-}; shift 2 ;;
        --repository-root) repository_root=${2:-}; shift 2 ;;
        *) usage ;;
    esac
done
[[ -n "$target" && -n "$webkit_root" && -n "$output" && -n "$repository_root" ]] || usage
[[ "$(uname -s)" == Linux ]] || { echo "error: build-linux.sh must run on Linux" >&2; exit 1; }

repository_root=$(cd "$repository_root" && pwd)
webkit_root=$(cd "$webkit_root" && pwd)
mkdir -p "$output"
output=$(cd "$output" && pwd)
manifest=$repository_root/distribution/jsc/manifest.json
artifact_tool=$repository_root/distribution/jsc/scripts/jsc_artifact.py
"$repository_root/distribution/jsc/scripts/verify-linux-toolchain.sh" "$manifest" "$target"

expected_revision=$(jq -er '.source.revision' "$manifest")
actual_revision=$(git -C "$webkit_root" rev-parse HEAD)
if [[ "$actual_revision" != "$expected_revision" ]]; then
    echo "error: WebKit revision mismatch: expected $expected_revision, observed $actual_revision" >&2
    exit 1
fi
if [[ -n "$(git -C "$webkit_root" status --porcelain=v1 --untracked-files=all)" ]]; then
    echo "error: WebKit worktree must be clean before applying reviewed patches" >&2
    exit 1
fi

while IFS=$'\t' read -r patch_path patch_digest; do
    [[ -n "$patch_path" ]] || continue
    actual_digest=$(sha256sum "$repository_root/$patch_path" | awk '{print $1}')
    if [[ "$actual_digest" != "$patch_digest" ]]; then
        echo "error: patch digest mismatch for $patch_path" >&2
        exit 1
    fi
    git -C "$webkit_root" apply --check "$repository_root/$patch_path"
    git -C "$webkit_root" apply "$repository_root/$patch_path"
done < <(jq -r '.patches[] | [.path, .sha256] | @tsv' "$manifest")

LC_ALL=$(jq -er '.build.environment.LC_ALL' "$manifest")
TZ=$(jq -er '.build.environment.TZ' "$manifest")
SOURCE_DATE_EPOCH=$(jq -er '.build.environment.SOURCE_DATE_EPOCH' "$manifest")
export LC_ALL TZ SOURCE_DATE_EPOCH
export CC=clang-18
export CXX=clang++-18
export AR=ar
export RANLIB=ranlib
export WEBKIT_OUTPUTDIR=$output/webkit-build

build_arguments=()
while IFS= read -r argument; do
    build_arguments+=("$argument")
done < <(jq -r '.build.arguments.linux[]' "$manifest")
while IFS=$'\t' read -r name value; do
    build_arguments+=("--cmakeargs=-D${name}=${value}")
done < <(jq -r '.build.feature_flags | to_entries[] | [.key, (if (.value | type) == "boolean" then (if .value then "ON" else "OFF" end) else (.value | tostring) end)] | @tsv' "$manifest")
build_arguments+=("--cmakeargs=-DCMAKE_LINKER=/usr/bin/ld.lld-18")
build_arguments+=("--cmakeargs=-DCMAKE_EXE_LINKER_FLAGS=-fuse-ld=lld")
build_arguments+=("--cmakeargs=-DCMAKE_SHARED_LINKER_FLAGS=-fuse-ld=lld")
build_arguments+=("--cmakeargs=-DCMAKE_MODULE_LINKER_FLAGS=-fuse-ld=lld")

echo "building WebKit JavaScriptCore $expected_revision for $target"
(
    cd "$webkit_root"
    Tools/Scripts/build-jsc "${build_arguments[@]}"
)

product_dir=$WEBKIT_OUTPUTDIR
jsc_binary=$product_dir/lib/libJavaScriptCore.so
if [[ ! -f "$jsc_binary" ]]; then
    candidate=$(find "$product_dir/lib" -maxdepth 1 -type f -name 'libJavaScriptCore.so.*' -print -quit)
    [[ -n "$candidate" ]] || {
        echo "error: libJavaScriptCore.so was not produced below $product_dir" >&2
        exit 1
    }
    jsc_binary=$candidate
fi
headers=$product_dir/include
[[ -f "$headers/JavaScriptCore/JavaScript.h" ]] || {
    echo "error: JavaScriptCore public headers were not produced below $headers" >&2
    exit 1
}

native_output=$output/native
mkdir -p "$native_output"
jsc_so=$native_output/libJavaScriptCore.so
shim_so=$native_output/libkunlun_jsc.so
cp -L "$jsc_binary" "$jsc_so"
patchelf --set-soname libJavaScriptCore.so "$jsc_so"
patchelf --set-rpath "\$ORIGIN" "$jsc_so"

echo "building Kunlun C ABI shim for $target"
clang++-18 \
    -std=c++17 \
    -shared \
    -fuse-ld=lld \
    -fvisibility=hidden \
    -Wall -Wextra -Werror \
    -DKUNLUN_JSC_BUILDING_LIBRARY \
    -I "$repository_root/crates/kunlun-jsc-sys/include" \
    -I "$repository_root/crates/kunlun-jsc-sys/native" \
    -I "$headers" \
    "$repository_root/crates/kunlun-jsc-sys/native/kunlun_jsc.cpp" \
    -L "$native_output" \
    -lJavaScriptCore \
    -Wl,-soname,libkunlun_jsc.so \
    -Wl,-rpath,"\$ORIGIN" \
    -o "$shim_so"
patchelf --set-rpath "\$ORIGIN" "$shim_so"

archive_path=$(jq -er --arg target "$target" \
    '.targets[] | select(.triple == $target) | .artifact.archive_path' "$manifest")
sbom_path=$(jq -er --arg target "$target" \
    '.targets[] | select(.triple == $target) | .artifact.sbom.path' "$manifest")
staging=$output/staging/$target

python3 "$artifact_tool" assemble \
    --manifest "$manifest" \
    --repository-root "$repository_root" \
    --webkit-root "$webkit_root" \
    --target "$target" \
    --jsc-library "$jsc_so" \
    --shim-library "$shim_so" \
    --staging "$staging" \
    --output "$output"

python3 "$artifact_tool" verify \
    --manifest "$manifest" \
    --target "$target" \
    --archive "$output/$archive_path" \
    --sbom "$output/$sbom_path"
