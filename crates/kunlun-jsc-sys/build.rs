use std::env;
use std::path::PathBuf;

#[path = "../../distribution/jsc/backend.rs"]
mod backend;

const MANIFEST: &str = "../../distribution/jsc/manifest.json";
const HEADER: &str = "include/kunlun_jsc.h";

fn main() {
    println!("cargo:rustc-check-cfg=cfg(kunlun_jsc_native)");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={HEADER}");
    println!("cargo:rerun-if-changed=native/header_smoke.c");
    println!("cargo:rerun-if-changed=native/header_smoke.cpp");
    println!("cargo:rerun-if-changed=native/exception_boundary.hpp");
    println!("cargo:rerun-if-changed=native/exception_smoke.cpp");
    println!("cargo:rerun-if-changed=native/kunlun_jsc.cpp");
    println!("cargo:rerun-if-changed=native/buffers.inc");
    println!("cargo:rerun-if-changed=native/external_bytes.hpp");
    println!("cargo:rerun-if-env-changed=KUNLUN_JSC_DIST_DIR");

    println!("cargo:rerun-if-env-changed=KUNLUN_JSC_RECEIPT_SHA256");
    println!("cargo:rerun-if-changed=../../distribution/jsc/backend.rs");
    println!("cargo:rerun-if-changed={MANIFEST}");

    let target = env::var("TARGET").expect("Cargo sets TARGET");
    let selected = backend::select(
        env::var_os("CARGO_FEATURE_BUNDLED_JSC").is_some(),
        env::var_os("CARGO_FEATURE_SYSTEM_JSC").is_some(),
        &target,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let distribution = env::var_os("KUNLUN_JSC_DIST_DIR").filter(|path| !path.is_empty());
    let receipt = env::var("KUNLUN_JSC_RECEIPT_SHA256")
        .ok()
        .filter(|value| !value.is_empty());
    let verified = match selected {
        backend::Backend::Bundled => {
            let directory = distribution.expect("bundled-jsc requires KUNLUN_JSC_DIST_DIR pointing to an offline-verified local artifact; run jsc_artifact.py verify --install-dir first; Cargo never downloads JSC. For macOS development use --no-default-features --features system-jsc");
            let digest = receipt.expect("bundled-jsc requires KUNLUN_JSC_RECEIPT_SHA256 from the trusted offline verification step; do not derive trust from an arbitrary local directory");
            Some(
                backend::verify(
                    std::path::Path::new(&directory),
                    &digest,
                    std::path::Path::new(MANIFEST),
                    std::path::Path::new(HEADER),
                    &target,
                )
                .unwrap_or_else(|error| panic!("bundled-jsc verification failed: {error}")),
            )
        }
        backend::Backend::System => {
            assert!(
                distribution.is_none() && receipt.is_none(),
                "system-jsc cannot consume distribution settings; unset KUNLUN_JSC_DIST_DIR and KUNLUN_JSC_RECEIPT_SHA256 or select bundled-jsc"
            );
            println!(
                "cargo:warning=system-jsc uses the host macOS framework for development only; it is not a supported release engine"
            );
            None
        }
    };

    // Validate policy and every artifact byte before invoking any native compiler.
    generate_bindings();
    compile_header_smoke_tests();
    compile_exception_smoke();
    if let Some(verified) = &verified {
        // Watch the entire tree as well as each file to detect additions/deletions.
        println!("cargo:rerun-if-changed={}", verified.root.display());
        for path in &verified.files {
            println!("cargo:rerun-if-changed={}", path.display());
        }
        println!(
            "cargo:rustc-link-search=native={}",
            verified.root.join("lib").display()
        );
        println!("cargo:rustc-link-lib=dylib=kunlun_jsc");
        println!("cargo:rustc-link-lib=dylib=JavaScriptCore");
    } else {
        compile_macos_shim();
        println!("cargo:rustc-link-lib=framework=JavaScriptCore");
    }
    println!("cargo:rustc-cfg=kunlun_jsc_native");
    write_backend_info(verified.as_ref(), &target);
}

fn generate_bindings() {
    let bindings = bindgen::Builder::default()
        .header(HEADER)
        .allowlist_function("^kunlun_jsc_.*")
        .allowlist_type("^kunlun_jsc_.*")
        .allowlist_var("^KUNLUN_JSC_.*")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("generate Kunlun JSC bindings from the authoritative header");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    bindings
        .write_to_file(output.join("bindings.rs"))
        .expect("write generated Kunlun JSC bindings");
}

fn compile_header_smoke_tests() {
    let mut c = cc::Build::new();
    c.file("native/header_smoke.c")
        .include("include")
        .warnings_into_errors(true)
        .compile("kunlun_jsc_c_header_smoke");

    let mut cpp = cc::Build::new();
    cpp.cpp(true)
        .file("native/header_smoke.cpp")
        .include("include")
        .std("c++17")
        .warnings_into_errors(true)
        .compile("kunlun_jsc_cpp_header_smoke");
}

fn compile_macos_shim() {
    println!("cargo:rerun-if-env-changed=SDKROOT");

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .file("native/kunlun_jsc.cpp")
        .include("include")
        .include("native")
        .define("KUNLUN_JSC_BUILDING_LIBRARY", None)
        .std("c++17")
        .warnings_into_errors(true);

    if let Some(sdk) = env::var_os("SDKROOT").filter(|path| !path.is_empty()) {
        build.flag("-isysroot").flag(sdk);
    }
    // Keep the development-only static shim distinct from the distributed
    // libkunlun_jsc.dylib so switching backends cannot select a stale archive.
    build.compile("kunlun_jsc_bootstrap");
}

fn compile_exception_smoke() {
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .file("native/exception_smoke.cpp")
        .include("include")
        .include("native")
        .std("c++17")
        .warnings_into_errors(true)
        .compile("kunlun_jsc_exception_smoke");
}

fn write_backend_info(verified: Option<&backend::VerifiedDistribution>, target: &str) {
    let (backend, distribution, mode, revision) = match verified {
        Some(info) => (
            "bundled-jsc",
            "pinned Kunlun JSC artifact",
            info.mode.as_str(),
            info.revision.as_str(),
        ),
        None => (
            "system-jsc",
            "macOS system framework (development only)",
            "system",
            "unknown (host OS managed; not pinned)",
        ),
    };
    let hermetic = verified.is_some();
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    let source = format!(
        "pub const BACKEND_NAME: &str = {backend:?};\n\
         pub const BACKEND_DISTRIBUTION: &str = {distribution:?};\n\
         pub const BACKEND_MODE: &str = {mode:?};\n\
         pub const BACKEND_REVISION: &str = {revision:?};\n\
         pub const BACKEND_TARGET: &str = {target:?};\n\
         pub const BACKEND_HERMETIC: bool = {hermetic};\n"
    );
    std::fs::write(output.join("backend.rs"), source).expect("write selected JSC backend metadata");
}
