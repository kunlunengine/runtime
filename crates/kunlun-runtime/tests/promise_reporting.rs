use kunlun_jsc::JscVm;
use std::process::Command;

#[test]
fn cli_checkpoints_scripts_and_reports_unhandled_and_late_handled_transitions() {
    if !JscVm::backend_info().supports_explicit_microtask_checkpoint {
        return;
    }
    let script = Command::new(env!("CARGO_BIN_EXE_kunlun-runtime"))
        .args([
            "eval",
            "Promise.resolve().then(() => { throw new Error('nested boom'); });",
        ])
        .output()
        .unwrap();
    assert!(script.status.success());
    let stderr = String::from_utf8(script.stderr).unwrap();
    assert_eq!(
        stderr.matches("unhandled Promise rejection").count(),
        1,
        "{stderr}"
    );
    assert!(stderr.contains("kunlun:eval"));
    assert!(stderr.contains("nested boom"));

    let handled = Command::new(env!("CARGO_BIN_EXE_kunlun-runtime"))
        .args(["eval-async", "const p = Promise.reject(new Error('late boom')); await sleep(0); p.catch(() => {}); return 'done';"])
        .output().unwrap();
    assert!(handled.status.success(), "{handled:?}");
    let stderr = String::from_utf8(handled.stderr).unwrap();
    assert_eq!(
        stderr.matches("unhandled Promise rejection").count(),
        1,
        "{stderr}"
    );
    assert_eq!(
        stderr.matches("Promise rejection handled").count(),
        1,
        "{stderr}"
    );
    assert_eq!(String::from_utf8(handled.stdout).unwrap().trim(), "done");

    let suppressed = Command::new(env!("CARGO_BIN_EXE_kunlun-runtime"))
        .args(["eval", "const p = Promise.reject('handled this turn'); Promise.resolve().then(() => p.catch(() => {}));"])
        .output().unwrap();
    assert!(suppressed.status.success());
    assert!(suppressed.stderr.is_empty(), "{suppressed:?}");
}

#[test]
fn cli_reports_rejections_even_when_script_evaluation_throws() {
    if !JscVm::backend_info().supports_explicit_microtask_checkpoint {
        return;
    }
    let output = Command::new(env!("CARGO_BIN_EXE_kunlun-runtime"))
        .args([
            "eval",
            "Promise.reject('unobserved'); throw new Error('synchronous failure');",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("unhandled Promise rejection"));
    assert!(stderr.contains("unobserved"));
    assert!(stderr.contains("synchronous failure"));
}
