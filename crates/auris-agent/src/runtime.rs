//! Provider preflight, context budgeting and failed-call loop detection.
use super::*;
use rig::agent::{CompletionCallAction, CompletionCallEvent};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Confirms that the selected Ollama model supports tools and the requested context.
pub(super) async fn preflight(options: &Options) -> Result<(), String> {
    if options.provider != Provider::Ollama {
        return Ok(());
    }
    let base = options
        .url
        .as_deref()
        .unwrap_or("http://localhost:11434")
        .trim_end_matches('/');
    let mut request = reqwest::Client::new()
        .post(format!("{base}/api/show"))
        .timeout(MODEL_LIST_PATIENCE)
        .json(&serde_json::json!({"model":options.model}));
    if let Some(key) = &options.key {
        request = request.bearer_auth(key);
    }
    let shown: serde_json::Value = request
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| format!("cannot inspect Ollama model '{}': {e}", options.model))?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    validate_model(&shown, options.context_tokens)
}

fn validate_model(shown: &serde_json::Value, context: u32) -> Result<(), String> {
    if shown
        .get("capabilities")
        .and_then(|v| v.as_array())
        .is_some_and(|caps| !caps.iter().any(|c| c == "tools"))
    {
        return Err(
            "the selected Ollama model does not support tool calls; choose a tool-capable model"
                .into(),
        );
    }
    let maximum = shown
        .get("model_info")
        .and_then(|v| v.as_object())
        .and_then(|v| v.iter().find(|(key, _)| key.ends_with(".context_length")))
        .and_then(|(_, v)| v.as_u64());
    if maximum.is_some_and(|max| u64::from(context) > max) {
        return Err(format!(
            "requested context {context} exceeds this model's maximum {}; lower --context-tokens or choose a larger-context model",
            maximum.unwrap()
        ));
    }
    Ok(())
}

#[derive(Default)]
struct State {
    failures: BTreeMap<(String, String), usize>,
    active_tools: usize,
}

/// Per-conversation guard; successful calls reset that signature's failure count.
pub(super) struct Guard {
    context: Option<u32>,
    schema_tokens: usize,
    state: Mutex<State>,
    pub(super) activity: Activity,
}

/// A tool can render for longer than a model timeout. Only provider inactivity is bounded.
pub(super) type Activity = Arc<Mutex<(Instant, usize)>>;

fn signature(tool: &str, args: &str) -> (String, String) {
    let normalized = serde_json::from_str::<serde_json::Value>(args)
        .map(|mut v| {
            v.sort_all_objects();
            v.to_string()
        })
        .unwrap_or_else(|_| args.into());
    (tool.into(), normalized)
}

/// An estimate, not a tokenizer: reserve extra space for framing, output and varying languages.
fn estimated_tokens(text: &str) -> usize {
    text.len().div_ceil(3)
}

impl Guard {
    pub(super) async fn new(agent: &Agent, context: Option<u32>) -> Result<Self, String> {
        let definitions = agent
            .tool_definitions(None)
            .await
            .map_err(|e| e.to_string())?;
        let serialized = serde_json::to_string(&definitions).map_err(|e| e.to_string())?;
        Ok(Self {
            context,
            schema_tokens: estimated_tokens(&serialized) + estimated_tokens(&preamble()) + 4096,
            state: Mutex::new(State::default()),
            activity: Arc::new(Mutex::new((Instant::now(), 0))),
        })
    }
    fn mark(&self, running: usize) {
        *self.activity.lock().unwrap() = (Instant::now(), running);
    }
}

impl AgentHook for Guard {
    async fn on_completion_call(
        &self,
        _: &HookContext,
        event: CompletionCallEvent<'_>,
    ) -> CompletionCallAction {
        self.mark(0);
        if let Some(limit) = self.context {
            let text =
                serde_json::json!({"prompt":event.prompt,"history":event.history}).to_string();
            let estimate = self.schema_tokens + estimated_tokens(&text);
            if estimate > limit as usize {
                return CompletionCallAction::Stop(format!(
                    "estimated context budget {estimate} tokens (tools, history and 4096 output reserve) exceeds requested {limit}; increase Agent Panel context/--context-tokens or start a fresh conversation. Saved edits remain on disk."
                ));
            }
        }
        CompletionCallAction::Continue
    }
    async fn on_tool_call(&self, _: &HookContext, event: ToolCall<'_>) -> ToolCallAction {
        let mut state = self.state.lock().unwrap();
        if state
            .failures
            .get(&signature(event.tool_name, event.args))
            .copied()
            .unwrap_or(0)
            >= 2
        {
            return ToolCallAction::Stop(format!(
                "{} failed twice with identical arguments; stopping the repeated call. Correct the arguments or choose another approach. Saved edits remain on disk.",
                event.tool_name
            ));
        }
        state.active_tools += 1;
        self.mark(state.active_tools);
        ToolCallAction::Run
    }
    async fn on_tool_result(
        &self,
        _: &HookContext,
        event: ToolResultEvent<'_>,
    ) -> ToolResultAction {
        let mut state = self.state.lock().unwrap();
        let key = signature(event.tool_name, event.args);
        if event.raw_result.error().is_some() {
            *state.failures.entry(key).or_default() += 1;
        } else {
            state.failures.remove(&key);
        }
        state.active_tools = state.active_tools.saturating_sub(1);
        self.mark(state.active_tools);
        ToolResultAction::Keep
    }
}

pub(super) async fn await_active<F, T, E>(request: F, activity: Activity) -> Result<T, String>
where
    F: std::future::IntoFuture<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let request = request.into_future();
    tokio::pin!(request);
    loop {
        tokio::select! {
            result = &mut request => return result.map_err(|e| e.to_string()),
            _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                let (last, tools) = *activity.lock().unwrap();
                if tools == 0 && last.elapsed() > CONVERSATION_PATIENCE {
                    return Err("model request timed out after 300 seconds without progress; saved project edits remain on disk".into());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn context_and_tools_are_validated_before_chat() {
        let model = serde_json::json!({"capabilities":["tools"],"model_info":{"arch.context_length":65536}});
        assert!(validate_model(&model, 32768).is_ok());
        assert!(
            validate_model(&model, 131072)
                .unwrap_err()
                .contains("maximum")
        );
        assert!(
            validate_model(&serde_json::json!({"capabilities":["completion"]}), 32768).is_err()
        );
        assert_eq!(
            signature("edit", r#"{"b":2,"a":1}"#),
            signature("edit", r#"{ "a": 1, "b": 2 }"#)
        );
    }
}
