use std::env;
use std::path::Path;
use std::path::PathBuf;

const HEADER: &str = "include/kunlun_jsc.h";

fn main() {
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

    let verified_distribution = if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        compile_exception_smoke();
        match env::var_os("KUNLUN_JSC_DIST_DIR").filter(|path| !path.is_empty()) {
            Some(distribution) => {
                link_verified_distribution(Path::new(&distribution));
                true
            }
            None => {
                compile_macos_shim();
                println!("cargo:rustc-link-lib=framework=JavaScriptCore");
                false
            }
        }
    } else if env::var_os("KUNLUN_JSC_DIST_DIR")
        .filter(|path| !path.is_empty())
        .is_some()
    {
        panic!("KUNLUN_JSC_DIST_DIR currently supports only macOS artifact verification");
    } else {
        false
    };
    write_backend_info(verified_distribution);
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

fn link_verified_distribution(distribution: &Path) {
    let distribution = distribution.canonicalize().unwrap_or_else(|error| {
        panic!(
            "KUNLUN_JSC_DIST_DIR {} is not a readable artifact staging directory: {error}",
            distribution.display()
        )
    });
    let required = [
        "include/kunlun_jsc.h",
        "lib/libJavaScriptCore.dylib",
        "lib/libkunlun_jsc.dylib",
        "metadata/build.json",
    ];
    for relative in required {
        let path = distribution.join(relative);
        if !path.is_file() {
            panic!(
                "KUNLUN_JSC_DIST_DIR is incomplete: required file {} is missing",
                path.display()
            );
        }
        println!("cargo:rerun-if-changed={}", path.display());
    }

    println!(
        "cargo:rustc-link-search=native={}",
        distribution.join("lib").display()
    );
    println!("cargo:rustc-link-lib=dylib=kunlun_jsc");
}

fn write_backend_info(verified_distribution: bool) {
    let distribution = if verified_distribution {
        "pinned Kunlun JSC artifact"
    } else {
        "macOS system framework (bootstrap only)"
    };
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    let source = format!(
        "/// Human-readable backend selected by the build script.\n\
         pub const BACKEND_DISTRIBUTION: &str = {distribution:?};\n\
         /// Whether this build links a verified, locally staged distribution.\n\
         pub const BACKEND_HERMETIC: bool = {verified_distribution};\n"
    );
    std::fs::write(output.join("backend.rs"), source).expect("write selected JSC backend metadata");
}
