//! Isolated text-only requests without a live tool host or conversation history.

use crate::*;
use rig::agent::{ModelTurnAction, ModelTurnFinished};
use std::{sync::mpsc, time::Duration};

struct CompleteResponse;

impl AgentHook for CompleteResponse {
    async fn on_model_turn_finished(
        &self,
        _: &HookContext,
        event: ModelTurnFinished<'_>,
    ) -> ModelTurnAction {
        if event.raw["done_reason"] == "length"
            || event.raw["choices"][0]["finish_reason"] == "length"
        {
            ModelTurnAction::stop("Text-only response reached the output limit")
        } else {
            ModelTurnAction::continue_run()
        }
    }
}

/// Dropping a text-only request cancels its network or CLI work.
pub(crate) struct CompletionJob {
    receiver: mpsc::Receiver<Result<String, String>>,
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
}

impl CompletionJob {
    pub(crate) fn poll(&self) -> Option<Result<String, String>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err("The text-only agent stopped".into()))
            }
        }
    }
}

impl Drop for CompletionJob {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
    }
}

pub(crate) fn start(
    prefs: auris_session::AgentPreferences,
    instructions: String,
    input: String,
    max_tokens: u64,
    timeout: Duration,
) -> Result<CompletionJob, String> {
    let (sender, receiver) = mpsc::channel();
    let (cancel, cancelled) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("auris-text-completion".into())
        .spawn(move || {
            let result = (|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| error.to_string())?;
                runtime.block_on(async {
                    tokio::select! {
                        biased;
                        _ = cancelled => Err("Text-only request cancelled".into()),
                        result = tokio::time::timeout(timeout, prompt(&prefs, &instructions, &input, max_tokens)) =>
                            result.map_err(|_| "Text-only request timed out".to_string())?,
                    }
                })
            })();
            let _ = sender.send(result);
        })
        .map_err(|error| error.to_string())?;
    Ok(CompletionJob {
        receiver,
        cancel: Some(cancel),
    })
}

pub(crate) async fn prompt(
    prefs: &auris_session::AgentPreferences,
    instructions: &str,
    input: &str,
    max_tokens: u64,
) -> Result<String, String> {
    // CLI adapters receive explicit task instructions, not the user's live-agent preamble.
    let mut prefs = prefs.clone();
    prefs.system_prompt = instructions.into();
    match prefs.backend.as_str() {
        "codex" => backends::codex::complete(&prefs, input).await,
        "claude" => backends::claude::complete(&prefs, input).await,
        "opencode" => backends::opencode::complete(&prefs, input).await,
        "antigravity" => backends::antigravity::complete(&prefs, input).await,
        "" | "rig" => {
            let Command::Run(options) =
                parse_command(&[], &|name| std::env::var(name).ok(), &prefs)?
            else {
                return Err("Expected text-only model settings".into());
            };
            let agent = build_with(&options, |builder| {
                builder
                    .preamble(instructions)
                    .max_tokens(max_tokens)
                    .build()
            })?;
            agent
                .prompt(input)
                .add_hook(CompleteResponse)
                .await
                .map_err(|error| error.to_string())
                .map(|response| response.output().to_owned())
        }
        _ => Err("Unknown text-only agent backend".into()),
    }
}
