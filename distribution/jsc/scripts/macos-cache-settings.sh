#!/usr/bin/env bash
set -euo pipefail

# Emit Xcode settings for the pinned WebKit make driver. With no shared cache,
# every invocation gets a fresh CAS, even if the caller reuses its output root.
output=${1:-}
cache_dir=${2:-}
[[ $# -ge 1 && $# -le 2 && -d "$output" ]] || {
    echo "usage: macos-cache-settings.sh <existing-output-directory> [shared-cache-directory]" >&2
    exit 2
}

if [[ -z "$cache_dir" ]]; then
    cache_dir=$(mktemp -d "$output/compilation-cache-XXXXXX")
else
    mkdir -p "$cache_dir"
fi
cache_dir=$(cd "$cache_dir" && pwd -P)
# build-jsc passes ARGS through make and a shell. Fail explicitly for paths that
# cannot be passed through that upstream interface as a single literal setting.
if [[ ! "$cache_dir" =~ ^[a-zA-Z0-9/_.-]+$ ]]; then
    echo "error: macOS compilation cache path must not contain whitespace or shell metacharacters: $cache_dir" >&2
    exit 1
fi

printf '%s\n' \
    'WK_USE_CCACHE=NO' \
    'COMPILATION_CACHE_ENABLE_CACHING=YES' \
    'COMPILATION_CACHE_ENABLE_DIAGNOSTIC_REMARKS=YES' \
    "COMPILATION_CACHE_CAS_PATH=$cache_dir" \
    'COMPILATION_CACHE_KEEP_CAS_DIRECTORY=YES' \
    'COMPILATION_CACHE_LIMIT_SIZE=2G' \
    'COMPILATION_CACHE_ENABLE_PLUGIN=NO' \
    'COMPILATION_CACHE_REMOTE_SERVICE_PATH='
