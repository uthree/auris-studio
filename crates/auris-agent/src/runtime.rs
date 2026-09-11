//! Provider preflight, context budgeting and failed-call loop detection.
use super::*;
use rig::agent::{
    CompletionCallAction, CompletionCallEvent, CompletionResponseEvent, InvalidToolCallAction,
    InvalidToolCallContext, ObservationAction,
};
use rig::message::{AssistantContent, ReasoningContent, UserContent};
use rig::tool::ToolErrorKind;
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
    invalid_calls: BTreeMap<(String, String), usize>,
    active_tools: usize,
    pending_request: Option<Vec<Message>>,
    observed_context: Option<ObservedContext>,
}

/// A measured prompt, usable only while the next request preserves this exact prefix.
struct ObservedContext {
    messages: Vec<Message>,
    input_tokens: usize,
}

impl State {
    fn observe_context(&mut self, input_tokens: u64) {
        self.observed_context = self.pending_request.take().and_then(|messages| {
            usize::try_from(input_tokens)
                .ok()
                .filter(|tokens| *tokens > 0)
                .map(|input_tokens| ObservedContext {
                    messages,
                    input_tokens,
                })
        });
    }
}

/// Per-user-turn guard; the caller creates a new guard for each model/tool configuration.
/// Successful calls reset that signature's failure count.
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

/// Ollama's per-response generation limit and the matching context reserve.
pub(super) const OUTPUT_RESERVE: usize = 4096;
const MESSAGE_FRAMING: usize = 16;

/// An estimate, not a tokenizer. ASCII keeps the conservative three-byte ratio; non-ASCII
/// gets extra room for Japanese and other text whose characters can span multiple tokens.
fn estimated_tokens(text: &str) -> usize {
    let ascii = text.bytes().filter(u8::is_ascii).count();
    ascii.div_ceil(3) + (text.len() - ascii).div_ceil(2)
}

fn serialized_tokens(value: &impl serde::Serialize) -> usize {
    estimated_tokens(&serde_json::to_string(value).expect("message content is serializable"))
}

/// Media tokenization depends on resolution, duration and provider. Keep at least a full
/// reserve per opaque block, retain the previous size-based fallback, and never calibrate
/// a request containing it from a text-only usage baseline.
fn opaque_tokens(value: &impl serde::Serialize) -> usize {
    serialized_tokens(value).max(OUTPUT_RESERVE)
}

fn text_tokens(text: &rig::message::Text) -> usize {
    estimated_tokens(&text.text) + text.additional_params.as_ref().map_or(0, opaque_tokens)
}

fn result_tokens(part: &ToolResultContent) -> usize {
    match part {
        ToolResultContent::Text(text) => text_tokens(text),
        ToolResultContent::Json { value } => serialized_tokens(value),
        ToolResultContent::Image(image) => opaque_tokens(image),
    }
}

fn message_tokens(message: &Message) -> usize {
    // Provider conversion unwraps Text verbatim, and serializes JSON arguments/results once.
    // Serializing Message instead counts transport escaping of every nested tool reply.
    MESSAGE_FRAMING
        + match message {
            Message::System { content } => estimated_tokens(content),
            Message::User { content } => content
                .iter()
                .map(|part| match part {
                    UserContent::Text(text) => text_tokens(text),
                    UserContent::ToolResult(result) => {
                        MESSAGE_FRAMING
                            + estimated_tokens(&result.name)
                            + estimated_tokens(result.wire_call_id())
                            + result.content.iter().map(result_tokens).sum::<usize>()
                    }
                    UserContent::Image(image) => opaque_tokens(image),
                    UserContent::Audio(audio) => opaque_tokens(audio),
                    UserContent::Video(video) => opaque_tokens(video),
                    UserContent::Document(document) => opaque_tokens(document),
                })
                .sum::<usize>(),
            Message::Assistant { content, .. } => content
                .iter()
                .map(|part| match part {
                    AssistantContent::Text(text) => text_tokens(text),
                    AssistantContent::ToolCall(call) => {
                        MESSAGE_FRAMING
                            + estimated_tokens(&call.function.name)
                            + estimated_tokens(call.wire_call_id())
                            + serialized_tokens(&call.function.arguments)
                            + call.signature.as_deref().map_or(0, estimated_tokens)
                            + call.additional_params.as_ref().map_or(0, serialized_tokens)
                    }
                    AssistantContent::Reasoning(reasoning) => reasoning
                        .content
                        .iter()
                        .map(|part| match part {
                            ReasoningContent::Text { text, signature } => {
                                estimated_tokens(text)
                                    + signature.as_deref().map_or(0, estimated_tokens)
                            }
                            ReasoningContent::Summary(text) => estimated_tokens(text),
                            ReasoningContent::Encrypted(_) | ReasoningContent::Redacted { .. } => {
                                opaque_tokens(part)
                            }
                        })
                        .sum(),
                    AssistantContent::Image(image) => opaque_tokens(image),
                })
                .sum::<usize>(),
        }
}

fn history_tokens(prompt: &Message, history: &[Message]) -> usize {
    history.iter().chain([prompt]).map(message_tokens).sum()
}

fn has_only_known_text(message: &Message) -> bool {
    match message {
        Message::System { .. } => true,
        Message::User { content } => content.iter().all(|part| match part {
            UserContent::Text(text) => text.additional_params.is_none(),
            UserContent::ToolResult(result) => result.content.iter().all(|part| match part {
                ToolResultContent::Text(text) => text.additional_params.is_none(),
                ToolResultContent::Json { .. } => true,
                ToolResultContent::Image(_) => false,
            }),
            _ => false,
        }),
        Message::Assistant { content, .. } => content.iter().all(|part| match part {
            AssistantContent::Text(text) => text.additional_params.is_none(),
            AssistantContent::ToolCall(call) => {
                call.signature.is_none() && call.additional_params.is_none()
            }
            AssistantContent::Reasoning(reasoning) => reasoning.content.iter().all(|part| {
                matches!(
                    part,
                    ReasoningContent::Text {
                        signature: None,
                        ..
                    } | ReasoningContent::Summary(_)
                )
            }),
            AssistantContent::Image(_) => false,
        }),
    }
}

impl ObservedContext {
    fn estimate(&self, prompt: &Message, history: &[Message]) -> Option<usize> {
        let request = history.iter().chain([prompt]);
        if !request.clone().all(has_only_known_text)
            || self.messages.len() > history.len() + 1
            || !self
                .messages
                .iter()
                .eq(request.clone().take(self.messages.len()))
        {
            return None;
        }
        Some(
            self.input_tokens
                .saturating_add(
                    request
                        .skip(self.messages.len())
                        .map(message_tokens)
                        .sum::<usize>(),
                )
                .saturating_add(OUTPUT_RESERVE),
        )
    }
}

/// Persisted memory is alternating text-only user/assistant exchanges. Refuse to split any
/// richer transcript: assistant tool calls and their results must stay correlated.
fn completed_exchange(messages: &[Message]) -> bool {
    matches!(messages, [Message::User { content: user }, Message::Assistant { content: assistant, .. }, ..]
        if user.iter().all(|part| matches!(part, UserContent::Text(_)))
            && assistant.iter().all(|part| matches!(part, AssistantContent::Text(_))))
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
            schema_tokens: estimated_tokens(&serialized)
                + estimated_tokens(&preamble())
                + OUTPUT_RESERVE,
            state: Mutex::new(State::default()),
            activity: Arc::new(Mutex::new((Instant::now(), 0))),
        })
    }
    fn mark(&self, running: usize) {
        *self.activity.lock().unwrap() = (Instant::now(), running);
    }

    fn context_estimate(&self, prompt: &Message, history: &[Message]) -> usize {
        self.state
            .lock()
            .unwrap()
            .observed_context
            .as_ref()
            .and_then(|observed| observed.estimate(prompt, history))
            .unwrap_or_else(|| self.schema_tokens + history_tokens(prompt, history))
    }

    /// Drops whole completed exchanges before a new user turn, never the current prompt or
    /// the calls/results accumulated while carrying it out. Persisted memory is unchanged.
    pub(super) fn fit_history(&self, prompt: &Message, history: &mut Vec<Message>) -> usize {
        let Some(limit) = self.context else {
            return 0;
        };
        let mut omitted = 0;
        while self.context_estimate(prompt, history) > limit as usize && completed_exchange(history)
        {
            history.drain(..2);
            omitted += 1;
        }
        omitted
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
            let estimate = self.context_estimate(event.prompt, event.history);
            if estimate > limit as usize {
                return CompletionCallAction::Stop(format!(
                    "estimated context budget {estimate} tokens (tools, history and 4096 output reserve) exceeds requested {limit}; increase Agent Panel context/--context-tokens or start a fresh conversation. Saved edits remain on disk."
                ));
            }
            let request = event.history.iter().chain([event.prompt]);
            self.state.lock().unwrap().pending_request = request
                .clone()
                .all(has_only_known_text)
                .then(|| request.cloned().collect());
        }
        CompletionCallAction::Continue
    }
    async fn on_completion_response(
        &self,
        _: &HookContext,
        event: CompletionResponseEvent<'_>,
    ) -> ObservationAction {
        self.mark(0);
        // Rig accepts a length-truncated response when it contains text or tool calls.
        // Ollama's cap must instead report incomplete work before those calls execute.
        if self.context.is_some()
            && event
                .raw
                .get("done_reason")
                .and_then(serde_json::Value::as_str)
                == Some("length")
        {
            return ObservationAction::Stop(format!(
                "Ollama reached the {OUTPUT_RESERVE}-token output limit; the response is incomplete. Retry with a smaller task. Earlier saved edits remain on disk."
            ));
        }
        // Ollama reports prompt_eval_count here, including tools and preamble. The measured
        // prefix belongs to this guard's fixed agent; a changed/compacted prefix cannot use it.
        self.state
            .lock()
            .unwrap()
            .observe_context(event.usage.input_tokens);
        ObservationAction::Continue
    }
    async fn on_invalid_tool_call(
        &self,
        _: &HookContext,
        event: &InvalidToolCallContext,
    ) -> Option<InvalidToolCallAction> {
        self.mark(0);
        let mut state = self.state.lock().unwrap();
        let attempts = state
            .invalid_calls
            .entry(signature(
                &event.tool_name,
                event.args.as_deref().unwrap_or(""),
            ))
            .or_default();
        *attempts += 1;
        if *attempts > 2 {
            return Some(InvalidToolCallAction::stop(format!(
                "{} failed twice as an unavailable tool; stopping the repeated call. Saved edits remain on disk.",
                event.tool_name
            )));
        }
        // Rig validates the whole batch before running any tool. A retry therefore cannot
        // repeat an edit from this batch; all its calls are returned as unexecuted feedback.
        Some(InvalidToolCallAction::retry(format!(
            "Tool '{}' is unavailable. No tool in this batch was executed. Choose the exact name of an advertised tool, then submit the corrected calls. Available tools: {}.",
            event.tool_name,
            event.allowed_tools.join(", ")
        )))
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
        if !event.raw_result.is_success() {
            *state.failures.entry(key).or_default() += 1;
        } else {
            state.failures.remove(&key);
        }
        state.active_tools = state.active_tools.saturating_sub(1);
        self.mark(state.active_tools);
        if event.raw_result.is_error_kind(ToolErrorKind::InvalidArgs) {
            return ToolResultAction::rewrite(format!(
                "{}\nCheck the tool schema for {} and correct its argument fields before trying again. Do not repeat unchanged arguments.",
                full_text(event.presentation),
                event.tool_name
            ));
        }
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
    fn tool_reply_accounting_matches_unwrapped_provider_text() {
        let payload = serde_json::json!({
            "description": "Use {\"operation\":\"list\"}\nthen read the saved project.",
            "project": r"C:\Music\Song\Song.auris",
        })
        .to_string()
        .repeat(100);
        let message = Message::tool_result("call_1", "tool_help", &payload);
        let provider: Vec<rig::providers::ollama::Message> = message.clone().try_into().unwrap();
        let rig::providers::ollama::Message::ToolResult { content, .. } = &provider[0] else {
            panic!("a tool reply must remain a provider tool message");
        };
        assert_eq!(content, &payload);
        assert_eq!(
            message_tokens(&message),
            MESSAGE_FRAMING * 2
                + estimated_tokens("call_1")
                + estimated_tokens("tool_help")
                + estimated_tokens(content)
        );
        assert!(serialized_tokens(&message) > message_tokens(&message));

        let value = serde_json::json!({"project":r"C:\Music\Song.auris", "notes":[60, 64]});
        assert_eq!(
            result_tokens(&ToolResultContent::json(value.clone())),
            result_tokens(&ToolResultContent::text(value.to_string()))
        );
    }

    #[test]
    fn measured_context_counts_only_growth_and_keeps_the_output_reserve() {
        let first_prompt = Message::user("Read the compressor and then set its sidechain");
        let call = Message::Assistant {
            id: None,
            content: vec![AssistantContent::tool_call(
                "call_1",
                "tool_help",
                serde_json::json!({"name":"effects"}),
            )],
        };
        let result = Message::tool_result("call_1", "tool_help", "Sidechain needs slot and source");
        let history = vec![first_prompt.clone(), call.clone()];
        let mut state = State {
            pending_request: Some(vec![first_prompt]),
            ..Default::default()
        };
        state.observe_context(22_000);
        let guard = Guard {
            context: Some(32768),
            schema_tokens: 32_768,
            state: Mutex::new(state),
            activity: Arc::new(Mutex::new((Instant::now(), 0))),
        };
        assert_eq!(
            guard.context_estimate(&result, &history),
            22_000 + message_tokens(&call) + message_tokens(&result) + OUTPUT_RESERVE
        );
        assert!(guard.context_estimate(&result, &history) < 32768);
        assert!(guard.schema_tokens + history_tokens(&result, &history) > 32768);

        let oversized = Message::tool_result("call_1", "tool_help", "説明".repeat(10_000));
        assert!(guard.context_estimate(&oversized, &history) > 32768);
        let mut preserved = history.clone();
        assert_eq!(guard.fit_history(&oversized, &mut preserved), 0);
        assert_eq!(
            preserved, history,
            "an in-flight call must not be compacted"
        );
    }

    #[test]
    fn changed_prefix_missing_usage_and_opaque_content_use_the_fallback() {
        let first = Message::user("Inspect this project");
        let original = vec![
            first.clone(),
            Message::assistant("It contains three tracks"),
        ];
        let mut state = State {
            pending_request: Some(original.clone()),
            ..Default::default()
        };
        state.observe_context(20_000);
        let observed = state.observed_context.as_ref().unwrap();
        let next = Message::user("Now lower the lead");
        assert!(observed.estimate(&next, &original).is_some());
        let mut changed = original.clone();
        changed[0] = Message::user("A different project");
        assert!(observed.estimate(&next, &changed).is_none());
        assert!(observed.estimate(&next, &original[1..]).is_none());

        let image = Message::User {
            content: vec![UserContent::Image(rig::message::Image {
                data: rig::message::DocumentSourceKind::Url(
                    "https://example.test/large.png".into(),
                ),
                ..Default::default()
            })],
        };
        assert!(observed.estimate(&image, &original).is_none());
        assert!(message_tokens(&image) >= OUTPUT_RESERVE);
        assert!(estimated_tokens("日本語の依頼") >= "日本語の依頼".chars().count());

        state.pending_request = Some(vec![first]);
        state.observe_context(0);
        assert!(
            state.observed_context.is_none(),
            "zero means usage is unavailable"
        );
        state.observe_context(20_000);
        assert!(
            state.observed_context.is_none(),
            "usage requires a matching sent request"
        );
    }

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

    #[test]
    fn japanese_history_fits_by_whole_exchanges_without_losing_the_current_request() {
        let prompt = Message::user("この曲のサビの音量を少し下げてください。");
        let older = [
            Message::user("古い依頼".repeat(2000)),
            Message::assistant("古い回答".repeat(2000)),
        ];
        let newest = [
            Message::user("Song.aurisを開いて"),
            Message::assistant("開きました"),
        ];
        let guard = Guard {
            context: Some((4096 + history_tokens(&prompt, &newest)) as u32),
            schema_tokens: 4096,
            state: Mutex::new(State::default()),
            activity: Arc::new(Mutex::new((Instant::now(), 0))),
        };
        let mut history = [older.to_vec(), newest.to_vec()].concat();
        assert_eq!(guard.fit_history(&prompt, &mut history), 1);
        assert_eq!(history, newest);
        assert!(
            guard.schema_tokens + history_tokens(&prompt, &history)
                <= guard.context.unwrap() as usize
        );

        // The request can exceed the whole budget on its own. It must still reach the guard
        // intact and receive the normal explicit budget error, never be silently shortened.
        let oversized = Message::user("今の依頼".repeat(10_000));
        let original = oversized.clone();
        assert_eq!(guard.fit_history(&oversized, &mut history), 1);
        assert!(history.is_empty());
        assert_eq!(oversized, original);
        assert!(
            guard.schema_tokens + history_tokens(&oversized, &history)
                > guard.context.unwrap() as usize
        );
    }

    #[test]
    fn history_compaction_does_not_detach_a_tool_result_from_its_call() {
        let mut history = vec![
            Message::user("Inspect the project"),
            Message::Assistant {
                id: None,
                content: vec![rig::message::AssistantContent::tool_call(
                    "call_1",
                    "describe",
                    serde_json::json!({"project":"Song.auris"}),
                )],
            },
            Message::tool_result("call_1", "describe", "The project has three tracks"),
            Message::assistant("The project has three tracks"),
        ];
        let original = history.clone();
        let guard = Guard {
            context: Some(1),
            schema_tokens: 4096,
            state: Mutex::new(State::default()),
            activity: Arc::new(Mutex::new((Instant::now(), 0))),
        };
        assert_eq!(
            guard.fit_history(&Message::user("Continue"), &mut history),
            0
        );
        assert_eq!(history, original);
    }
}
