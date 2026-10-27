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
            cli_version(path).await
        } else {
            None
        }
    };
    let listing = async {
        tokio::time::timeout(crate::MODEL_LIST_PATIENCE, async {
            match prefs.backend.as_str() {
                "codex" | "claude" | "opencode" => backends::acp::list_models(&prefs).await,
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
    let (version, listing) = tokio::join!(version, listing);
    status.version = version;
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
