//! Native Codex discovery for the ACP adapter and connection diagnostics.

use std::path::PathBuf;

pub(crate) fn executable(prefs: &auris_session::AgentPreferences) -> Result<PathBuf, String> {
    if !prefs.codex_path.trim().is_empty() {
        let path = PathBuf::from(prefs.codex_path.trim());
        if cfg!(windows)
            && path
                .extension()
                .is_none_or(|extension| !extension.eq_ignore_ascii_case("exe"))
        {
            return Err(
                "Choose the native codex.exe executable, not a .cmd or .ps1 launcher".into(),
            );
        }
        return Ok(path);
    }
    for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let path = directory.join(if cfg!(windows) { "codex.exe" } else { "codex" });
        if path.is_file() {
            return Ok(path);
        }
        // npm's Windows entry point is a shell shim. Launch its native payload directly.
        if cfg!(windows) {
            let triple = if cfg!(target_arch = "aarch64") {
                "aarch64-pc-windows-msvc"
            } else {
                "x86_64-pc-windows-msvc"
            };
            let package = if cfg!(target_arch = "aarch64") {
                "codex-win32-arm64"
            } else {
                "codex-win32-x64"
            };
            for vendor in [
                directory.join(format!(
                    "node_modules/@openai/codex/node_modules/@openai/{package}/vendor"
                )),
                directory.join("node_modules/@openai/codex/vendor"),
            ] {
                let path = vendor.join(triple).join("bin/codex.exe");
                if path.is_file() {
                    return Ok(path);
                }
            }
        }
    }
    Err("Codex CLI was not found. Install Codex, run codex login, or set its executable path in Agent settings".into())
}
