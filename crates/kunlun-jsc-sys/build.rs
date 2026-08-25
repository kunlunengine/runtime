use std::env;
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

    generate_bindings();
    compile_header_smoke_tests();

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        compile_macos_shim();
        println!("cargo:rustc-link-lib=framework=JavaScriptCore");
    }
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
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .file("native/kunlun_jsc.cpp")
        .file("native/exception_smoke.cpp")
        .include("include")
        .include("native")
        .define("KUNLUN_JSC_BUILDING_LIBRARY", None)
        .std("c++17")
        .warnings_into_errors(true);

    if let Some(sdk) = env::var_os("SDKROOT").filter(|path| !path.is_empty()) {
        build.flag("-isysroot").flag(sdk);
    }
    build.compile("kunlun_jsc");
}
