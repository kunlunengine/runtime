# Windows x64 port

Status: **portability foundation, not a supported Runtime release target**.
`x86_64-pc-windows-msvc` still fails closed in Cargo's JSC backend selector. No Windows engine
artifact, product binary, debugger qualification, or release evidence is implied by this work.

## Implemented foundation

- Runtime filesystem reads, module source reads, and artifact admission share a capability-relative,
  read-only regular-file open policy. Unix retains nonblocking opens to reject FIFOs without waiting
  for a writer. Windows uses handle metadata and rejects final reparse points for artifact admission.
  `cap-std` remains responsible for confining path resolution to the granted directory.
- Artifact admission sets the resolver-level `FollowSymlinks::No` policy, not only `O_NOFOLLOW`.
  `cap-std` can otherwise resolve a symlink and retry despite the raw Unix flag.
- Portable module-resolution and invalid-UTF-8 tests are no longer hidden by Unix-only suite guards.
  Unix signal/symlink tests stay platform-specific; they are not Windows shutdown evidence.
- Distribution receipt inventories use canonical `/` separators on every host. Manifest and archive
  names reject backslashes, drive prefixes, traversal, empty/dot components, Win32 device aliases,
  alternate data streams, trailing-dot/space aliases, and control characters. Archive extraction also
  rejects case-folded duplicate member names before writing any extracted file.
- Git checks out text inputs with LF endings, preserving the reviewed patch/license/header digests.
- `Check Rust` has a native `windows-2025` x64 job. It runs the engine-free tooling, provider and
  DevTools contract tests, and the **actual shared runtime file-open implementation** through
  `xtask`. This includes a junction-escape regression that does not require symlink privilege.
  Portable Python path/extraction tests and actual offline Cargo backend rejection probes also run.

The Windows job intentionally does **not** build `kunlun-runtime`, substitute a stub engine, run
`doctor`, or satisfy the four-platform M2 gate. The moving hosted image is adequate for portability
tests, not an exact engine-build toolchain pin.

### Run the current Windows checks

From a native x64 Windows Rust developer environment, at the repository root:

```powershell
cargo xtask jsc-manifest validate
cargo clippy --locked -p xtask -p kunlun-runtime-protocol -p kunlun-devtools-protocol --all-targets -- -D warnings
cargo test --locked -p xtask -p kunlun-runtime-protocol -p kunlun-devtools-protocol
python -m unittest discover -s distribution/jsc/scripts/tests -p test_jsc_paths.py -v
cargo fetch --locked
python distribution/jsc/scripts/test_cargo_backends.py
```

`cargo check --target x86_64-pc-windows-msvc` on macOS/Linux is a useful type-check, but cannot
replace these native filesystem tests. Neither is evidence for a linked Windows JSC Runtime.

## Pinned upstream build path

The reviewed source remains WebKit `4b62d53ec6c16753020dbe69e59bf761ed0948e3`; do not switch to an
arbitrary Windows JSC download or the host's engine to unblock Cargo.

Upstream already has a viable JSCOnly Windows path:

- [`OptionsJSCOnly.cmake`](https://github.com/WebKit/WebKit/blob/4b62d53ec6c16753020dbe69e59bf761ed0948e3/Source/cmake/OptionsJSCOnly.cmake)
  handles `WIN32`, shared JSC, ICU/Threads and the Generic event loop. Remote inspector is off by
  default, so the controlled build must explicitly enable the reviewed feature set.
- [`OptionsMSVC.cmake`](https://github.com/WebKit/WebKit/blob/4b62d53ec6c16753020dbe69e59bf761ed0948e3/Source/cmake/OptionsMSVC.cmake)
  explicitly requires **clang-cl**, not Microsoft's `cl.exe`. The Rust `-msvc` triple describes the
  Windows ABI; it does not require compiling WebKit with Microsoft's C++ frontend.
- [`PlatformJSCOnly.cmake`](https://github.com/WebKit/WebKit/blob/4b62d53ec6c16753020dbe69e59bf761ed0948e3/Source/JavaScriptCore/PlatformJSCOnly.cmake)
  and the [socket source selection](https://github.com/WebKit/WebKit/blob/4b62d53ec6c16753020dbe69e59bf761ed0948e3/Source/JavaScriptCore/inspector/remote/SourcesSocket.txt)
  provide a Windows Winsock/TCP inspector path. The existing local POSIX-only inspector patch must
  not be generalized to force Unix-domain sockets on Windows.
- The [Windows cross-build dependency check](https://github.com/WebKit/WebKit/blob/4b62d53ec6c16753020dbe69e59bf761ed0948e3/Tools/Scripts/check-win-cross-build-deps)
  documents clang-cl/LLVM tools, the Windows SDK/CRT sysroot, clang builtins and vcpkg.
  A native Windows builder is the recommended first qualification path; cross-building does not
  remove the requirement to execute the resulting corpus on Windows.

See also the official [WebKit Windows port guide](https://docs.webkit.org/Ports/WindowsPort.html).
These are source findings, not a claim that Kunlun's patched engine has already been built.

## Remaining gates, in order

1. **Controlled engine build.** Pin exact clang-cl/LLVM, SDK, CRT, ICU, CMake, Ninja and scripting
   tools, the dependency acquisition hashes/licenses, and a minimum Windows deployment version.
   Apply and digest-check the existing patch set against the pinned source, then build JSCOnly and
   the complete Kunlun shim. Fix any Windows-specific compile/link failures through reviewed patches.
2. **PE/COFF distribution and Cargo consumption.** Package engine/shim DLLs and MSVC-compatible
   import `.lib` files, plus every required non-system DLL. Record and verify x64 architecture,
   exports/imports, dependency inventory, ABI header, licenses, SBOM and provenance. Define the
   trusted DLL lookup/staging rule; relying on an arbitrary process `PATH` is not release policy.
   Enable the triple coherently in the manifest, schema, Rust validator, artifact tools and Cargo
   backend only once this engine path is qualified. Keep offline receipt/digest verification.
3. **Native runtime and debugger evidence.** Run the full workspace corpus and bundled `doctor`
   with the verified Windows installation. Cover ESM/Promises, file URLs/drive and UNC behavior,
   reparse races, capability denial, cancellation and process shutdown, buffers/ownership,
   Temporal, watchdog/heap telemetry, Fetch/application integration, and DLL loading from the
   shipped layout. Qualify inspector attach, breakpoint/step/scopes, async pause, source maps,
   reconnect and transport authorization through the same DevTools contract.
4. **Release gate.** Add a Windows builder/evidence report and explicit combined-gate coverage.
   Require cold independent rebuild comparison and signed archive/SPDX provenance at the same
   immutable reviewed commit as the other targets. Only after durable asset publication and
   reviewed digests may the target become `published` and a Desktop installer consume it.

Do not weaken the existing macOS/Linux M2 evidence requirements to make Windows preparation green.
The [release process](./release-process.md) remains authoritative for publication.
