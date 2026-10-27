//! Installation and discovery of optional ACP adapter shims.

use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use serde_json::Value;
use tokio::{io::AsyncReadExt, process::Command, runtime::Runtime, time::timeout};

use crate::backends::acp_process::ProcessScope;

const REGISTRY: &str = "https://registry.npmjs.org";
const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// ACP adapters supported by Auris.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcpAdapter {
    /// Codex ACP.
    Codex,
    /// Claude Code ACP.
    Claude,
}
impl AcpAdapter {
    /// All supported adapters.
    pub const ALL: [Self; 2] = [Self::Codex, Self::Claude];
    /// npm package name.
    pub const fn package(self) -> &'static str {
        match self {
            Self::Codex => "@agentclientprotocol/codex-acp",
            Self::Claude => "@agentclientprotocol/claude-agent-acp",
        }
    }
    /// Pinned npm version.
    pub const fn version(self) -> &'static str {
        match self {
            Self::Codex => "2.3.0",
            Self::Claude => "0.97.0",
        }
    }
    /// Display name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
        }
    }
}

/// Why an adapter is unavailable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdapterSetupProblem {
    /// Node is missing.
    NodeMissing,
    /// npm is missing.
    NpmMissing,
    /// Node is too old.
    NodeTooOld,
    /// Other setup error.
    Other(String),
}
/// Current adapter status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterSetupStatus {
    /// Whether the managed adapter is valid.
    pub installed: bool,
    /// Optional setup problem.
    pub problem: Option<AdapterSetupProblem>,
}
/// Installation progress.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdapterInstallEvent {
    /// Preparing.
    Preparing,
    /// Running npm.
    Installing,
    /// Verifying.
    Verifying,
    /// Published.
    Complete,
    /// Cancelled.
    Cancelled,
    /// Failed.
    Failed(String),
}
/// Cancellable background installation.
pub struct AdapterInstall {
    cancel: Arc<AtomicBool>,
    /// Progress events.
    pub events: mpsc::Receiver<AdapterInstallEvent>,
}
impl AdapterInstall {
    /// Cancel the installation.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}
impl Drop for AdapterInstall {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn root() -> PathBuf {
    auris_session::config_dir().join("adapters")
}
fn node() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("AURIS_NODE") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let name = if cfg!(windows) { "node.exe" } else { "node" };
    let mut paths: Vec<PathBuf> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    if cfg!(windows) {
        if let Some(value) = std::env::var_os("ProgramFiles") {
            paths.push(PathBuf::from(value).join("nodejs"));
        }
        if let Some(value) = std::env::var_os("LOCALAPPDATA") {
            paths.push(PathBuf::from(value).join("Programs/nodejs"));
        }
    } else {
        paths.extend([
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/opt/homebrew/bin"),
        ]);
    }
    paths
        .into_iter()
        .map(|path| path.join(name))
        .find(|path| path.is_file())
}
/// Resolve npm's JavaScript CLI, never a shell shim.
fn npm_cli(node: &Path) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(value) = std::env::var_os("AURIS_NPM") {
        candidates.push(PathBuf::from(value));
    }
    if let Some(parent) = node.parent() {
        candidates.push(parent.join("node_modules/npm/bin/npm-cli.js"));
        if let Some(prefix) = parent.parent() {
            candidates.push(prefix.join("lib/node_modules/npm/bin/npm-cli.js"));
            candidates.push(prefix.join("share/nodejs/npm/bin/npm-cli.js"));
        }
    }
    if let Some(value) = std::env::var_os("APPDATA") {
        let value = PathBuf::from(value);
        candidates.push(value.join("npm/node_modules/npm/bin/npm-cli.js"));
        candidates.push(value.join("npm/npm-cli.js"));
    }
    for path in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        candidates.push(path.join("npm-cli.js"));
        candidates.push(path.join("../lib/node_modules/npm/bin/npm-cli.js"));
        candidates.push(path.join("../share/nodejs/npm/bin/npm-cli.js"));
    }
    candidates
        .into_iter()
        .find(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "js"))
}
fn managed(adapter: AcpAdapter) -> PathBuf {
    managed_in(adapter, &root())
}
fn managed_in(adapter: AcpAdapter, base: &Path) -> PathBuf {
    base.join(adapter.name().to_ascii_lowercase().replace(' ', "-"))
        .join(adapter.version())
}
/// Resolve a validated managed adapter entry for the ACP launcher.
pub(crate) fn managed_entry(adapter: AcpAdapter) -> Option<(PathBuf, Vec<String>)> {
    if !valid(adapter) {
        return None;
    }
    let entry = managed(adapter)
        .join("node_modules")
        .join(adapter.package())
        .join("dist/index.js");
    if !entry.is_file() {
        return None;
    }
    Some((node()?, vec![entry.to_string_lossy().into_owned()]))
}
fn valid(adapter: AcpAdapter) -> bool {
    valid_staged(adapter, &managed(adapter))
}
fn unavailable(problem: AdapterSetupProblem) -> AdapterSetupStatus {
    AdapterSetupStatus {
        installed: false,
        problem: Some(problem),
    }
}
fn probe(adapter: AcpAdapter) -> AdapterSetupStatus {
    let Some(path) = node() else {
        return unavailable(AdapterSetupProblem::NodeMissing);
    };
    let runtime = match Runtime::new() {
        Ok(value) => value,
        Err(error) => return unavailable(AdapterSetupProblem::Other(error.to_string())),
    };
    let major = match runtime.block_on(node_version(&path)) {
        Ok(value) => value,
        Err(error) => return unavailable(AdapterSetupProblem::Other(error.to_string())),
    };
    if major < 22 {
        return unavailable(AdapterSetupProblem::NodeTooOld);
    }
    if !valid(adapter) && npm_cli(&path).is_none() {
        return unavailable(AdapterSetupProblem::NpmMissing);
    }
    AdapterSetupStatus {
        installed: valid(adapter),
        problem: None,
    }
}
/// Inspect adapters without network access.
pub fn inspect_acp_adapters_background() -> mpsc::Receiver<(AcpAdapter, AdapterSetupStatus)> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for adapter in AcpAdapter::ALL {
            let _ = tx.send((adapter, probe(adapter)));
        }
    });
    rx
}

async fn node_version(path: &Path) -> io::Result<u32> {
    let mut command = Command::new(path);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    sanitized_environment(&mut command, std::env::temp_dir().as_path());
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut scope = ProcessScope::spawn(command)?;
    let output = scope
        .child_mut()
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("Node stdout unavailable"))?;
    let mut bytes = Vec::new();
    let probe = async {
        output.take(128).read_to_end(&mut bytes).await?;
        let status = scope.wait().await?;
        Ok::<_, io::Error>(status)
    };
    match timeout(PROBE_TIMEOUT, probe).await {
        Ok(result) => {
            let status = result?;
            if !status.success() {
                return Err(io::Error::other(format!("Node exited with {status}")));
            }
        }
        Err(_) => {
            let _ = scope.kill().await;
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Node version probe timed out",
            ));
        }
    }
    String::from_utf8_lossy(&bytes)
        .trim()
        .trim_start_matches('v')
        .split('.')
        .next()
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| io::Error::other("Node returned an invalid version"))
}

/// Install one fixed-version adapter into Auris's private managed directory.
pub fn install_acp_adapter_background(adapter: AcpAdapter) -> AdapterInstall {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    std::thread::spawn(move || {
        let result = Runtime::new()
            .map_err(io::Error::other)
            .and_then(|runtime| runtime.block_on(install(adapter, &root(), &flag, &tx)));
        if let Err(error) = result {
            let _ = tx.send(if error.kind() == io::ErrorKind::Interrupted {
                AdapterInstallEvent::Cancelled
            } else {
                AdapterInstallEvent::Failed(error.to_string())
            });
        }
    });
    AdapterInstall { cancel, events: rx }
}
async fn install(
    adapter: AcpAdapter,
    base: &Path,
    cancelled: &AtomicBool,
    events: &mpsc::Sender<AdapterInstallEvent>,
) -> io::Result<()> {
    send(events, AdapterInstallEvent::Preparing);
    check_cancel(cancelled)?;
    if valid_staged(adapter, &managed_in(adapter, base)) {
        send(events, AdapterInstallEvent::Complete);
        return Ok(());
    }
    let node_path =
        node().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Node.js was not found"))?;
    if node_version(&node_path).await? < 22 {
        return Err(io::Error::other("Node.js 22 or newer is required"));
    }
    let npm_path = npm_cli(&node_path)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "npm's npm-cli.js was not found"))?;
    let _lock = InstallLock::acquire(base, cancelled)?;
    // A second window may have installed the same adapter while this worker waited.
    if valid_staged(adapter, &managed_in(adapter, base)) {
        send(events, AdapterInstallEvent::Complete);
        return Ok(());
    }
    let staging = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(base)?;
    let staging_path = staging.path().to_path_buf();
    fs::create_dir_all(staging_path.join("cache"))?;
    fs::write(staging_path.join("npmrc"), b"")?;
    fs::write(staging_path.join("global-npmrc"), b"")?;
    send(events, AdapterInstallEvent::Installing);
    let mut command = Command::new(&node_path);
    command
        .current_dir(&staging_path)
        .arg(&npm_path)
        .args([
            "install",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
            "--save-exact",
            "--package-lock=true",
            "--registry",
            REGISTRY,
        ])
        .arg(format!("{}@{}", adapter.package(), adapter.version()))
        .args(["--prefix"])
        .arg(&staging_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    sanitized_environment(&mut command, &staging_path);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    run_install_command(command, cancelled, INSTALL_TIMEOUT).await?;
    check_cancel(cancelled)?;
    send(events, AdapterInstallEvent::Verifying);
    if !valid_staged(adapter, &staging_path) {
        return Err(io::Error::other(
            "Installed package version, entry point, or integrity could not be verified",
        ));
    }
    // The cache is private to this attempt and has no role in running the adapter.
    fs::remove_dir_all(staging_path.join("cache"))?;
    check_cancel(cancelled)?;
    publish(adapter, base, &staging_path)?;
    send(events, AdapterInstallEvent::Complete);
    Ok(())
}

async fn run_install_command(
    command: Command,
    cancelled: &AtomicBool,
    limit: Duration,
) -> io::Result<()> {
    check_cancel(cancelled)?;
    #[cfg(windows)]
    let command = {
        let mut command = command;
        command.creation_flags(0x08000000);
        command
    };
    let mut scope = ProcessScope::spawn(command)?;
    let stderr = scope.child_mut().stderr.take();
    let errors = tokio::spawn(async move {
        let mut tail = Vec::new();
        if let Some(mut stderr) = stderr {
            let mut chunk = [0; 2048];
            while let Ok(count) = stderr.read(&mut chunk).await {
                if count == 0 {
                    break;
                }
                tail.extend_from_slice(&chunk[..count]);
                if tail.len() > 4096 {
                    tail.drain(..tail.len() - 4096);
                }
            }
        }
        String::from_utf8_lossy(&tail).trim().to_owned()
    });
    let deadline = Instant::now() + limit;
    loop {
        if cancelled.load(Ordering::Acquire) {
            let _ = scope.kill().await;
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "adapter installation cancelled",
            ));
        }
        if Instant::now() >= deadline {
            let _ = scope.kill().await;
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "npm installation timed out",
            ));
        }
        if let Some(status) = scope.child_mut().try_wait()? {
            // Also close descendants before awaiting their inherited stderr handles.
            drop(scope);
            let detail = timeout(Duration::from_secs(2), errors)
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default();
            if !status.success() {
                return Err(io::Error::other(format!(
                    "npm exited with {status}. {detail}"
                )));
            }
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
fn sanitized_environment(command: &mut Command, staging: &Path) {
    command.env_clear();
    for key in [
        "PATH",
        "HOME",
        "USERPROFILE",
        "TEMP",
        "TMP",
        "SystemRoot",
        "ComSpec",
        "APPDATA",
        "LOCALAPPDATA",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
        .env("NPM_CONFIG_USERCONFIG", staging.join("npmrc"))
        .env("NPM_CONFIG_GLOBALCONFIG", staging.join("global-npmrc"))
        .env("NPM_CONFIG_CACHE", staging.join("cache"))
        .env("NPM_CONFIG_PREFIX", staging)
        .env("NPM_CONFIG_GLOBAL", "false")
        .env("NPM_CONFIG_IGNORE_SCRIPTS", "true")
        .env("NPM_CONFIG_AUDIT", "false")
        .env("NPM_CONFIG_FUND", "false")
        .env_remove("NODE_OPTIONS")
        .env_remove("NODE_PATH")
        .env_remove("NPM_CONFIG_REGISTRY");
}
fn send(events: &mpsc::Sender<AdapterInstallEvent>, event: AdapterInstallEvent) {
    let _ = events.send(event);
}
fn check_cancel(cancelled: &AtomicBool) -> io::Result<()> {
    if cancelled.load(Ordering::Acquire) {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "adapter installation cancelled",
        ))
    } else {
        Ok(())
    }
}

struct InstallLock {
    file: File,
}
impl InstallLock {
    fn acquire(base: &Path, cancelled: &AtomicBool) -> io::Result<Self> {
        fs::create_dir_all(base)?;
        let path = base.join(".install.lock");
        let deadline = Instant::now() + INSTALL_TIMEOUT;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        loop {
            check_cancel(cancelled)?;
            match file.try_lock() {
                Ok(()) => return Ok(Self { file }),
                Err(std::fs::TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "another adapter installation is still running",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(std::fs::TryLockError::Error(error)) => return Err(error),
            }
        }
    }
}
impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}
fn publish(adapter: AcpAdapter, base: &Path, staging: &Path) -> io::Result<()> {
    let target = managed_in(adapter, base);
    let parent = target
        .parent()
        .ok_or_else(|| io::Error::other("adapter target has no parent"))?;
    fs::create_dir_all(parent)?;
    if target.exists() && valid_staged(adapter, &target) {
        return Ok(());
    }
    let backup = parent.join(format!(".invalid-{}", uuid::Uuid::new_v4()));
    if target.exists() {
        fs::rename(&target, &backup)?;
    }
    match fs::rename(staging, &target) {
        Ok(()) => {
            let _ = fs::remove_dir_all(backup);
            Ok(())
        }
        Err(error) => {
            if backup.exists() {
                let _ = fs::rename(&backup, &target);
            }
            Err(error)
        }
    }
}
fn expected_integrity(adapter: AcpAdapter) -> &'static str {
    match adapter {
        AcpAdapter::Codex => {
            "sha512-VdTYaOGXqLh/aVuAKIgFG7BIzbeq537FHGo28kgBhybDbeOdFVFTtc43k/tCpScFhZyqPQWHuWf8w/QQd0qtXQ=="
        }
        AcpAdapter::Claude => {
            "sha512-4bSqO6eb8PjxHTtKsXvjqmsk0qeYfxSNwIG58mn6+7U7XOqHSLMvJ8aR4LeJIdNz1sSlzYniPXxO0DEs29SCPw=="
        }
    }
}
fn valid_staged(adapter: AcpAdapter, path: &Path) -> bool {
    let package_dir = path.join("node_modules").join(adapter.package());
    let version_ok = fs::read_to_string(package_dir.join("package.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| {
            v["version"]
                .as_str()
                .map(|version| version == adapter.version())
        })
        .unwrap_or(false);
    let entry_ok = package_dir.join("dist/index.js").is_file();
    let integrity_ok = fs::read_to_string(path.join("package-lock.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| {
            v["packages"][format!("node_modules/{}", adapter.package())]["integrity"]
                .as_str()
                .map(|integrity| integrity == expected_integrity(adapter))
        })
        .unwrap_or(false);
    version_ok && entry_ok && integrity_ok
}

#[cfg(test)]
mod process_tests;

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(root: &Path, adapter: AcpAdapter, integrity: &str) {
        let dir = root.join("node_modules").join(adapter.package());
        fs::create_dir_all(dir.join("dist")).unwrap();
        fs::write(
            dir.join("package.json"),
            format!(r#"{{"version":"{}"}}"#, adapter.version()),
        )
        .unwrap();
        fs::write(dir.join("dist/index.js"), "fixture").unwrap();
        fs::write(root.join("package-lock.json"), serde_json::json!({"packages": {format!("node_modules/{}", adapter.package()): {"integrity": integrity}}}).to_string()).unwrap();
    }
    #[test]
    fn valid_fixture_requires_entry_and_sri() {
        let temp = tempfile::tempdir().unwrap();
        fixture(
            temp.path(),
            AcpAdapter::Codex,
            expected_integrity(AcpAdapter::Codex),
        );
        assert!(valid_staged(AcpAdapter::Codex, temp.path()));
        fs::remove_file(
            temp.path()
                .join("node_modules/@agentclientprotocol/codex-acp/dist/index.js"),
        )
        .unwrap();
        assert!(!valid_staged(AcpAdapter::Codex, temp.path()));
    }
    #[test]
    fn duplicate_lock_is_released_by_raii() {
        let temp = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let lock = InstallLock::acquire(temp.path(), &cancel).unwrap();
        let second = OpenOptions::new()
            .read(true)
            .write(true)
            .open(temp.path().join(".install.lock"))
            .unwrap();
        assert!(matches!(
            second.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        assert!(InstallLock::acquire(temp.path(), &AtomicBool::new(true)).is_err());
        drop(lock);
        assert!(InstallLock::acquire(temp.path(), &cancel).is_ok());
    }

    /// Opt-in end-to-end installer check. It uses a disposable config root and never touches an
    /// Auris account or starts an agent session; run it manually when Node/npm network access is
    /// available with `AURIS_ADAPTER_SMOKE_DIR=<empty-temp-dir> cargo test -p auris-agent actual_adapter_install_smoke -- --ignored`.
    #[test]
    #[ignore = "downloads the pinned adapter from the official npm registry"]
    fn actual_adapter_install_smoke() {
        let base = PathBuf::from(
            std::env::var_os("AURIS_ADAPTER_SMOKE_DIR").expect("set an isolated smoke directory"),
        )
        .join("adapters");
        let runtime = Runtime::new().unwrap();
        for adapter in AcpAdapter::ALL {
            let (tx, rx) = mpsc::channel();
            let result = runtime.block_on(install(adapter, &base, &AtomicBool::new(false), &tx));
            let events: Vec<_> = rx.try_iter().collect();
            assert!(result.is_ok(), "{adapter:?}: {result:?}; {events:?}");
            assert!(
                matches!(events.last(), Some(AdapterInstallEvent::Complete)),
                "{adapter:?} installer events: {events:?}"
            );
            runtime.block_on(async {
                use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
                let entry = managed_in(adapter, &base).join("node_modules").join(adapter.package()).join("dist/index.js");
                let mut command = Command::new(node().unwrap());
                command.arg(entry).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
                sanitized_environment(&mut command, &base);
                #[cfg(windows)]
                command.creation_flags(0x08000000);
                let mut process = ProcessScope::spawn(command).unwrap();
                process.child_mut().stdin.as_mut().unwrap().write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":1,\"clientCapabilities\":{},\"clientInfo\":{\"name\":\"auris-install-test\",\"version\":\"1\"}}}\n").await.unwrap();
                let mut lines = BufReader::new(process.child_mut().stdout.take().unwrap()).lines();
                let response = timeout(Duration::from_secs(20), async {
                    while let Some(line) = lines.next_line().await.unwrap() {
                        let value: Value = serde_json::from_str(&line).unwrap();
                        if value["id"] == 1 { return value; }
                    }
                    panic!("adapter exited before initialize");
                }).await.unwrap();
                let _ = process.kill().await;
                assert!(response.get("error").is_none(), "{response}");
                assert_eq!(response["result"]["protocolVersion"], 1);
                eprintln!("{adapter:?}: installed and initialized {}", response["result"]["agentInfo"]);
            });
        }
    }

    #[test]
    fn rejected_package_metadata_never_becomes_ready() {
        let temp = tempfile::tempdir().unwrap();
        for adapter in AcpAdapter::ALL {
            fixture(temp.path(), adapter, "sha512-unexpected");
            assert!(!valid_staged(adapter, temp.path()));
            fixture(temp.path(), adapter, expected_integrity(adapter));
            fs::write(
                temp.path()
                    .join("node_modules")
                    .join(adapter.package())
                    .join("package.json"),
                r#"{"version":"0.0.0"}"#,
            )
            .unwrap();
            assert!(!valid_staged(adapter, temp.path()));
        }
    }

    #[test]
    fn publish_preserves_working_install_and_rolls_back_failed_repair() {
        let temp = tempfile::tempdir().unwrap();
        let target = managed_in(AcpAdapter::Codex, temp.path());
        fixture(
            &target,
            AcpAdapter::Codex,
            expected_integrity(AcpAdapter::Codex),
        );
        let missing = temp.path().join("missing-stage");
        publish(AcpAdapter::Codex, temp.path(), &missing).unwrap();
        assert!(valid_staged(AcpAdapter::Codex, &target));
        fs::write(target.join("preserve-me"), "original").unwrap();
        fs::remove_file(target.join("package-lock.json")).unwrap();
        assert!(publish(AcpAdapter::Codex, temp.path(), &missing).is_err());
        assert_eq!(
            fs::read_to_string(target.join("preserve-me")).unwrap(),
            "original"
        );
        let staging = tempfile::tempdir_in(temp.path()).unwrap();
        fixture(
            staging.path(),
            AcpAdapter::Codex,
            expected_integrity(AcpAdapter::Codex),
        );
        publish(AcpAdapter::Codex, temp.path(), staging.path()).unwrap();
        assert!(valid_staged(AcpAdapter::Codex, &target));
    }

    #[tokio::test]
    async fn cancelled_install_does_not_touch_the_existing_directory() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("existing"), "unchanged").unwrap();
        let (events, _) = mpsc::channel();
        let error = install(
            AcpAdapter::Codex,
            temp.path(),
            &AtomicBool::new(true),
            &events,
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }
}
