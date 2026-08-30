fn main() {
    // The shared ownership test module excludes native-only accessors here.
    println!("cargo:rustc-check-cfg=cfg(kunlun_jsc_native)");
}
