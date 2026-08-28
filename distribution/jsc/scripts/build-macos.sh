#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat >&2 <<'EOF'
usage: build-macos.sh --target <triple> --webkit-root <path> --output <path> [--repository-root <path>]

Builds the pinned JavaScriptCore source and Kunlun shim, assembles the archive and SPDX SBOM,
and verifies the result. The output directory must be unique to this build attempt.
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

[[ -n "$target" && -n "$webkit_root" && -n "$output" ]] || usage
script_dir=$(cd "$(dirname "$0")" && pwd)
if [[ -z "$repository_root" ]]; then
    repository_root=$(cd "$script_dir/../../.." && pwd)
else
    repository_root=$(cd "$repository_root" && pwd)
fi
webkit_root=$(cd "$webkit_root" && pwd)
mkdir -p "$output"
output=$(cd "$output" && pwd)

case "$target" in
    aarch64-apple-darwin) architecture=arm64 ;;
    x86_64-apple-darwin) architecture=x86_64 ;;
    *) echo "error: unsupported macOS target: $target" >&2; exit 1 ;;
esac

manifest=$repository_root/distribution/jsc/manifest.json
artifact_tool=$repository_root/distribution/jsc/scripts/jsc_artifact.py
[[ -f "$manifest" && -f "$artifact_tool" ]] || {
    echo "error: repository does not contain the JSC distribution inputs" >&2
    exit 1
}
for command in jq xcrun xcodebuild install_name_tool codesign zstd; do
    command -v "$command" >/dev/null || { echo "error: required command is unavailable: $command" >&2; exit 1; }
done

"$repository_root/distribution/jsc/scripts/verify-macos-toolchain.sh" "$manifest"
(cd "$repository_root" && cargo xtask jsc-manifest validate "$manifest")

expected_revision=$(jq -er '.source.revision' "$manifest")
actual_revision=$(/usr/bin/git -C "$webkit_root" rev-parse HEAD)
if [[ "$actual_revision" != "$expected_revision" ]]; then
    echo "error: WebKit revision mismatch: expected $expected_revision, observed $actual_revision" >&2
    exit 1
fi
if [[ -n "$(/usr/bin/git -C "$webkit_root" status --porcelain=v1 --untracked-files=all)" ]]; then
    echo "error: WebKit worktree must be clean before applying reviewed patches" >&2
    exit 1
fi

while IFS=$'\t' read -r patch_path patch_digest; do
    [[ -n "$patch_path" ]] || continue
    actual_digest=$(shasum -a 256 "$repository_root/$patch_path" | awk '{print $1}')
    if [[ "$actual_digest" != "$patch_digest" ]]; then
        echo "error: patch digest mismatch for $patch_path" >&2
        exit 1
    fi
    /usr/bin/git -C "$webkit_root" apply --check "$repository_root/$patch_path"
    /usr/bin/git -C "$webkit_root" apply "$repository_root/$patch_path"
done < <(jq -r '.patches[] | [.path, .sha256] | @tsv' "$manifest")

LC_ALL=$(jq -er '.build.environment.LC_ALL' "$manifest")
TZ=$(jq -er '.build.environment.TZ' "$manifest")
SOURCE_DATE_EPOCH=$(jq -er '.build.environment.SOURCE_DATE_EPOCH' "$manifest")
export LC_ALL TZ SOURCE_DATE_EPOCH
deployment_target=$(jq -er --arg target "$target" \
    '.targets[] | select(.triple == $target) | .deployment_target.minimum' "$manifest")
export MACOSX_DEPLOYMENT_TARGET=$deployment_target
export WEBKIT_OUTPUTDIR=$output/webkit-build

build_arguments=()
while IFS= read -r argument; do
    build_arguments+=("$argument")
done < <(jq -r '.build.arguments.macos[]' "$manifest")
xcode_settings=()
while IFS=$'\t' read -r name value; do
    xcode_settings+=("$name=$value")
done < <(jq -r '.build.feature_flags | to_entries[] | [.key, (if (.value | type) == "boolean" then (if .value then "1" else "0" end) else (.value | tostring) end)] | @tsv' "$manifest")
xcode_settings+=("ARCHS=$architecture")
xcode_settings+=("ONLY_ACTIVE_ARCH=NO")
xcode_settings+=("MACOSX_DEPLOYMENT_TARGET=$deployment_target")
build_arguments+=("ARGS=${xcode_settings[*]}")

echo "building WebKit JavaScriptCore $expected_revision for $target"
(
    cd "$webkit_root"
    "Tools/Scripts/build-jsc" "${build_arguments[@]}"
)

configuration=$(jq -er '.build.configuration' "$manifest")
product_dir=$WEBKIT_OUTPUTDIR/$configuration
framework=$product_dir/JavaScriptCore.framework
if [[ -f "$framework/Versions/A/JavaScriptCore" ]]; then
    jsc_binary=$framework/Versions/A/JavaScriptCore
elif [[ -f "$framework/JavaScriptCore" ]]; then
    jsc_binary=$framework/JavaScriptCore
else
    echo "error: JavaScriptCore framework binary was not produced below $product_dir" >&2
    exit 1
fi

native_output=$output/native
mkdir -p "$native_output"
jsc_dylib=$native_output/libJavaScriptCore.dylib
shim_dylib=$native_output/libkunlun_jsc.dylib
cp -L "$jsc_binary" "$jsc_dylib"
original_jsc_id=$(otool -D "$jsc_dylib" | sed -n '2p' | xargs)
[[ -n "$original_jsc_id" ]] || { echo "error: JavaScriptCore has no Mach-O install name" >&2; exit 1; }
install_name_tool -id @rpath/libJavaScriptCore.dylib "$jsc_dylib"
codesign --force --sign - "$jsc_dylib"

echo "building Kunlun C ABI shim for $target"
xcrun clang++ \
    -std=c++17 \
    -dynamiclib \
    -fvisibility=hidden \
    -Wall -Wextra -Werror \
    -arch "$architecture" \
    -mmacosx-version-min="$deployment_target" \
    -DKUNLUN_JSC_BUILDING_LIBRARY \
    -I "$repository_root/crates/kunlun-jsc-sys/include" \
    -I "$repository_root/crates/kunlun-jsc-sys/native" \
    -F "$product_dir" \
    "$repository_root/crates/kunlun-jsc-sys/native/kunlun_jsc.cpp" \
    -framework JavaScriptCore \
    -Wl,-install_name,@rpath/libkunlun_jsc.dylib \
    -Wl,-rpath,@loader_path \
    -o "$shim_dylib"

if otool -L "$shim_dylib" | awk '{print $1}' | grep -Fqx "$original_jsc_id"; then
    install_name_tool -change "$original_jsc_id" @rpath/libJavaScriptCore.dylib "$shim_dylib"
fi
codesign --force --sign - "$shim_dylib"

archive_path=$(jq -er --arg target "$target" \
    '.targets[] | select(.triple == $target) | .artifact.archive_path' "$manifest")
sbom_path=$(jq -er --arg target "$target" \
    '.targets[] | select(.triple == $target) | .artifact.sbom.path' "$manifest")
staging=$output/staging/$target

/usr/bin/python3 "$artifact_tool" assemble \
    --manifest "$manifest" \
    --repository-root "$repository_root" \
    --webkit-root "$webkit_root" \
    --target "$target" \
    --jsc-library "$jsc_dylib" \
    --shim-library "$shim_dylib" \
    --staging "$staging" \
    --output "$output"

/usr/bin/python3 "$artifact_tool" verify \
    --manifest "$manifest" \
    --target "$target" \
    --archive "$output/$archive_path" \
    --sbom "$output/$sbom_path"

if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
    {
        echo "output_root=$output"
        echo "archive=$output/$archive_path"
        echo "sbom=$output/$sbom_path"
        echo "staging=$staging"
        echo "deployment_target=$deployment_target"
        echo "archive_sha256=$(shasum -a 256 "$output/$archive_path" | awk '{print $1}')"
        echo "sbom_sha256=$(shasum -a 256 "$output/$sbom_path" | awk '{print $1}')"
    } >> "$GITHUB_OUTPUT"
fi
