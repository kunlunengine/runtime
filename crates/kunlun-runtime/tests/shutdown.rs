#![cfg(all(unix, kunlun_jsc_native))]

use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

#[test]
fn sigterm_runs_the_documented_graceful_shutdown_path() {
    let child = Command::new(env!("CARGO_BIN_EXE_kunlun-runtime"))
        .args([
            "eval-async",
            "await sleep(60000); return 'unreachable';",
            "--shutdown-grace-ms",
            "1000",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    thread::sleep(Duration::from_secs(1));
    // SAFETY: the child PID identifies the live process created above.
    let result = unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    assert_eq!(result, 0);
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("received SIGTERM; stopping admission"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        stderr.contains("execution interrupted by SIGTERM"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn a_second_signal_forces_shutdown_without_waiting_for_the_grace_period() {
    let child = Command::new(env!("CARGO_BIN_EXE_kunlun-runtime"))
        .args([
            "eval-async",
            "await sleep(60000); return 'unreachable';",
            "--shutdown-grace-ms",
            "60000",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    thread::sleep(Duration::from_secs(1));
    // SAFETY: the child PID identifies the live process created above.
    assert_eq!(
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGINT) },
        0
    );
    // A different signal has a separate Tokio signal queue, so it cannot be
    // coalesced with the first one before the shutdown selector observes it.
    assert_eq!(
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("during graceful shutdown; forcing termination"),
        "unexpected stderr: {stderr}"
    );
}
