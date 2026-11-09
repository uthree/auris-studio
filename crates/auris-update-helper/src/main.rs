//! The small process that finishes an update after Auris Studio has exited.
//!
//! The desktop process downloads and verifies a package, then starts this helper.  Keeping the
//! file replacement here means the app itself never tries to overwrite a running executable.

#![warn(missing_docs)]
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use std::env;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::process::Stdio;
use std::process::{Command, ExitCode};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use sha2::{Digest, Sha256};

const MAX_WAIT: Duration = Duration::from_secs(5 * 60);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug)]
enum UpdateError {
    Usage(String),
    InvalidPath(String),
    Io(std::io::Error),
    Process(String),
}

impl Display for UpdateError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) | Self::InvalidPath(message) | Self::Process(message) => {
                f.write_str(message)
            }
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl Error for UpdateError {}

impl From<std::io::Error> for UpdateError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ApplyArgs {
    parent_pid: u32,
    package: PathBuf,
    app: PathBuf,
    restart: bool,
    sha256: String,
    result_file: Option<PathBuf>,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
enum ResultReport<'a> {
    Success,
    Error { message: &'a str },
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("auris-update-helper: {error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), UpdateError> {
    let args = parse_args(env::args_os().collect())?;
    let outcome = (|| {
        validate_args(&args)?;
        wait_for_parent(args.parent_pid)?;
        verify_package(&args.package, &args.sha256)?;

        if cfg!(target_os = "windows") {
            apply_windows(&args)
        } else if cfg!(target_os = "macos") {
            apply_macos(&args)
        } else {
            Err(UpdateError::Process(
                "automatic updates are not supported on this platform".into(),
            ))
        }
    })();
    if let Some(path) = &args.result_file {
        match &outcome {
            Ok(()) => write_result(path, ResultReport::Success)?,
            Err(error) => {
                if let Err(report_error) = write_result(
                    path,
                    ResultReport::Error {
                        message: &error.to_string(),
                    },
                ) {
                    eprintln!("auris-update-helper: could not write result file: {report_error}");
                }
                if args.restart
                    && let Err(relaunch_error) = relaunch_existing_app(&args.app)
                {
                    eprintln!(
                        "auris-update-helper: could not relaunch the existing app: {relaunch_error}"
                    );
                }
            }
        }
    }
    outcome
}

fn parse_args(args: Vec<std::ffi::OsString>) -> Result<ApplyArgs, UpdateError> {
    let mut values = args.into_iter().skip(1);
    if values.next().as_deref() != Some(std::ffi::OsStr::new("apply")) {
        return Err(UpdateError::Usage(
            "usage: auris-update-helper apply --parent-pid <u32> --package <absolute path> --app <absolute path> --sha256 <64 hex> [--restart]".into(),
        ));
    }

    let mut parent_pid = None;
    let mut package = None;
    let mut app = None;
    let mut restart = false;
    let mut sha256 = None;
    let mut result_file = None;
    while let Some(flag) = values.next() {
        if flag == std::ffi::OsStr::new("--restart") {
            restart = true;
            continue;
        }
        let value = values.next().ok_or_else(|| {
            UpdateError::Usage(format!("missing value for {}", flag.to_string_lossy()))
        })?;
        match flag.to_str() {
            Some("--parent-pid") => {
                parent_pid = Some(
                    value
                        .to_str()
                        .and_then(|value| value.parse().ok())
                        .ok_or_else(|| {
                            UpdateError::Usage("--parent-pid must be an unsigned integer".into())
                        })?,
                );
            }
            Some("--package") => package = Some(PathBuf::from(value)),
            Some("--app") => app = Some(PathBuf::from(value)),
            Some("--sha256") => sha256 = Some(value),
            Some("--result-file") => result_file = Some(PathBuf::from(value)),
            Some(unknown) => {
                return Err(UpdateError::Usage(format!("unknown argument {unknown}")));
            }
            None => return Err(UpdateError::Usage("arguments must be UTF-8".into())),
        }
    }
    Ok(ApplyArgs {
        parent_pid: parent_pid.ok_or_else(|| UpdateError::Usage("missing --parent-pid".into()))?,
        package: package.ok_or_else(|| UpdateError::Usage("missing --package".into()))?,
        app: app.ok_or_else(|| UpdateError::Usage("missing --app".into()))?,
        restart,
        sha256: sha256
            .ok_or_else(|| UpdateError::Usage("missing --sha256".into()))?
            .to_str()
            .ok_or_else(|| UpdateError::Usage("--sha256 must be UTF-8".into()))?
            .to_owned(),
        result_file,
    })
}

fn validate_args(args: &ApplyArgs) -> Result<(), UpdateError> {
    if args.parent_pid == 0 || args.parent_pid == std::process::id() {
        return Err(UpdateError::InvalidPath(
            "parent PID must identify the running Auris process".into(),
        ));
    }
    if args.sha256.len() != 64 || hex::decode(&args.sha256).is_err() {
        return Err(UpdateError::InvalidPath(
            "--sha256 must contain exactly 64 hexadecimal characters".into(),
        ));
    }
    if !args.package.is_absolute() || !args.app.is_absolute() {
        return Err(UpdateError::InvalidPath(
            "package and app paths must be absolute".into(),
        ));
    }
    if let Some(path) = &args.result_file {
        if !path.is_absolute() {
            return Err(UpdateError::InvalidPath(
                "--result-file must be an absolute path".into(),
            ));
        }
        let parent = path.parent().ok_or_else(|| {
            UpdateError::InvalidPath("result file has no parent directory".into())
        })?;
        if !parent.is_dir() {
            return Err(UpdateError::InvalidPath(format!(
                "result file parent directory does not exist: {}",
                parent.display()
            )));
        }
    }
    if !args.package.is_file() {
        return Err(UpdateError::InvalidPath(format!(
            "update package does not exist: {}",
            args.package.display()
        )));
    }
    let app_parent = args
        .app
        .parent()
        .ok_or_else(|| UpdateError::InvalidPath("app path has no parent directory".into()))?;
    if !app_parent.is_dir() {
        return Err(UpdateError::InvalidPath(format!(
            "app parent directory does not exist: {}",
            app_parent.display()
        )));
    }

    if cfg!(target_os = "windows") {
        validate_suffix(&args.package, "-windows-x86_64-setup.exe")?;
        validate_file_name(&args.app, "auris-studio.exe")?;
    } else if cfg!(target_os = "macos") {
        validate_suffix(&args.package, "-macos-aarch64.dmg")?;
        validate_file_name(&args.app, "Auris Studio.app")?;
        let home = env::var_os("HOME").map(PathBuf::from);
        if !is_macos_app_path(&args.app, home.as_deref()) {
            return Err(UpdateError::InvalidPath(
                "macOS automatic updates require /Applications/Auris Studio.app or ~/Applications/Auris Studio.app".into(),
            ));
        }
    }
    Ok(())
}

fn write_result(path: &Path, report: ResultReport<'_>) -> Result<(), UpdateError> {
    let payload = serde_json::to_vec(&report)
        .map_err(|error| UpdateError::Process(format!("could not encode result: {error}")))?;
    let name = path
        .file_name()
        .ok_or_else(|| UpdateError::InvalidPath("result file has no file name".into()))?;
    let mut temporary_name = name.to_os_string();
    temporary_name.push(format!(".tmp-{}", std::process::id()));
    let temporary = path
        .parent()
        .ok_or_else(|| UpdateError::InvalidPath("result file has no parent directory".into()))?
        .join(temporary_name);
    let write_result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        use std::io::Write;
        file.write_all(&payload)?;
        file.sync_all()?;
        match std::fs::rename(&temporary, path) {
            Ok(()) => Ok(()),
            Err(_error) if path.exists() => {
                // Windows cannot rename over an existing file. The temporary file is still
                // complete and fsynced, so remove the stale report before replacing it.
                std::fs::remove_file(path)?;
                std::fs::rename(&temporary, path).map_err(UpdateError::from)
            }
            Err(error) => Err(UpdateError::from(error)),
        }
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    write_result
}

#[cfg(target_os = "windows")]
fn relaunch_existing_app(app: &Path) -> Result<(), UpdateError> {
    if !app.is_file() {
        return Err(UpdateError::Process(
            "the previous app executable is missing".into(),
        ));
    }
    Command::new(app)
        .spawn()
        .map(|_| ())
        .map_err(UpdateError::from)
}

#[cfg(target_os = "macos")]
fn relaunch_existing_app(app: &Path) -> Result<(), UpdateError> {
    if !app.is_dir() {
        return Err(UpdateError::Process(
            "the previous app bundle is missing".into(),
        ));
    }
    Command::new("/usr/bin/open")
        .arg(app)
        .spawn()
        .map(|_| ())
        .map_err(UpdateError::from)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn relaunch_existing_app(_app: &Path) -> Result<(), UpdateError> {
    Err(UpdateError::Process(
        "relaunch is unsupported on this platform".into(),
    ))
}

fn is_macos_app_path(app: &Path, home: Option<&Path>) -> bool {
    app == Path::new("/Applications/Auris Studio.app")
        || home.is_some_and(|home| app == home.join("Applications/Auris Studio.app"))
}

fn validate_suffix(path: &Path, suffix: &str) -> Result<(), UpdateError> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if !name.ends_with(suffix) || name.len() == suffix.len() {
        return Err(UpdateError::InvalidPath(format!(
            "unexpected update package name: {}",
            path.display()
        )));
    }
    Ok(())
}

fn validate_file_name(path: &Path, expected: &str) -> Result<(), UpdateError> {
    if path.file_name().and_then(|name| name.to_str()) != Some(expected) {
        return Err(UpdateError::InvalidPath(format!(
            "expected app path to end in {expected}: {}",
            path.display()
        )));
    }
    Ok(())
}

fn verify_package(package: &Path, expected: &str) -> Result<(), UpdateError> {
    let mut file = std::fs::File::open(package)?;
    let mut hasher = Sha256::new();
    // The Windows main thread has a 1 MiB stack. Keep streaming storage on the heap;
    // an inlined stack buffer can overflow before argument parsing or error reporting.
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let count = std::io::Read::read(&mut file, &mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let actual = hex::encode(hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(UpdateError::Process(format!(
            "update package SHA-256 mismatch (expected {expected}, got {actual})"
        )));
    }
    Ok(())
}

fn wait_for_parent(parent_pid: u32) -> Result<(), UpdateError> {
    if cfg!(target_os = "windows") {
        wait_for_parent_windows(parent_pid)
    } else if cfg!(target_os = "macos") {
        let deadline = Instant::now() + MAX_WAIT;
        while Instant::now() < deadline {
            let status = Command::new("/bin/kill")
                .args(["-0", &parent_pid.to_string()])
                .status()?;
            if !status.success() {
                return Ok(());
            }
            thread::sleep(POLL_INTERVAL);
        }
        Err(UpdateError::Process(
            "timed out waiting for Auris to exit".into(),
        ))
    } else {
        Err(UpdateError::Process(
            "parent waiting is unsupported on this platform".into(),
        ))
    }
}

#[cfg(target_os = "windows")]
fn wait_for_parent_windows(parent_pid: u32) -> Result<(), UpdateError> {
    use windows::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    let handle = unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            parent_pid,
        )
    };
    let handle = match handle {
        Ok(handle) => handle,
        Err(error)
            if error.code() == windows::core::HRESULT::from_win32(ERROR_INVALID_PARAMETER.0) =>
        {
            // The parent can exit between the helper starting and OpenProcess. That is the
            // desired state, so continue with the verified package instead of reporting failure.
            return Ok(());
        }
        Err(error) => {
            return Err(UpdateError::Process(format!(
                "could not open parent process: {error}"
            )));
        }
    };
    let result = unsafe { WaitForSingleObject(handle, MAX_WAIT.as_millis() as u32) };
    unsafe {
        CloseHandle(handle).ok();
    }
    if result == WAIT_OBJECT_0 {
        Ok(())
    } else {
        Err(UpdateError::Process(
            "timed out waiting for Auris to exit".into(),
        ))
    }
}

#[cfg(not(target_os = "windows"))]
fn wait_for_parent_windows(_parent_pid: u32) -> Result<(), UpdateError> {
    unreachable!()
}

#[cfg(target_os = "windows")]
fn apply_windows(args: &ApplyArgs) -> Result<(), UpdateError> {
    let install_dir = args.app.parent().expect("validated app parent");
    let status = Command::new(&args.package)
        .args([
            "/VERYSILENT",
            "/SUPPRESSMSGBOXES",
            "/NORESTART",
            "/NORESTARTAPPLICATIONS",
        ])
        .arg(format!("/DIR={}", install_dir.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        return Err(UpdateError::Process(format!(
            "installer exited with {status}"
        )));
    }
    if args.restart {
        Command::new(&args.app).spawn()?;
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn apply_windows(_args: &ApplyArgs) -> Result<(), UpdateError> {
    unreachable!()
}

#[cfg(target_os = "macos")]
fn apply_macos(args: &ApplyArgs) -> Result<(), UpdateError> {
    let suffix = format!("auris-update-{}", std::process::id());
    let mount = env::temp_dir().join(format!("{suffix}-mount"));
    let stage = args
        .app
        .parent()
        .expect("validated app parent")
        .join(format!(".{suffix}.app"));
    let backup = args
        .app
        .parent()
        .expect("validated app parent")
        .join(format!(".{suffix}.backup"));
    std::fs::create_dir(&mount)?;
    let mounted = Command::new("/usr/bin/hdiutil")
        .args(["attach", "-readonly", "-nobrowse", "-mountpoint"])
        .arg(&mount)
        .arg(&args.package)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()?;
    if !mounted.success() {
        let _ = std::fs::remove_dir(&mount);
        return Err(UpdateError::Process(
            "could not mount update disk image".into(),
        ));
    }
    let source = mount.join("Auris Studio.app");
    let result = (|| {
        let _ = std::fs::remove_dir_all(&stage);
        let copy = Command::new("/usr/bin/ditto")
            .arg(&source)
            .arg(&stage)
            .status()?;
        if !copy.success() {
            return Err(UpdateError::Process("could not stage updated app".into()));
        }
        let _ = std::fs::remove_dir_all(&backup);
        std::fs::rename(&args.app, &backup)?;
        if let Err(error) = std::fs::rename(&stage, &args.app) {
            let _ = std::fs::rename(&backup, &args.app);
            return Err(error.into());
        }
        if let Err(error) = std::fs::remove_dir_all(&backup) {
            eprintln!("auris-update-helper: could not remove old app backup: {error}");
        }
        Ok(())
    })();
    if result.is_err() {
        // A failed copy or swap leaves the staged bundle beside the app, hidden by its leading
        // dot. The backup is not touched here: if restoring it failed, it is the only copy left.
        let _ = std::fs::remove_dir_all(&stage);
    }
    let _ = Command::new("/usr/bin/hdiutil")
        .args(["detach", "-force"])
        .arg(&mount)
        .status();
    let _ = std::fs::remove_dir_all(&mount);
    result?;
    if args.restart {
        Command::new("/usr/bin/open").arg(&args.app).spawn()?;
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn apply_macos(_args: &ApplyArgs) -> Result<(), UpdateError> {
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_requires_the_apply_command_and_all_values() {
        assert!(parse_args(vec!["helper".into()]).is_err());
        assert!(
            parse_args(vec![
                "helper".into(),
                "apply".into(),
                "--package".into(),
                "x".into()
            ])
            .is_err()
        );
    }

    #[test]
    fn parser_accepts_the_documented_contract() {
        let directory = std::env::temp_dir();
        let package = directory.join("auris-studio-v1-windows-x86_64-setup.exe");
        let app = directory.join("auris-studio.exe");
        let result_file = directory.join("update-result.json");
        let parsed = parse_args(vec![
            "helper".into(),
            "apply".into(),
            "--parent-pid".into(),
            "42".into(),
            "--package".into(),
            package.into_os_string(),
            "--app".into(),
            app.into_os_string(),
            "--sha256".into(),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
            "--result-file".into(),
            result_file.clone().into_os_string(),
            "--restart".into(),
        ])
        .unwrap();
        assert_eq!(parsed.parent_pid, 42);
        assert!(parsed.restart);
        assert_eq!(parsed.result_file.as_deref(), Some(result_file.as_path()));
        assert_eq!(
            parsed.package.file_name().unwrap(),
            "auris-studio-v1-windows-x86_64-setup.exe"
        );
    }

    #[test]
    fn package_names_must_include_a_version_before_the_platform_suffix() {
        assert!(
            validate_suffix(
                Path::new("auris-studio-v1-windows-x86_64-setup.exe"),
                "-windows-x86_64-setup.exe"
            )
            .is_ok()
        );
        assert!(
            validate_suffix(
                Path::new("windows-x86_64-setup.exe"),
                "-windows-x86_64-setup.exe"
            )
            .is_err()
        );
    }

    #[test]
    fn app_name_is_checked_exactly() {
        assert!(
            validate_file_name(
                Path::new("/Applications/Auris Studio.app"),
                "Auris Studio.app"
            )
            .is_ok()
        );
        assert!(
            validate_file_name(Path::new("/Applications/Other.app"), "Auris Studio.app").is_err()
        );
    }

    #[test]
    fn package_digest_must_match_before_apply() {
        let directory = tempfile::tempdir().unwrap();
        let package = directory.path().join("package.bin");
        let contents = vec![0x5a; 1024 * 1024 + 1];
        std::fs::write(&package, &contents).unwrap();
        let digest = hex::encode(Sha256::digest(&contents));
        assert!(verify_package(&package, &digest).is_ok());
        assert!(verify_package(&package, &"00".repeat(32)).is_err());
        directory.close().unwrap();
    }

    #[test]
    fn result_file_is_json_and_replaces_an_existing_report() {
        let directory = tempfile::tempdir().unwrap();
        let result = directory.path().join("update-result.json");
        write_result(&result, ResultReport::Success).unwrap();
        assert_eq!(
            std::fs::read_to_string(&result).unwrap(),
            r#"{"status":"success"}"#
        );
        write_result(
            &result,
            ResultReport::Error {
                message: "installer exited with code 1",
            },
        )
        .unwrap();
        let parsed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&result).unwrap()).unwrap();
        assert_eq!(parsed["status"], "error");
        assert_eq!(parsed["message"], "installer exited with code 1");
        directory.close().unwrap();
    }

    #[test]
    fn macos_app_path_is_limited_to_the_two_supported_install_locations() {
        let home = Path::new("/Users/tester");
        assert!(is_macos_app_path(
            Path::new("/Applications/Auris Studio.app"),
            Some(home)
        ));
        assert!(is_macos_app_path(
            Path::new("/Users/tester/Applications/Auris Studio.app"),
            Some(home)
        ));
        assert!(!is_macos_app_path(
            Path::new("/Volumes/Other/Applications/Auris Studio.app"),
            Some(home)
        ));
    }
}
