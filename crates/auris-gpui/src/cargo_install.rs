//! Copies installed with `cargo install`: recognising one, the command that upgrades it, and the
//! application shortcut Cargo itself never makes.
//!
//! Cargo puts executables in its `bin` directory and records each install in `.crates2.json`
//! beside it, and that is all it does. Without a bundle around it macOS treats `auris-studio` as
//! a tool (no Dock icon of its own, a Terminal window on double-click), and Windows and Linux
//! list nothing in their application menus. The in-app updater cannot help either: it replaces
//! installers, and a Cargo copy was not made by one, so for this copy an update is a command the
//! person runs in a terminal — and the window should say which one.
//!
//! The shortcut points back at Cargo's copy wherever the platform allows it, so a later
//! `cargo install` updates what the shortcut opens without anybody re-creating it:
//!
//! * **Windows** — a `.lnk` in the user's Start menu whose target is the installed executable.
//! * **Linux** — a `.desktop` entry in `$XDG_DATA_HOME/applications`, likewise.
//! * **macOS** — `~/Applications/Auris Studio.app`. A bundle's identity (its name in the menu bar,
//!   its icon, the microphone sentence) is the bundle its executable is *in*, so the executable
//!   is a copy rather than a link. The bundle records which copy it was made from, and a launch
//!   that finds that one newer copies it in and starts it instead ([`refresh_app_bundle`]).
//!
//! Every shortcut carries a marker, and one without it is never replaced: an `Auris Studio.app`
//! in `~/Applications` may be the released application.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// The repository the released versions are tagged in.
const REPOSITORY: &str = "https://github.com/uthree/auris-studio";

/// The packages of this workspace that `cargo install` can install, in the order they are
/// installed: the desktop first, because it is the one asking.
const PACKAGES: [&str; 3] = ["auris-gpui", "auris-cli", "auris-mcp"];

/// The file inside a shortcut bundle naming the Cargo executable it was copied from.
const BUNDLE_SOURCE: &str = "auris-cargo-source";

/// The line marking a desktop entry as this module's.
const DESKTOP_MARKER: &str = "X-Auris-Cargo-Shortcut=true";

/// Where a Cargo copy was installed from, which decides how it is upgraded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InstallSource {
    /// `cargo install --git …`: upgraded by naming the release's tag.
    Git,
    /// `cargo install --path …`: upgraded by pulling that checkout and installing it again.
    Path {
        /// The workspace the package was installed from.
        workspace: PathBuf,
    },
    /// A registry such as crates.io.
    Registry,
}

/// A copy of Auris Studio installed with `cargo install`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CargoInstall {
    /// Cargo's install root, the directory holding `bin` and `.crates2.json`.
    pub root: PathBuf,
    /// The installed `auris-studio` executable.
    pub executable: PathBuf,
    /// Where it came from.
    pub source: InstallSource,
    /// Workspace packages installed into the same root, desktop first.
    pub packages: Vec<String>,
}

/// Whether the platform's shortcut exists, refreshed when [`create_shortcut`] makes one.
static SHORTCUT_PRESENT: AtomicBool = AtomicBool::new(false);

/// The Cargo installation this process belongs to, found once.
///
/// A process started from a shortcut bundle belongs to the Cargo copy the bundle was made from.
pub(crate) fn current() -> Option<&'static CargoInstall> {
    static CURRENT: OnceLock<Option<CargoInstall>> = OnceLock::new();
    CURRENT
        .get_or_init(|| {
            let executable = std::env::current_exe().ok()?;
            let executable = bundle_source(&executable).unwrap_or(executable);
            let install = detect(&executable)?;
            SHORTCUT_PRESENT.store(shortcut_exists(), Ordering::Release);
            Some(install)
        })
        .as_ref()
}

/// Whether the start screen should offer to make the shortcut.
pub(crate) fn offers_shortcut() -> bool {
    current().is_some() && !SHORTCUT_PRESENT.load(Ordering::Acquire)
}

/// Reads the installation `executable` belongs to, or `None` when Cargo did not install it.
fn detect(executable: &Path) -> Option<CargoInstall> {
    let bin = executable.parent()?;
    if bin.file_name()? != "bin" {
        return None;
    }
    let root = bin.parent()?;
    let manifest = fs::read(root.join(".crates2.json")).ok()?;
    let name = executable.file_name()?.to_str()?;
    parse_manifest(root, executable, name, &manifest)
}

/// [`detect`] after the file reads, so it can be tested on any machine.
fn parse_manifest(
    root: &Path,
    executable: &Path,
    executable_name: &str,
    manifest: &[u8],
) -> Option<CargoInstall> {
    #[derive(serde::Deserialize)]
    struct Manifest {
        installs: std::collections::BTreeMap<String, Entry>,
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        #[serde(default)]
        bins: Vec<String>,
    }
    let manifest: Manifest = serde_json::from_slice(manifest).ok()?;
    let bin_name = executable_name
        .strip_suffix(".exe")
        .unwrap_or(executable_name);
    let mut source = None;
    let mut installed = Vec::new();
    for (key, entry) in &manifest.installs {
        // "<package> <version> (<source>)"
        let mut parts = key.splitn(3, ' ');
        let (Some(package), Some(_version), Some(origin)) =
            (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if !PACKAGES.contains(&package) {
            continue;
        }
        let Some(origin) = origin.strip_prefix('(').and_then(|o| o.strip_suffix(')')) else {
            continue;
        };
        if package == PACKAGES[0] {
            if !entry
                .bins
                .iter()
                .any(|bin| bin.strip_suffix(".exe").unwrap_or(bin) == bin_name)
            {
                return None;
            }
            source = Some(parse_source(origin)?);
        }
        installed.push(package.to_owned());
    }
    let source = source?;
    installed.sort_by_key(|package| PACKAGES.iter().position(|known| known == package));
    Some(CargoInstall {
        root: root.to_path_buf(),
        executable: executable.to_path_buf(),
        source,
        packages: installed,
    })
}

/// Reads a source id such as `git+https://…?tag=v0.18.0#abc` or `path+file:///…`.
fn parse_source(origin: &str) -> Option<InstallSource> {
    if origin.starts_with("git+") {
        Some(InstallSource::Git)
    } else if let Some(url) = origin.strip_prefix("path+") {
        let crate_dir = file_url_path(url)?;
        // `--path crates/auris-gpui` from the workspace root is what the documentation says.
        let workspace = if crate_dir.ends_with(Path::new("crates").join(PACKAGES[0])) {
            crate_dir.parent()?.parent()?.to_path_buf()
        } else {
            crate_dir
        };
        Some(InstallSource::Path { workspace })
    } else if origin.starts_with("registry+") || origin.starts_with("sparse+") {
        Some(InstallSource::Registry)
    } else {
        None
    }
}

/// The path a `file://` URL names, on the platform that wrote it.
fn file_url_path(url: &str) -> Option<PathBuf> {
    let parsed = url::Url::parse(url).ok()?;
    if parsed.scheme() != "file" {
        return None;
    }
    if let Ok(path) = parsed.to_file_path() {
        return Some(path);
    }
    // `to_file_path` refuses a URL written on the other platform, which only a test sees.
    let decoded = percent_decode(parsed.path());
    let trimmed = match decoded.as_bytes() {
        [b'/', drive, b':', ..] if drive.is_ascii_alphabetic() => decoded[1..].to_owned(),
        _ => decoded,
    };
    Some(PathBuf::from(trimmed))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(byte) = bytes
                .get(index + 1..index + 3)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Cargo's install root when nothing is said about one.
fn default_root() -> Option<PathBuf> {
    let set = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    set("CARGO_INSTALL_ROOT")
        .or_else(|| set("CARGO_HOME"))
        .map(PathBuf::from)
        .or_else(|| home().map(|home| home.join(".cargo")))
}

fn home() -> Option<PathBuf> {
    let name = if cfg!(target_os = "windows") {
        "USERPROFILE"
    } else {
        "HOME"
    };
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Quotes a path for the terminal the command will be pasted into.
fn quoted(path: &Path) -> String {
    format!("\"{}\"", path.display())
}

impl CargoInstall {
    /// The terminal command that installs `latest` over this copy, one step per line so it
    /// pastes into PowerShell as well as into a POSIX shell.
    pub(crate) fn upgrade_command(&self, latest: &str) -> String {
        self.upgrade_command_with_default_root(latest, default_root().as_deref())
    }

    fn upgrade_command_with_default_root(&self, latest: &str, default: Option<&Path>) -> String {
        let root = if default == Some(self.root.as_path()) {
            String::new()
        } else {
            format!(" --root {}", quoted(&self.root))
        };
        let packages = if self.packages.is_empty() {
            vec![PACKAGES[0].to_owned()]
        } else {
            self.packages.clone()
        };
        match &self.source {
            InstallSource::Git => format!(
                "cargo install --locked{root} --git {REPOSITORY} --tag v{latest} {}",
                packages.join(" ")
            ),
            InstallSource::Registry => {
                format!("cargo install --locked{root} {}", packages.join(" "))
            }
            InstallSource::Path { workspace } => {
                let mut lines = vec![
                    format!("cd {}", quoted(workspace)),
                    "git pull --ff-only".to_owned(),
                ];
                lines.extend(packages.iter().map(|package| {
                    format!("cargo install --locked{root} --path crates/{package}")
                }));
                lines.join("\n")
            }
        }
    }
}

// ------------------------------------------------------------------ the shortcut

/// Where the platform's shortcut goes.
pub(crate) fn shortcut_path() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        Some(home()?.join("Applications").join("Auris Studio.app"))
    } else if cfg!(target_os = "windows") {
        let roaming = std::env::var_os("APPDATA").filter(|value| !value.is_empty())?;
        Some(
            PathBuf::from(roaming)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
                .join("Auris Studio.lnk"),
        )
    } else {
        Some(
            xdg_data_home()?
                .join("applications")
                .join("auris-studio.desktop"),
        )
    }
}

fn xdg_data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home().map(|home| home.join(".local").join("share")))
}

fn shortcut_exists() -> bool {
    shortcut_path().is_some_and(|path| path.exists())
}

/// Why a shortcut could not be made.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ShortcutError {
    /// Something not made here is already in the shortcut's place.
    Occupied(PathBuf),
    /// The platform's directory could not be found.
    NoLocation,
    /// A file operation or a helper program failed.
    Failed(String),
}

impl std::fmt::Display for ShortcutError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Occupied(path) => write!(formatter, "{} already exists", path.display()),
            Self::NoLocation => formatter.write_str("no home directory"),
            Self::Failed(message) => formatter.write_str(message),
        }
    }
}

fn failed(error: impl std::fmt::Display) -> ShortcutError {
    ShortcutError::Failed(error.to_string())
}

/// Makes (or remakes) the shortcut for `install`, returning where it is.
pub(crate) fn create_shortcut(install: &CargoInstall) -> Result<PathBuf, ShortcutError> {
    let path = shortcut_path().ok_or(ShortcutError::NoLocation)?;
    if cfg!(target_os = "macos") {
        write_app_bundle(&path, &install.executable)?;
    } else if cfg!(target_os = "windows") {
        write_windows_shortcut(&path, &install.executable)?;
    } else {
        write_desktop_entry(&path, &install.executable)?;
    }
    SHORTCUT_PRESENT.store(true, Ordering::Release);
    Ok(path)
}

/// The Info.plist the release bundle carries, with this build's version.
fn bundle_info_plist() -> String {
    let mut plist = include_str!("../macos/Info.plist").to_owned();
    for key in ["CFBundleShortVersionString", "CFBundleVersion"] {
        plist = with_plist_string(&plist, key, env!("CARGO_PKG_VERSION"));
    }
    plist
}

/// `plist` with the string following `<key>{key}</key>` replaced by `value`.
fn with_plist_string(plist: &str, key: &str, value: &str) -> String {
    let marker = format!("<key>{key}</key>");
    let Some(key_at) = plist.find(&marker) else {
        return plist.to_owned();
    };
    let after = key_at + marker.len();
    let Some(open) = plist[after..].find("<string>").map(|at| after + at + 8) else {
        return plist.to_owned();
    };
    let Some(close) = plist[open..].find("</string>").map(|at| open + at) else {
        return plist.to_owned();
    };
    format!("{}{value}{}", &plist[..open], &plist[close..])
}

/// Builds the bundle beside `bundle` and swaps it in, so a failure leaves the old one whole.
fn write_app_bundle(bundle: &Path, executable: &Path) -> Result<(), ShortcutError> {
    if bundle.exists() && bundle_source(&bundle.join("Contents/MacOS/auris-studio")).is_none() {
        return Err(ShortcutError::Occupied(bundle.to_path_buf()));
    }
    let parent = bundle.parent().ok_or(ShortcutError::NoLocation)?;
    fs::create_dir_all(parent).map_err(failed)?;
    let staging = tempfile::Builder::new()
        .prefix(".auris-studio-bundle-")
        .tempdir_in(parent)
        .map_err(failed)?;
    let staged = staging.path().join("Auris Studio.app");
    let contents = staged.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    fs::create_dir_all(&macos).map_err(failed)?;
    fs::create_dir_all(&resources).map_err(failed)?;
    fs::write(contents.join("Info.plist"), bundle_info_plist()).map_err(failed)?;
    fs::write(
        resources.join("auris-studio.icns"),
        include_bytes!("../assets/auris-studio.icns"),
    )
    .map_err(failed)?;
    fs::write(
        resources.join(BUNDLE_SOURCE),
        executable.as_os_str().as_encoded_bytes(),
    )
    .map_err(failed)?;
    fs::copy(executable, macos.join("auris-studio")).map_err(failed)?;
    sign_bundle(&staged);
    if bundle.exists() {
        fs::remove_dir_all(bundle).map_err(failed)?;
    }
    fs::rename(&staged, bundle).map_err(failed)?;
    register_bundle(bundle);
    Ok(())
}

/// Seals the bundle with an ad-hoc signature, without which Apple silicon refuses a bundle whose
/// executable's signature no longer matches its surroundings. Best effort: `codesign` ships with
/// the Command Line Tools, which a Cargo build needed anyway.
fn sign_bundle(bundle: &Path) {
    let status = std::process::Command::new("codesign")
        .args(["--force", "--sign", "-"])
        .arg(bundle)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if !status.is_ok_and(|status| status.success()) {
        log::warn!("could not sign {}", bundle.display());
    }
}

/// Tells Launch Services about the bundle now rather than when Finder next looks.
fn register_bundle(bundle: &Path) {
    const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/\
                              LaunchServices.framework/Support/lsregister";
    let _ = std::process::Command::new(LSREGISTER)
        .arg("-f")
        .arg(bundle)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// The Cargo executable a shortcut bundle's `executable` was copied from, or `None` when it is
/// not in such a bundle.
fn bundle_source(executable: &Path) -> Option<PathBuf> {
    let contents = executable.parent()?.parent()?;
    let recorded = fs::read(contents.join("Resources").join(BUNDLE_SOURCE)).ok()?;
    let text = String::from_utf8(recorded).ok()?;
    let source = PathBuf::from(text.trim_end());
    source.is_absolute().then_some(source)
}

/// On macOS, before anything else runs: when this process is a shortcut bundle's copy and the
/// Cargo executable it was made from has been reinstalled since, copies the new one in and
/// becomes it. Returns only when there is nothing to do or the swap failed, in which case this
/// copy simply carries on.
pub(crate) fn refresh_app_bundle() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let Ok(own) = std::env::current_exe() else {
        return;
    };
    let Some(source) = bundle_source(&own) else {
        return;
    };
    if !is_newer(&source, &own) {
        return;
    }
    let Some(bundle) = own.parent().and_then(Path::parent).and_then(Path::parent) else {
        return;
    };
    let staged = own.with_file_name(".auris-studio-update");
    let swapped = fs::copy(&source, &staged).and_then(|_| fs::rename(&staged, &own));
    if let Err(error) = swapped {
        let _ = fs::remove_file(&staged);
        eprintln!("could not refresh {}: {error}", bundle.display());
        return;
    }
    let plist = bundle.join("Contents").join("Info.plist");
    if let Ok(text) = fs::read_to_string(&plist)
        && let Some(version) = installed_version(&source)
    {
        let mut updated = text;
        for key in ["CFBundleShortVersionString", "CFBundleVersion"] {
            updated = with_plist_string(&updated, key, &version);
        }
        let _ = fs::write(&plist, updated);
    }
    sign_bundle(bundle);
    reexec(&own);
}

/// The version `.crates2.json` records for the desktop package installed at `executable`.
fn installed_version(executable: &Path) -> Option<String> {
    let root = executable.parent()?.parent()?;
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(".crates2.json")).ok()?).ok()?;
    manifest["installs"].as_object()?.keys().find_map(|key| {
        let mut parts = key.split(' ');
        (parts.next()? == PACKAGES[0]).then(|| parts.next().map(str::to_owned))?
    })
}

/// Whether `source` differs from the copy at `copy` in a way a reinstall would cause.
fn is_newer(source: &Path, copy: &Path) -> bool {
    let (Ok(source), Ok(copy)) = (fs::metadata(source), fs::metadata(copy)) else {
        return false;
    };
    match (source.modified(), copy.modified()) {
        (Ok(source_time), Ok(copy_time)) => source_time > copy_time || source.len() != copy.len(),
        _ => source.len() != copy.len(),
    }
}

#[cfg(unix)]
fn reexec(executable: &Path) {
    use std::os::unix::process::CommandExt;
    let error = std::process::Command::new(executable)
        .args(std::env::args_os().skip(1))
        .exec();
    eprintln!("could not restart {}: {error}", executable.display());
}

#[cfg(not(unix))]
fn reexec(_: &Path) {}

/// A Start menu shortcut, written by the shell's own COM object through PowerShell. The paths
/// travel as environment variables, so no quoting rule of PowerShell's can mangle them.
fn write_windows_shortcut(link: &Path, executable: &Path) -> Result<(), ShortcutError> {
    // An existing link is ours when it opens this executable; the installer's opens its own.
    const SCRIPT: &str = "$ErrorActionPreference = 'Stop'; \
        $shell = New-Object -ComObject WScript.Shell; \
        if (Test-Path -LiteralPath $env:AURIS_LINK) { \
            $old = $shell.CreateShortcut($env:AURIS_LINK).TargetPath; \
            if ($old -and ($old -ne $env:AURIS_TARGET)) { exit 3 } \
        }; \
        $link = $shell.CreateShortcut($env:AURIS_LINK); \
        $link.TargetPath = $env:AURIS_TARGET; \
        $link.WorkingDirectory = $env:AURIS_HOME; \
        $link.IconLocation = $env:AURIS_TARGET + ',0'; \
        $link.Description = 'Auris Studio'; \
        $link.Save()";
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent).map_err(failed)?;
    }
    let mut command = std::process::Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .env("AURIS_LINK", link)
        .env("AURIS_TARGET", executable)
        .env("AURIS_HOME", home().unwrap_or_default())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    hide_console(&mut command);
    let output = command.output().map_err(failed)?;
    match output.status.code() {
        Some(0) => Ok(()),
        Some(3) => Err(ShortcutError::Occupied(link.to_path_buf())),
        _ => Err(ShortcutError::Failed(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        )),
    }
}

/// Keeps PowerShell's console from flashing up over the window.
#[cfg(windows)]
fn hide_console(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_: &mut std::process::Command) {}

/// The desktop entry for `executable`, with the icon at `icon`.
fn desktop_entry(executable: &Path, icon: &Path) -> String {
    // The Exec key's own quoting: a double-quoted argument with `"`, `` ` ``, `$` and `\`
    // escaped.
    let exec = executable
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$");
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Auris Studio\n\
         Comment=Digital audio workstation\n\
         Exec=\"{exec}\" %f\n\
         Icon={}\n\
         Terminal=false\n\
         Categories=AudioVideo;Audio;Music;\n\
         StartupWMClass=auris-studio\n\
         {DESKTOP_MARKER}\n",
        icon.display()
    )
}

fn write_desktop_entry(entry: &Path, executable: &Path) -> Result<(), ShortcutError> {
    if entry.exists()
        && !fs::read_to_string(entry).is_ok_and(|text| text.lines().any(|l| l == DESKTOP_MARKER))
    {
        return Err(ShortcutError::Occupied(entry.to_path_buf()));
    }
    let data = xdg_data_home().ok_or(ShortcutError::NoLocation)?;
    let icon = data
        .join("icons")
        .join("hicolor")
        .join("512x512")
        .join("apps")
        .join("auris-studio.png");
    fs::create_dir_all(icon.parent().ok_or(ShortcutError::NoLocation)?).map_err(failed)?;
    fs::write(&icon, include_bytes!("../assets/auris-studio.png")).map_err(failed)?;
    fs::create_dir_all(entry.parent().ok_or(ShortcutError::NoLocation)?).map_err(failed)?;
    fs::write(entry, desktop_entry(executable, &icon)).map_err(failed)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(entries: &[(&str, &str)]) -> Vec<u8> {
        let installs: serde_json::Map<String, serde_json::Value> = entries
            .iter()
            .map(|(key, bin)| ((*key).to_owned(), serde_json::json!({ "bins": [bin] })))
            .collect();
        serde_json::to_vec(&serde_json::json!({ "installs": installs })).unwrap()
    }

    fn install(source: InstallSource, packages: &[&str]) -> CargoInstall {
        CargoInstall {
            root: PathBuf::from("/home/me/.cargo"),
            executable: PathBuf::from("/home/me/.cargo/bin/auris-studio"),
            source,
            packages: packages
                .iter()
                .map(|package| (*package).to_owned())
                .collect(),
        }
    }

    #[test]
    fn a_path_install_names_its_workspace_and_the_packages_beside_it() {
        let bytes = manifest(&[
            (
                "ripgrep 14.1.0 (registry+https://github.com/rust-lang/crates.io-index)",
                "rg",
            ),
            (
                "auris-mcp 0.18.0 (path+file:///Users/me/My%20Code/auris-studio/crates/auris-mcp)",
                "auris-mcp",
            ),
            (
                "auris-gpui 0.18.0 (path+file:///Users/me/My%20Code/auris-studio/crates/auris-gpui)",
                "auris-studio",
            ),
        ]);
        let found = parse_manifest(
            Path::new("/Users/me/.cargo"),
            Path::new("/Users/me/.cargo/bin/auris-studio"),
            "auris-studio",
            &bytes,
        )
        .unwrap();
        assert_eq!(
            found.source,
            InstallSource::Path {
                workspace: PathBuf::from("/Users/me/My Code/auris-studio")
            }
        );
        assert_eq!(found.packages, ["auris-gpui", "auris-mcp"]);
    }

    #[test]
    fn a_windows_executable_and_a_git_install_are_recognised() {
        let bytes = manifest(&[(
            "auris-gpui 0.18.0 (git+https://github.com/uthree/auris-studio?tag=v0.18.0#8fe7b53)",
            "auris-studio.exe",
        )]);
        let found = parse_manifest(
            Path::new("C:/Users/me/.cargo"),
            Path::new("C:/Users/me/.cargo/bin/auris-studio.exe"),
            "auris-studio.exe",
            &bytes,
        )
        .unwrap();
        assert_eq!(found.source, InstallSource::Git);
        assert_eq!(found.packages, ["auris-gpui"]);
    }

    #[test]
    fn a_manifest_without_the_desktop_is_not_an_install_of_it() {
        let bytes = manifest(&[(
            "auris-cli 0.18.0 (path+file:///src/auris-studio/crates/auris-cli)",
            "auris",
        )]);
        assert!(
            parse_manifest(
                Path::new("/r"),
                Path::new("/r/bin/auris-studio"),
                "auris-studio",
                &bytes
            )
            .is_none()
        );
        assert!(
            parse_manifest(
                Path::new("/r"),
                Path::new("/r/bin/auris-studio"),
                "auris-studio",
                b"not json"
            )
            .is_none()
        );
    }

    #[test]
    fn a_build_outside_a_cargo_bin_directory_is_not_an_install() {
        let folder = tempfile::tempdir().unwrap();
        let debug = folder.path().join("target").join("debug");
        fs::create_dir_all(&debug).unwrap();
        fs::write(folder.path().join("target").join(".crates2.json"), b"{}").unwrap();
        assert!(detect(&debug.join("auris-studio")).is_none());
    }

    #[test]
    fn a_git_install_upgrades_to_the_release_tag() {
        let command = install(
            InstallSource::Git,
            &["auris-gpui", "auris-cli", "auris-mcp"],
        )
        .upgrade_command_with_default_root("0.19.0", Some(Path::new("/home/me/.cargo")));
        assert_eq!(
            command,
            "cargo install --locked --git https://github.com/uthree/auris-studio --tag v0.19.0 \
             auris-gpui auris-cli auris-mcp"
        );
    }

    #[test]
    fn a_path_install_pulls_its_checkout_and_installs_each_package_again() {
        let command = install(
            InstallSource::Path {
                workspace: PathBuf::from("/home/me/src/auris studio"),
            },
            &["auris-gpui", "auris-cli"],
        )
        .upgrade_command_with_default_root("0.19.0", Some(Path::new("/elsewhere")));
        assert_eq!(
            command.lines().collect::<Vec<_>>(),
            [
                "cd \"/home/me/src/auris studio\"",
                "git pull --ff-only",
                "cargo install --locked --root \"/home/me/.cargo\" --path crates/auris-gpui",
                "cargo install --locked --root \"/home/me/.cargo\" --path crates/auris-cli",
            ]
        );
    }

    #[test]
    fn the_bundle_plist_carries_this_build_version() {
        // A Windows checkout may hold the plist with CRLF endings.
        let plist = bundle_info_plist().replace('\r', "");
        let version = env!("CARGO_PKG_VERSION");
        assert!(plist.contains(&format!(
            "<key>CFBundleShortVersionString</key>\n    <string>{version}</string>"
        )));
        assert!(plist.contains(&format!(
            "<key>CFBundleVersion</key>\n    <string>{version}</string>"
        )));
        assert!(plist.contains("<string>auris-studio</string>"));
    }

    #[test]
    fn a_bundle_without_the_record_is_never_replaced() {
        let folder = tempfile::tempdir().unwrap();
        let bundle = folder.path().join("Auris Studio.app");
        fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        fs::write(bundle.join("Contents/MacOS/auris-studio"), b"released").unwrap();
        let source = folder.path().join("auris-studio");
        fs::write(&source, b"cargo").unwrap();
        assert_eq!(
            write_app_bundle(&bundle, &source),
            Err(ShortcutError::Occupied(bundle.clone()))
        );
        assert_eq!(
            fs::read(bundle.join("Contents/MacOS/auris-studio")).unwrap(),
            b"released"
        );
    }

    #[test]
    fn a_bundle_is_built_around_a_copy_and_remembers_its_source() {
        let folder = tempfile::tempdir().unwrap();
        let bundle = folder.path().join("Applications").join("Auris Studio.app");
        let source = folder.path().join("auris-studio");
        fs::write(&source, b"first").unwrap();
        write_app_bundle(&bundle, &source).unwrap();
        let copy = bundle.join("Contents/MacOS/auris-studio");
        assert_eq!(fs::read(&copy).unwrap(), b"first");
        assert_eq!(bundle_source(&copy), Some(source.clone()));
        assert!(
            bundle
                .join("Contents/Resources/auris-studio.icns")
                .is_file()
        );
        assert!(
            fs::read_to_string(bundle.join("Contents/Info.plist"))
                .unwrap()
                .contains("io.github.uthree.auris-studio")
        );

        // Made again over its own earlier self, and nothing staged is left behind.
        fs::write(&source, b"second, longer").unwrap();
        assert!(is_newer(&source, &copy));
        write_app_bundle(&bundle, &source).unwrap();
        assert_eq!(fs::read(&copy).unwrap(), b"second, longer");
        assert!(!is_newer(&source, &copy));
        let leftovers: Vec<_> = fs::read_dir(bundle.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, ["Auris Studio.app"]);
    }

    #[test]
    fn a_desktop_entry_quotes_its_executable_and_is_marked_as_ours() {
        let folder = tempfile::tempdir().unwrap();
        let entry = folder
            .path()
            .join("applications")
            .join("auris-studio.desktop");
        fs::create_dir_all(entry.parent().unwrap()).unwrap();
        fs::write(&entry, "[Desktop Entry]\nName=Something else\n").unwrap();
        assert_eq!(
            write_desktop_entry(&entry, Path::new("/x/auris-studio")),
            Err(ShortcutError::Occupied(entry.clone()))
        );
        let text = desktop_entry(
            Path::new("/home/me/$cargo/bin/auris-studio"),
            Path::new("/i.png"),
        );
        assert!(text.contains("Exec=\"/home/me/\\$cargo/bin/auris-studio\" %f\n"));
        assert!(text.lines().any(|line| line == DESKTOP_MARKER));
    }
}
