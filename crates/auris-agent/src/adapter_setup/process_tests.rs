//! Process-tree and bounded-install helper tests.

#![cfg(test)]

use std::{
    env, fs,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::process::Command;

/// Child fixture used by the process helper tests. It deliberately creates a grandchild with
/// the same test binary so a direct-child kill cannot make the test pass accidentally.
#[tokio::test]
async fn fixture_child() {
    if env::var_os("AURIS_PROCESS_FIXTURE").is_none() {
        return;
    }
    let heartbeat = PathBuf::from(env::var_os("AURIS_PROCESS_HEARTBEAT").unwrap());
    if env::var_os("AURIS_PROCESS_GRANDCHILD").is_some() {
        for count in 0u64..=u64::MAX {
            let _ = fs::write(&heartbeat, count.to_string());
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    let exe = env::current_exe().unwrap();
    let mut child = Command::new(exe);
    child
        .args([
            "--exact",
            "adapter_setup::process_tests::fixture_child",
            "--nocapture",
        ])
        .env("AURIS_PROCESS_FIXTURE", "1")
        .env("AURIS_PROCESS_GRANDCHILD", "1")
        .env("AURIS_PROCESS_HEARTBEAT", &heartbeat)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _ = child.spawn().unwrap();
    loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn fixture_command(path: &std::path::Path) -> Command {
    let mut command = Command::new(env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "adapter_setup::process_tests::fixture_child",
            "--nocapture",
        ])
        .env("AURIS_PROCESS_FIXTURE", "1")
        .env("AURIS_PROCESS_HEARTBEAT", path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[tokio::test]
async fn process_helper_reports_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let error = super::run_install_command(
        fixture_command(&dir.path().join("heartbeat")),
        &cancelled,
        Duration::from_millis(150),
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
}

#[tokio::test]
async fn process_helper_reports_failed_exit() {
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut command = Command::new(if cfg!(windows) { "cmd.exe" } else { "sh" });
    if cfg!(windows) {
        command.args(["/C", "exit", "7"]);
    } else {
        command.args(["-c", "exit 7"]);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let error = super::run_install_command(command, &cancelled, Duration::from_secs(5))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("exited"));
}

#[tokio::test]
async fn process_helper_cancellation_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let heartbeat = dir.path().join("heartbeat");
    let command = fixture_command(&heartbeat);
    let task = tokio::spawn(async move {
        super::run_install_command(command, &flag, Duration::from_secs(10)).await
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !heartbeat.exists() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    cancelled.store(true, Ordering::Release);
    let error = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    assert!(heartbeat.exists(), "grandchild did not start");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let last = fs::read(&heartbeat).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        fs::read(&heartbeat).unwrap(),
        last,
        "grandchild survived cancellation"
    );
}
