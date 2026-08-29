use std::env;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(kunlun_jsc_native)");
    println!("cargo:rerun-if-env-changed=KUNLUN_JSC_DIST_DIR");

    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("Cargo sets CARGO_CFG_TARGET_OS");
    let has_distribution = env::var_os("KUNLUN_JSC_DIST_DIR").is_some_and(|path| !path.is_empty());
    if target_os == "macos" || (target_os == "linux" && has_distribution) {
        println!("cargo:rustc-cfg=kunlun_jsc_native");
    }
}
