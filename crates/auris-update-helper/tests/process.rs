//! Exercise the helper's real entry point and main-thread stack, including release builds.

use std::process::Command;

#[test]
fn missing_arguments_report_usage_instead_of_crashing() {
    let output = Command::new(env!("CARGO_BIN_EXE_auris-update-helper"))
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("usage: auris-update-helper"));
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[test]
fn invalid_package_reports_digest_failure_after_parent_exit() {
    let directory = tempfile::tempdir().unwrap();
    let suffix = if cfg!(target_os = "windows") {
        "windows-x86_64-setup.exe"
    } else {
        "macos-aarch64.dmg"
    };
    let package = directory.path().join(format!("auris-studio-v1-{suffix}"));
    std::fs::write(&package, vec![0x5a; 1024 * 1024 + 1]).unwrap();
    let app = if cfg!(target_os = "windows") {
        directory.path().join("auris-studio.exe")
    } else {
        directory.path().join("Applications/Auris Studio.app")
    };
    std::fs::create_dir_all(app.parent().unwrap()).unwrap();
    let report = directory.path().join("update-result.json");

    // Use a real, exited PID so the helper never waits for this test process to exit.
    let mut parent = Command::new(env!("CARGO_BIN_EXE_auris-update-helper"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let parent_pid = parent.id();
    parent.wait().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_auris-update-helper"))
        .arg("apply")
        .arg("--parent-pid")
        .arg(parent_pid.to_string())
        .arg("--package")
        .arg(&package)
        .arg("--app")
        .arg(&app)
        .arg("--sha256")
        .arg("00".repeat(32))
        .arg("--result-file")
        .arg(&report)
        .env("HOME", directory.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(report).unwrap()).unwrap();
    assert_eq!(report["status"], "error");
    assert!(
        report["message"]
            .as_str()
            .unwrap()
            .contains("update package SHA-256 mismatch")
    );
}
