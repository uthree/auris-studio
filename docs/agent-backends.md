# Agent backends

The Agent panel discovers the signed-in Codex, Claude Code and OpenCode CLIs, local Ollama,
and the API connection configured in Settings > Agent. An `OPENAI_API_KEY` also enables
discovery at OpenAI's default endpoint. Providers are queried concurrently;
an unavailable provider does not hide models returned by another.

`OPENAI_API_KEY` is sent only to OpenAI itself (`https://api.openai.com`). A custom
OpenAI-compatible URL gets no key unless the connection names one with its API key variable,
so pointing Auris at a LAN server or another service never hands it your OpenAI credential.

Choose a model in the command row at the bottom of the message composer. Each option names its connection,
for example `GPT-6.1 Sol (Codex)`. Custom API connections also show their host.
Choosing an option switches its backend automatically and preserves the configured
API connection for future discovery. Refresh in the model menu repeats discovery
(`Ctrl+R` / `Command+R` while that selector has focus). Reasoning effort sits next
to the model; Codex's advertised reasoning levels determine its choices. Permission
rules open from the approval-mode menu on the left. The circular send control
becomes Stop during a turn.

Consecutive tool calls share a collapsed summary showing their count and any
running or failed calls. Open the group to inspect each call's arguments and
result; user messages and assistant replies keep separate groups apart.

## Reference attachments

Drop files onto the Agent panel to attach them to the next message. Dropping audio
there attaches a reference; dropping audio onto the arrangement imports it into
the song. Attached filenames remain visible and each file can be removed before
sending. A message accepts at most eight files. Audio sources may be up to 512 MiB each
and 1 GiB combined; they are copied to private local snapshots with bounded memory and
cancellation. Images remain limited to 25 MiB each.

Rig, Codex, Claude Code and OpenCode accept reference attachments. PNG, JPEG, GIF and WebP are sent as
image parts. UTF-8 text, Markdown, lyrics, CSV, JSON and source files are sent as
delimited reference data, up to 256 KiB
per text file. Instructions inside an attachment remain reference content.
The selected model must support the supplied media. With a CLI backend,
audio files are analyzed locally and sent as text measurements: duration,
constant-tempo alternatives, estimated beats and major/minor chord candidates.
Long reports sample up to 256 beats and 256 chord windows across the recording,
with source timestamps and sampling strides. Analysis runs in the background
without editing or importing into the song. Raw audio stays local on these routes.

To create a piano cover, attach the recording and ask, for example, “Transcribe
this audio and make a piano cover.” The agent's `transcribe_audio` tool with backend `basic_pitch` runs
the bundled Apache-2.0 Basic Pitch model locally and returns a draft. `import_transcription` accepts it as an editable physical-piano
track with overlapping notes, source timing and estimated relative velocities.
No Python, model setup or separate audio-model connection is needed. The draft
preserves the project's tempo and existing tracks, remains unsaved, and one Undo
removes it. Review extra/missing notes, timing and dynamics before export.

Transcription accepts up to ten minutes and eight channels. Piano or isolated
pitched recordings work best; dense mixes can contain errors and percussion is
not separated. This is a note draft, not guaranteed faithful sheet music or a
two-hand playable arrangement. See [music analysis](music-analysis.md).

Audio references are immutable snapshots scoped to the conversation, retained
within eight files/1 GiB on disk in aggregate. Attaching more files can expire older
references; reattach a file if the agent reports that its reference expired.

For native audio input, use an OpenAI-compatible model implementing `input_audio`.
The panel keeps the draft available when the selected rig provider cannot send audio.
Native API turns can use the same attachment-to-piano tool.

Native API requests have a separate 128 MiB budget for the actual prepared payloads
before base64 encoding. Audio sources larger than 25 MiB are automatically converted
to full-length 16 kHz, 16-bit PCM WAV, preserving mono/stereo channels and timing.
This accepts up to thirty minutes of mono/stereo audio within the ordinary decoder's
512 MiB decoded-sample budget. No excerpt is silently selected. The original snapshot
is retained unchanged for local transcription, and the model receives metadata describing
the upload conversion. Smaller sources are sent in their original format. Preparation
runs on a cancellable worker; decoder/resampler calls finish before observing cancellation.
The selected endpoint/model can impose additional format, duration or request limits;
128 MiB is an Auris resource budget, not a promise about a provider's capabilities.
Reduce the attachments or use a CLI local-reference route if the native request cannot fit.
CLI image/text payloads retain their separate 25 MiB combined budget, excluding local audio.

## Codex setup

Install [Codex CLI](https://learn.chatgpt.com/docs/cli) and run `codex login` with
your ChatGPT account. Auris starts `codex app-server --listen stdio://`; Codex
manages its authentication and inference. Auris does not read credential files or
use subscription tokens through rig. API-key authentication is refused by this
backend. Use rig for an API endpoint.

Leave the executable path empty to search PATH. On Windows, Auris also recognizes
the native executable inside an npm installation; a custom path must name
`codex.exe`, not a PowerShell or batch launcher. The integration is tested with
Codex CLI 0.163.0. The app-server protocol is experimental, so incompatible CLI
versions may report startup errors.

Open the Agent Panel on a new, unsaved song and ask it to add an instrument track.
The track appears immediately, the song remains unsaved, and Undo removes the
track through the normal document history. The same permission modes and deny
rules apply to all backends. Saving is still an explicit user action.

## Claude Code setup

Install [Claude Code](https://code.claude.com/docs/en/setup) and run `claude` to
sign in. Configure its executable in Settings > Agent, then choose a Claude Code
model in the Agent panel. Authentication and inference stay inside the installed
CLI; the rig endpoint and API-key preference are not used. CLI authentication through
its own supported environment variables remains available.

Leave the executable path empty to search PATH and the native `.local/bin`
installation. On Windows, use the native `claude.exe`, not a `.cmd` or `.ps1`
launcher. The integration uses the bidirectional stream-json control protocol
and is tested with Claude Code 2.1.304. Models come from the CLI's initialization
response. Default effort keeps the CLI default; None and Minimal map to Low.

Claude Code connects to the same authenticated live MCP endpoint as Codex, so it
can edit unsaved songs through the normal permission and Undo paths. Each worker
gets private MCP configuration, with inherited settings, hooks, plugins, slash
commands and Chrome disabled. Its explicit native shell and file tools pass
through the desktop's approval policy. Saved conversations use the project folder
as their working directory; unsaved conversations use disposable scratch space.
The bearer token stays in the temporary
configuration file, which is removed when the worker ends. CLI session
persistence is disabled; Auris stores the conversation and restores its text
when a worker is replaced. Stop revokes tools, interrupts the turn and terminates
the child. Audio can be inspected through `inspect_audio`.

The optional transport smoke test uses an installed CLI without inference:

```sh
cargo test -p auris-agent installed_claude_connects_to_the_live_mcp_catalog -- --ignored
```

To check inference, `cargo test -p auris-agent
installed_claude_edits_an_unsaved_song -- --ignored` asks Haiku to add one track
to an in-memory song and verifies Undo. It uses the CLI's configured account and
does not open or modify a saved song.

## OpenCode setup

Install [OpenCode](https://opencode.ai/docs/) and run `opencode auth login` to
configure a provider. Leave the executable path empty in Settings > Agent to search
PATH, `.opencode/bin` and `.local/bin` (plus standard Homebrew locations on macOS),
or provide its native executable explicitly.
On Windows, select `opencode.exe`; `.cmd` and `.ps1` launchers are not supported.
Refresh the model menu in the Agent panel and choose a model labelled OpenCode.
Model IDs retain the provider and any advertised variant, so similarly named
models on different providers stay distinct. Select a model variant to control
reasoning; the separate effort control stays at Default on this backend.

Auris launches `opencode acp` and speaks [ACP v1 over stdio](https://opencode.ai/docs/acp/).
The transport supports both the original `models.availableModels` catalogue and
model `configOptions`, including grouped choices. Authentication and inference
stay inside OpenCode; Auris does not read its authentication store. The integration
checks the CLI version before launch and generates the matching configuration for
OpenCode 1.x or 2.x. V2 uses `agents`, `system`, ordered `permissions` rules and
the native `shell` tool; V1 uses its original configuration fields. Unknown major
versions are rejected. See the [current ACP reference](https://opencode.ai/v2/docs/cli/acp/)
and [V2 permissions](https://opencode.ai/v2/docs/permissions/).

Each worker uses a private configuration home and disables project configuration,
external skills, automatic sharing, snapshots, LSP and formatters. User plugins,
custom tools, custom provider configuration and other configured MCP servers are
not inherited. OpenCode's normal authentication data directory is retained, along
with the default authentication plugins and provider environment variables. Use a
provider supported by OpenCode's authentication flow or environment configuration;
the rig API settings do not configure this route. User configuration files are
not edited. Native shell and file tools request the same Auris permission gate;
unknown requests and unsolicited client file/terminal requests are refused.
V2 receives the live server through private configuration with `codemode: false`;
its token is expanded from a child-only environment variable rather than stored
in that file. This keeps each MCP operation on the direct tool list, subject to
the existing Auris gate. V1 receives the authenticated server through ACP stdin.

The authenticated live MCP server edits the open song, including unsaved changes,
through the existing permission and Undo paths. Images and text use ACP content
blocks; audio uses the same local analysis and transcription tools as the other
CLI backends. Stop revokes live calls, sends `session/cancel` and terminates the
child. Auris persists and restores its own text transcript; OpenCode may also keep
sessions in its normal data directory. Manual compaction from Auris is not yet
supported. Approval review uses a separate session with all tools denied.

Real composition, audio feedback and feature trials are recorded in
[OpenCode backend validation](opencode-backend-validation.md), including the free
provider's restriction on tool-free automatic review. Startup waits for an actual
authenticated MCP tool catalogue request before allowing inference.

Default tests use a scripted ACP peer and the real HTTP MCP endpoint to verify
live edits, Undo, streaming, permission decisions and cancellation without model
credentials. If OpenCode is installed and configured, this checks startup and the
model catalogue without inference:

```sh
cargo test -p auris-agent installed_opencode_lists_models_and_opens_a_live_mcp_session -- --ignored
```

## Runtime boundary

`auris-agent::Worker` owns the background runtime and cancellation. Its backend
owns the conversation loop: rig runs it locally, while Codex drives an app-server
child process, Claude Code uses stream-json, and OpenCode uses ACP. Provider protocol types stay
inside the backend. `ToolHost` normalizes flat toolbox calls, serializes authorization and execution, and sends
live commands to the window. The window owns the session and rechecks permissions
immediately before an edit.

Codex connects to an embedded Streamable HTTP MCP server on an ephemeral loopback
port with a random per-conversation bearer token. This endpoint exposes the live
tool catalog, not the saved-file `auris-mcp` catalog. It supports unsaved documents
and never opens a second copy of the song. Codex's tool approvals are delegated
to the common ToolHost; the application's live permission checks still authorize
every call. Its listener and child process are revoked when the worker ends.
Stop requests interrupt the Codex turn before the
process is terminated, while the window immediately stops accepting old edits.

CLI configuration uses a private directory, while native tools work in the saved
project folder or disposable scratch space for an unsaved conversation. Auris
disables inherited MCP servers, plugins, apps, hooks and subagents for this
invocation, and, for Codex and Claude Code, checks the effective MCP catalog before sending a prompt. Native
shell and file operations use the same approval policy; their external effects
are not covered by Auris Undo. User configuration files are
not edited. Unexpected server-side requests are refused. Process arguments never
include the live MCP bearer token.

Completed exchanges use the existing Auris transcript storage. Codex threads are
ephemeral; a replacement worker receives the stored text as historical context
once, then Codex owns the active conversation. The transcript preview and reset
generation checks remain shared with rig.

The panel's [conversation list](agent-conversations.md) keeps multiple independent
histories, drafts and readable tool results for each project.

## Validation

`cargo test -p auris-agent` includes a scripted app-server peer that calls the real
HTTP MCP endpoint, passes the common permission gate, edits a real unsaved
headless session and verifies Undo. The existing desktop harness verifies live
edits and permissions in the window. The default tests require no model account.

With a ChatGPT-authenticated Codex installation, this optional transport check
starts app-server and verifies its live MCP catalog without running inference:

```sh
cargo test -p auris-agent installed_codex_connects_to_the_live_mcp_catalog -- --ignored
```

To verify a real model turn, `cargo test -p auris-agent
installed_codex_edits_an_unsaved_song -- --ignored` uses the signed-in ChatGPT
subscription to add one track to an in-memory session and verify Undo. It does
not open or modify a saved song.
