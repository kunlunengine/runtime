# RustRover on macOS

Open the repository-root `Cargo.toml`. For system-framework development, disable `default` and
`bundled-jsc` and enable `system-jsc` in all three runtime crates. Cargo run/test configurations
also need `--no-default-features --features system-jsc`. Do not select all features: the two
backends are mutually exclusive. See JetBrains' [Cargo feature settings](https://www.jetbrains.com/help/rust/rust-cfg-support.html).

A missing `KUNLUN_JSC_DIST_DIR` error means the IDE selected the default bundled backend without
its verified artifact. Setting `CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG` adds backtrace details;
it does not select a backend or supply an artifact. For bundled development, configure both
verification environment variables from [offline artifact setup](./jsc-distribution.md#selecting-a-cargo-backend).

## RustRover 2026.2 build-script synchronization workaround

Some RustRover build-script synchronization commands omit `--no-default-features` even when the
feature selector has disabled defaults. The IDE log then shows `cargo check --workspace --features
kunlun-jsc/system-jsc,...` and the build reports conflicting backends. Normal terminal commands with
the full flags still succeed.

For this version, a project-local toolchain directory can forward `rustc`, `rustdoc`, `rustup`,
`rustfmt`, `cargo-fmt`, `cargo-clippy` and `clippy-driver` to the existing toolchain and supply a
small `cargo` adapter. Put it under the ignored `.idea/cargo-toolchain` directory and choose that
directory in **Settings → Rust → Toolchain location**. The adapter's complete policy is:

```python
#!/usr/bin/env python3
import os
import sys

real_cargo = os.path.expanduser('~/.cargo/bin/cargo')
args = sys.argv[1:]
options = args[:args.index('--')] if '--' in args else args
if ('check' in options and '--no-default-features' not in options
        and '--all-features' not in options
        and any('system-jsc' in arg for arg in options)):
    args.insert(args.index('check') + 1, '--no-default-features')
os.execv(real_cargo, ['cargo', *args])
```

Make the adapter executable. Adjust `real_cargo` if Rust is installed elsewhere; it must point to
the real Cargo executable, not this adapter. All metadata, explicit all-feature requests and
non-check commands pass through. Cargo still performs the normal backend selection and verification.
This changes no global toolchain settings or repository build defaults. After an IDE update that
fixes synchronization, restore the original toolchain location and remove the local adapter.

Refresh the Cargo project and check that synchronization completes; the `system-jsc` development
warning is expected. Keep machine-specific `.idea` state out of pull requests.
