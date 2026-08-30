use std::env;
use std::path::Path;
use std::path::PathBuf;

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
    println!("cargo:rerun-if-env-changed=KUNLUN_JSC_DIST_DIR");

    generate_bindings();
    compile_header_smoke_tests();

    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("Cargo sets CARGO_CFG_TARGET_OS");
    let distribution = env::var_os("KUNLUN_JSC_DIST_DIR").filter(|path| !path.is_empty());
    let has_staged_distribution = match (target_os.as_str(), distribution) {
        ("macos" | "linux", Some(distribution)) => {
            compile_exception_smoke();
            link_staged_distribution(Path::new(&distribution), &target_os);
            true
        }
        ("macos", None) => {
            compile_exception_smoke();
            compile_macos_shim();
            println!("cargo:rustc-link-lib=framework=JavaScriptCore");
            false
        }
        (_, Some(_)) => {
            panic!("KUNLUN_JSC_DIST_DIR supports only staged macOS and Linux artifacts")
        }
        (_, None) => false,
    };
    if has_staged_distribution || target_os == "macos" {
        println!("cargo:rustc-cfg=kunlun_jsc_native");
    }
    write_backend_info(has_staged_distribution, &target_os);
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

fn link_staged_distribution(distribution: &Path, target_os: &str) {
    let distribution = distribution.canonicalize().unwrap_or_else(|error| {
        panic!(
            "KUNLUN_JSC_DIST_DIR {} is not a readable artifact staging directory: {error}",
            distribution.display()
        )
    });
    let libraries = match target_os {
        "macos" => ["lib/libJavaScriptCore.dylib", "lib/libkunlun_jsc.dylib"],
        "linux" => ["lib/libJavaScriptCore.so", "lib/libkunlun_jsc.so"],
        _ => unreachable!("caller validates the artifact platform"),
    };
    let mut metadata = vec!["metadata/build.json"];
    if target_os == "linux" {
        metadata.push("metadata/runtime-dependencies.json");
    }
    for relative in ["include/kunlun_jsc.h"]
        .into_iter()
        .chain(metadata.iter().copied())
        .chain(libraries)
    {
        let path = distribution.join(relative);
        if !path.is_file() {
            panic!(
                "KUNLUN_JSC_DIST_DIR is incomplete: required file {} is missing",
                path.display()
            );
        }
        println!("cargo:rerun-if-changed={}", path.display());
    }
    validate_staging_metadata(&distribution, target_os);

    println!(
        "cargo:rustc-link-search=native={}",
        distribution.join("lib").display()
    );
    println!("cargo:rustc-link-lib=dylib=kunlun_jsc");
    println!("cargo:rustc-link-lib=dylib=JavaScriptCore");
}

fn validate_staging_metadata(distribution: &Path, target_os: &str) {
    let cargo_target = env::var("TARGET").expect("Cargo sets TARGET");
    let build_path = distribution.join("metadata/build.json");
    let build: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&build_path)
            .unwrap_or_else(|error| panic!("could not read {}: {error}", build_path.display())),
    )
    .unwrap_or_else(|error| panic!("could not parse {}: {error}", build_path.display()));
    let staged_target = build
        .pointer("/target/triple")
        .and_then(serde_json::Value::as_str);
    let staged_os = build
        .pointer("/target/os")
        .and_then(serde_json::Value::as_str);
    if staged_target != Some(cargo_target.as_str()) || staged_os != Some(target_os) {
        panic!(
            "KUNLUN_JSC_DIST_DIR target mismatch: Cargo requested {cargo_target} ({target_os}), \
             staging metadata records {staged_target:?} ({staged_os:?})"
        );
    }

    if target_os == "linux" {
        let runtime_path = distribution.join("metadata/runtime-dependencies.json");
        let runtime: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&runtime_path).unwrap_or_else(|error| {
                panic!("could not read {}: {error}", runtime_path.display())
            }))
            .unwrap_or_else(|error| panic!("could not parse {}: {error}", runtime_path.display()));
        let runtime_target = runtime.get("target").and_then(serde_json::Value::as_str);
        if runtime_target != Some(cargo_target.as_str()) {
            panic!(
                "KUNLUN_JSC_DIST_DIR runtime metadata target mismatch: Cargo requested \
                 {cargo_target}, staging metadata records {runtime_target:?}"
            );
        }
    }
}

fn write_backend_info(has_staged_distribution: bool, target_os: &str) {
    let distribution = match (has_staged_distribution, target_os) {
        (true, _) => "pinned Kunlun JSC artifact",
        (false, "macos") => "macOS system framework (bootstrap only)",
        (false, _) => "unsupported non-macOS stub",
    };
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    let source = format!(
        "/// Human-readable backend selected by the build script.\n\
         pub const BACKEND_DISTRIBUTION: &str = {distribution:?};\n\
         /// Whether this build links a verified, locally staged distribution.\n\
         pub const BACKEND_HERMETIC: bool = {has_staged_distribution};\n"
    );
    std::fs::write(output.join("backend.rs"), source).expect("write selected JSC backend metadata");
}
