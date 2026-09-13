#![cfg(all(unix, kunlun_jsc_native))]

use std::io::Read;
use std::os::fd::AsRawFd;
use std::process::{Child, Command, Stdio};

const READY_ENV: &str = "KUNLUN_RUNTIME_TEST_SIGNAL_READY";
const READY_MESSAGE: &[u8] = b"kunlun-runtime signal handlers ready\n";
const READY_TIMEOUT_MS: libc::c_int = 5_000;

fn wait_for_signal_readiness(child: &mut Child) {
    let stderr = child.stderr.as_mut().expect("child stderr is piped");
    let mut descriptor = libc::pollfd {
        fd: stderr.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: descriptor points to one initialized pollfd for the live child pipe.
    let ready = unsafe { libc::poll(&mut descriptor, 1, READY_TIMEOUT_MS) };
    assert_eq!(ready, 1, "runtime did not announce signal readiness");
    assert_ne!(descriptor.revents & libc::POLLIN, 0);

    let mut message = [0; READY_MESSAGE.len()];
    stderr.read_exact(&mut message).unwrap();
    assert_eq!(message, READY_MESSAGE);
}

#[test]
fn sigterm_runs_the_documented_graceful_shutdown_path() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kunlun-runtime"))
        .args([
            "eval-async",
            "await sleep(60000); return 'unreachable';",
            "--shutdown-grace-ms",
            "1000",
        ])
        .env(READY_ENV, "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for_signal_readiness(&mut child);
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
    let mut child = Command::new(env!("CARGO_BIN_EXE_kunlun-runtime"))
        .args([
            "eval-async",
            "await sleep(60000); return 'unreachable';",
            "--shutdown-grace-ms",
            "60000",
        ])
        .env(READY_ENV, "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for_signal_readiness(&mut child);
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
