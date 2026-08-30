//! `auris-agent` — the frontend that dials the model itself.
//!
//! The fourth frontend, and the mirror of `auris-mcp`: there, a language model's harness
//! connects to Auris; here, Auris connects to a language model — a local Ollama server or any
//! OpenAI-compatible API — hands it the tools from [`auris_toolbox`], and runs the loop. The
//! two doors serve the same tools from the same crate, so a model that has learnt one has
//! learnt the other.
//!
//! `rig` is the client library, and it stays inside this crate along with the `tokio` runtime
//! it needs. Three decisions of this frontend's own:
//!
//! * **Two channels.** The model's words go to stdout, where a pipe can catch them; everything
//!   this program says about the run — which tool was called, what it answered — goes to
//!   stderr. `auris-agent "..." > answer.md` keeps the answer and shows the work.
//! * **English chrome, like the CLI.** The frame around the conversation is fixed English for
//!   the same reason `auris` prints English: a terminal makes no promises about other scripts.
//!   The conversation itself is the model's, and the preamble tells it to answer in the
//!   language the user writes in.
//! * **The key never rides the command line.** An API key is named by environment variable
//!   (`--api-key-env`), because arguments are visible to every process listing on the machine.

#![warn(missing_docs)]

use std::io::{BufRead, Write};
use std::process::ExitCode;

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
}

/// What `main` was asked to do.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    /// Run with these options.
    Run(Options),
    /// Print usage and leave.
    Help,
}

const USAGE: &str = "auris-agent — drive Auris Studio with a language model

usage: auris-agent [options] [prompt]

With a prompt, asks once, prints the model's answer on stdout and leaves.
Without one, opens a conversation; an empty line or end-of-file closes it.
Tool calls are narrated on stderr either way.

options:
  --model <name>        the model to use (required) — e.g. qwen3:8b, gpt-5.2
  --provider <name>     ollama (the default) or openai, meaning any
                        OpenAI-compatible chat-completions API
  --url <base>          the API's base URL; defaults to http://localhost:11434
                        for ollama and https://api.openai.com/v1 for openai
  --api-key-env <VAR>   environment variable holding the API key; OPENAI_API_KEY
                        is used for openai when it is set and this is not given
  --max-turns <n>       model-call budget per prompt (default 40)
  -h, --help            this text";

/// Reads the command line, with the environment handed in so a test can be its own machine.
///
/// A free function and not a chunk of `main`, because everything here is a decision: which
/// provider a word names, where the key comes from, what is missing. `env` is consulted only
/// for the key — the one value that must never be typed into a command line.
fn parse_args(args: &[String], env: &dyn Fn(&str) -> Option<String>) -> Result<Command, String> {
    let mut provider = Provider::Ollama;
    let mut url = None;
    let mut model = None;
    let mut key_env = None;
    let mut max_turns = 40usize;
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
            "--provider" => {
                provider = match value_of("--provider")?.as_str() {
                    "ollama" => Provider::Ollama,
                    "openai" => Provider::OpenAi,
                    other => {
                        return Err(format!(
                            "--provider is 'ollama' or 'openai' (any OpenAI-compatible API), \
                             not '{other}'"
                        ));
                    }
                };
            }
            "--url" => url = Some(value_of("--url")?),
            "--model" => model = Some(value_of("--model")?),
            "--api-key-env" => key_env = Some(value_of("--api-key-env")?),
            "--max-turns" => {
                let value = value_of("--max-turns")?;
                max_turns = value
                    .parse()
                    .map_err(|_| format!("--max-turns needs a number, not '{value}'"))?;
            }
            flag if flag.starts_with('-') => {
                return Err(format!("unknown option '{flag}' — try --help"));
            }
            _ => prompt_words.push(word),
        }
    }

    let Some(model) = model else {
        return Err(
            "--model names the model to use; there is no sensible default, because \
                    it is whatever the server at the other end actually serves"
                .to_string(),
        );
    };

    // The key: an explicitly named variable must exist — a silently empty key would come back
    // from the server as a 401 with this program's name on it. The OpenAI convention is picked
    // up when present, because that is what every OpenAI-compatible tool trains people to set.
    let key = match key_env {
        Some(name) => Some(env(&name).ok_or_else(|| {
            format!("--api-key-env names '{name}', but that variable is not set")
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
    Ok(Command::Run(Options {
        provider,
        url,
        model,
        key,
        max_turns,
        prompt,
    }))
}

/// What the model is told once, before the conversation: the shared workflow, plus what only
/// this frontend knows — where it is standing, and who it is talking to.
fn preamble() -> String {
    let here = std::env::current_dir()
        .map(|dir| dir.display().to_string())
        .unwrap_or_else(|_| "the current directory (unreadable)".to_string());
    format!(
        "{}\n\nYou are running on the user's machine; the working directory is {here}, and \
         that is where files belong when the user does not say otherwise. Answer the user in \
         the language they write in.",
        toolbox::INSTRUCTIONS
    )
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
    serde_json::to_value(schemars::schema_for!(T)).expect("a derived schema serialises")
}

/// No arguments, said as a schema — for the reference and listing tools.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct NoArgs {}

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

text_tool!(SpecReference, spec_reference);
session_tool!(CheckSpec, check_spec);
session_tool!(Compose, compose);
session_tool!(Render, render);
session_tool!(Describe, describe);
session_tool!(Analyze, analyze);
session_tool!(Mixer, mixer);
session_tool!(SetLevel, set_level);
session_tool!(SetSend, set_send);
session_tool!(SetEffect, set_effect);
session_tool!(SectionGain, section_gain);
session_tool!(AnotherTake, another_take);
session_tool!(WriteAgain, write_again);
session_tool!(TeachProgression, teach_progression);
session_tool!(ForgetProgression, forget_progression);
text_tool!(ListProgressions, list_progressions);
text_tool!(ListPresets, list_presets);

/// Every tool in the box, onto one agent — the one list to keep when a tool is added.
fn armed(builder: AgentBuilder) -> Agent {
    builder
        .preamble(&preamble())
        .tool(SpecReference)
        .tool(CheckSpec)
        .tool(Compose)
        .tool(Render)
        .tool(Describe)
        .tool(Analyze)
        .tool(Mixer)
        .tool(SetLevel)
        .tool(SetSend)
        .tool(SetEffect)
        .tool(SectionGain)
        .tool(AnotherTake)
        .tool(WriteAgain)
        .tool(TeachProgression)
        .tool(ForgetProgression)
        .tool(ListProgressions)
        .tool(ListPresets)
        .build()
}

/// Builds the agent for whichever door the options chose.
///
/// Both arms end in the same [`armed`] call; only the client construction differs, because the
/// two dialects are two client types. An empty key means no key — both providers' key types
/// treat it that way or tolerate it, and it saves an `Option` dance at each arm.
fn build_agent(options: &Options) -> Result<Agent, String> {
    let key = options.key.clone().unwrap_or_default();
    let could_not = |error: rig::http_client::Error| format!("could not build a client: {error}");
    match options.provider {
        Provider::Ollama => {
            let mut builder = ollama::Client::builder().api_key(key);
            if let Some(url) = &options.url {
                builder = builder.base_url(url);
            }
            let client = builder.build().map_err(could_not)?;
            Ok(armed(client.agent(&options.model)))
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

/// Narrates the tool loop on stderr while the model works.
///
/// The person at the terminal sees what the CLI would have shown them — which tool ran, on
/// what, and whether it answered or refused — without any of it landing in stdout, which
/// belongs to the model's words alone.
struct Narrator;

/// The first line of a tool's answer, for the narration.
fn first_line(output: &ToolOutput) -> Option<&str> {
    output
        .as_content()
        .iter()
        .find_map(|content| match content {
            ToolResultContent::Text(text) => text.text.lines().next(),
            _ => None,
        })
}

impl AgentHook for Narrator {
    async fn on_tool_call(&self, _ctx: &HookContext, event: ToolCall<'_>) -> ToolCallAction {
        // The arguments as the model wrote them, clipped: a whole spec in a compose call is
        // legitimate and would drown the narration it is meant to serve.
        let args: String = event.args.chars().take(120).collect();
        let ellipsis = if args.len() < event.args.len() {
            "…"
        } else {
            ""
        };
        eprintln!("→ {} {args}{ellipsis}", event.tool_name);
        ToolCallAction::Run
    }

    async fn on_tool_result(
        &self,
        _ctx: &HookContext,
        event: ToolResultEvent<'_>,
    ) -> ToolResultAction {
        match event.raw_result.error() {
            None => {
                let line = first_line(event.presentation).unwrap_or("done");
                eprintln!("  {line}");
            }
            Some(error) => eprintln!("  refused: {}", error.message()),
        }
        ToolResultAction::Keep
    }
}

/// One prompt through the loop: ask, narrate, answer — and hand back the transcript so a
/// conversation can keep it.
async fn converse(
    agent: &Agent,
    prompt: String,
    history: Vec<Message>,
    max_turns: usize,
) -> Result<(String, Vec<Message>), String> {
    let asked = prompt.clone();
    let response = agent
        .prompt(prompt)
        .history(history.clone())
        .max_turns(max_turns)
        .add_hook(Narrator)
        .extended_details()
        .await
        .map_err(|error| error.to_string())?;
    // The runner hands the accumulated transcript back; when it does not, the two ends of the
    // exchange are still worth keeping — better a thin memory than none.
    let history = response.messages.unwrap_or_else(|| {
        let mut kept = history;
        kept.push(Message::user(asked));
        kept.push(Message::assistant(&response.output));
        kept
    });
    Ok((response.output, history))
}

/// The conversation: read a line, run the loop, print the answer, remember everything.
async fn conversation(agent: &Agent, max_turns: usize) -> Result<(), String> {
    let mut history: Vec<Message> = Vec::new();
    let stdin = std::io::stdin();
    loop {
        eprint!("> ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(error) => return Err(error.to_string()),
        }
        let line = line.trim();
        if line.is_empty() {
            return Ok(());
        }
        let (answer, kept) = converse(agent, line.to_string(), history, max_turns).await?;
        history = kept;
        println!("{answer}\n");
    }
}

fn main() -> ExitCode {
    // Stderr by default already, and stderr it must stay: stdout carries the model's answer.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    // Like the other frontends: this may be the first one to run on a machine, and an
    // installation predating `~/.config/auris-studio` only has its settings carried across by
    // whichever one does.
    auris_session::migrate_legacy_config();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse_args(&args, &|name| std::env::var(name).ok()) {
        Ok(Command::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(Command::Run(options)) => options,
        Err(message) => {
            eprintln!("auris-agent: {message}");
            return ExitCode::FAILURE;
        }
    };

    let outcome = tokio::runtime::Runtime::new()
        .map_err(|error| error.to_string())
        .and_then(|runtime| {
            runtime.block_on(async {
                let agent = build_agent(&options)?;
                match &options.prompt {
                    Some(prompt) => {
                        let (answer, _) =
                            converse(&agent, prompt.clone(), Vec::new(), options.max_turns).await?;
                        println!("{answer}");
                        Ok(())
                    }
                    None => conversation(&agent, options.max_turns).await,
                }
            })
        });

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("auris-agent: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(String::from).collect()
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn a_model_is_required_and_the_rest_defaults() {
        let refused = parse_args(&words("write me a song"), &no_env).unwrap_err();
        assert!(refused.contains("--model"), "{refused}");

        let Command::Run(options) =
            parse_args(&words("--model qwen3:8b write me a song"), &no_env).unwrap()
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
        let Command::Run(options) = parse_args(&words("--model m"), &no_env).unwrap() else {
            panic!("a bare model still runs");
        };
        assert_eq!(options.prompt, None);
    }

    #[test]
    fn the_key_comes_from_the_environment_and_its_absence_is_an_error() {
        let env = |name: &str| (name == "MY_KEY").then(|| "secret".to_string());

        let Command::Run(options) =
            parse_args(&words("--model m --api-key-env MY_KEY"), &env).unwrap()
        else {
            panic!("a named key that exists runs");
        };
        assert_eq!(options.key.as_deref(), Some("secret"));

        let missing = parse_args(&words("--model m --api-key-env NOT_SET"), &env).unwrap_err();
        assert!(missing.contains("NOT_SET"), "{missing}");

        // The OpenAI convention is picked up unasked, but only for the openai provider.
        let convention = |name: &str| (name == "OPENAI_API_KEY").then(|| "sk-x".to_string());
        let Command::Run(options) =
            parse_args(&words("--model m --provider openai"), &convention).unwrap()
        else {
            panic!("openai with the conventional key runs");
        };
        assert_eq!(options.key.as_deref(), Some("sk-x"));
        let Command::Run(options) = parse_args(&words("--model m"), &convention).unwrap() else {
            panic!("ollama ignores the OpenAI convention");
        };
        assert_eq!(options.key, None);
    }

    #[test]
    fn bad_flags_are_named_back() {
        let unknown = parse_args(&words("--model m --loud"), &no_env).unwrap_err();
        assert!(unknown.contains("--loud"), "{unknown}");
        let provider = parse_args(&words("--model m --provider gemini"), &no_env).unwrap_err();
        assert!(provider.contains("gemini"), "{provider}");
        let turns = parse_args(&words("--model m --max-turns many"), &no_env).unwrap_err();
        assert!(turns.contains("many"), "{turns}");
        let dangling = parse_args(&words("--model"), &no_env).unwrap_err();
        assert!(dangling.contains("--model"), "{dangling}");
    }

    /// A scripted OpenAI-compatible server: each connection gets the next canned body.
    ///
    /// Real enough for the client (HTTP/1.1, `Content-Length`, `Connection: close`) and no
    /// more; what it captures is the request bodies, which is what the assertions read.
    fn mock_server(
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
    fn completion(message: &str, finish_reason: &str) -> String {
        format!(
            r#"{{"id":"chatcmpl-1","object":"chat.completion","created":0,"model":"mock","choices":[{{"index":0,"message":{message},"logprobs":null,"finish_reason":"{finish_reason}"}}],"usage":null}}"#
        )
    }

    /// The whole loop against a scripted model: the "model" asks for `list_presets`, the tool
    /// really runs, its answer really goes back over the wire, and the final text reaches the
    /// caller. No network, no key, no model — but every seam of this frontend crossed once.
    #[tokio::test]
    async fn the_tool_loop_runs_end_to_end_against_a_scripted_model() {
        let call = r#"{"role":"assistant","tool_calls":[{"id":"call_1","type":"function","function":{"name":"list_presets","arguments":"{}"}}]}"#;
        let done = r#"{"role":"assistant","content":"The presets are listed above."}"#;
        let (url, seen) = mock_server(vec![
            completion(call, "tool_calls"),
            completion(done, "stop"),
        ]);

        let agent = build_agent(&Options {
            provider: Provider::OpenAi,
            url: Some(url),
            model: "mock".to_string(),
            key: Some("test-key".to_string()),
            max_turns: 5,
            prompt: None,
        })
        .unwrap();
        let (answer, history) =
            converse(&agent, "What styles are there?".to_string(), Vec::new(), 5)
                .await
                .unwrap();

        assert_eq!(answer, "The presets are listed above.");
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

    #[test]
    fn every_tool_wears_its_toolbox_name_and_schema() {
        // The names the loop dispatches on are the toolbox constants, once each.
        let names = [
            Compose::NAME,
            Render::NAME,
            Describe::NAME,
            Analyze::NAME,
            Mixer::NAME,
            SetLevel::NAME,
            SetSend::NAME,
            SetEffect::NAME,
            SectionGain::NAME,
            AnotherTake::NAME,
            WriteAgain::NAME,
            CheckSpec::NAME,
            SpecReference::NAME,
            TeachProgression::NAME,
            ForgetProgression::NAME,
            ListProgressions::NAME,
            ListPresets::NAME,
        ];
        let unique: std::collections::BTreeSet<&str> = names.into_iter().collect();
        assert_eq!(unique.len(), 17, "seventeen tools, no name worn twice");

        // The schema is the toolbox derive, fields and all — the same one the MCP door hands
        // its clients.
        let compose = schema::<toolbox::compose::Args>();
        let fields = compose["properties"].as_object().unwrap();
        assert!(fields.contains_key("output"));
        assert!(fields.contains_key("spec"), "the flattened spec triangle");
        let none = schema::<NoArgs>();
        assert_eq!(none["type"], "object");
    }
}
