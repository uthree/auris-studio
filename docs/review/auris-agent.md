# Review findings: auris-agent

Part of the [whole-repository adversarial review](README.md) of commit `52d1702`. 2 verified findings: 1 high, 1 medium.

Each entry survived an independent skeptic and an independent reproducer (and a tie-breaker when they disagreed); "executed reproduction" means the reproducer ran a test, a binary or a scratch program and observed the behaviour, "traced" means it followed the call path with concrete values.

| ID | Severity | Location | Finding |
|---|---|---|---|
| F-093 | high | `crates/auris-agent/src/main.rs:839` | converse() in auris-agent has no request timeout, so a black-holed LLM host hangs the agent process (and the panel's parked thread) forever, unlike list_models […] |
| F-252 | medium | `crates/auris-agent/src/main.rs:794` | auris-agent's Reporter/Narrator hooks always return ToolCallAction::Run and compose's `output` path is unconfined, so project-embedded text can steer […] |

### F-093 · high · converse() in auris-agent has no request timeout, so a black-holed LLM host hangs the agent process (and the panel's parked thread) forever, unlike list_models which is explicitly bounded for exactly this reason.

`crates/auris-agent/src/main.rs:839` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** When the desktop app's agent panel talks to auris-agent in --json mode and the configured LLM host stalls or black-holes the connection mid-response, the auris-agent child process hangs forever awaiting extended_details() with no timeout. The panel's spinner never resolves, the process cannot be recovered short of being killed, and — per the existing comment on MODEL_LIST_PATIENCE about "the thread the panel parked on it" — whatever thread the GUI parked waiting on this call is stuck too.

**Trigger.** Point `--url` (or the desktop-saved `AgentPreferences.url`) at a server/proxy that accepts the TCP connection and the HTTP request but never sends a response body (a network black hole, a stalled reverse proxy, or simply an overloaded self-hosted OpenAI-compatible/Ollama server) and send any prompt, or drive `--json` with a `{"say": "..."}` line — which is exactly how the desktop's agent panel talks to this process.

**Mechanism.** `converse()` builds the model request (`agent.prompt(prompt).history(history.clone()).max_turns(max_turns)`, lines 831-838) and then does `let response = request.extended_details().await.map_err(...)?;` at line 839-842 with no timeout of any kind around the await. This is the only place a prompt or a `--json` `{"say": ...}` line is turned into a network call to the provider chosen by `--provider`/`--url`. Contrast this with `list_models` (used only by the `models` subcommand), which the same file explicitly wraps in `tokio::time::timeout(MODEL_LIST_PATIENCE, list_models(&options))` (lines 984-991) with the comment at lines 485-489: "Listing is a couple of small requests to a local or nearby server... a host that black-holes the connection would otherwise hang this process — and the thread the panel parked on it — forever." The exact hazard the author reasoned about and fixed for listing is left unfixed for the actual conversation loop, which is reached from both `conversation()` (interactive prompt) and `json_conversation()` — the latter is, per the file's own top-of-file doc […]

**Expected.** The conversation request should be time-bounded the same way `list_models` deliberately is (e.g. wrapped in `tokio::time::timeout`), so a stuck or malicious endpoint surfaces as a reported error rather than hanging the panel/terminal forever — consistent with the reasoning the file already states for `MODEL_LIST_PATIENCE`.

**Fix direction.** Wrap the `request.extended_details().await` call in converse() with `tokio::time::timeout`, using either MODEL_LIST_PATIENCE or a separate, longer constant appropriate for generation (since responses can legitimately take longer than a model listing), and surface a clear timeout error through the existing Err(String) path so both conversation() and json_conversation() report it as a normal failure rather than hanging.

**Written rule it breaks.** // Bounded, because the caller is a panel with a spinner: a host that black-holes the connection would otherwise hang this process — and the thread the panel parked on it — forever. (doc comment on MODEL_LIST_PATIENCE, crates/auris-agent/src/main.rs:482-484)

### F-252 · medium · auris-agent's Reporter/Narrator hooks always return ToolCallAction::Run and compose's `output` path is unconfined, so project-embedded text can steer autonomous writes anywhere the OS user can write, with no confirmation.

`crates/auris-agent/src/main.rs:794` · security · confirmed (traced through the code; reported independently 1×)

**What a user sees.** A user running the auris-agent (CLI or the desktop Agent panel that drives it in --json mode) opens or is handed a .auris project file whose track/clip names or lyrics contain hidden instruction-like text. When the user later asks the agent to inspect that project (describe/notes/mixer), the model reads the embedded text with the same trust as the user's own prompt and can, within the same conversation turn (up to 40 chained tool calls by default), call compose/render/add_track with an absolute output path outside the project folder — writing or overwriting an arbitrary file the OS user can write to, with zero confirmation step anywhere in the frontend.

**Trigger.** A user opens a `.auris` project (or shares one) whose track/clip names or lyrics contain text crafted to look like an instruction, then asks the agent to do anything that causes it to call `describe`/`notes`/`mixer` on that project; the model reads the embedded text as part of its context and can act on it by calling `compose`/`render`/`add_track` with an absolute `output`/`project` path outside the project folder, and the call runs with no confirmation step anywhere in this frontend.

**Mechanism.** `Reporter::on_tool_call` and `Narrator::on_tool_call` (lines 606-616 and 790-796) both unconditionally return `ToolCallAction::Run` — there is no hook that inspects a call before it executes, so every model-issued tool call (including `compose`, `render`, `add_track`, `remove_track`, all in `WRITES_PROJECTS`) runs immediately with no user confirmation, and `compose`/`render`/`compose_lyrics`'s `output`/`project`/`stems` arguments accept any absolute path with no confinement to the project folder (`resolve_project`, `crates/auris-toolbox/src/lib.rs:2380-2396`, only absolutises and looks one folder down — it never refuses a path outside the working directory). The desktop panel sets the agent's cwd to the open project's own folder (`spawn_link`, `crates/auris-gpui/src/ui/agent_chat.rs:519-527`) but that is only a default location, not a boundary. Meanwhile tool results — track/clip names, lyrics, `describe`'s free-text project summary — flow straight back into the model's context with no delimiting or provenance marking, so text embedded in a shared/downloaded `.auris` project (a […]

**Expected.** The concern's own brief calls this out directly ("whether the agent can be steered into writing files outside the project folder"); a destructive/writing tool call reachable from model-controlled or document-embedded text should be confirmed or at least confined to the working directory before it runs.

**Fix direction.** Give AgentHook::on_tool_call a real decision point: before returning ToolCallAction::Run for any tool in auris_toolbox::WRITES_PROJECTS, either prompt the user (Narrator: a stdin y/n; Reporter/JSON: emit a "confirm" event and block for the host's reply) or confine resolve_project/compose's output argument to the current project directory (canonicalize and reject any path that escapes it) unless an explicit --allow-outside-project flag was passed at startup.
