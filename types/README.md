# @kunlun-js/runtime-types

Ambient TypeScript declarations for Kunlun Runtime built-in modules.

This directory is a types-only publication artifact. It contains no JavaScript implementation; the
modules are supplied by `kunlun-runtime`. Run `cargo run -p kunlun-runtime -- types` to print the exact
declarations embedded in the native runtime.

The `runtime-manifest-v1` subpath is the data-only artifact contract. The
`server-entry-v1` subpath describes the Fetch handler for M3; native inbound
invocation is tracked by #51. The outbound Fetch profile is documented in
[the Fetch profile](../docs/fetch-profile-v1.md).
