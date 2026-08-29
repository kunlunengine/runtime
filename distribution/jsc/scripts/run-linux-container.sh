#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat >&2 <<'EOF'
usage: run-linux-container.sh --target <triple> --webkit-root <path> --output <path> [--repository-root <path>]

Builds the pinned Linux toolchain image from an Ubuntu archive snapshot, then runs the JSC build
without network access. The host architecture must match the requested target.
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
    aarch64-unknown-linux-gnu) platform=linux/arm64; expected_machine=aarch64 ;;
    x86_64-unknown-linux-gnu) platform=linux/amd64; expected_machine=x86_64 ;;
    *) echo "error: unsupported Linux target: $target" >&2; exit 1 ;;
esac
[[ "$(uname -s)" == Linux ]] || { echo "error: Linux artifact builds require a Linux host" >&2; exit 1; }
[[ "$(uname -m)" == "$expected_machine" ]] || {
    echo "error: $target must build natively on $expected_machine" >&2
    exit 1
}
for command in cargo docker jq sha256sum; do
    command -v "$command" >/dev/null || { echo "error: required command is unavailable: $command" >&2; exit 1; }
done

manifest=$repository_root/distribution/jsc/manifest.json
(cd "$repository_root" && cargo xtask jsc-manifest validate "$manifest")
toolchain=$(jq -er --arg target "$target" \
    '.targets[] | select(.triple == $target) | .toolchain' "$manifest")
base_image=$(jq -er --arg toolchain "$toolchain" \
    '.toolchains[] | select(.id == $toolchain) | .container_image' "$manifest")
trust_store_image=$(jq -er --arg toolchain "$toolchain" \
    '.toolchains[] | select(.id == $toolchain) | .trust_store_image' "$manifest")
apt_snapshot=$(jq -er --arg toolchain "$toolchain" \
    '.toolchains[] | select(.id == $toolchain) | .package_snapshot' "$manifest")
dockerfile=$repository_root/distribution/jsc/linux/Dockerfile

docker pull --platform "$platform" "$base_image"
docker pull --platform "$platform" "$trust_store_image"
tag="kunlun-jsc-builder-${target}:${apt_snapshot}"
docker build \
    --platform "$platform" \
    --build-arg "BASE_IMAGE=$base_image" \
    --build-arg "TRUST_STORE_IMAGE=$trust_store_image" \
    --build-arg "APT_SNAPSHOT=$apt_snapshot" \
    --file "$dockerfile" \
    --tag "$tag" \
    "$repository_root/distribution/jsc/linux"
builder_image_id=$(docker image inspect --format '{{.Id}}' "$tag")
[[ "$builder_image_id" =~ ^sha256:[0-9a-f]{64}$ ]] || {
    echo "error: Docker returned an invalid builder image ID: $builder_image_id" >&2
    exit 1
}

docker run --rm \
    --network none \
    --platform "$platform" \
    --user "$(id -u):$(id -g)" \
    --env "KUNLUN_APT_SNAPSHOT=$apt_snapshot" \
    --env "KUNLUN_CONTAINER_IMAGE=$base_image" \
    --env "KUNLUN_TRUST_STORE_IMAGE=$trust_store_image" \
    --mount "type=bind,src=$repository_root,dst=/workspace/runtime,readonly" \
    --mount "type=bind,src=$webkit_root,dst=/workspace/webkit" \
    --mount "type=bind,src=$output,dst=/workspace/output" \
    --workdir /workspace/runtime \
    "$builder_image_id" \
    distribution/jsc/scripts/build-linux.sh \
        --target "$target" \
        --webkit-root /workspace/webkit \
        --output /workspace/output \
        --repository-root /workspace/runtime

archive_path=$(jq -er --arg target "$target" \
    '.targets[] | select(.triple == $target) | .artifact.archive_path' "$manifest")
sbom_path=$(jq -er --arg target "$target" \
    '.targets[] | select(.triple == $target) | .artifact.sbom.path' "$manifest")
staging=$output/staging/$target

if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
    {
        echo "output_root=$output"
        echo "archive=$output/$archive_path"
        echo "sbom=$output/$sbom_path"
        echo "staging=$staging"
        echo "glibc_baseline=$(jq -er --arg target "$target" '.targets[] | select(.triple == $target) | .deployment_target.minimum' "$manifest")"
        echo "builder_image_id=$builder_image_id"
        echo "archive_sha256=$(sha256sum "$output/$archive_path" | awk '{print $1}')"
        echo "sbom_sha256=$(sha256sum "$output/$sbom_path" | awk '{print $1}')"
    } >> "$GITHUB_OUTPUT"
fi
