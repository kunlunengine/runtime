use std::process::Command;

#[test]
fn cargo_development_default_enables_temporal_without_overriding_the_environment() {
    // Cargo sets the variable before spawning the test process, avoiding any
    // process-wide mutation after JSC or worker threads have started.
    assert!(std::env::var_os("JSC_useTemporal").is_some());
    let output = Command::new(env!("CARGO_BIN_EXE_kunlun-runtime"))
        .env("JSC_useTemporal", "false")
        .args(["eval", "typeof Temporal"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "undefined");
}

#[cfg(feature = "bundled-jsc")]
#[test]
fn pinned_engine_supports_the_temporal_profile_without_an_environment_override() {
    let output = Command::new(env!("CARGO_BIN_EXE_kunlun-runtime"))
        .env_remove("JSC_useTemporal")
        .args(["eval", include_str!("fixtures/temporal.js")])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "temporal-ok"
    );
}
