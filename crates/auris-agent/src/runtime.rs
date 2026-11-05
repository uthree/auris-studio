//! Provider preflight, context budgeting and failed-call loop detection.
use super::*;
use crate::tool_failures::signature;
use rig::agent::{
    CompletionCallAction, CompletionCallEvent, DispatchAction, DispatchEvent,
    InvalidToolCallAction, InvalidToolCallContext, ModelTurnAction, ModelTurnFinished,
    OutcomeAction, OutcomeEvent,
};
use rig::tool::ToolErrorKind;
use rig_core::effect::EffectKind;
use rig_core::message::{AssistantContent, UserContent};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Resolves the usable Ollama context before constructing the request and its budget guard.
pub(super) async fn preflight(options: &mut Options) -> Result<bool, String> {
    if options.provider != Provider::Ollama {
        return Ok(true);
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
    let context = validate_model(&shown, options.context_tokens)?;
    if options.output_tokens >= context {
        return Err(format!(
            "Output token limit {} must be smaller than this model's effective context {context}; lower the Agent Panel output token limit",
            options.output_tokens
        ));
    }
    options.context_tokens = context;
    Ok(shown["capabilities"]
        .as_array()
        .is_some_and(|caps| caps.iter().any(|cap| cap == "vision")))
}

/// Whether the model takes audio input, which is what offers it `listen`.
///
/// rig's Ollama adapter cannot send audio, so only the OpenAI-compatible dialect can hear. An
/// Ollama server behind that dialect says what a model accepts, and is asked; any other
/// endpoint is trusted to take audio, as audio attachments already trust it.
pub(super) async fn hears(options: &Options) -> bool {
    if options.provider != Provider::OpenAi {
        return false;
    }
    let Some(origin) = options.url.as_deref().and_then(ollama_origin) else {
        return true;
    };
    let mut request = reqwest::Client::new()
        .post(format!("{origin}/api/show"))
        .timeout(MODEL_LIST_PATIENCE)
        .json(&serde_json::json!({"model":options.model}));
    if let Some(key) = &options.key {
        request = request.bearer_auth(key);
    }
    let shown: Option<serde_json::Value> = async {
        request
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .json()
            .await
            .ok()
    }
    .await;
    shown.is_some_and(|shown| advertises_audio(&shown))
}

/// The Ollama server behind an OpenAI-compatible URL: port 11434, path `/v1`.
fn ollama_origin(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    (parsed.port() == Some(11434) && parsed.path().trim_end_matches('/') == "/v1")
        .then(|| parsed.origin().ascii_serialization())
}

fn advertises_audio(shown: &serde_json::Value) -> bool {
    shown["capabilities"]
        .as_array()
        .is_some_and(|caps| caps.iter().any(|cap| cap == "audio"))
}

/// Prefer the language architecture's ceiling over auxiliary audio/vision metadata.
pub(super) fn model_context_limit(shown: &serde_json::Value) -> Option<u64> {
    let info = shown.get("model_info")?.as_object()?;
    if let Some(architecture) = info.get("general.architecture").and_then(|v| v.as_str())
        && let Some(maximum) = info
            .get(&format!("{architecture}.context_length"))
            .and_then(|v| v.as_u64())
    {
        return Some(maximum);
    }
    info.iter()
        .filter(|(key, _)| key.ends_with(".context_length"))
        .filter_map(|(_, value)| value.as_u64())
        .min()
}

fn validate_model(shown: &serde_json::Value, context: u32) -> Result<u32, String> {
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
    let context = model_context_limit(shown)
        .map_or(context, |maximum| u64::from(context).min(maximum) as u32);
    if context < 16384 {
        return Err(format!(
            "this model's effective context is {context} tokens; Auris tools need at least 16384. Choose a larger-context model"
        ));
    }
    Ok(context)
}

#[derive(Default)]
struct State {
    failures: crate::tool_failures::ToolFailures,
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

fn record_tool_outcome(state: &mut State, key: (String, String), failed: bool) {
    state.failures.record(&key.0, &key.1, failed);
    state.active_tools = state.active_tools.saturating_sub(1);
}

/// Per-user-turn guard; the caller creates a new guard for each model/tool configuration.
/// Successful calls reset that signature's failure count.
pub(super) struct Guard {
    pub(super) failure_limit: usize,
    context: Option<u32>,
    output_tokens: usize,
    schema_tokens: usize,
    state: Mutex<State>,
    pub(super) activity: Activity,
    pub(super) bridge: Option<Bridge>,
}

/// A tool can render for longer than a model timeout. Only provider inactivity is bounded.
pub(super) type Activity = Arc<Mutex<(Instant, usize)>>;

/// Conservative minimum estimate for non-text payloads.
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

fn text_tokens(text: &rig_core::message::Text) -> usize {
    estimated_tokens(&text.text) + text.additional_params.as_ref().map_or(0, opaque_tokens)
}

fn inspection_tokens(message: &Message) -> usize {
    match message {
        Message::User { content } => {
            MESSAGE_FRAMING
                + content
                    .iter()
                    .map(|part| match part {
                        UserContent::Image(_) => OUTPUT_RESERVE,
                        UserContent::Text(text) => text_tokens(text),
                        _ => opaque_tokens(part),
                    })
                    .sum::<usize>()
        }
        _ => message_tokens(message),
    }
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
                            + estimated_tokens(&result.call.to_string())
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
                            + estimated_tokens(&call.id.to_string())
                            + serialized_tokens(&call.function.arguments)
                            + call.signature.as_deref().map_or(0, estimated_tokens)
                            + call.additional_params.as_ref().map_or(0, serialized_tokens)
                    }
                    // Reasoning is sealed to its issuing provider. Without that issuer we
                    // cannot inspect its signed/encrypted payload safely; count the opaque
                    // serialized block conservatively.
                    AssistantContent::Reasoning(reasoning) => opaque_tokens(reasoning),
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
            AssistantContent::Reasoning(_) => false,
            AssistantContent::Image(_) => false,
        }),
    }
}

impl ObservedContext {
    fn estimate(
        &self,
        prompt: &Message,
        history: &[Message],
        output_tokens: usize,
    ) -> Option<usize> {
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
                .saturating_add(output_tokens),
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
    pub(super) async fn new(
        agent: &Agent,
        context: Option<u32>,
        output_tokens: u32,
    ) -> Result<Self, String> {
        let definitions = agent
            .tool_definitions(None)
            .await
            .map_err(|e| e.to_string())?;
        let serialized = serde_json::to_string(&definitions).map_err(|e| e.to_string())?;
        Ok(Self {
            failure_limit: auris_session::AgentPreferences::default().tool_failure_limit(),
            context,
            bridge: None,
            output_tokens: output_tokens as usize,
            schema_tokens: estimated_tokens(&serialized)
                + estimated_tokens(agent.run_spec().preamble.as_deref().unwrap_or_default())
                + output_tokens as usize,
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
            .and_then(|observed| observed.estimate(prompt, history, self.output_tokens))
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
        if let Some(error) = self.bridge.as_ref().and_then(Bridge::tool_limit_error) {
            return CompletionCallAction::Stop(error);
        }
        if let Some(bridge) = &self.bridge {
            bridge.emit(serde_json::json!({"event":"phase", "phase":"prefill"}));
        }
        let visual = self.bridge.as_ref().and_then(Bridge::visual);
        if let Some(limit) = self.context {
            // Generated inspection images are fixed at 512x384; reserve image tokens rather
            // than counting the base64 encoding as natural-language text.
            let estimate = self.context_estimate(event.prompt, event.history)
                + visual.as_ref().map_or(0, inspection_tokens);
            if estimate > limit as usize {
                let reserve = self.output_tokens;
                return CompletionCallAction::Stop(format!(
                    "estimated context budget {estimate} tokens (tools, history and {reserve} output reserve) exceeds requested {limit}; increase Agent Panel context/--context-tokens or start a fresh conversation. Earlier edits remain in the open document and may be unsaved."
                ));
            }
            let request = event.history.iter().chain([event.prompt]);
            self.state.lock().unwrap().pending_request = request
                .clone()
                .all(has_only_known_text)
                .then(|| request.cloned().collect());
        }
        // An audition is heard once, in the request right after the listen call answered.
        let audible = self.bridge.as_ref().and_then(Bridge::take_audible);
        if visual.is_some() || audible.is_some() {
            self.state.lock().unwrap().pending_request = None;
            let mut history = event.history.to_vec();
            // Preserve contiguous assistant-call/tool-result pairs. The extra context is
            // ephemeral and tagged with the snapshot revision, never persisted as a user turn.
            let index = history.iter().rposition(|message| matches!(message, Message::Assistant { content, .. } if content.iter().any(|part| matches!(part, AssistantContent::ToolCall(_))))).unwrap_or(history.len());
            for message in audible.into_iter().chain(visual).rev() {
                history.insert(index, message);
            }
            return CompletionCallAction::Patch(
                rig::agent::RequestPatch::default().history(history),
            );
        }
        CompletionCallAction::Continue
    }
    async fn on_outcome(&self, _: &HookContext, event: OutcomeEvent<'_>) -> OutcomeAction {
        self.mark(0);
        if let Some(response) = event.completion() {
            if self.context.is_some()
                && response
                    .raw
                    .get("done_reason")
                    .and_then(serde_json::Value::as_str)
                    == Some("length")
            {
                let limit = self.output_tokens;
                return OutcomeAction::stop(format!(
                    "Ollama reached the {limit}-token output limit; the response is incomplete. Increase the Agent Panel output token limit or retry with a smaller task. Earlier edits remain in the project."
                ));
            }
            if let Some(input_tokens) = response.usage.input_tokens {
                self.state.lock().unwrap().observe_context(input_tokens);
            }
        }
        if matches!(event.kind, EffectKind::ToolCall { .. }) {
            let key = match event.kind {
                EffectKind::ToolCall { name, args } => signature(name, args),
                _ => unreachable!(),
            };
            let mut state = self.state.lock().unwrap();
            let failed = match (event.outcome, event.tool_result()) {
                (Err(_), _) => true,
                (Ok(_), Some(result)) => !result.is_success(),
                // A successful tool effect always has a tool result; keep a defensive failure
                // count if a future Rig version violates that invariant.
                (Ok(_), None) => true,
            };
            record_tool_outcome(&mut state, key, failed);
            self.mark(state.active_tools);
            if let Some(result) = event.tool_result()
                && result.is_error_kind(ToolErrorKind::InvalidArgs)
            {
                return OutcomeAction::rewrite_tool_result(
                    &event,
                    format!(
                        "{}\nCheck the tool schema and correct its argument fields before trying again. Do not repeat unchanged arguments.",
                        full_text(result.output())
                    ),
                );
            }
        }
        OutcomeAction::proceed()
    }
    async fn on_model_turn_finished(
        &self,
        _: &HookContext,
        event: ModelTurnFinished<'_>,
    ) -> ModelTurnAction {
        if self.context.is_some()
            && event
                .finish_reason
                .is_some_and(rig::completion::FinishReason::truncated_output)
        {
            let limit = self.output_tokens;
            return ModelTurnAction::stop(format!(
                "Ollama reached the {limit}-token output limit; the response is incomplete. Increase the Agent Panel output token limit or retry with a smaller task. Earlier edits remain in the project."
            ));
        }
        ModelTurnAction::Continue
    }
    async fn on_invalid_tool_call(
        &self,
        _: &HookContext,
        event: &InvalidToolCallContext,
    ) -> Option<InvalidToolCallAction> {
        self.mark(0);
        if let Some(bridge) = &self.bridge
            && let Err(error) = bridge.reserve_tool_call()
        {
            return Some(InvalidToolCallAction::stop(error));
        }
        let mut state = self.state.lock().unwrap();
        let attempts = state
            .invalid_calls
            .entry(signature(
                &event.tool_name,
                event.args.as_deref().unwrap_or(""),
            ))
            .or_default();
        *attempts += 1;
        if *attempts > self.failure_limit {
            let limit = self.failure_limit;
            return Some(InvalidToolCallAction::stop(format!(
                "{} failed {limit} times as an unavailable tool; stopping the repeated call. Earlier edits remain in the open document and may be unsaved. Send a new instruction to continue.",
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
    async fn on_dispatch(&self, _: &HookContext, event: DispatchEvent<'_>) -> DispatchAction {
        let EffectKind::ToolCall { name, args } = event.kind else {
            return DispatchAction::proceed();
        };
        let mut state = self.state.lock().unwrap();
        if state.failures.exhausted(name, args, self.failure_limit) {
            let limit = self.failure_limit;
            return DispatchAction::stop(format!(
                "{name} failed {limit} times with identical arguments; stopping the repeated call. Correct the arguments or choose another approach. Earlier edits remain in the open document and may be unsaved. Send a new instruction to continue."
            ));
        }
        state.active_tools += 1;
        self.mark(state.active_tools);
        DispatchAction::proceed()
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
                    return Err("model request timed out after 300 seconds without progress; earlier edits remain in the open document and may be unsaved".into());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_ollama_v1_url_is_asked_about_audio() {
        assert_eq!(
            ollama_origin("http://localhost:11434/v1").as_deref(),
            Some("http://localhost:11434")
        );
        assert_eq!(
            ollama_origin("http://127.0.0.1:11434/v1/").as_deref(),
            Some("http://127.0.0.1:11434")
        );
        assert_eq!(ollama_origin("http://localhost:11434"), None);
        assert_eq!(ollama_origin("https://api.openai.com/v1"), None);
        assert_eq!(ollama_origin("not a url"), None);
        assert!(advertises_audio(
            &serde_json::json!({"capabilities":["completion","vision","audio","tools"]})
        ));
        assert!(!advertises_audio(
            &serde_json::json!({"capabilities":["completion","vision"]})
        ));
        assert!(!advertises_audio(&serde_json::json!({})));
    }

    #[tokio::test]
    async fn default_native_catalog_leaves_room_for_a_user_prompt() {
        let prefs = auris_session::AgentPreferences {
            model: "mock".into(),
            ..Default::default()
        };
        let Command::Run(options) = parse_args(&[], &|_| None, &prefs).unwrap() else {
            panic!()
        };
        let agent = build_for_worker(&options, None, false).unwrap();
        let guard = Guard::new(&agent, Some(32768), 4096).await.unwrap();
        assert!(
            guard.schema_tokens + 1024 <= 32768,
            "catalog estimate {} leaves too little default context",
            guard.schema_tokens
        );
    }

    #[tokio::test]
    async fn context_guard_budgets_the_configured_prompt() {
        let mut prefs = auris_session::AgentPreferences {
            model: "mock".into(),
            system_prompt: "Short prompt".into(),
            ..Default::default()
        };
        let Command::Run(short) = parse_args(&[], &|_| None, &prefs).unwrap() else {
            panic!()
        };
        let agent = build_for_worker(&short, None, false).unwrap();
        let short_guard = Guard::new(&agent, Some(32768), 4096).await.unwrap();
        prefs.system_prompt = "長い制作方針。\n".repeat(2000);
        let Command::Run(long) = parse_args(&[], &|_| None, &prefs).unwrap() else {
            panic!()
        };
        let agent = build_for_worker(&long, None, false).unwrap();
        let long_guard = Guard::new(&agent, Some(32768), 4096).await.unwrap();
        assert_eq!(
            long_guard.schema_tokens - short_guard.schema_tokens,
            estimated_tokens(&long.system_prompt) - estimated_tokens(&short.system_prompt)
        );
    }
    use rig_core::message::{CallId, ToolName};

    #[test]
    fn tool_reply_accounting_matches_unwrapped_provider_text() {
        let payload = serde_json::json!({
            "description": "Use {\"operation\":\"list\"}\nthen read the saved project.",
            "project": r"C:\Music\Song\Song.auris",
        })
        .to_string()
        .repeat(100);
        let message = Message::tool_result(
            CallId::from_wire("call_1"),
            ToolName::new("tool_help").unwrap(),
            &payload,
        );
        let provider: Vec<rig_core::providers::ollama::Message> =
            message.clone().try_into().unwrap();
        let rig_core::providers::ollama::Message::ToolResult { content, .. } = &provider[0] else {
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
                ToolName::new("tool_help").unwrap(),
                serde_json::json!({"name":"effects"}),
            )],
        };
        let result = Message::tool_result(
            CallId::from_wire("call_1"),
            ToolName::new("tool_help").unwrap(),
            "Sidechain needs slot and source",
        );
        let history = vec![first_prompt.clone(), call.clone()];
        let mut state = State {
            pending_request: Some(vec![first_prompt]),
            ..Default::default()
        };
        state.observe_context(22_000);
        let guard = Guard {
            failure_limit: 5,
            bridge: None,
            output_tokens: OUTPUT_RESERVE,
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
        let mut guard = guard;
        guard.output_tokens = 8192;
        assert_eq!(
            guard.context_estimate(&result, &history),
            22_000 + message_tokens(&call) + message_tokens(&result) + 8192
        );
        assert!(guard.schema_tokens + history_tokens(&result, &history) > 32768);

        let oversized = Message::tool_result(
            CallId::from_wire("call_1"),
            ToolName::new("tool_help").unwrap(),
            "説明".repeat(10_000),
        );
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
        assert!(
            observed
                .estimate(&next, &original, OUTPUT_RESERVE)
                .is_some()
        );
        let mut changed = original.clone();
        changed[0] = Message::user("A different project");
        assert!(observed.estimate(&next, &changed, OUTPUT_RESERVE).is_none());
        assert!(
            observed
                .estimate(&next, &original[1..], OUTPUT_RESERVE)
                .is_none()
        );

        let image = Message::User {
            content: vec![UserContent::Image(rig_core::message::Image {
                data: rig_core::message::DocumentSourceKind::Url(
                    "https://example.test/large.png".into(),
                ),
                ..Default::default()
            })],
        };
        assert!(
            observed
                .estimate(&image, &original, OUTPUT_RESERVE)
                .is_none()
        );
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
        assert_eq!(validate_model(&model, 32768).unwrap(), 32768);
        assert_eq!(validate_model(&model, 131072).unwrap(), 65536);
        assert!(
            validate_model(
                &serde_json::json!({"model_info":{"arch.context_length":8192}}),
                32768
            )
            .unwrap_err()
            .contains("16384")
        );
        assert_eq!(
            validate_model(&serde_json::json!({}), 32768).unwrap(),
            32768
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
    fn multimodal_context_uses_the_language_architecture() {
        let model = serde_json::json!({"model_info":{
            "general.architecture":"lfm3", "audio.context_length":2048,
            "lfm3.context_length":128000
        }});
        assert_eq!(validate_model(&model, 131072).unwrap(), 128000);
    }

    #[test]
    fn denied_tool_outcomes_release_activity_and_count_as_failures() {
        let key = signature("write_file", r#"{"path":"Song.auris"}"#);
        let mut state = State {
            active_tools: 1,
            ..Default::default()
        };
        record_tool_outcome(&mut state, key.clone(), true);
        assert_eq!(state.active_tools, 0);
        assert!(state.failures.exhausted(&key.0, &key.1, 1));
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
            failure_limit: 5,
            bridge: None,
            output_tokens: OUTPUT_RESERVE,
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
                content: vec![rig_core::message::AssistantContent::tool_call(
                    "call_1",
                    ToolName::new("describe").unwrap(),
                    serde_json::json!({"project":"Song.auris"}),
                )],
            },
            Message::tool_result(
                CallId::from_wire("call_1"),
                ToolName::new("describe").unwrap(),
                "The project has three tracks",
            ),
            Message::assistant("The project has three tracks"),
        ];
        let original = history.clone();
        let guard = Guard {
            failure_limit: 5,
            bridge: None,
            output_tokens: OUTPUT_RESERVE,
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
