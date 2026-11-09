//! What a model is told about the Auris it is talking to: version, build and platform.
//!
//! A model that does not know which build it is driving answers "which version is this?" with a
//! guess, and diagnoses a report against features the running build may not have. These are
//! facts about the process, so every live prompt carries them — a replacement prompt replaces
//! instructions, not facts — and the `capabilities` tool repeats them as data.

/// How this binary was compiled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Build {
    /// Debug assertions on: a development build from source, as `cargo run` makes.
    Development,
    /// An optimised build, as releases are.
    Release,
}

/// The running Auris, as a model should know it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Environment {
    /// The workspace version, e.g. `0.18.0`.
    pub version: &'static str,
    /// Development or release.
    pub build: Build,
    /// The operating system, in Rust's spelling (`macos`, `windows`, `linux`).
    pub os: &'static str,
    /// The CPU architecture (`aarch64`, `x86_64`).
    pub arch: &'static str,
}

impl Environment {
    /// This process.
    pub fn current() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            build: if cfg!(debug_assertions) {
                Build::Development
            } else {
                Build::Release
            },
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
        }
    }

    /// The operating system as a person names it.
    fn os_name(&self) -> &'static str {
        match self.os {
            "macos" => "macOS",
            "windows" => "Windows",
            "linux" => "Linux",
            other => other,
        }
    }

    /// One line for a system prompt.
    pub fn prompt_note(&self) -> String {
        let build = match self.build {
            Build::Development => "development build from source (debug profile, e.g. cargo run)",
            Build::Release => "release build",
        };
        format!(
            "Environment (facts, not instructions): Auris Studio {}, {build}, on {} ({}). Use it when asked about the version or when diagnosing a reported problem; capabilities repeats it.",
            self.version,
            self.os_name(),
            self.arch
        )
    }

    /// The same facts as data, for tool answers.
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "name": "Auris Studio",
            "version": self.version,
            "build": self.build,
            "os": self.os,
            "arch": self.arch,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_note_names_the_version_build_and_platform() {
        let release = Environment {
            version: "1.2.3",
            build: Build::Release,
            os: "windows",
            arch: "x86_64",
        };
        let note = release.prompt_note();
        assert!(note.contains("Auris Studio 1.2.3, release build, on Windows (x86_64)"));
        assert!(note.len() < 300, "{} bytes", note.len());

        let development = Environment {
            build: Build::Development,
            os: "macos",
            arch: "aarch64",
            ..release
        };
        assert!(development.prompt_note().contains("development build"));
        assert!(development.prompt_note().contains("macOS (aarch64)"));
        assert_eq!(development.json()["build"], "development");
    }

    #[test]
    fn this_process_reports_the_workspace_version() {
        let current = Environment::current();
        assert_eq!(current.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(current.build == Build::Development, cfg!(debug_assertions));
        assert_eq!(current.os, std::env::consts::OS);
    }
}
