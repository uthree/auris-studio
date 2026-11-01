//! Release discovery, verified download, and installed-app detection.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const LATEST_RELEASE_API: &str = "https://api.github.com/repos/uthree/auris-studio/releases/latest";
pub(crate) const RELEASE_PAGE: &str = "https://github.com/uthree/auris-studio/releases/latest";
const MAX_RESPONSE_BYTES: u64 = 512 * 1024;
const MAX_PACKAGE_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReleaseAsset {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, PartialEq)]
pub(crate) enum UpdateStatus {
    Current(String),
    Available {
        current: String,
        latest: String,
        asset: Option<ReleaseAsset>,
    },
    NoRelease,
}

#[derive(Debug)]
pub(crate) enum CheckError {
    Request(String),
    InvalidResponse(String),
    File(String),
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Request(message) | Self::InvalidResponse(message) | Self::File(message) => {
                formatter.write_str(message)
            }
        }
    }
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    assets: Vec<ReleaseAssetResponse>,
}

#[derive(Deserialize)]
struct ReleaseAssetResponse {
    name: String,
    size: u64,
    digest: Option<String>,
    browser_download_url: String,
}

#[derive(Deserialize)]
struct UpdateResult {
    status: String,
    message: Option<String>,
}

pub(crate) enum UpdateOutcome {
    Success,
    Failure(String),
}

fn update_result_path() -> PathBuf {
    let directory = auris_session::config_dir().join("updates");
    if directory.is_absolute() {
        directory.join("update-result.json")
    } else {
        std::env::current_dir()
            .unwrap_or_default()
            .join(directory)
            .join("update-result.json")
    }
}

/// Reads and removes the result left by the helper after the previous run.
pub(crate) fn take_update_result() -> Option<UpdateOutcome> {
    let path = update_result_path();
    let bytes = fs::read(&path).ok()?;
    if let Err(error) = fs::remove_file(&path) {
        log::warn!("could not clear update result: {error}");
    }
    let result: UpdateResult = match serde_json::from_slice(&bytes) {
        Ok(result) => result,
        Err(error) => return Some(UpdateOutcome::Failure(error.to_string())),
    };
    match result.status.as_str() {
        "success" => Some(UpdateOutcome::Success),
        "error" => Some(UpdateOutcome::Failure(
            result.message.unwrap_or_else(|| "unknown error".into()),
        )),
        _ => Some(UpdateOutcome::Failure("invalid update result".into())),
    }
}

/// Deletes what earlier updates left in the update folder, once per process.
///
/// Every update downloads an installer of some seventy megabytes and copies the helper beside it
/// under the quitting process's id, and neither is needed once the update has run: the installer
/// is for a version no newer than this one, and the helper copy belonged to a process that has
/// gone. Without this the folder grows by one installer per release for ever.
///
/// A package for a *newer* version is kept, because [`download_update`] reuses one that still
/// verifies. A helper copy that is still running cannot be deleted on Windows, and the error is
/// ignored: it will go next launch.
pub(crate) fn prune_update_folder() {
    static PRUNED: AtomicBool = AtomicBool::new(false);
    if PRUNED.swap(true, Ordering::AcqRel) {
        return;
    }
    let Some(directory) = update_result_path().parent().map(Path::to_path_buf) else {
        return;
    };
    let Ok(current) = Version::parse(env!("CARGO_PKG_VERSION")) else {
        return;
    };
    for path in stale_update_files(&directory, &current, std::process::id()) {
        if let Err(error) = fs::remove_file(&path) {
            log::debug!("could not remove {}: {error}", path.display());
        }
    }
}

/// The files in `directory` that [`prune_update_folder`] removes.
fn stale_update_files(directory: &Path, current: &Version, own_pid: u32) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let own_helper = format!("auris-update-helper-{own_pid}");
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter(|entry| {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return false;
            };
            if let Some(rest) = name.strip_prefix("auris-update-helper-") {
                let stem = rest.strip_suffix(".exe").unwrap_or(rest);
                return !stem.is_empty()
                    && stem.bytes().all(|byte| byte.is_ascii_digit())
                    && name.strip_suffix(".exe").unwrap_or(name) != own_helper;
            }
            name.strip_prefix("auris-studio-v")
                .and_then(|rest| {
                    rest.strip_suffix("-windows-x86_64-setup.exe")
                        .or_else(|| rest.strip_suffix("-macos-universal.dmg"))
                })
                .and_then(|version| Version::parse(version).ok())
                .is_some_and(|version| version <= *current)
        })
        .map(|entry| entry.path())
        .collect()
}

/// Leaves a visible failure for the next launch if the helper could not start at all.
pub(crate) fn record_update_launch_failure(error: &CheckError) {
    let result = serde_json::json!({"status":"error","message":error.to_string()});
    if let Err(write_error) = fs::write(update_result_path(), result.to_string()) {
        log::error!("could not save update launch failure: {write_error}");
    }
}

#[derive(Clone, Debug)]
pub(crate) struct InstallTarget {
    /// Installed executable on Windows, installed bundle on macOS.
    pub app: PathBuf,
    /// Small helper shipped inside the installed application.
    pub helper: PathBuf,
}

#[derive(Clone, Debug)]
pub(crate) struct ReadyUpdate {
    pub latest: String,
    pub package: PathBuf,
    pub sha256: String,
    pub target: InstallTarget,
}

pub(crate) fn check_latest_release() -> Result<UpdateStatus, CheckError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .https_only(true)
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into();
    let response = agent
        .get(LATEST_RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .header(
            "User-Agent",
            concat!("auris-studio/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|error| CheckError::Request(error.to_string()))?;
    if response.status().as_u16() == 404 {
        return Ok(UpdateStatus::NoRelease);
    }
    if !response.status().is_success() {
        return Err(CheckError::Request(format!("HTTP {}", response.status())));
    }
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_reader()
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| CheckError::Request(error.to_string()))?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(CheckError::InvalidResponse(
            "release response is too large".into(),
        ));
    }
    compare_release(env!("CARGO_PKG_VERSION"), &bytes)
}

fn compare_release(current: &str, response: &[u8]) -> Result<UpdateStatus, CheckError> {
    let release: Release = serde_json::from_slice(response)
        .map_err(|error| CheckError::InvalidResponse(error.to_string()))?;
    let installed =
        Version::parse(current).map_err(|error| CheckError::InvalidResponse(error.to_string()))?;
    let latest = Version::parse(
        release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name),
    )
    .map_err(|error| CheckError::InvalidResponse(error.to_string()))?;
    if latest <= installed {
        return Ok(UpdateStatus::Current(current.to_owned()));
    }
    let asset = select_asset(&release.assets, &release.tag_name, &latest)?;
    Ok(UpdateStatus::Available {
        current: current.to_owned(),
        latest: latest.to_string(),
        asset,
    })
}

fn select_asset(
    assets: &[ReleaseAssetResponse],
    tag: &str,
    version: &Version,
) -> Result<Option<ReleaseAsset>, CheckError> {
    let suffix = if cfg!(target_os = "windows") {
        "windows-x86_64-setup.exe"
    } else if cfg!(target_os = "macos") {
        "macos-universal.dmg"
    } else {
        return Ok(None);
    };
    let name = format!("auris-studio-v{version}-{suffix}");
    let mut matching = assets.iter().filter(|asset| asset.name == name);
    let Some(asset) = matching.next() else {
        return Ok(None);
    };
    if matching.next().is_some() {
        return Err(CheckError::InvalidResponse(
            "duplicate installer assets".into(),
        ));
    }
    let expected_url =
        format!("https://github.com/uthree/auris-studio/releases/download/{tag}/{name}");
    if asset.browser_download_url != expected_url || tag != format!("v{version}") {
        return Err(CheckError::InvalidResponse(
            "unexpected installer URL".into(),
        ));
    }
    if asset.size == 0 || asset.size > MAX_PACKAGE_BYTES {
        return Err(CheckError::InvalidResponse(
            "installer size is out of range".into(),
        ));
    }
    let Some(digest) = asset
        .digest
        .as_deref()
        .and_then(|value| value.strip_prefix("sha256:"))
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
    else {
        // Older releases may have no digest. Keep the manual release-page path available.
        return Ok(None);
    };
    Ok(Some(ReleaseAsset {
        name,
        url: expected_url,
        size: asset.size,
        sha256: digest.to_ascii_lowercase(),
    }))
}

/// Returns the current installer-backed application, excluding portable archives and dev builds.
pub(crate) fn installed_target() -> Option<InstallTarget> {
    let executable = std::env::current_exe().ok()?;
    let binary_dir = executable.parent()?;
    let helper_name = if cfg!(target_os = "windows") {
        "auris-update-helper.exe"
    } else {
        "auris-update-helper"
    };
    let helper = binary_dir.join(helper_name);
    if !helper.is_file() {
        return None;
    }
    if cfg!(target_os = "windows") {
        if executable.file_name()? != "auris-studio.exe"
            || !binary_dir.join("unins000.exe").is_file()
        {
            return None;
        }
        Some(InstallTarget {
            app: executable,
            helper,
        })
    } else if cfg!(target_os = "macos") {
        let contents = binary_dir.parent()?;
        let app = contents.parent()?;
        if binary_dir.file_name()? != "MacOS"
            || contents.file_name()? != "Contents"
            || app.file_name()? != "Auris Studio.app"
        {
            return None;
        }
        let parent = app.parent()?;
        let user_apps = std::env::var_os("HOME")
            .map(PathBuf::from)?
            .join("Applications");
        if parent != Path::new("/Applications") && parent != user_apps {
            return None;
        }
        // A standard user cannot replace an app in a protected Applications directory.
        // Offer the release page instead of downloading a package that cannot be applied.
        tempfile::NamedTempFile::new_in(parent).ok()?;
        Some(InstallTarget {
            app: app.to_path_buf(),
            helper,
        })
    } else {
        None
    }
}

/// Downloads into private application storage, publishing only after length and digest match.
pub(crate) fn download_update(
    asset: &ReleaseAsset,
    latest: &str,
    target: InstallTarget,
    received: &Arc<AtomicU64>,
    cancelled: &Arc<AtomicBool>,
) -> Result<ReadyUpdate, CheckError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(CheckError::Request("update download canceled".into()));
    }
    let directory = auris_session::config_dir().join("updates");
    fs::create_dir_all(&directory).map_err(|error| CheckError::File(error.to_string()))?;
    let directory = if directory.is_absolute() {
        directory
    } else {
        std::env::current_dir()
            .map_err(|error| CheckError::File(error.to_string()))?
            .join(directory)
    };
    let package = directory.join(&asset.name);
    if package.is_file() && verified_file(&package, asset)? {
        received.store(asset.size, Ordering::Relaxed);
        return Ok(ReadyUpdate {
            latest: latest.to_owned(),
            package,
            sha256: asset.sha256.clone(),
            target,
        });
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(900)))
        .https_only(true)
        .build()
        .into();
    let response = agent
        .get(&asset.url)
        .header(
            "User-Agent",
            concat!("auris-studio/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|error| CheckError::Request(error.to_string()))?;
    if !response.status().is_success() {
        return Err(CheckError::Request(format!("HTTP {}", response.status())));
    }
    let mut source = response.into_body().into_reader().take(asset.size + 1);
    let mut staged = tempfile::NamedTempFile::new_in(&directory)
        .map_err(|error| CheckError::File(error.to_string()))?;
    let mut hasher = Sha256::new();
    let mut bytes = 0_u64;
    let mut block = [0_u8; 64 * 1024];
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(CheckError::Request("update download canceled".into()));
        }
        let count = source
            .read(&mut block)
            .map_err(|error| CheckError::Request(error.to_string()))?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        if bytes > asset.size {
            return Err(CheckError::InvalidResponse(
                "installer exceeded declared size".into(),
            ));
        }
        staged
            .write_all(&block[..count])
            .map_err(|error| CheckError::File(error.to_string()))?;
        hasher.update(&block[..count]);
        received.store(bytes, Ordering::Relaxed);
    }
    if bytes != asset.size || hex::encode(hasher.finalize()) != asset.sha256 {
        return Err(CheckError::InvalidResponse(
            "installer digest or size mismatch".into(),
        ));
    }
    staged
        .as_file()
        .sync_all()
        .map_err(|error| CheckError::File(error.to_string()))?;
    if package.exists() {
        fs::remove_file(&package).map_err(|error| CheckError::File(error.to_string()))?;
    }
    staged
        .persist(&package)
        .map_err(|error| CheckError::File(error.error.to_string()))?;
    Ok(ReadyUpdate {
        latest: latest.to_owned(),
        package,
        sha256: asset.sha256.clone(),
        target,
    })
}

fn verified_file(path: &Path, asset: &ReleaseAsset) -> Result<bool, CheckError> {
    let file = File::open(path).map_err(|error| CheckError::File(error.to_string()))?;
    if file
        .metadata()
        .map_err(|error| CheckError::File(error.to_string()))?
        .len()
        != asset.size
    {
        return Ok(false);
    }
    let mut reader = file;
    let mut block = [0_u8; 64 * 1024];
    let mut digest = Sha256::new();
    loop {
        let count = reader
            .read(&mut block)
            .map_err(|error| CheckError::File(error.to_string()))?;
        if count == 0 {
            break;
        }
        digest.update(&block[..count]);
    }
    Ok(hex::encode(digest.finalize()) == asset.sha256)
}

/// Set once a quit has started the helper, whichever window's quit callback got there first.
///
/// Every project window holds its own update state and its own quit callback, so two windows that
/// both reached Ready would otherwise start two helpers. Both would copy themselves to the same
/// per-process name, and on macOS race each other to swap the same bundle.
static HELPER_STARTED: AtomicBool = AtomicBool::new(false);

/// Starts the shipped helper from outside the installed app after the save guard approves quit.
///
/// Only the first call in a process does anything; later ones return `Ok` without starting a
/// second helper.
pub(crate) fn spawn_update_helper(update: &ReadyUpdate, restart: bool) -> Result<(), CheckError> {
    if HELPER_STARTED.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    let result_file = update_result_path();
    if result_file.exists() {
        fs::remove_file(&result_file).map_err(|error| CheckError::File(error.to_string()))?;
    }
    let helper_name = if cfg!(target_os = "windows") {
        format!("auris-update-helper-{}.exe", std::process::id())
    } else {
        format!("auris-update-helper-{}", std::process::id())
    };
    let copied = update
        .package
        .parent()
        .ok_or_else(|| CheckError::File("missing update folder".into()))?
        .join(helper_name);
    fs::copy(&update.target.helper, &copied)
        .map_err(|error| CheckError::File(error.to_string()))?;
    let mut command = std::process::Command::new(copied);
    command
        .arg("apply")
        .arg("--parent-pid")
        .arg(std::process::id().to_string())
        .arg("--package")
        .arg(&update.package)
        .arg("--app")
        .arg(&update.target.app)
        .arg("--sha256")
        .arg(&update.sha256)
        .arg("--result-file")
        .arg(&result_file)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if restart {
        command.arg("--restart");
    }
    command
        .spawn()
        .map_err(|error| CheckError::File(error.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_and_selects_only_the_expected_installer() {
        let suffix = if cfg!(target_os = "windows") {
            "windows-x86_64-setup.exe"
        } else {
            "macos-universal.dmg"
        };
        let name = format!("auris-studio-v0.10.0-{suffix}");
        let url =
            format!("https://github.com/uthree/auris-studio/releases/download/v0.10.0/{name}");
        let json = format!(
            r#"{{"tag_name":"v0.10.0","assets":[{{"name":"{name}","size":123,"digest":"sha256:{}","browser_download_url":"{url}"}}]}}"#,
            "a".repeat(64)
        );
        let status = compare_release("0.9.0", json.as_bytes()).unwrap();
        if cfg!(target_os = "windows") || cfg!(target_os = "macos") {
            let UpdateStatus::Available {
                asset: Some(asset), ..
            } = status
            else {
                panic!("expected installer");
            };
            assert_eq!(asset.name, name);
            assert_eq!(asset.sha256, "a".repeat(64));
        }
        assert_eq!(
            compare_release("0.10.0", json.as_bytes()).unwrap(),
            UpdateStatus::Current("0.10.0".into())
        );
    }

    #[test]
    fn rejects_unusable_release_metadata() {
        assert!(compare_release("0.9.0", br#"{"tag_name":"latest"}"#).is_err());
        assert!(compare_release("0.9.0", br#"{}"#).is_err());
    }

    #[test]
    fn rejects_a_release_asset_from_an_unexpected_url() {
        let suffix = if cfg!(target_os = "windows") {
            "windows-x86_64-setup.exe"
        } else {
            "macos-universal.dmg"
        };
        let name = format!("auris-studio-v0.10.0-{suffix}");
        let json = format!(
            r#"{{"tag_name":"v0.10.0","assets":[{{"name":"{name}","size":123,"digest":"sha256:{}","browser_download_url":"https://example.org/{name}"}}]}}"#,
            "a".repeat(64)
        );
        if cfg!(target_os = "windows") || cfg!(target_os = "macos") {
            assert!(compare_release("0.9.0", json.as_bytes()).is_err());
        }
    }

    #[test]
    fn release_without_a_digest_still_offers_manual_download() {
        let suffix = if cfg!(target_os = "windows") {
            "windows-x86_64-setup.exe"
        } else {
            "macos-universal.dmg"
        };
        let name = format!("auris-studio-v0.10.0-{suffix}");
        let url =
            format!("https://github.com/uthree/auris-studio/releases/download/v0.10.0/{name}");
        let json = format!(
            r#"{{"tag_name":"v0.10.0","assets":[{{"name":"{name}","size":123,"browser_download_url":"{url}"}}]}}"#
        );
        if cfg!(target_os = "windows") || cfg!(target_os = "macos") {
            assert_eq!(
                compare_release("0.9.0", json.as_bytes()).unwrap(),
                UpdateStatus::Available {
                    current: "0.9.0".into(),
                    latest: "0.10.0".into(),
                    asset: None,
                }
            );
        }
    }

    #[test]
    fn cached_package_requires_matching_digest_and_length() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("setup.exe");
        fs::write(&path, b"expected").unwrap();
        let asset = ReleaseAsset {
            name: "setup.exe".into(),
            url: String::new(),
            size: 8,
            sha256: hex::encode(Sha256::digest(b"expected")),
        };
        assert!(verified_file(&path, &asset).unwrap());
        fs::write(&path, b"tampered").unwrap();
        assert!(!verified_file(&path, &asset).unwrap());
    }

    #[test]
    fn applied_installers_and_old_helpers_are_pruned_but_a_newer_package_is_kept() {
        let folder = tempfile::tempdir().unwrap();
        for name in [
            "auris-studio-v0.17.0-windows-x86_64-setup.exe",
            "auris-studio-v0.18.0-windows-x86_64-setup.exe",
            "auris-studio-v0.19.0-windows-x86_64-setup.exe",
            "auris-studio-v0.16.0-macos-universal.dmg",
            "auris-update-helper-41880.exe",
            "auris-update-helper-28804",
            "auris-update-helper-777.exe",
            "update-result.json",
            "auris-studio-vbogus-windows-x86_64-setup.exe",
            "auris-update-helper-.exe",
            "notes.txt",
        ] {
            fs::write(folder.path().join(name), b"x").unwrap();
        }
        fs::create_dir(folder.path().join("auris-update-helper-1")).unwrap();
        let mut stale: Vec<_> =
            stale_update_files(folder.path(), &Version::parse("0.18.0").unwrap(), 777)
                .into_iter()
                .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
                .collect();
        stale.sort();
        assert_eq!(
            stale,
            [
                "auris-studio-v0.16.0-macos-universal.dmg",
                "auris-studio-v0.17.0-windows-x86_64-setup.exe",
                "auris-studio-v0.18.0-windows-x86_64-setup.exe",
                "auris-update-helper-28804",
                "auris-update-helper-41880.exe",
            ]
        );
    }
}
