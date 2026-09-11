//! Background language-model workers for Auris Studio.
//!
//! Each worker owns its async runtime and communicates with the desktop through channels.
//! Document edits and permission decisions remain on the desktop's session thread.

#![warn(missing_docs)]

use std::path::PathBuf;

use auris_toolbox as toolbox;

use rig::agent::{
    AgentHook, HookContext, ToolCall, ToolCallAction, ToolResultAction, ToolResultEvent,
};
use rig::completion::Message;
use rig::message::ToolResultContent;
use rig::prelude::*;
use rig::providers::{ollama, openai};
use rig::tool::{ToolExecutionError, ToolOutput};
use rig::{Agent, AgentBuilder};

/// Which API dialect the model is spoken to in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Provider {
    /// Ollama's own API, `http://localhost:11434` unless `--url` says otherwise.
    Ollama,
    /// The OpenAI chat-completions dialect — OpenAI itself, or anything compatible
    /// (LM Studio, vLLM, OpenRouter, a proxied Ollama) behind `--url`.
    OpenAi,
}

/// Everything the command line decided.
#[derive(Debug, PartialEq, Eq)]
struct Options {
    /// Explicit Ollama request context, including tools, history and output.
    context_tokens: u32,
    /// Ollama output limit per response.
    output_tokens: u32,
    /// Ollama thinking override.
    thinking: Option<bool>,
    /// The API dialect.
    provider: Provider,
    /// Base URL override; each provider has its own default.
    url: Option<String>,
    /// The model to ask for, in the provider's own naming.
    model: String,
    /// The API key, already read out of the environment.
    key: Option<String>,
    /// The model-call budget for one prompt, counting every turn of the tool loop.
    max_turns: usize,
    /// The one-shot prompt; a conversation is opened instead when there is none.
    prompt: Option<String>,
    /// Audio files to send with the one-shot prompt, in the order given.
    attachments: Vec<String>,
    /// Speak JSON lines on stdin and stdout instead of prose — the mode another program
    /// drives, the desktop's agent panel first among them.
    json: bool,
}

/// What `main` was asked to do.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    /// Run with these options.
    Run(Options),
    /// Ask the provider what models it serves, and print them as one JSON line.
    Models(Options),
    /// Print usage and leave.
    Help,
}

impl Options {
    fn context_limit(&self) -> Option<u32> {
        (self.provider == Provider::Ollama).then_some(self.context_tokens)
    }
}

/// Reads a provider name — the one vocabulary shared by the flag and the preference.
fn provider_named(name: &str) -> Result<Provider, String> {
    match name {
        "" | "ollama" => Ok(Provider::Ollama),
        "openai" => Ok(Provider::OpenAi),
        other => Err(format!(
            "the provider is 'ollama' or 'openai' (any OpenAI-compatible API), not '{other}'"
        )),
    }
}

/// Reads the command line, with the environment and the saved preferences handed in so a test
/// can be its own machine.
///
/// A free function and not a chunk of `main`, because everything here is a decision: which
/// provider a word names, where the key comes from, what is missing. A flag beats the saved
/// preference beats the built-in default — so `auris-agent "..."` with a model in the settings
/// just works, and the settings are what the desktop's agent panel writes. `env` is consulted
/// only for the key — the one value that must never be typed into a command line.
fn parse_args(
    args: &[String],
    env: &dyn Fn(&str) -> Option<String>,
    prefs: &auris_session::AgentPreferences,
) -> Result<Command, String> {
    let mut provider = None;
    let mut url = None;
    let mut model = None;
    let mut key_env = None;
    let mut max_turns = 40usize;
    let mut context_tokens = prefs.context_tokens.unwrap_or(32768);
    let output_tokens = prefs.output_tokens.unwrap_or(4096);
    let mut thinking = prefs.thinking;
    let mut json = false;
    let mut live_session = false;
    let mut attachments: Vec<String> = Vec::new();
    let mut prompt_words: Vec<&str> = Vec::new();

    let mut words = args.iter();
    while let Some(word) = words.next() {
        let mut value_of = |flag: &str| {
            words
                .next()
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match word.as_str() {
            "-h" | "--help" | "help" => return Ok(Command::Help),
            "--provider" => provider = Some(provider_named(&value_of("--provider")?)?),
            "--url" => url = Some(value_of("--url")?),
            "--model" => model = Some(value_of("--model")?),
            "--api-key-env" => key_env = Some(value_of("--api-key-env")?),
            "--max-turns" => {
                let value = value_of("--max-turns")?;
                max_turns = value
                    .parse()
                    .map_err(|_| format!("--max-turns needs a number, not '{value}'"))?;
            }
            "--json" => json = true,
            "--live-session" | "--permission-protocol" => live_session = true,
            "--context-tokens" => {
                context_tokens = value_of("--context-tokens")?
                    .parse()
                    .map_err(|_| "--context-tokens needs a positive integer")?
            }
            "--thinking" => {
                thinking = match value_of("--thinking")?.as_str() {
                    "on" => Some(true),
                    "off" => Some(false),
                    "auto" => None,
                    _ => return Err("--thinking must be on, off or auto".into()),
                }
            }
            "--attach" => attachments.push(value_of("--attach")?),
            flag if flag.starts_with('-') => {
                return Err(format!("unknown option '{flag}' — try --help"));
            }
            _ => prompt_words.push(word),
        }
    }

    let provider = match provider {
        Some(chosen) => chosen,
        None => provider_named(prefs.provider.trim())
            .map_err(|error| format!("the saved agent settings are off: {error}"))?,
    };
    let model = model
        .or_else(|| {
            let saved = prefs.model.trim();
            (!saved.is_empty()).then(|| saved.to_string())
        })
        .ok_or_else(|| {
            "--model names the model to use; there is no sensible default, because it is \
             whatever the server at the other end actually serves"
                .to_string()
        })?;
    let url = url.or_else(|| {
        let saved = prefs.url.trim();
        (!saved.is_empty()).then(|| saved.to_string())
    });
    let key_env = key_env.or_else(|| {
        let saved = prefs.api_key_env.trim();
        (!saved.is_empty()).then(|| saved.to_string())
    });

    // The key: an explicitly named variable must exist — a silently empty key would come back
    // from the server as a 401 with this program's name on it. The OpenAI convention is picked
    // up when present, because that is what every OpenAI-compatible tool trains people to set.
    let key = match key_env {
        Some(name) => Some(env(&name).ok_or_else(|| {
            format!("the API key is named by '{name}', but that variable is not set")
        })?),
        None => match provider {
            Provider::OpenAi => env("OPENAI_API_KEY"),
            Provider::Ollama => None,
        },
    };

    let prompt = match prompt_words.is_empty() {
        true => None,
        false => Some(prompt_words.join(" ")),
    };
    if live_session && !json {
        return Err("--live-session requires --json and a desktop host".into());
    }
    if json && prompt.is_some() {
        return Err("--json is driven over stdin; drop the prompt".to_string());
    }
    if !attachments.is_empty() {
        if json {
            return Err(
                "--json carries audio per message: {\"say\": \"...\", \"audio\": [\"file.wav\"]}"
                    .to_string(),
            );
        }
        if prompt.is_none() {
            return Err("--attach rides with a prompt; give one".to_string());
        }
    }
    if provider == Provider::Ollama && context_tokens < 16384 {
        return Err("Auris tools need at least 16384 context tokens; use --context-tokens 32768 or increase the Agent Panel context setting".into());
    }
    if provider == Provider::Ollama && (output_tokens == 0 || output_tokens >= context_tokens) {
        return Err(
            "Output tokens must be positive and smaller than the Agent Panel context window".into(),
        );
    }
    if max_turns == 0 {
        return Err("--max-turns must be positive".into());
    }
    Ok(Command::Run(Options {
        context_tokens,
        output_tokens,
        thinking,
        provider,
        url,
        model,
        key,
        max_turns,
        prompt,
        attachments,
        json,
    }))
}

/// [`parse_args`], with the `models` subcommand carved off first.
///
/// Listing needs no model — it is how a caller finds one — so the word is taken before the
/// parse that would otherwise insist on `--model`. The stand-in name is never sent anywhere:
/// listing builds a client, and a client does not name a model until it is asked to complete.
fn parse_command(
    args: &[String],
    env: &dyn Fn(&str) -> Option<String>,
    prefs: &auris_session::AgentPreferences,
) -> Result<Command, String> {
    match args.first().map(String::as_str) {
        Some("models") => {
            let mut rest: Vec<String> = args[1..].to_vec();
            rest.push("--model".to_string());
            rest.push("(listing)".to_string());
            match parse_args(&rest, env, prefs)? {
                Command::Run(options) => Ok(Command::Models(options)),
                other => Ok(other),
            }
        }
        _ => parse_args(args, env, prefs),
    }
}

/// What the model is told once, before the conversation: the shared workflow, plus what only
/// this frontend knows — where it is standing, and who it is talking to.
fn preamble() -> String {
    "You edit the document currently open in Auris Studio through edit_project.
Answer in the user's language. Call edit_project with command.action inspect first,
then make dependent edits one at a time using returned IDs.
For a song request, use spec_reference and list_presets, then edit_project with
command.action compose and inline spec or preset. This changes the open arrangement,
including an unsaved empty document. Never replace existing tracks for a local edit.
Use replace=true only for a user-requested replacement. Changes remain unsaved and undoable.
Verify with inspect before claiming completion. Read notes with command.action read_notes.
The live tool uses numeric stable IDs; add_note uses zero-based quarter-note beats
relative to the clip. Use search_documentation for application questions."
        .into()
}

/// A tool's refusal, carried as an error the runtime can classify.
///
/// The text is the whole point: [`auris_toolbox`] writes errors as answers a model can act on,
/// so `map_error` in each tool sends it back model-visible via [`ToolExecutionError::other`] —
/// the explicit constructor that keeps its message — where the default conversion would show
/// the model only "the tool failed".
#[derive(Debug)]
struct ToolFailed(String);

impl std::fmt::Display for ToolFailed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ToolFailed {}

/// The argument schema, exactly as the MCP door serves it: the same `schemars` derive on the
/// same type in `auris-toolbox`.
fn schema<T: schemars::JsonSchema>() -> serde_json::Value {
    toolbox::parameter_schema::<T>()
}

/// No arguments, said as a schema — for the reference and listing tools.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct NoArgs {}

mod worker;
use worker::Bridge;
pub use worker::{Worker, list_models_background};
mod compaction;
mod memory;
mod permissions;
mod runtime;

/// One [`rig::tool::Tool`] over one `auris-toolbox` module that takes arguments.
///
/// The work runs in `spawn_blocking` for the same two reasons as at the MCP door: it blocks
/// honestly (a session open parses SoundFonts, a render is minutes of DSP), and a session
/// created and dropped inside one closure never has to be `Send`.
macro_rules! session_tool {
    ($tool:ident, $module:ident) => {
        struct $tool;

        impl Tool for $tool {
            const NAME: &'static str = toolbox::$module::NAME;
            type Args = toolbox::$module::Args;
            type Output = String;
            type Error = ToolFailed;

            fn description(&self) -> String {
                toolbox::$module::DESCRIPTION.to_string()
            }

            fn parameters(&self) -> serde_json::Value {
                schema::<Self::Args>()
            }

            fn map_error(&self, error: ToolFailed) -> ToolExecutionError {
                ToolExecutionError::other(error.0)
            }

            async fn call(
                &self,
                _context: &mut ToolContext,
                args: Self::Args,
            ) -> Result<String, ToolFailed> {
                tokio::task::spawn_blocking(move || toolbox::$module::run(&args))
                    .await
                    .map_err(|error| ToolFailed(error.to_string()))?
                    .map_err(ToolFailed)
            }
        }
    };
}

/// One [`rig::tool::Tool`] over an argument-less `auris-toolbox` module.
///
/// Through `spawn_blocking` all the same: the listings read the progression book off disk,
/// and uniformity is cheaper than a judgement call per tool.
macro_rules! text_tool {
    ($tool:ident, $module:ident) => {
        struct $tool;

        impl Tool for $tool {
            const NAME: &'static str = toolbox::$module::NAME;
            type Args = NoArgs;
            type Output = String;
            type Error = ToolFailed;

            fn description(&self) -> String {
                toolbox::$module::DESCRIPTION.to_string()
            }

            fn parameters(&self) -> serde_json::Value {
                schema::<NoArgs>()
            }

            fn map_error(&self, error: ToolFailed) -> ToolExecutionError {
                ToolExecutionError::other(error.0)
            }

            async fn call(
                &self,
                _context: &mut ToolContext,
                _args: NoArgs,
            ) -> Result<String, ToolFailed> {
                tokio::task::spawn_blocking(|| Ok(toolbox::$module::run()))
                    .await
                    .map_err(|error| ToolFailed(error.to_string()))?
            }
        }
    };
}

session_tool!(SearchDocumentation, search_documentation);
text_tool!(SpecReference, spec_reference);
text_tool!(ListProgressions, list_progressions);
text_tool!(ListPresets, list_presets);
text_tool!(ListInstruments, list_instruments);

/// One live session command, wrapped in an object for provider tool schemas.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditProjectArgs {
    /// Operation to execute on the document currently open in the desktop.
    command: auris_session::live_agent::Command,
}

/// Editing is executed by the desktop in its current session.
struct EditProject {
    bridge: Option<Bridge>,
}

impl Tool for EditProject {
    const NAME: &'static str = "edit_project";
    type Args = EditProjectArgs;
    type Output = String;
    type Error = ToolFailed;
    fn description(&self) -> String {
        "Inspect or edit the currently open document. Edits appear immediately and are undoable. Use command.action inspect for numeric track/clip IDs. No project path is needed.".into()
    }
    fn parameters(&self) -> serde_json::Value {
        schema::<EditProjectArgs>()
    }
    fn map_error(&self, error: ToolFailed) -> ToolExecutionError {
        ToolExecutionError::other(error.0)
    }
    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<String, ToolFailed> {
        let bridge = self.bridge.as_ref().ok_or_else(|| {
            ToolFailed("Open the Agent Panel in Auris Studio to edit the current document".into())
        })?;
        let response = bridge
            .exchange(serde_json::json!({"event":"edit", "command":args.command}))
            .await
            .map_err(ToolFailed)?;
        edit_reply(response)
    }
}

fn edit_reply(response: serde_json::Value) -> Result<String, ToolFailed> {
    if response["event"] != "edit_result" {
        return Err(ToolFailed("The live session response was missing".into()));
    }
    let text = response["text"]
        .as_str()
        .ok_or_else(|| ToolFailed("Missing edit result text".into()))?
        .to_string();
    if response["ok"] == true {
        Ok(text)
    } else {
        Err(ToolFailed(text))
    }
}

/// Arguments to the agent-only internet search tool.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct InternetSearchArgs {
    /// Words or a short phrase to search for on the public internet.
    query: String,
    /// Maximum results to return, from 1 to 10. Defaults to 5.
    limit: Option<usize>,
}

/// Public internet search for the rig agent.
///
/// This does not live in [`auris_toolbox`]: it is a capability of the agent that dials out,
/// not a command over the Auris session, and the MCP host is expected to provide its own web
/// access when it wants one.
struct InternetSearch;

impl Tool for InternetSearch {
    const NAME: &'static str = "search_internet";
    type Args = InternetSearchArgs;
    type Output = String;
    type Error = ToolFailed;

    fn description(&self) -> String {
        "Searches the public internet for current or external information. The query is sent to \
         the Brave Search API and the answer contains result titles, URLs, and snippets; use \
         `search_documentation` instead for Auris Studio's own behavior."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        schema::<Self::Args>()
    }

    fn map_error(&self, error: ToolFailed) -> ToolExecutionError {
        ToolExecutionError::other(error.0)
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<String, ToolFailed> {
        search_internet(&args).await.map_err(ToolFailed)
    }
}

const INTERNET_SEARCH_PATIENCE: std::time::Duration = std::time::Duration::from_secs(20);

#[derive(Debug, serde::Deserialize)]
struct SearchResponse {
    web: Option<SearchWeb>,
}

#[derive(Debug, serde::Deserialize)]
struct SearchWeb {
    #[serde(default)]
    results: Vec<SearchResult>,
}

#[derive(Debug, serde::Deserialize)]
struct SearchResult {
    title: String,
    url: String,
    #[serde(default)]
    description: String,
}

async fn search_internet(args: &InternetSearchArgs) -> Result<String, String> {
    let query = args.query.trim();
    if query.is_empty() {
        return Err("the internet search needs a non-empty query".to_string());
    }
    if query.chars().count() > 400 || query.split_whitespace().count() > 50 {
        return Err(
            "the internet search query must fit in 400 characters and 50 words".to_string(),
        );
    }
    let key = std::env::var("BRAVE_SEARCH_API_KEY").map_err(|_| {
        "internet search needs BRAVE_SEARCH_API_KEY in the application environment; create a \
         Brave Search API key and restart the agent"
            .to_string()
    })?;
    if key.trim().is_empty() {
        return Err("BRAVE_SEARCH_API_KEY is set but empty".to_string());
    }
    let limit = args.limit.unwrap_or(5).clamp(1, 10);
    let count = limit.to_string();
    let client = reqwest::Client::builder()
        .timeout(INTERNET_SEARCH_PATIENCE)
        .user_agent(concat!("auris-agent/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| format!("could not build the internet search client: {error}"))?;
    let response = client
        .get("https://api.search.brave.com/res/v1/web/search")
        .header("Accept", "application/json")
        .header("X-Subscription-Token", key)
        .query(&[("q", query), ("count", count.as_str())])
        .send()
        .await
        .map_err(|error| format!("internet search could not reach Brave Search: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Brave Search refused the internet search: {error}"))?;
    let response = response
        .json::<SearchResponse>()
        .await
        .map_err(|error| format!("could not read Brave Search's answer: {error}"))?;
    let results = response.web.map(|web| web.results).unwrap_or_default();
    if results.is_empty() {
        return Ok(format!("No internet results matched `{query}`."));
    }
    let mut answer = format!("Internet search results for `{query}`:\n");
    for (index, result) in results.iter().take(limit).enumerate() {
        answer.push_str(&format!(
            "\n{}. {}\n{}\n{}\n",
            index + 1,
            strip_markup(&result.title),
            result.url,
            strip_markup(&result.description)
        ));
    }
    Ok(answer)
}

fn strip_markup(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut inside_tag = false;
    for character in text.chars() {
        match character {
            '<' => inside_tag = true,
            '>' => inside_tag = false,
            _ if !inside_tag => plain.push(character),
            _ => {}
        }
    }
    plain
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// File-free editing and reference tools for the rig agent.
fn armed(builder: AgentBuilder, bridge: Option<Bridge>) -> Agent {
    builder
        .preamble(&preamble())
        .tool(EditProject { bridge })
        .tool(SearchDocumentation)
        .tool(InternetSearch)
        .tool(SpecReference)
        .tool(ListProgressions)
        .tool(ListPresets)
        .tool(ListInstruments)
        .build()
}

/// Builds the agent for whichever door the options chose.
///
/// Both arms end in the same [`armed`] call; only the client construction differs, because the
/// two dialects are two client types. An empty key means no key — both providers' key types
/// treat it that way or tolerate it, and it saves an `Option` dance at each arm.
#[cfg(test)]
fn build_agent(options: &Options) -> Result<Agent, String> {
    build_for_worker(options, None)
}

fn build_for_worker(options: &Options, bridge: Option<Bridge>) -> Result<Agent, String> {
    build_with(options, |builder| armed(builder, bridge))
}

fn build_with(
    options: &Options,
    armed: impl FnOnce(AgentBuilder) -> Agent,
) -> Result<Agent, String> {
    let key = options.key.clone().unwrap_or_default();
    let could_not = |error: rig::http_client::Error| format!("could not build a client: {error}");
    match options.provider {
        Provider::Ollama => {
            let mut builder = ollama::Client::builder().api_key(key);
            if let Some(url) = &options.url {
                builder = builder.base_url(url);
            }
            let client = builder.build().map_err(could_not)?;
            let mut params = serde_json::json!({"num_ctx":options.context_tokens});
            if let Some(thinking) = options.thinking {
                params["think"] = thinking.into();
            }
            Ok(armed(
                // Tool arguments benefit from repeatability; do not inherit a local model's
                // high-temperature chat preset. This changes only this request, not Ollama.
                client
                    .agent(&options.model)
                    .temperature(0.0)
                    .max_tokens(u64::from(options.output_tokens))
                    .additional_params(params),
            ))
        }
        Provider::OpenAi => {
            // The chat-completions client, not the default responses-API one: compatible
            // servers implement `/chat/completions`, and real OpenAI serves it too.
            let mut builder = openai::CompletionsClient::builder().api_key(key);
            if let Some(url) = &options.url {
                builder = builder.base_url(url);
            }
            let client = builder.build().map_err(could_not)?;
            Ok(armed(client.agent(&options.model)))
        }
    }
}

/// How long `models` waits for the provider before calling it unreachable.
///
/// Listing is a couple of small requests to a local or nearby server; twenty seconds is
/// generous for a slow one and short enough that a picker's spinner resolves into an answer
/// rather than spinning forever.
const MODEL_LIST_PATIENCE: std::time::Duration = std::time::Duration::from_secs(20);

/// How long one prompt, including its tool loop, may keep the caller waiting.
///
/// Generation and project tools can reasonably take much longer than listing models, but the
/// desktop panel still needs a stalled provider to become an error rather than a permanent
/// spinner.
const CONVERSATION_PATIENCE: std::time::Duration = std::time::Duration::from_secs(5 * 60);
const MODEL_DETAIL_PATIENCE: std::time::Duration = std::time::Duration::from_secs(5);

/// Ask the provider what models it serves, returning one
/// JSON line — `{"models": [{"name", "context_length"}, …]}`.
///
/// A machine-readable list because its one caller so far is the desktop's agent panel, which
/// fills its model picker from it; the context length rides along because the panel's context
/// gauge has no other way to know how big the window it is filling is.
async fn list_models(options: &Options) -> Result<String, String> {
    let key = options.key.clone().unwrap_or_default();
    let models: Vec<serde_json::Value> = match options.provider {
        // Ollama is asked in its own words rather than through the SDK's lister, which keeps
        // the names and drops the context lengths — and a gauge with no ceiling is no gauge.
        Provider::Ollama => {
            let base = options
                .url
                .clone()
                .unwrap_or_else(|| "http://localhost:11434".to_string());
            let base = base.trim_end_matches('/');
            let http = reqwest::Client::new();
            let ask = |request: reqwest::RequestBuilder| {
                let key = key.clone();
                async move {
                    let request = match key.is_empty() {
                        true => request,
                        false => request.bearer_auth(key),
                    };
                    request
                        .send()
                        .await
                        .map_err(|error| error.to_string())?
                        .json::<serde_json::Value>()
                        .await
                        .map_err(|error| error.to_string())
                }
            };
            let tags = ask(http.get(format!("{base}/api/tags"))).await?;
            let listed = tags
                .get("models")
                .and_then(|models| models.as_array())
                .ok_or("no models in Ollama's answer")?;
            let mut models = Vec::new();
            let mut details = tokio::task::JoinSet::new();
            for entry in listed {
                let Some(name) = entry.get("name").and_then(|name| name.as_str()) else {
                    continue;
                };
                // Newer servers put the window in the tags themselves; for the rest it is in
                // `/api/show`, under the architecture's own key — found by suffix, because the
                // prefix is the architecture's name.
                let window = entry
                    .get("details")
                    .and_then(|details| details.get("context_length"))
                    .and_then(|window| window.as_u64());
                if window.is_none() {
                    let http = http.clone();
                    let base = base.to_string();
                    let key = key.clone();
                    let name = name.to_string();
                    details.spawn(async move {
                        let request = http
                            .post(format!("{base}/api/show"))
                            .json(&serde_json::json!({ "model": name }));
                        let request = if key.is_empty() {
                            request
                        } else {
                            request.bearer_auth(key)
                        };
                        let shown = tokio::time::timeout(MODEL_DETAIL_PATIENCE, async {
                            request
                                .send()
                                .await
                                .map_err(|error| error.to_string())?
                                .json::<serde_json::Value>()
                                .await
                                .map_err(|error| error.to_string())
                        })
                        .await
                        .ok()
                        .and_then(Result::ok);
                        let window = shown
                            .as_ref()
                            .and_then(|shown| shown.get("model_info"))
                            .and_then(|info| info.as_object())
                            .and_then(|info| {
                                info.iter()
                                    .find(|(key, _)| key.ends_with(".context_length"))
                                    .and_then(|(_, value)| value.as_u64())
                            });
                        (name, window)
                    });
                }
                models.push(serde_json::json!({ "name": name, "context_length": window }));
            }
            while let Some(Ok((name, window))) = details.join_next().await {
                if let Some(window) = window
                    && let Some(model) = models.iter_mut().find(|model| {
                        model.get("name").and_then(|value| value.as_str()) == Some(&name)
                    })
                {
                    model["context_length"] = serde_json::json!(window);
                }
            }
            for model in &mut models {
                let maximum = model["context_length"].as_u64();
                model["max_context_length"] = maximum.into();
                model["context_length"] =
                    serde_json::json!(maximum.map_or(u64::from(options.context_tokens), |max| {
                        max.min(u64::from(options.context_tokens))
                    }));
                model["context_source"] = "requested_num_ctx".into();
            }
            models
        }
        Provider::OpenAi => {
            let mut builder = openai::CompletionsClient::builder().api_key(key);
            if let Some(url) = &options.url {
                builder = builder.base_url(url);
            }
            let list = builder
                .build()
                .map_err(|error| format!("could not build a client: {error}"))?
                .list_models()
                .await
                .map_err(|error| error.to_string())?;
            list.data
                .iter()
                .map(|model| {
                    serde_json::json!({
                        "name": model.id,
                        "context_length": model.context_length,
                    })
                })
                .collect()
        }
    };
    Ok(serde_json::json!({ "models": models }).to_string())
}

/// A tool answer's whole text, for a host that will render it itself.
fn full_text(output: &ToolOutput) -> String {
    output
        .as_content()
        .iter()
        .filter_map(|content| match content {
            ToolResultContent::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// One line of the host's side of the wire: `{"say": "..."}`, with an optional `"audio"`
/// array naming files to send along with the words.
fn parse_say(line: &str) -> Result<(String, Vec<String>), String> {
    let parsed: serde_json::Value =
        serde_json::from_str(line).map_err(|error| format!("not JSON: {error}"))?;
    let said = parsed
        .get("say")
        .and_then(|say| say.as_str())
        .map(str::to_string)
        .ok_or_else(|| "expected {\"say\": \"...\"}".to_string())?;
    let audio = parsed
        .get("audio")
        .and_then(|audio| audio.as_array())
        .map(|paths| {
            paths
                .iter()
                .filter_map(|path| path.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Ok((said, audio))
}

/// The media type an audio file's extension names, in rig's vocabulary.
///
/// By extension and not by sniffing, because the mistake worth catching is a typo'd path or a
/// MIDI file, and both fail louder later anyway; a wrong extension on real audio is the one
/// case sniffing would win, and it is not worth a decoder.
fn audio_media_type(path: &std::path::Path) -> Result<rig::message::AudioMediaType, String> {
    use rig::message::AudioMediaType as Type;
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "wav" => Ok(Type::WAV),
        "mp3" => Ok(Type::MP3),
        "aiff" | "aif" => Ok(Type::AIFF),
        "aac" => Ok(Type::AAC),
        "ogg" => Ok(Type::OGG),
        "flac" => Ok(Type::FLAC),
        "m4a" => Ok(Type::M4A),
        _ => Err(format!(
            "cannot tell what kind of audio '{}' is — wav, mp3, aiff, aac, ogg, flac and m4a \
             are understood",
            path.display()
        )),
    }
}

/// The largest audio file worth base64-ing into one request, in bytes.
///
/// Twenty-five megabytes — the ceiling the big providers publish for audio uploads, so the
/// refusal here reads the same as the one the server would have sent, minus the upload. It
/// also keeps a mistyped path to something enormous from being read whole into memory and
/// grown a third again by the encoding before anything could object.
const ATTACHMENT_CEILING: u64 = 25 * 1024 * 1024;

/// One user turn with instructions followed by base64-encoded, typed audio attachments.
/// This order also matches the Gemma4 audio model's documented input format.
fn framed_message(text: &str, audio: &[String]) -> Result<Message, String> {
    let mut content = vec![rig::message::UserContent::text(text)];
    for path in audio {
        let path = std::path::Path::new(path);
        let media_type = audio_media_type(path)?;
        let size = std::fs::metadata(path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?
            .len();
        if size > ATTACHMENT_CEILING {
            return Err(format!(
                "{} is {} MB; audio over {} MB is refused before it is read — trim or \
                 convert it first",
                path.display(),
                size / (1024 * 1024),
                ATTACHMENT_CEILING / (1024 * 1024),
            ));
        }
        let bytes = std::fs::read(path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(bytes);
        content.push(rig::message::UserContent::audio(data, Some(media_type)));
    }
    Ok(Message::User { content })
}

/// Refuses attachments unsupported by rig's Ollama adapter before reading any file.
///
/// rig's Ollama conversion would refuse too, but only after the whole request is built; this
/// says it in this program's own words, at the moment the attachment is asked for.
fn check_audio(provider: Provider, audio: &[String]) -> Result<(), String> {
    match provider {
        Provider::Ollama if !audio.is_empty() => Err(
            "This agent's rig Ollama adapter cannot send audio attachments. Use listen for \
             a separate audio critic, or --provider openai against an audio-capable endpoint \
             implementing input_audio (including a compatible Ollama /v1 endpoint)."
                .to_string(),
        ),
        _ => Ok(()),
    }
}

/// Reports the tool loop as JSON events on stdout, for a host program to render.
///
/// Reports `call` when a
/// tool is asked, `result` when it answers, and `changed` when the answer means a project
/// file on disk is no longer what the host last read.
struct Reporter {
    bridge: Option<Bridge>,
}

/// A skipped or refused call is a failed result to the host, even without an execution error.
fn result_event(event: ToolResultEvent<'_>) -> serde_json::Value {
    serde_json::json!({
        "event": "result", "tool": event.tool_name,
        "ok": event.raw_result.is_success(),
        "text": full_text(event.presentation),
    })
}

impl AgentHook for Reporter {
    async fn on_tool_call(&self, _ctx: &HookContext, event: ToolCall<'_>) -> ToolCallAction {
        if let Some(bridge) = &self.bridge {
            bridge.emit(serde_json::json!({
                "event": "call", "tool": event.tool_name, "args": event.args,
            }));
        }
        match permissions::authorize(self.bridge.as_ref(), event.tool_name, event.args).await {
            Ok(()) => ToolCallAction::Run,
            Err(reason) => ToolCallAction::Skip(reason),
        }
    }

    async fn on_tool_result(
        &self,
        _ctx: &HookContext,
        event: ToolResultEvent<'_>,
    ) -> ToolResultAction {
        if let Some(bridge) = &self.bridge {
            bridge.emit(result_event(event));
        }
        ToolResultAction::Keep
    }
}

/// One prompt through the loop: ask, narrate, answer — and hand back the transcript so a
/// conversation can keep it.
async fn converse_with_bridge(
    agent: &Agent,
    prompt: Message,
    mut history: Vec<Message>,
    max_turns: usize,
    bridge: Option<Bridge>,
    context_tokens: Option<u32>,
    output_tokens: u32,
) -> Result<(String, Vec<Message>, rig::completion::Usage, u64), String> {
    let guard = runtime::Guard::new(agent, context_tokens, output_tokens).await?;
    let omitted = guard.fit_history(&prompt, &mut history);
    if omitted > 0 {
        let message = format!(
            "Omitted {omitted} older conversation exchanges from this request to fit the model context. Saved conversation history is unchanged."
        );
        if let Some(bridge) = &bridge {
            bridge.emit(serde_json::json!({"event":"notice", "message":message}));
        }
    }
    let activity = guard.activity.clone();
    let asked = prompt.clone();
    let request = agent
        .prompt(prompt)
        .history(history.clone())
        .max_turns(max_turns)
        .max_invalid_tool_call_retries(2)
        .add_hook(guard);
    let request = request.add_hook(Reporter { bridge });
    let response = runtime::await_active(request.extended_details(), activity).await?;
    // The run's usage sums every model call; only the final request measures occupied context.
    let context_tokens = response
        .completion_calls
        .last()
        .map_or(0, |call| call.usage.input_tokens);
    // The runner hands the accumulated transcript back; when it does not, the two ends of the
    // exchange are still worth keeping — better a thin memory than none.
    let history = response.messages.unwrap_or_else(|| {
        let mut kept = history;
        kept.push(asked);
        kept.push(Message::assistant(&response.output));
        kept
    });
    Ok((response.output, history, response.usage, context_tokens))
}

/// Waits for a provider operation without allowing a dead connection to park its caller forever.
#[cfg(test)]
async fn await_model_request<F, T, E>(
    request: F,
    patience: std::time::Duration,
) -> Result<T, String>
where
    F: std::future::IntoFuture<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    tokio::time::timeout(patience, request.into_future())
        .await
        .map_err(|_| {
            format!(
                "model request timed out after {:.1} seconds",
                patience.as_secs_f64()
            )
        })?
        .map_err(|error| error.to_string())
}

async fn json_conversation(
    agent: &Agent,
    options: &Options,
    bridge: &Bridge,
    memory_path: Option<PathBuf>,
    fresh_history: bool,
) -> Result<(), String> {
    let mut memory = match &memory_path {
        Some(path) if !fresh_history => bridge.history(|| memory::Memory::load(path))?,
        _ => memory::Memory::default(),
    };
    if let Some(path) = &memory_path {
        bridge.history(|| memory.save(path))?;
    }
    bridge.emit(serde_json::json!({ "event": "history", "turns": memory.turns }));
    bridge.emit(serde_json::json!({
        "event": "ready",
        "provider": match options.provider {
            Provider::Ollama => "ollama",
            Provider::OpenAi => "openai",
        },
        "model": options.model,
    }));
    let mut history = memory.messages();
    while let Some(line) = bridge.receive().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let wire: serde_json::Value = match serde_json::from_str(line) {
            Ok(wire) => wire,
            Err(error) => {
                bridge.emit(serde_json::json!({"event":"error", "message":error.to_string()}));
                continue;
            }
        };
        let manual = wire.get("compact").and_then(|v| v.as_bool()) == Some(true);
        let percent = wire
            .get("auto_compact_percent")
            .and_then(|v| v.as_u64())
            .unwrap_or(85)
            .min(99);
        if manual || compaction::needed(&memory, options.context_tokens, percent) {
            bridge.emit(serde_json::json!({"event":"compacting"}));
            let result = compaction::compact(options, &mut memory).await;
            history = memory.messages();
            if result.is_ok()
                && let Some(path) = &memory_path
                && let Err(error) = bridge.history(|| memory.save(path))
            {
                bridge.emit(
                    serde_json::json!({"event":"notice", "message":format!("Summary was not saved: {error}")}),
                );
            }
            let (ok, text) = match result {
                Ok(text) => (true, text),
                Err(error) => (false, error),
            };
            bridge.emit(
                serde_json::json!({"event":"compacted", "ok":ok, "message":text, "manual":manual,
                "context_tokens":compaction::tokens(&memory)}),
            );
            if manual {
                continue;
            }
        }
        let (mut said, audio) = match parse_say(line) {
            Ok(parsed) => parsed,
            Err(message) => {
                bridge.emit(serde_json::json!({ "event": "error", "message": message }));
                continue;
            }
        };
        if let Some(policy) = wire.get("policy")
            && let Ok(policy) =
                serde_json::from_value::<auris_session::agent_policy::Policy>(policy.clone())
        {
            said = format!(
                "[User-selected permission mode: {}. In plan mode investigate and present a plan without changes. Deny rules: {:?}; allow rules: {:?}.]\n{said}",
                policy.mode.name(),
                policy.deny,
                policy.allow
            );
        }
        let message = match check_audio(options.provider, &audio)
            .and_then(|()| framed_message(&said, &audio))
        {
            Ok(message) => message,
            Err(message) => {
                bridge.emit(serde_json::json!({ "event": "error", "message": message }));
                continue;
            }
        };
        match converse_with_bridge(
            agent,
            message,
            history.clone(),
            options.max_turns,
            Some(bridge.clone()),
            options.context_limit(),
            options.output_tokens,
        )
        .await
        {
            Ok((answer, _, usage, context_tokens)) => {
                let display = serde_json::from_str::<serde_json::Value>(line)
                    .ok()
                    .and_then(|wire| {
                        wire.get("display")
                            .and_then(|value| value.as_str())
                            .map(String::from)
                    })
                    .unwrap_or_else(|| said.clone());
                memory.push(&display, &answer);
                history = memory.messages();
                if let Some(path) = &memory_path
                    && let Err(error) = bridge.history(|| memory.save(path))
                {
                    bridge.emit(
                        serde_json::json!({ "event": "notice", "message": format!("Conversation history was not saved: {error}") }),
                    );
                }
                bridge.emit(serde_json::json!({
                    "event": "answer",
                    "text": answer,
                    "input_tokens": context_tokens,
                    "total_input_tokens": usage.input_tokens,
                    "output_tokens": usage.output_tokens,
                }));
            }
            Err(message) => {
                memory.push_interrupted(&said, &message);
                history = memory.messages();
                if let Some(path) = &memory_path
                    && let Err(error) = bridge.history(|| memory.save(path))
                {
                    bridge.emit(
                        serde_json::json!({"event":"notice","message":format!("Conversation history was not saved: {error}")}),
                    );
                }
                bridge.emit(serde_json::json!({ "event": "error", "message": message }));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
async fn converse(
    agent: &Agent,
    prompt: Message,
    history: Vec<Message>,
    max_turns: usize,
    _json: bool,
    context: Option<u32>,
) -> Result<(String, Vec<Message>, rig::completion::Usage, u64), String> {
    converse_with_bridge(agent, prompt, history, max_turns, None, context, 4096).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_stalled_model_request_becomes_an_error() {
        let stalled = std::future::pending::<Result<(), &'static str>>();
        let error = await_model_request(stalled, std::time::Duration::from_millis(5))
            .await
            .unwrap_err();
        assert!(error.contains("timed out"), "{error}");
    }

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(String::from).collect()
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn no_prefs() -> auris_session::AgentPreferences {
        auris_session::AgentPreferences::default()
    }

    fn parse(args: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<Command, String> {
        parse_args(&words(args), env, &no_prefs())
    }

    #[test]
    fn a_model_is_required_and_the_rest_defaults() {
        let refused = parse("write me a song", &no_env).unwrap_err();
        assert!(refused.contains("--model"), "{refused}");

        let Command::Run(options) = parse("--model qwen3:8b write me a song", &no_env).unwrap()
        else {
            panic!("a full command runs");
        };
        assert_eq!(options.provider, Provider::Ollama);
        assert_eq!(options.url, None);
        assert_eq!(options.key, None);
        assert_eq!(options.max_turns, 40);
        assert_eq!(options.prompt.as_deref(), Some("write me a song"));
    }

    #[test]
    fn no_prompt_means_a_conversation() {
        let Command::Run(options) = parse("--model m", &no_env).unwrap() else {
            panic!("a bare model still runs");
        };
        assert_eq!(options.prompt, None);
    }

    #[test]
    fn the_key_comes_from_the_environment_and_its_absence_is_an_error() {
        let env = |name: &str| (name == "MY_KEY").then(|| "secret".to_string());

        let Command::Run(options) = parse("--model m --api-key-env MY_KEY", &env).unwrap() else {
            panic!("a named key that exists runs");
        };
        assert_eq!(options.key.as_deref(), Some("secret"));

        let missing = parse("--model m --api-key-env NOT_SET", &env).unwrap_err();
        assert!(missing.contains("NOT_SET"), "{missing}");

        // The OpenAI convention is picked up unasked, but only for the openai provider.
        let convention = |name: &str| (name == "OPENAI_API_KEY").then(|| "sk-x".to_string());
        let Command::Run(options) = parse("--model m --provider openai", &convention).unwrap()
        else {
            panic!("openai with the conventional key runs");
        };
        assert_eq!(options.key.as_deref(), Some("sk-x"));
        let Command::Run(options) = parse("--model m", &convention).unwrap() else {
            panic!("ollama ignores the OpenAI convention");
        };
        assert_eq!(options.key, None);
    }

    #[test]
    fn the_saved_preferences_fill_in_what_the_flags_leave_out() {
        let prefs = auris_session::AgentPreferences {
            provider: "openai".to_string(),
            model: "saved-model".to_string(),
            url: "http://saved:1234/v1".to_string(),
            api_key_env: "SAVED_KEY".to_string(),
            ..Default::default()
        };
        let env = |name: &str| (name == "SAVED_KEY").then(|| "from-saved".to_string());

        let Command::Run(options) = parse_args(&words(""), &env, &prefs).unwrap() else {
            panic!("a fully saved configuration runs with no flags at all");
        };
        assert_eq!(options.provider, Provider::OpenAi);
        assert_eq!(options.model, "saved-model");
        assert_eq!(options.url.as_deref(), Some("http://saved:1234/v1"));
        assert_eq!(options.key.as_deref(), Some("from-saved"));

        // A flag beats the preference, field by field.
        let Command::Run(options) =
            parse_args(&words("--model spoken --provider ollama"), &env, &prefs).unwrap()
        else {
            panic!("flags over preferences run");
        };
        assert_eq!(options.model, "spoken");
        assert_eq!(options.provider, Provider::Ollama);

        // A preference file holding nonsense is named as the problem, not the flags.
        let broken = auris_session::AgentPreferences {
            provider: "gemini".to_string(),
            model: "m".to_string(),
            ..Default::default()
        };
        let refused = parse_args(&words(""), &no_env, &broken).unwrap_err();
        assert!(refused.contains("saved"), "{refused}");
    }

    #[test]
    fn the_models_subcommand_needs_no_model() {
        // Listing is how a caller finds a model, so insisting on one would be circular.
        let Command::Models(options) =
            parse_command(&words("models"), &no_env, &no_prefs()).unwrap()
        else {
            panic!("`models` lists");
        };
        assert_eq!(options.provider, Provider::Ollama);
        let Command::Models(options) = parse_command(
            &words("models --provider openai --url http://x/v1"),
            &no_env,
            &no_prefs(),
        )
        .unwrap() else {
            panic!("`models` takes the connection flags");
        };
        assert_eq!(options.provider, Provider::OpenAi);
        assert_eq!(options.url.as_deref(), Some("http://x/v1"));
    }

    #[test]
    fn json_mode_is_stdin_driven_and_refuses_a_prompt() {
        let Command::Run(options) = parse("--model m --json", &no_env).unwrap() else {
            panic!("json mode runs");
        };
        assert!(options.json);
        let refused = parse("--model m --json do a thing", &no_env).unwrap_err();
        assert!(refused.contains("stdin"), "{refused}");
    }

    #[test]
    fn the_wire_reads_says_and_nothing_else() {
        assert_eq!(
            parse_say(r#"{"say":"hello"}"#).unwrap(),
            ("hello".to_string(), Vec::new())
        );
        assert_eq!(
            parse_say(r#"{"say":"listen","audio":["mix.wav","stem.mp3"]}"#).unwrap(),
            (
                "listen".to_string(),
                vec!["mix.wav".to_string(), "stem.mp3".to_string()]
            )
        );
        assert!(parse_say("not json").unwrap_err().contains("JSON"));
        assert!(
            parse_say(r#"{"shout":"hello"}"#)
                .unwrap_err()
                .contains("say")
        );
    }

    #[test]
    fn audio_is_typed_by_extension_and_refused_where_no_api_takes_it() {
        use rig::message::AudioMediaType;
        let of = |name: &str| audio_media_type(std::path::Path::new(name));
        assert_eq!(of("Mix.WAV").unwrap(), AudioMediaType::WAV);
        assert_eq!(of("take.flac").unwrap(), AudioMediaType::FLAC);
        assert_eq!(of("old.aif").unwrap(), AudioMediaType::AIFF);
        let refused = of("song.mid").unwrap_err();
        assert!(refused.contains("song.mid"), "{refused}");

        let audio = vec!["mix.wav".to_string()];
        let refused = check_audio(Provider::Ollama, &audio).unwrap_err();
        assert!(refused.contains("openai"), "{refused}");
        check_audio(Provider::Ollama, &[]).unwrap();
        check_audio(Provider::OpenAi, &audio).unwrap();
    }

    #[test]
    fn attachments_ride_a_prompt_and_never_the_json_wire() {
        let Command::Run(options) = parse(
            "--model m --attach a.wav --attach b.mp3 listen to this",
            &no_env,
        )
        .unwrap() else {
            panic!("a prompt with attachments runs");
        };
        assert_eq!(options.attachments, vec!["a.wav", "b.mp3"]);

        let refused = parse("--model m --json --attach a.wav", &no_env).unwrap_err();
        assert!(refused.contains("audio"), "{refused}");
        let refused = parse("--model m --attach a.wav", &no_env).unwrap_err();
        assert!(refused.contains("prompt"), "{refused}");
    }

    #[test]
    fn skipped_and_refused_tools_are_reported_as_unsuccessful() {
        use rig::tool::ToolResult;

        let context = ToolContext::new();
        for result in [
            ToolResult::skipped("outside the working directory"),
            ToolResult::failed(ToolExecutionError::refused("not permitted")),
            ToolResult::failed(ToolExecutionError::invalid_args("missing project")),
            ToolResult::success(ToolOutput::text("saved")),
        ] {
            let event = result_event(ToolResultEvent {
                tool_name: "set_level",
                tool_call_id: Some("call_1"),
                internal_call_id: "internal_1",
                args: "{}",
                presentation: result.output(),
                raw_result: &result,
                tool_context: &context,
            });
            assert_eq!(event["ok"], result.is_success());
            assert_eq!(event["text"], full_text(result.output()));
        }
    }

    #[test]
    fn bad_flags_are_named_back() {
        let unknown = parse("--model m --loud", &no_env).unwrap_err();
        assert!(unknown.contains("--loud"), "{unknown}");
        let provider = parse("--model m --provider gemini", &no_env).unwrap_err();
        assert!(provider.contains("gemini"), "{provider}");
        let turns = parse("--model m --max-turns many", &no_env).unwrap_err();
        assert!(turns.contains("many"), "{turns}");
        let dangling = parse("--model", &no_env).unwrap_err();
        assert!(dangling.contains("--model"), "{dangling}");
    }

    /// A scripted OpenAI-compatible server: each connection gets the next canned body.
    ///
    /// Real enough for the client (HTTP/1.1, `Content-Length`, `Connection: close`) and no
    /// more; what it captures is the request bodies, which is what the assertions read.
    pub(super) fn mock_server(
        responses: Vec<String>,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for (index, connection) in listener.incoming().enumerate() {
                let Ok(mut connection) = connection else {
                    break;
                };
                let mut raw = Vec::new();
                let mut chunk = [0u8; 4096];
                // Read headers, find the length, then read exactly the body.
                let body = loop {
                    let got = connection.read(&mut chunk).unwrap_or(0);
                    if got == 0 {
                        break String::new();
                    }
                    raw.extend_from_slice(&chunk[..got]);
                    let text = String::from_utf8_lossy(&raw);
                    if let Some(split) = text.find("\r\n\r\n") {
                        let length: usize = text
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|value| value.trim().parse().unwrap())
                            })
                            .unwrap_or(0);
                        let mut body = raw[split + 4..].to_vec();
                        while body.len() < length {
                            let got = connection.read(&mut chunk).unwrap_or(0);
                            if got == 0 {
                                break;
                            }
                            body.extend_from_slice(&chunk[..got]);
                        }
                        break String::from_utf8_lossy(&body).into_owned();
                    }
                };
                log.lock().unwrap().push(body);
                let Some(reply) = responses.get(index) else {
                    break;
                };
                let _ = write!(
                    connection,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                if index + 1 == responses.len() {
                    break;
                }
            }
        });
        (url, seen)
    }

    /// One canned chat-completions response around the given `message` object.
    pub(super) fn completion(message: &str, finish_reason: &str) -> String {
        format!(
            r#"{{"id":"chatcmpl-1","object":"chat.completion","created":0,"model":"mock","choices":[{{"index":0,"message":{message},"logprobs":null,"finish_reason":"{finish_reason}"}}],"usage":null}}"#
        )
    }

    #[test]
    fn output_limit_preferences_are_validated_and_defaulted() {
        let mut prefs = auris_session::AgentPreferences {
            model: "mock".into(),
            ..Default::default()
        };
        let Command::Run(default) = parse_args(&[], &no_env, &prefs).unwrap() else {
            panic!()
        };
        assert_eq!(default.output_tokens, 4096);
        prefs.output_tokens = Some(8192);
        let Command::Run(custom) = parse_args(&[], &no_env, &prefs).unwrap() else {
            panic!()
        };
        assert_eq!(custom.output_tokens, 8192);
        for limit in [0, 32768, u32::MAX] {
            prefs.output_tokens = Some(limit);
            assert!(parse_args(&[], &no_env, &prefs).is_err());
        }
    }

    #[tokio::test]
    async fn ollama_receives_the_requested_context_and_thinking_setting() {
        let (url, seen) = mock_server(vec![
            serde_json::json!({"capabilities":["tools"],"model_info":{"mock.context_length":262144}}).to_string(),
            serde_json::json!({"model":"mock","created_at":"2026-09-06T00:00:00Z","message":{"role":"assistant","content":"ready"},"done":true,"prompt_eval_count":100,"eval_count":1}).to_string(),
        ]);
        let Command::Run(mut options) = parse(
            &format!("--model mock --url {url} --context-tokens 32768 --thinking off"),
            &no_env,
        )
        .unwrap() else {
            panic!()
        };
        options.output_tokens = 8192;
        runtime::preflight(&options).await.unwrap();
        let agent = build_agent(&options).unwrap();
        let (answer, ..) = converse(
            &agent,
            Message::user("Ready?"),
            Vec::new(),
            2,
            false,
            options.context_limit(),
        )
        .await
        .unwrap();
        assert_eq!(answer, "ready");
        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let body: serde_json::Value = serde_json::from_str(&requests[1]).unwrap();
        assert_eq!(body["options"]["num_ctx"], 32768);
        assert_eq!(body["options"]["temperature"], 0.0);
        assert_eq!(body["options"]["num_predict"], 8192);
        assert_eq!(body["think"], false);
        assert!(
            body["options"].get("options").is_none(),
            "Ollama options must not be nested twice"
        );
        assert!(parse("--model mock --context-tokens 4096", &no_env).is_err());
    }

    #[tokio::test]
    async fn ollama_output_limit_stops_partial_answers_and_tool_turns() {
        for message in [
            serde_json::json!({"role":"assistant","content":"I have finished the first"}),
            serde_json::json!({"role":"assistant","content":"","tool_calls":[{"function":{"name":"list_presets","arguments":{}}}]}),
        ] {
            let truncated = serde_json::json!({
                "model":"mock", "created_at":"2026-09-06T00:00:00Z",
                "message":message, "done":true, "done_reason":"length",
                "prompt_eval_count":100, "eval_count":runtime::OUTPUT_RESERVE,
            });
            let (url, seen) = mock_server(vec![truncated.to_string()]);
            let Command::Run(mut options) =
                parse(&format!("--model mock --url {url}"), &no_env).unwrap()
            else {
                panic!()
            };
            options.output_tokens = 8192;
            let agent = build_agent(&options).unwrap();
            let error = converse_with_bridge(
                &agent,
                Message::user("List the presets"),
                Vec::new(),
                2,
                None,
                options.context_limit(),
                options.output_tokens,
            )
            .await
            .unwrap_err();
            assert!(error.contains("8192-token output limit"), "{error}");
            assert!(error.contains("incomplete"), "{error}");
            assert_eq!(seen.lock().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn compaction_uses_no_tools_and_preserves_recent_exchanges() {
        let done =
            r#"{"role":"assistant","content":"Keep the bass. Three earlier edits are unsaved."}"#;
        let (url, seen) = mock_server(vec![completion(done, "stop")]);
        let Command::Run(options) = parse(
            &format!("--provider openai --model mock --url {url}"),
            &no_env,
        )
        .unwrap() else {
            panic!()
        };
        let mut memory = memory::Memory::default();
        for index in 0..5 {
            memory.push(
                &format!("Keep the bass, turn {index}"),
                &"Edited the live project. ".repeat(10),
            );
        }
        compaction::compact(&options, &mut memory).await.unwrap();
        assert!(memory.summary.contains("unsaved"));
        assert_eq!(memory.turns.len(), 2);
        assert!(memory.turns[0].user.ends_with("turn 3"));
        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let request: serde_json::Value = serde_json::from_str(&requests[0]).unwrap();
        assert!(
            request
                .get("tools")
                .is_none_or(|tools| tools.is_null() || tools.as_array().is_some_and(Vec::is_empty))
        );
    }

    #[tokio::test]
    async fn a_truncated_summary_does_not_replace_conversation() {
        let done = r#"{"role":"assistant","content":"Partial summary"}"#;
        let (url, _) = mock_server(vec![completion(done, "length")]);
        let Command::Run(options) = parse(
            &format!("--provider openai --model mock --url {url}"),
            &no_env,
        )
        .unwrap() else {
            panic!()
        };
        let mut memory = memory::Memory::default();
        for _ in 0..3 {
            memory.push("Keep the bass", &"Unfinished work. ".repeat(10));
        }
        let before = serde_json::to_string(&memory).unwrap();
        assert!(compaction::compact(&options, &mut memory).await.is_err());
        assert_eq!(serde_json::to_string(&memory).unwrap(), before);
    }

    #[tokio::test]
    async fn repeated_invalid_calls_stop_before_a_third_execution() {
        let call = r#"{"role":"assistant","tool_calls":[{"id":"bad","type":"function","function":{"name":"edit_project","arguments":"{}"}}]}"#;
        let (url, seen) = mock_server(vec![completion(call, "tool_calls"); 3]);
        let Command::Run(options) = parse(
            &format!("--provider openai --model mock --url {url}"),
            &no_env,
        )
        .unwrap() else {
            panic!()
        };
        let agent = build_agent(&options).unwrap();
        let error = converse(
            &agent,
            Message::user("Set a level"),
            Vec::new(),
            8,
            false,
            None,
        )
        .await
        .unwrap_err();
        assert!(error.contains("failed twice"), "{error}");
        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests[1].contains("missing field"));
        assert!(requests[1].contains("Check the tool schema"));
    }

    #[tokio::test]
    async fn unknown_tool_names_get_corrective_feedback_and_can_recover() {
        let bad = r#"{"role":"assistant","tool_calls":[{"id":"bad","type":"function","function":{"name":"list_preset","arguments":"{}"}}]}"#;
        let corrected = r#"{"role":"assistant","tool_calls":[{"id":"good","type":"function","function":{"name":"list_presets","arguments":"{}"}}]}"#;
        let done = r#"{"role":"assistant","content":"The presets are available."}"#;
        let (url, seen) = mock_server(vec![
            completion(bad, "tool_calls"),
            completion(corrected, "tool_calls"),
            completion(done, "stop"),
        ]);
        let Command::Run(options) = parse(
            &format!("--provider openai --model mock --url {url}"),
            &no_env,
        )
        .unwrap() else {
            panic!()
        };
        let agent = build_agent(&options).unwrap();
        let (answer, ..) = converse(
            &agent,
            Message::user("List the presets"),
            Vec::new(),
            5,
            false,
            None,
        )
        .await
        .unwrap();
        assert_eq!(answer, "The presets are available.");
        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests[1].contains("Tool 'list_preset' is unavailable"));
        assert!(requests[1].contains("No tool in this batch was executed"));
        assert!(requests[2].contains("Styles `compose` and `check_spec` accept"));
    }

    #[tokio::test]
    async fn unknown_tools_and_skipped_writes_cannot_loop_indefinitely() {
        let outside = std::env::temp_dir().join("auris-agent-outside-workspace.auris");
        let outside_args = serde_json::json!({"project":outside}).to_string();
        for (tool, args) in [("nonexistent_tool", "{}"), ("set_level", &outside_args)] {
            let call = serde_json::json!({
                "role":"assistant", "tool_calls":[{
                    "id":"repeated", "type":"function",
                    "function":{"name":tool,"arguments":args}
                }]
            })
            .to_string();
            let (url, seen) = mock_server(vec![completion(&call, "tool_calls"); 3]);
            let Command::Run(options) = parse(
                &format!("--provider openai --model mock --url {url}"),
                &no_env,
            )
            .unwrap() else {
                panic!()
            };
            let agent = build_agent(&options).unwrap();
            let error = converse(
                &agent,
                Message::user("Try a tool"),
                Vec::new(),
                8,
                true,
                None,
            )
            .await
            .unwrap_err();
            assert!(error.contains("failed twice"), "{error}");
            let requests = seen.lock().unwrap();
            assert_eq!(requests.len(), 3);
            if tool == "set_level" {
                assert!(requests[1].contains("unavailable"));
            }
        }
    }

    /// The whole loop against a scripted model: the "model" asks for `list_presets`, the tool
    /// really runs, its answer really goes back over the wire, and the final text reaches the
    /// caller. No network, no key, no model — but every seam of this frontend crossed once.
    #[tokio::test]
    async fn the_tool_loop_runs_end_to_end_against_a_scripted_model() {
        let call = r#"{"role":"assistant","tool_calls":[{"id":"call_1","type":"function","function":{"name":"list_presets","arguments":"{}"}}]}"#;
        let done = r#"{"role":"assistant","content":"The presets are listed above."}"#;
        let with_usage = |response: String, input, output| {
            let mut response: serde_json::Value = serde_json::from_str(&response).unwrap();
            response["usage"] = serde_json::json!({"prompt_tokens":input,"completion_tokens":output,"total_tokens":input+output});
            response.to_string()
        };
        let (url, seen) = mock_server(vec![
            with_usage(completion(call, "tool_calls"), 10, 2),
            with_usage(completion(done, "stop"), 20, 3),
        ]);

        let agent = build_agent(&Options {
            context_tokens: 32768,
            output_tokens: 4096,
            thinking: None,
            provider: Provider::OpenAi,
            url: Some(url),
            model: "mock".to_string(),
            key: Some("test-key".to_string()),
            max_turns: 5,
            prompt: None,
            attachments: Vec::new(),
            json: false,
        })
        .unwrap();
        let (answer, history, usage, context_tokens) = converse(
            &agent,
            Message::user("What styles are there?"),
            Vec::new(),
            5,
            true,
            None,
        )
        .await
        .unwrap();

        assert_eq!(answer, "The presets are listed above.");
        assert_eq!(usage.input_tokens, 30);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(
            context_tokens, 20,
            "the context gauge must not sum successive requests"
        );
        assert!(
            history.len() >= 2,
            "the transcript comes back for the next turn: {history:?}"
        );

        // The second request is the proof: it carries the tool's real answer back to the
        // model, so the loop ran through the toolbox and not around it.
        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(
            requests[0].contains("\"list_presets\""),
            "the tool is offered to the model: {}",
            requests[0]
        );
        assert!(requests[1].contains("\"tool\""), "{}", requests[1]);
        let listing = toolbox::list_presets::run();
        let first_preset = listing
            .lines()
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap();
        assert!(
            requests[1].contains(first_preset),
            "the toolbox's own answer rode back to the model: {}",
            requests[1]
        );
    }

    /// An attached file really crosses the wire: base64 in an `input_audio` content part,
    /// beside the words — the shape OpenAI's chat completions and every compatible server
    /// that takes audio expect.
    #[tokio::test]
    async fn an_attachment_reaches_the_wire_as_input_audio() {
        let done = r#"{"role":"assistant","content":"A fine mix."}"#;
        let (url, seen) = mock_server(vec![completion(done, "stop")]);

        let clip =
            std::env::temp_dir().join(format!("auris-agent-clip-{}.wav", std::process::id()));
        std::fs::write(&clip, b"RIFFfake-wav-bytes").unwrap();
        let attachments = vec![clip.display().to_string()];

        let agent = build_agent(&Options {
            context_tokens: 32768,
            output_tokens: 4096,
            thinking: None,
            provider: Provider::OpenAi,
            url: Some(url),
            model: "mock".to_string(),
            key: Some("test-key".to_string()),
            max_turns: 5,
            prompt: None,
            attachments: Vec::new(),
            json: false,
        })
        .unwrap();
        let message = framed_message("How is this mix?", &attachments).unwrap();
        let (answer, ..) = converse(&agent, message, Vec::new(), 5, false, None)
            .await
            .unwrap();
        std::fs::remove_file(&clip).unwrap();

        assert_eq!(answer, "A fine mix.");
        let requests = seen.lock().unwrap();
        let body: serde_json::Value = serde_json::from_str(&requests[0]).unwrap();
        let user = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["role"] == "user")
            .unwrap();
        assert_eq!(user["content"][0]["type"], "text");
        assert_eq!(user["content"][1]["type"], "input_audio");
        assert!(
            requests[0].contains("\"input_audio\""),
            "the audio content part is on the wire: {}",
            requests[0]
        );
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"RIFFfake-wav-bytes");
        assert!(
            requests[0].contains(&encoded),
            "the file's own bytes rode along: {}",
            requests[0]
        );
        assert!(
            requests[0].contains("How is this mix?"),
            "and so did the words: {}",
            requests[0]
        );
    }

    /// An attachment over the ceiling is refused by its size on disk, before a byte of it is
    /// read — the file here *is* over the ceiling but holds no data, which is the point.
    #[test]
    fn an_enormous_attachment_is_refused_before_it_is_read() {
        let clip =
            std::env::temp_dir().join(format!("auris-agent-vast-{}.wav", std::process::id()));
        let file = std::fs::File::create(&clip).unwrap();
        file.set_len(ATTACHMENT_CEILING + 1).unwrap();
        drop(file);
        let refused =
            framed_message("How is this mix?", &[clip.display().to_string()]).unwrap_err();
        std::fs::remove_file(&clip).unwrap();
        assert!(refused.contains("25 MB"), "{refused}");
    }

    #[test]
    fn live_reply_preserves_host_refusals() {
        for ok in [true, false] {
            let result = edit_reply(
                serde_json::json!({"event":"edit_result", "ok":ok, "text":"host answer"}),
            );
            assert_eq!(result.is_ok(), ok);
            assert_eq!(result.unwrap_or_else(|e| e.to_string()), "host answer");
        }
    }

    #[tokio::test]
    async fn rig_exposes_only_live_editing_and_read_only_references() {
        let Command::Run(options) = parse("--provider openai --model mock", &no_env).unwrap()
        else {
            panic!()
        };
        let agent = build_agent(&options).unwrap();
        let actual = agent.tool_definitions(None).await.unwrap();
        let mut names: Vec<_> = actual.iter().map(|tool| tool.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "edit_project",
                "list_instruments",
                "list_presets",
                "list_progressions",
                "search_documentation",
                "search_internet",
                "spec_reference"
            ]
        );
        let catalog = toolbox::tool_catalog();
        for name in [
            "create_project",
            "compose",
            "render",
            "export_midi",
            "import_midi",
            "listen",
        ] {
            assert!(
                catalog.iter().any(|tool| tool.name == name),
                "MCP retains {name}"
            );
            assert!(!names.contains(&name));
        }
        for expected in catalog.iter().filter(|tool| names.contains(&tool.name)) {
            let exposed = actual
                .iter()
                .find(|tool| tool.name == expected.name)
                .unwrap();
            assert_eq!(exposed.parameters, expected.parameters);
        }
        let schema = schema::<EditProjectArgs>().to_string();
        for field in ["output", "project", "path", "stems", "midi_output"] {
            assert!(!schema.contains(&format!("\"{field}\":")));
        }
    }

    #[test]
    fn internet_search_response_keeps_titles_urls_and_plain_snippets() {
        let response: SearchResponse = serde_json::from_str(
            r#"{"web":{"results":[
                {"title":"A &amp; B","url":"https://example.com/a?x=1&y=2","description":"One <b>useful</b> result."},
                {"title":"Second","url":"https://example.com/two","description":"Another <strong>result</strong>."}
            ]}}"#,
        )
        .unwrap();
        let results = response.web.unwrap().results;
        assert_eq!(results.len(), 2);
        assert_eq!(strip_markup(&results[0].title), "A & B");
        assert_eq!(results[0].url, "https://example.com/a?x=1&y=2");
        assert_eq!(strip_markup(&results[0].description), "One useful result.");
        assert_eq!(strip_markup(&results[1].description), "Another result.");
    }
}
