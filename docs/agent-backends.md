# Agent backends

The Agent panel discovers the signed-in Codex, Claude Code and Antigravity CLIs, local Ollama,
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

Rig, Codex and Claude Code accept reference attachments. PNG, JPEG, GIF and WebP are sent as
image parts. UTF-8 text, Markdown, lyrics, CSV, JSON and source files are sent as
delimited reference data, up to 256 KiB
per text file. Instructions inside an attachment remain reference content.
The selected model must support the supplied media. With Codex, Claude Code or Antigravity CLI,
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

## Antigravity CLI setup

Install [Antigravity CLI](https://antigravity.google/docs/cli/install/) and run
`agy` to sign in. Settings > Agent accepts an optional native `agy` / `agy.exe`
executable path; leaving it empty searches PATH and `.local/bin`. Refresh the
Agent model menu and select an option labeled **Antigravity CLI**. Model slugs
come from `agy models`; the rig API URL/key settings are preserved. Default
reasoning keeps the CLI default; Low, Medium and High are available.

The integration is tested with CLI 1.4.6. It uses the documented
[stream-json input and output](https://antigravity.google/docs/cli/headless/)
and structured output. Unlike Codex and Claude Code, this protocol has no external
tool approval channel and accepts text blocks only. An ephemeral
[custom agent](https://antigravity.google/docs/subagents/) exposes only `finish`,
which returns structured toolbox requests without executing them. Auris checks
all requested tool names and object shapes before dispatch, validates and executes
each call through the common ToolHost, and sends
the results back to the same CLI conversation. All document edits therefore use
the ordinary live permissions and Undo. The text catalog retains parameter units,
examples and field descriptions. Continuation messages distinguish host instructions
from untrusted tool results and explicitly request an empty call list when finished.
Repeated identical tool failures or three consecutive identical request/result
batches stop the turn without reverting earlier edits. Native CLI
shell/file/browser/subagent tools are unavailable; no permission bypass flag is used.

Text references and local audio measurements are supported. Image attachments are
refused before submission, retaining the draft for another backend. Structured
responses are buffered until validated rather than displaying raw JSON fragments.
Manual compaction is unavailable. Completed exchanges use Auris conversation
storage; replacement workers receive historical text and never resume a native
CLI conversation ID. The CLI may retain its own native transcripts under its
ordinary profile. Stop closes stdin and terminates the child while Auris cancels
pending tools and refuses late edits.

The invocation uses a private temporary folder for its agent, schema and log, and
removes it on shutdown. Authentication stays inside the installed CLI. Because
the CLI cannot isolate global hooks, MCP servers or plugins per invocation, the
backend refuses profiles containing those extensions rather than inheriting
them. It does not edit global CLI configuration or reuse the song folder as a
CLI workspace. Empty global configuration files are accepted.

`cargo test -p auris-agent antigravity` runs scripted transport, approval, denial
and Undo tests without an account. The optional signed-in inference check is:

```sh
cargo test -p auris-agent installed_antigravity_returns_structured_tool_requests_without_native_tools -- --ignored
```

It requests one structured `add_track` call without executing it or opening a song.

The broader opt-in suite uses the signed-in CLI and real inference:

```sh
cargo test -p auris-agent installed_antigravity_ -- --ignored --nocapture --test-threads=1
```

It composes a disposable four-bar melody/bass/drum arrangement, reads back notes and
harmony, edits pitch/performance/mixer controls, and verifies request-level Undo/Redo.
It tests sound selection, track renaming, clip copying and playback loops, then
renders and measures an eight-second WAV, exports MIDI, submits text/audio
references, restores conversation history in a replacement worker, tests denied and
cancelled edits, and checks model discovery and independent approval/denial reviews.
Outputs and conversation files live in temporary directories; no saved song is opened.
These checks require the `gemini-3.8-flash-low` model and consume inference credits.

## Runtime boundary

`auris-agent::Worker` owns the background runtime and cancellation. Its backend
owns the conversation loop: rig runs it locally, while Codex drives an app-server
child process, Claude Code uses stream-json and Antigravity returns structured
requests for Auris to execute. Provider protocol types stay
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

Codex and Claude Code use private configuration directories, while their native
tools work in the saved project folder or disposable scratch space for an unsaved
conversation. Auris disables inherited MCP servers, plugins, apps, hooks and subagents for these
invocations, and checks the effective MCP catalog before sending a prompt. Native
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
