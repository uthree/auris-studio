# Live Agent Panel and MCP workflows

The rig Agent Panel edits the currently open session through `edit_project`. It works before
the first save: ask for a song and the composed tracks appear in the arrangement. Commands
read the latest document, use stable track/clip IDs and record normal undo steps. Saving
remains a user action. Whole-song composition requires `replace: true` for a nonempty song;
local changes use track, clip and note commands instead.

MCP retains the full saved-file tool catalog. The file-based composition, rendering and
listening workflows below describe MCP.


The Agent Panel and MCP expose the same project commands through `auris-toolbox`.
Start with `capabilities` to check the General MIDI library, voice library paths and
project playback state. Registered sampler code is not a loaded SoundFont: when a
composition cannot load its requested GM preset, the session substitutes a built-in
instrument and reports the substitution. A singer without a rendered take plays a
temporary guide voice. Preview and export report missing instruments, guide vocals
and stale takes; `sing` produces or refreshes the actual vocals.

Voice discovery uses the same standard directories and additional `voice_paths` as
the desktop browser. Discovery lists candidate files without validating/loading them.
`capabilities` with a project also reports selected voice metadata and speaker names
when loaded. A playable preview is an audio artifact; a language model needs audio
input support to listen to it. Auris's rig Ollama adapter does not carry direct
audio attachments; use `listen` to send an audition through the audio critic's
OpenAI-compatible endpoint.

## Listen, revise, listen again

The controller and the listener can be different models. Qwen/Ornith can choose
editing tools while `listen` sends the rendered WAV to an audio-capable critic.
The critic returns observations and has no tools that can modify the project.

1. Inspect the existing song with `describe`, `inspect_composition` and `mixer`.
2. Call `listen` with `project`, `start_bar:1`, `bars:4` and an optional `focus`.
   It returns `audio_path`, the critic model, its review, and separate measurements.
3. Decide whether the observations support one specific local edit. Leave the mix
   unchanged when no correction is warranted or the critic cannot judge it. For a
   supported correction, keep a named checkpoint and use local note, clip,
   instrument, routing, effect or gain edits.
4. After editing, read back the affected state, then call `listen` on the same bars. Pass the earlier
   `audio_path` as `compare_to` for an A/B review. Both actual audio files are attached.
5. Keep, refine or restore the change according to the evidence. Stop when the
   requested result is met or the remaining uncertainty needs human judgment.

Keep `focus` concise and about the sound, such as "Describe the melody's prominence
relative to the accompaniment." Omit it to use the default instrumentation and
balance question. The session supplies the reviewer's grounding instructions;
focus does not need to repeat them or prescribe a finding. A local same-audio
comparison recovered music descriptions with a shorter question, but the resulting
details and suggested changes still require scrutiny.

The experimental default uses `gemma4:e2b` at `http://localhost:11434/v1`. Local
controls verified speech input with this model, but did not establish musical
discrimination. An answer about missing speech is not an assessment of instrumental
music. The server must already have the model. Configure a validated audio-capable OpenAI-compatible
server through the host environment before starting MCP or the agent:

```powershell
$env:AURIS_AUDIO_URL = 'http://localhost:11434/v1'
$env:AURIS_AUDIO_MODEL = 'gemma4:e2b'
# Optional: name an existing environment variable that holds the server's key.
$env:AURIS_AUDIO_API_KEY_ENV = 'MY_AUDIO_API_KEY'
```

`audio_sent` records successful transmission, not verified musical understanding.
Do not treat an HTTP 200, a fluent response, or numeric measurements as proof that a
model heard the excerpt correctly. If the critic says it cannot hear, report that
response as a limitation and check the request and audio backend, or ask for human
listening feedback.
Validate a new listener with known audible differences before trusting its advice.
The local interface trial records observed model limitations separately from tool
and wire correctness in `docs/reviews/local-model-interface-2026-09-07.md`.

An optional [local audio service](../tools/audio-review/README.md) provides
the same endpoint independently of Ollama, with Qwen2-Audio and an opt-in native
MusicFlamingo backend. It includes GPU/CPU setup, revision pinning and bounded WAV
requests. For the MusicFlamingo research configuration, start its backend as shown
in the service guide, then configure the Auris process:

```powershell
$env:AURIS_AUDIO_URL = 'http://127.0.0.1:11435/v1'
$env:AURIS_AUDIO_MODEL = 'nvidia/music-flamingo-2601-hf'
```

MusicFlamingo's weights are restricted to noncommercial research by their NVIDIA
license. The bridge reviews paired excerpts independently and labels each result;
it explicitly reports that the audio model made no direct A/B judgment. The
controller can compare those observations, but should retain that distinction.
The local service downmixes audio to mono at 16 kHz and reports this in every
response. Its reviews cannot assess stereo placement, even if the model claims
otherwise. Incomplete reviews are errors with retry guidance; the MusicFlamingo
pair path allows each independent observation the same maximum output budget as
a single review.
Successful model loading and music recognition still require separate validation
of proposed corrections. The local trial found Qwen2-Audio's comparisons unreliable
and MusicFlamingo's descriptions useful but fallible; neither is an automatic
guarantee of mix quality.
The tested AMD Windows audio runtime also had a native crash during a later model
reload. A running HTTP service and an earlier successful review do not establish
restart reliability; inspect `/healthz` and the server log when a request fails.

## Tool arguments and project setup

`tool_help` takes an exact tool `name` and returns its current schema and examples.
Both frontends use the same catalog and inline nested schemas. Invalid arguments
produce corrective feedback; unknown fields in clip edits and previews are refused.
For example, eight bars starting at bar 1 resize with
`action:{kind:"resize",end_bar:9}`, and a four-bar preview uses top-level
`start_bar:1,bars:4`. Note beats follow the meter; automation uses absolute
quarter-note beats starting at zero.

`create_project` starts with one empty instrument track. `import_audio` copies an
audio file onto a new track in an existing project; `import_midi` creates a new
project retaining the MIDI clock. `export_midi` writes a new MIDI file. Creation
and export refuse replacement; always copy the returned actual project path.
`add_track` requires an explicit `kind` (`instrument`, `singer`, `audio`, `bus`),
and `add_clip` requires a `name`. A track named Reverb is a bus only when its kind
is `bus`. Read back exact requested names and types along with musical values.

`routing` lists available buses and send IDs, changes a track output, and creates
or removes sends. Its `send_level` operation adjusts an existing send with `level_db`,
selected by `destination` or `send_id`. `set_track_state` sets mute
and solo explicitly, with other tracks' solo states preserved. Discover instrument
parameters, discrete choices and ranges through `automation` with
`target:{kind:"instrument"},operation:{action:"read"}`; `set_instrument_param`
sets a static value and reports when an existing lane overrides it.

## Local musical changes

Composition stores authored motif steps and rhythm patterns in each generated clip's
recipe. `regenerate_clips` and `edit_recipe` retain them. The required `take` in
`regenerate_clips` is `{kind:"same"}` to keep each seed, `{kind:"next"}` to advance it,
or `{kind:"seed",seed:42}` to choose a seed for one clip. `edit_recipe` accepts `motif` (relative
scale steps such as `0 2 4 2`) and `rhythm` (`x` hit, `X` accent, `o` ghost, `.` rest);
an empty string clears that authored control. Density, gate, dynamics and subdivision
remain available for articulation and phrasing.

Regeneration writes a local take in the current composer. Whole-song arrangement
context can produce different notes from a local take even with the same seed. Freeze
the clip to preserve an exact take. `edit_clip` with
`action: {kind: "copy", destination: "Flute", bar: 9, transpose: 12}` copies the stored
phrase to another note track. The source stays intact; a transposed copy is frozen so
regeneration cannot undo the transposition. Re-read clip numbers after arrangement edits.

## Effects and automation

`effects` takes a project, track (or `master`) and an `operation` object:

```json
{"action":"list"}
{"action":"add","effect":"auris.fx.compressor"}
{"action":"sidechain","slot":1,"source":"Kick"}
{"action":"enable","slot":1,"enabled":false}
{"action":"move","slot":1,"position":2}
{"action":"remove","slot":2}
```

List returns registered effect IDs and actual slot positions, enabled states and
sidechain capability/connections. Slot numbers are 1-based and change when the chain
is reordered. A null sidechain source disconnects it. Unsupported inputs and routing
cycles are rejected. Use `set_effect` for static values, in the parameter's own units.

`automation` takes a project, track, target and operation. Targets are
`{kind:"mixer"}`, `{kind:"instrument"}`, `{kind:"effect",slot:1}` or
`{kind:"send",destination:"Reverb"}`. Read with `operation:{action:"read"}` and no
`param` to discover parameter keys, units, ranges, static values and existing lanes.
Then put the selected key inside the operation, for example on the compressor:

```json
{
  "action": "set",
  "param": "threshold_db",
  "points": [{"beat":0,"value":-6},{"beat":32,"value":-24}],
  "curve": "linear",
  "replace": false
}
```

Beats are absolute quarter notes from zero, regardless of meter. Values use native
parameter units. Set merges points by position; `replace:true` replaces the entire
lane. `operation:{action:"clear",param:"threshold_db"}` removes it. Linear ramps and held values are supported;
discrete parameters require hold. A batch validates every point before changing
anything and is one session undo step. Tools checkpoint and save mutations. Removing
an effect also removes its lanes. Read and list operations do not trigger project reloads.

## Ollama configuration and failure recovery

The Agent Panel exposes context size (32K, 64K, 128K, 256K) and thinking (model default,
off, on), stored in shared preferences. Output token limit selects 4K, 8K, 16K,
32K, or 64K per response and applies to the next request. Raising output also raises
the context window when needed to leave room for input; lowering context can lower
the output limit. The default request sets `options.num_ctx=32768`,
defaults each completion to `options.num_predict=4096`, and uses temperature zero for
repeatable tool arguments rather than inheriting a
model's chat sampling preset. These are request settings and do not change the
installed model or Ollama server configuration. The requested context is
independent of Ollama's server default. Values below 16384 are refused because the
tool catalog alone needs substantial context. Model discovery reports requested
context separately from the architecture's maximum. Preflight checks tool support
and refuses requests exceeding a reported model maximum.

Before each model call, the context guard budgets the tools, preamble, history and
4096 tokens of output reserve. After a response reports its input token count, an
unchanged request prefix uses that measured count plus an estimate for newly added
messages. The estimate counts message content directly, without counting JSON
transport escaping again, and allows extra room for non-ASCII text. First calls,
changed or shortened histories, unavailable usage counts and opaque media use a
conservative structural estimate. These fallback counts are not tokenizer measurements.

Before a new request, old completed conversation exchanges can be removed together
to make room. The current request and its in-progress tool calls and results remain
intact. If the remaining budget exceeds the window, the agent stops with instructions
to increase context or start a fresh conversation. OpenAI-compatible providers keep
their own context policy and do not receive Ollama-specific parameters.
An Ollama response stopped by the generation limit is an incomplete turn, even if
it contains text or a syntactically valid tool call. The agent reports that limit
before executing those calls; previous live edits remain available for a smaller
follow-up request.

The panel's context gauge uses input tokens from the last model request, rather than
adding the inputs of every tool-loop step. JSON answers expose that count as
`input_tokens`, with the run's aggregate separately available as `total_input_tokens`.

Five minutes without provider progress ends a stalled model request. Tool work is
allowed to finish, and active multi-step composition can exceed five minutes overall.
Two failures with identical tool arguments stop a third execution; changing arguments
allows a correction. Successful calls clear that signature's failure count. Interrupted
JSON conversations retain the request and interruption summary so a following turn can
inspect the current document and resume. Completed live edits remain in the session.


## Agent permissions and context compaction

The rig Agent Panel has four persistent modes, following the permission model in
[Picocode](https://github.com/uthree/picocode):

| Mode | Behavior |
| --- | --- |
| Read-only | Read immediately; request approval for edits and internet search unless allowed by a rule. |
| Edit | Apply ordinary live edits; confirm removals, arrangement replacement, and internet search. |
| Plan | Inspect and propose a plan; reject document changes, including allow-listed changes. |
| Bypass | Skip confirmations while continuing to enforce deny rules. |

Deny rules win over every mode and allow rule. The Permissions section offers
Default, Allow, and Deny for each operation, `edit_project.*`, and `*`.
An approval shows the project, operation and exact arguments. Allow once applies
only to that command at the current document revision; edits made while awaiting
approval invalidate it. Always allow saves an operation rule. Rejecting an operation
returns a refusal to the model. Changing modes or rules cancels pending approvals.
Stopping the agent cancels its pending work.

Use the mode buttons or `/mode read_only|edit|plan|bypass`. Shift+Tab cycles the
three ordinary modes; bypass requires an explicit choice. `/permissions` opens
rules, and `/allow OPERATION`, `/deny OPERATION`, `/default OPERATION` edit them.
While confirmation is pending, Escape denies, Ctrl+Enter (Command+Enter on macOS)
allows once, and adding Shift always allows.

These modes govern rig calls only. Even bypass edits only the open document;
file creation, export, and saving remain outside its tool catalog. MCP retains its
existing tools and does not apply the Agent Panel's policy.

Compact context or `/compact` summarizes older exchanges with the configured model,
without granting that model tools. The latest two completed exchanges are kept
verbatim. The summary retains goals, constraints, decisions, identifiers, progress,
failures and unfinished work. It is saved with conversation history and restored
when that conversation resumes. An empty, oversized, or failed summary leaves the
original history intact. Very large exchanges that cannot fit a summary request
also remain intact.

Automatic compaction runs between requests at the selected estimated history
threshold (70% or 85%; default 85%), or after 16 completed exchanges. It can be
turned off in Permissions. Counts are estimates, not tokenizer measurements;
the existing per-request context guard remains the final budget check. Compression
never runs in the middle of a tool operation or an approval request.
