fn main() {
    println!("cargo:rustc-check-cfg=cfg(kunlun_jsc_native)");
    // kunlun-jsc-sys validates the unified backend features and artifact before
    // any dependent crate can compile. There is no unsupported runtime fallback.
    println!("cargo:rustc-cfg=kunlun_jsc_native");
}
