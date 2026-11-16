//! Bounded settings diagnostics; no prompt or inference request is sent.

use crate::*;
use auris_session::AgentPreferences;
use std::{path::Path, sync::mpsc, time::Duration};

/// A point-in-time provider check, independent of an active conversation.
#[derive(Clone, Debug, Default)]
pub struct ProviderStatus {
    /// CLI path resolved by the same resolver used for conversations.
    pub executable: Option<PathBuf>,
    /// CLI version output when available; API connections omit it.
    pub version: Option<String>,
    /// Number of models returned by a successful connection check.
    /// This verifies discovery, not permission to run inference with each model.
    pub models: Option<usize>,
    /// Connection or discovery failure, independent of version detection.
    pub error: Option<String>,
    /// Public release comparison, when this CLI has a supported release source.
    /// Independent of model discovery and authentication.
    pub update: Option<ProviderUpdate>,
    /// How Auris can install that newer release itself, when one is
    /// [available](ProviderUpdate::Available) and the installation method is recognised.
    /// `None` leaves the update to the published instructions.
    pub update_command: Option<UpdateCommand>,
}

/// A command that brings an installed CLI up to its latest release without a terminal:
/// the CLI's own updater, or the package manager that installed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateCommand {
    /// The program to run.
    pub program: PathBuf,
    /// Its arguments.
    pub args: Vec<String>,
}

impl UpdateCommand {
    /// The command as a person would type it, shown before it runs.
    pub fn display(&self) -> String {
        let name = self
            .program
            .file_stem()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.program.display().to_string());
        std::iter::once(name)
            .chain(self.args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Where a program called `name` is installed, looking where installers put package managers
/// as well as on `PATH`, which an application opened from Finder barely has.
fn find_program(name: &str) -> Option<PathBuf> {
    let mut directories: Vec<_> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    if cfg!(target_os = "macos") {
        directories.extend([
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ]);
    }
    if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
        directories.push(PathBuf::from(home).join(".bun/bin"));
    }
    let names: Vec<String> = if cfg!(windows) {
        vec![format!("{name}.cmd"), format!("{name}.exe")]
    } else {
        vec![name.to_owned()]
    };
    directories
        .iter()
        .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
        .find(|path| path.is_file())
}

/// The update for `backend`'s CLI at `executable`, whose real location is `resolved`.
///
/// Claude Code and OpenCode update themselves (`claude update`, `opencode upgrade`) and know
/// how they were installed; Homebrew casks are the exception, since the cask owns the files.
/// Codex has no updater of its own, so the package manager is read off the path it lives at.
fn plan_update(
    backend: &str,
    executable: &Path,
    resolved: &Path,
    find: &dyn Fn(&str) -> Option<PathBuf>,
) -> Option<UpdateCommand> {
    let location = resolved.to_string_lossy().replace('\\', "/");
    let run = |program: PathBuf, args: &[&str]| UpdateCommand {
        program,
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
    };
    let brew = |args: &[&str]| find("brew").map(|brew| run(brew, args));
    match backend {
        "claude" if location.contains("/Caskroom/claude-code/") => {
            brew(&["upgrade", "--cask", "claude-code"])
        }
        "claude" => Some(run(executable.to_path_buf(), &["update"])),
        "opencode" => Some(run(executable.to_path_buf(), &["upgrade"])),
        "codex" if location.contains("/Caskroom/codex/") => brew(&["upgrade", "--cask", "codex"]),
        "codex" if location.contains("/Cellar/codex/") => brew(&["upgrade", "codex"]),
        "codex" if location.contains("/.bun/") => {
            find("bun").map(|bun| run(bun, &["add", "-g", "@openai/codex@latest"]))
        }
        "codex" if location.contains("/node_modules/") && location.contains("pnpm") => {
            find("pnpm").map(|pnpm| run(pnpm, &["add", "-g", "@openai/codex@latest"]))
        }
        "codex" if location.contains("/node_modules/") => {
            find("npm").map(|npm| run(npm, &["install", "-g", "@openai/codex@latest"]))
        }
        _ => None,
    }
}

fn update_command(backend: &str, executable: &Path) -> Option<UpdateCommand> {
    let resolved = std::fs::canonicalize(executable).unwrap_or_else(|_| executable.to_path_buf());
    plan_update(backend, executable, &resolved, &find_program)
}

/// The longest stretch of a failed update's output worth showing.
const UPDATE_OUTPUT_TAIL: usize = 600;

async fn run_update(command: &UpdateCommand) -> Result<(), String> {
    let mut process = tokio::process::Command::new(&command.program);
    process
        .args(&command.args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    // npm, pnpm and a script-installed CLI are Node programs whose `#!/usr/bin/env node` needs
    // `node` on `PATH`; it is installed beside them, and an application opened from Finder
    // has no `PATH` that would find it otherwise.
    if let Some(directory) = command.program.parent() {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let joined = std::env::join_paths(
            std::iter::once(directory.to_path_buf()).chain(std::env::split_paths(&path)),
        )
        .map_err(|error| error.to_string())?;
        process.env("PATH", joined);
    }
    #[cfg(windows)]
    process.creation_flags(0x08000000);
    let output = tokio::time::timeout(Duration::from_secs(600), process.output())
        .await
        .map_err(|_| format!("{} did not finish within ten minutes", command.display()))?
        .map_err(|error| format!("{}: {error}", command.display()))?;
    if output.status.success() {
        return Ok(());
    }
    let text = if output.stderr.iter().any(|byte| !byte.is_ascii_whitespace()) {
        String::from_utf8_lossy(&output.stderr)
    } else {
        String::from_utf8_lossy(&output.stdout)
    };
    let text = text.trim();
    let start = text
        .char_indices()
        .rev()
        .nth(UPDATE_OUTPUT_TAIL)
        .map_or(0, |(index, _)| index);
    Err(format!(
        "{} failed ({}): {}",
        command.display(),
        output.status,
        &text[start..]
    ))
}

/// Run `command` on a worker thread and report once it has finished.
/// The process is killed if it runs longer than ten minutes; dropping the receiver does not
/// stop it, since an update interrupted halfway is worse than one left to finish.
pub fn update_provider_background(command: UpdateCommand) -> mpsc::Receiver<Result<(), String>> {
    let (sender, receiver) = mpsc::channel();
    let failed = sender.clone();
    let spawn = std::thread::Builder::new()
        .name("auris-provider-update".into())
        .spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())
                .and_then(|runtime| runtime.block_on(run_update(&command)));
            let _ = sender.send(result);
        });
    if let Err(error) = spawn {
        let _ = failed.send(Err(error.to_string()));
    }
    receiver
}

/// Result of comparing an installed CLI with its public stable release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderUpdate {
    /// The version or release source could not be checked.
    Unavailable,
    /// The public source has no newer stable release.
    Current,
    /// A newer public release exists; installation channels may lag behind it.
    Available(String),
}

fn release_url(backend: &str) -> Option<&'static str> {
    match backend {
        "codex" => Some("https://api.github.com/repos/openai/codex/releases/latest"),
        "claude" => Some("https://api.github.com/repos/anthropics/claude-code/releases/latest"),
        "opencode" => Some("https://api.github.com/repos/anomalyco/opencode/releases/latest"),
        _ => None,
    }
}

fn compare_release(installed: &str, release: &serde_json::Value) -> ProviderUpdate {
    let installed = installed
        .split_whitespace()
        .find_map(|word| semver::Version::parse(word.strip_prefix('v').unwrap_or(word)).ok());
    let latest = release["tag_name"].as_str().and_then(|tag| {
        semver::Version::parse(
            tag.strip_prefix("rust-v")
                .or_else(|| tag.strip_prefix('v'))
                .unwrap_or(tag),
        )
        .ok()
    });
    match (installed, latest) {
        (Some(installed), Some(latest))
            if installed.pre.is_empty()
                && latest.pre.is_empty()
                && release["draft"] == false
                && release["prerelease"] == false =>
        {
            if latest.cmp_precedence(&installed).is_gt() {
                ProviderUpdate::Available(latest.to_string())
            } else {
                ProviderUpdate::Current
            }
        }
        _ => ProviderUpdate::Unavailable,
    }
}

async fn check_release_at(installed: &str, url: &str) -> ProviderUpdate {
    let check = async {
        // This client carries no provider credentials and never follows a release redirect.
        let client = reqwest::Client::builder()
            .user_agent("Auris-Studio")
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(4))
            .build()
            .ok()?;
        let mut response = client.get(url).send().await.ok()?.error_for_status().ok()?;
        if !response.status().is_success() {
            return None;
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.ok()? {
            if bytes.len() + chunk.len() > 1024 * 1024 {
                return None;
            }
            bytes.extend_from_slice(&chunk);
        }
        let release = serde_json::from_slice(&bytes).ok()?;
        Some(compare_release(installed, &release))
    };
    tokio::time::timeout(Duration::from_secs(4), check)
        .await
        .ok()
        .flatten()
        .unwrap_or(ProviderUpdate::Unavailable)
}

fn model_count(answer: &str) -> Result<usize, String> {
    let answer: serde_json::Value =
        serde_json::from_str(answer).map_err(|error| error.to_string())?;
    answer["models"]
        .as_array()
        .map(|models| {
            models
                .iter()
                .filter(|model| {
                    model["name"]
                        .as_str()
                        .is_some_and(|name| !name.trim().is_empty())
                })
                .count()
        })
        .ok_or_else(|| "Provider did not return a model list".into())
}

async fn cli_version(path: &Path) -> Option<String> {
    let mut command = tokio::process::Command::new(path);
    command
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let output = tokio::time::timeout(Duration::from_secs(5), command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().filter(|c| !c.is_control()).take(120).collect())
}

async fn probe(mut prefs: AgentPreferences) -> ProviderStatus {
    // The selected model may belong to another provider; discovery chooses no model.
    prefs.model.clear();
    let mut status = ProviderStatus::default();
    let executable = match prefs.backend.as_str() {
        "codex" => Some(backends::codex::executable(&prefs)),
        "claude" => Some(backends::claude::executable(&prefs)),
        "opencode" => Some(backends::opencode::executable(&prefs)),
        "antigravity" => Some(backends::antigravity::executable(&prefs)),
        _ => None,
    };
    if let Some(executable) = executable {
        match executable {
            Ok(path) => status.executable = Some(path),
            Err(error) => {
                return ProviderStatus {
                    error: Some(error),
                    ..status
                };
            }
        }
    }
    let version = async {
        if let Some(path) = &status.executable {
            let version = cli_version(path).await;
            let update = if let Some(url) = release_url(&prefs.backend) {
                Some(if let Some(version) = &version {
                    check_release_at(version, url).await
                } else {
                    ProviderUpdate::Unavailable
                })
            } else {
                None
            };
            (version, update)
        } else {
            (None, None)
        }
    };
    let listing = async {
        tokio::time::timeout(crate::MODEL_LIST_PATIENCE, async {
            match prefs.backend.as_str() {
                "codex" => backends::codex::list_models(&prefs).await,
                "claude" => backends::claude::list_models(&prefs).await,
                "opencode" => backends::opencode::list_models(&prefs).await,
                "antigravity" => backends::antigravity::list_models(&prefs).await,
                _ => {
                    let Command::Models(options) = parse_command(
                        &["models".into()],
                        &|name| std::env::var(name).ok(),
                        &prefs,
                    )?
                    else {
                        return Err("Expected model listing settings".into());
                    };
                    list_models(&options).await
                }
            }
        })
        .await
        .map_err(|_| "Provider connection check timed out".to_string())?
        .and_then(|answer| model_count(&answer))
    };
    let ((version, update), listing) = tokio::join!(version, listing);
    status.version = version;
    if matches!(update, Some(ProviderUpdate::Available(_)))
        && let Some(executable) = &status.executable
    {
        status.update_command = update_command(&prefs.backend, executable);
    }
    status.update = update;
    match listing {
        Ok(count) => status.models = Some(count),
        Err(error) => status.error = Some(error),
    }
    status
}

/// Check the four CLI providers and the configured API without running inference.
/// Results arrive independently as `(row, status)`; rows 0–3 are Codex, Claude Code,
/// OpenCode and Antigravity, and row 4 is the configured API (defaulting to Ollama).
/// Dropping the receiver discards results; each pending check has a bounded timeout.
/// Supported installed CLIs also query their public GitHub release metadata without credentials.
pub fn check_providers_background(
    prefs: AgentPreferences,
) -> mpsc::Receiver<(usize, ProviderStatus)> {
    let (sender, receiver) = mpsc::channel();
    let failed = sender.clone();
    let spawn = std::thread::Builder::new()
        .name("auris-provider-status".into())
        .spawn(move || {
            let run = || -> Result<(), String> {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| e.to_string())?;
                runtime.block_on(async {
                    let mut pending = tokio::task::JoinSet::new();
                    for (index, backend) in ["codex", "claude", "opencode", "antigravity", "rig"]
                        .into_iter()
                        .enumerate()
                    {
                        let mut candidate = prefs.clone();
                        candidate.backend = backend.into();
                        if index == 4
                            && let Some(endpoint) = &prefs.discovery_endpoint
                        {
                            candidate.provider.clone_from(&endpoint.provider);
                            candidate.url.clone_from(&endpoint.url);
                            candidate.api_key_env.clone_from(&endpoint.api_key_env);
                        }
                        pending.spawn(async move { (index, probe(candidate).await) });
                    }
                    while let Some(result) = pending.join_next().await {
                        let result = result.map_err(|e| e.to_string())?;
                        if sender.send(result).is_err() {
                            break;
                        }
                    }
                    Ok(())
                })
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run))
                .unwrap_or_else(|_| Err("Provider status worker stopped".into()));
            if let Err(error) = result {
                for index in 0..5 {
                    let _ = sender.send((
                        index,
                        ProviderStatus {
                            error: Some(error.clone()),
                            ..Default::default()
                        },
                    ));
                }
            }
        });
    if let Err(error) = spawn {
        for index in 0..5 {
            let _ = failed.send((
                index,
                ProviderStatus {
                    error: Some(error.to_string()),
                    ..Default::default()
                },
            ));
        }
    }
    receiver
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_comparison_uses_numeric_versions_and_rejects_uncertain_channels() {
        let release = |tag: &str| {
            serde_json::json!({
                "tag_name": tag, "draft": false, "prerelease": false,
            })
        };
        for (installed, tag) in [
            ("codex-cli 0.9.0", "rust-v0.10.0"),
            ("2.1.9 (Claude Code)", "v2.1.10"),
            ("1.9.0", "v1.10.0"),
        ] {
            assert!(matches!(
                compare_release(installed, &release(tag)),
                ProviderUpdate::Available(_)
            ));
        }
        for installed in ["1.10.0", "1.11.0", "1.10.0+local"] {
            assert_eq!(
                compare_release(installed, &release("v1.10.0")),
                ProviderUpdate::Current
            );
        }
        for installed in ["unknown", "1.9.0-beta.1"] {
            assert_eq!(
                compare_release(installed, &release("v1.10.0")),
                ProviderUpdate::Unavailable
            );
        }
        for tag in ["unrecognized", "v1.10.0-beta.1"] {
            assert_eq!(
                compare_release("1.9.0", &release(tag)),
                ProviderUpdate::Unavailable
            );
        }
        for invalid in [
            serde_json::json!({"message": "rate limited"}),
            serde_json::json!({
                "tag_name": "v1.10.0", "draft": false, "prerelease": true,
            }),
        ] {
            assert_eq!(
                compare_release("1.9.0", &invalid),
                ProviderUpdate::Unavailable
            );
        }
    }

    #[tokio::test]
    async fn release_checks_handle_valid_and_failed_responses_without_provider_credentials() {
        use std::io::{BufRead, BufReader, Write};

        for (status, body, expected) in [
            (
                "200 OK",
                r#"{"tag_name":"v2.0.0","draft":false,"prerelease":false}"#,
                ProviderUpdate::Available("2.0.0".into()),
            ),
            (
                "403 Forbidden",
                r#"{"message":"rate limited"}"#,
                ProviderUpdate::Unavailable,
            ),
            ("200 OK", "not json", ProviderUpdate::Unavailable),
            (
                "302 Found",
                r#"{"tag_name":"v2.0.0","draft":false,"prerelease":false}"#,
                ProviderUpdate::Unavailable,
            ),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = String::new();
                let mut reader = BufReader::new(&stream);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                    request.push_str(&line);
                }
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                request
            });
            assert_eq!(check_release_at("1.0.0", &url).await, expected);
            let request = server.join().unwrap().to_ascii_lowercase();
            assert!(request.starts_with("get / http/1.1"));
            assert!(!request.contains("authorization:"));
        }
    }

    #[test]
    fn updates_follow_the_installation_method() {
        let tools = |name: &str| Some(PathBuf::from(format!("/tools/{name}")));
        let plan = |backend: &str, path: &str| {
            plan_update(backend, Path::new("/bin/cli"), Path::new(path), &tools)
                .map(|command| command.display())
        };
        assert_eq!(
            plan("claude", "/Users/a/.local/share/claude/versions/2.1.0").as_deref(),
            Some("cli update")
        );
        assert_eq!(
            plan("claude", "/opt/homebrew/Caskroom/claude-code/2.1.0/claude").as_deref(),
            Some("brew upgrade --cask claude-code")
        );
        assert_eq!(
            plan("opencode", "/Users/a/.opencode/bin/opencode").as_deref(),
            Some("cli upgrade")
        );
        assert_eq!(
            plan(
                "codex",
                "/opt/homebrew/Caskroom/codex/0.9.0/codex-aarch64-apple-darwin"
            )
            .as_deref(),
            Some("brew upgrade --cask codex")
        );
        assert_eq!(
            plan(
                "codex",
                "/usr/local/lib/node_modules/@openai/codex-darwin-arm64/vendor/x/bin/codex"
            )
            .as_deref(),
            Some("npm install -g @openai/codex@latest")
        );
        assert_eq!(
            plan(
                "codex",
                r"C:\Users\a\AppData\Roaming\npm\node_modules\@openai\codex\vendor\codex.exe"
            )
            .as_deref(),
            Some("npm install -g @openai/codex@latest")
        );
        assert_eq!(
            plan(
                "codex",
                "/Users/a/.bun/install/global/node_modules/@openai/codex/bin/codex"
            )
            .as_deref(),
            Some("bun add -g @openai/codex@latest")
        );
        // A Codex built from source, or one whose package manager is not installed, is left to
        // the published instructions rather than guessed at.
        assert_eq!(
            plan("codex", "/Users/a/src/codex/target/release/codex"),
            None
        );
        assert_eq!(
            plan_update(
                "codex",
                Path::new("/bin/codex"),
                Path::new("/usr/lib/node_modules/@openai/codex/bin/codex"),
                &|_| None
            ),
            None
        );
        assert_eq!(plan("antigravity", "/usr/local/bin/agy"), None);
    }

    #[tokio::test]
    async fn a_failed_update_reports_the_end_of_its_output() {
        let cargo = UpdateCommand {
            program: PathBuf::from(env!("CARGO")),
            args: vec!["no-such-subcommand-for-auris".into()],
        };
        let error = run_update(&cargo).await.unwrap_err();
        assert!(
            error.starts_with("cargo no-such-subcommand-for-auris failed"),
            "{error}"
        );
        assert!(error.contains("no-such-subcommand-for-auris"), "{error}");
        let version = UpdateCommand {
            program: PathBuf::from(env!("CARGO")),
            args: vec!["--version".into()],
        };
        assert_eq!(run_update(&version).await, Ok(()));
    }

    #[test]
    fn malformed_or_empty_model_lists_do_not_report_usable_models() {
        assert!(model_count(r#"{"error":"unauthorized"}"#).is_err());
        assert_eq!(model_count(r#"{"models":[]}"#).unwrap(), 0);
        assert_eq!(
            model_count(r#"{"models":[{"name":""},{"name":"fixture"}]}"#).unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn version_probe_reads_the_native_program_output() {
        let version = cli_version(Path::new(env!("CARGO"))).await;
        assert!(
            version
                .as_deref()
                .is_some_and(|line| line.starts_with("cargo ")),
            "{version:?}"
        );
    }

    #[tokio::test]
    async fn missing_cli_returns_a_failure_without_a_version() {
        let status = probe(AgentPreferences {
            backend: "codex".into(),
            codex_path: "/does-not-exist/codex.exe".into(),
            ..Default::default()
        })
        .await;
        assert!(status.error.is_some());
        assert!(status.models.is_none());
        assert!(status.version.is_none());
    }

    #[tokio::test]
    async fn api_check_counts_models_from_the_configured_endpoint() {
        let (url, _seen) = crate::tests::mock_server(vec![
            r#"{"models":[{"name":"fixture","details":{"context_length":32768}}]}"#.into(),
        ]);
        let status = probe(AgentPreferences {
            backend: "rig".into(),
            provider: "ollama".into(),
            url,
            ..Default::default()
        })
        .await;
        assert_eq!(status.models, Some(1), "{status:?}");
        assert!(status.error.is_none());
    }
}
