//! Native Claude Code discovery for the ACP adapter and connection diagnostics.

use std::path::PathBuf;

pub(crate) fn executable(prefs: &auris_session::AgentPreferences) -> Result<PathBuf, String> {
    if !prefs.claude_path.trim().is_empty() {
        let path = PathBuf::from(prefs.claude_path.trim());
        if cfg!(windows)
            && path
                .extension()
                .is_none_or(|ext| !ext.eq_ignore_ascii_case("exe"))
        {
            return Err(
                "Choose the native claude.exe executable, not a .cmd or .ps1 launcher".into(),
            );
        }
        return Ok(path);
    }
    let name = if cfg!(windows) {
        "claude.exe"
    } else {
        "claude"
    };
    let mut directories: Vec<_> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
        directories.push(PathBuf::from(home).join(".local/bin"));
    }
    directories.into_iter().map(|dir| dir.join(name)).find(|path| path.is_file())
        .ok_or_else(|| "Claude Code CLI was not found. Install Claude Code, sign in with claude, or set its executable path in Agent settings".into())
}
