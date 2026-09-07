# Local-model interface audit

This audit compares the public session commands with the tools presented through
MCP and the rig agent. It targets practical operation by local models around
9B–27B parameters, including `ornith-1.5:9b` and `qwen3.8:27b`. Compatibility is an
observed behavior of a model, its tool template and its server configuration; a
parameter count alone does not establish it.

The starting implementation registered the same 42 project tools at both doors.
The main omissions were shared capabilities, rather than a tool available through
only one transport. Basic workflows could create a bus without routing anything
into it, create an audio track without importing audio, discover an instrument
parameter without setting its static value, and read mute/solo switches without
changing them. The earlier [genre trial](https://github.com/uthree/auris-studio/blob/95dea3586f06b8f14328ec5fb5f8bdf72dcb3b3d/docs/reviews/agent-genre-fixes-2026-09-06.md) also
demonstrated repeated mistakes in clip-resize and automation arguments by a real
`ornith-1.5:9b` run.

## Coverage and changes

| Workflow | Starting coverage and gap | Change in this work |
| --- | --- | --- |
| Create music | `compose`, `compose_lyrics` and manual note tools existed, but an empty document required a composition first. | `create_project` saves a new empty document with one default instrument track, optional tempo/meter and no clips. It returns the actual nested project path. |
| Bring in existing material | The session could import audio/MIDI and export MIDI, but neither model transport exposed those commands. | `import_audio`, `import_midi` and `export_midi` provide file interchange. MIDI import creates a separate document to retain its own clock. New project/export destinations do not replace existing files. |
| Build a bus mix | `add_track` could create buses; `set_send` could only move existing send levels. | `routing` reads outputs/sends and available buses, changes outputs, adds/removes sends and selects pre/post-fader taps. Cycle checks stay in the session. Stable send IDs disambiguate duplicate sends. |
| Audition track combinations | `mixer` reported mute and solo, but tools could not change them. | `set_track_state` writes explicit mute/solo values and returns the actual solo selection. Solo remains additive and omitted switches are preserved. |
| Shape an instrument | `set_instrument` selected the plugin or GM patch. `automation` exposed its parameter ranges but no static setter. | `set_instrument_param` changes a validated static value and reports existing automation. Parameter discovery now includes discrete step counts and choice labels. |
| Effects and automation | Effect slots, sidechains, native-unit parameter changes and automation were already available. | Existing commands remain shared. Additional discovery metadata tells a model what a discrete value means. |
| Edit an arrangement | Generated recipes, local regeneration, note entry, clip copying and checkpoints were already available. | Clip descriptions show exact resize/move argument objects; unknown fields are rejected. `end_bar` is documented as exclusive, and note-position beats are distinguished from automation beats. |
| Recover from argument mistakes | Schemas existed, but nested references were difficult for some tool templates to present, and a model had no focused reference tool. | `tool_help` returns one tool's exact schema and selected examples. Schemas inline nested definitions. Both transports compare registrations and schemas against a shared catalog. |
| Recover through MCP | Tool execution errors were readable results; argument decoding could instead produce protocol errors. | Known-tool argument errors become recoverable tool errors with a `tool_help` hint. Unknown tools and server errors retain their protocol classification. |
| Recover through rig | Repeated execution failures were bounded, but invalid tool names and some argument failures interrupted correction. | Invalid calls receive corrective feedback; repeated identical failures remain bounded. Old completed text exchanges can be omitted from an oversized request without splitting tool calls from their results. |
| Review rendered sound | `preview` produced WAV files; `analyze` and `analyze_music` returned measurements rather than auditory judgments. | `listen` submits actual rendered WAV bytes to a separate configured audio-capable critic, optionally with a previous preview for comparison. Its critique is returned to the controller for a local edit and another listen. |

The implementation remains below the frontend boundary where appropriate:
[project file tools](../../crates/auris-toolbox/src/project_files.rs),
[track controls](../../crates/auris-toolbox/src/track_editing.rs),
[shared catalog](../../crates/auris-toolbox/src/catalog.rs), and
[audio review command](../../crates/auris-session/src/audio_review.rs).
The original session APIs supply routing, import/export and document mutation; the
toolbox supplies argument validation and model-readable results.

## Smaller-model usability

The standing instructions now group the workflow into reading, editing, checking
and listening. They ask for dependent calls one at a time, exact tool-help lookup
after argument errors, and saved-state verification before reporting completion.
They distinguish four easily confused quantities:

- Bars and note-position beats start at one. In 6/8, a note-position beat is an
  eighth note.
- Automation positions start at zero and always count quarter notes. Position 16
  is the beginning of bar 5 in 4/4.
- A clip beginning at bar 1 and resized to exclusive `end_bar: 9` spans eight bars.
- Preview's `start_bar` and `bars` belong at the top level, not inside a `range`
  object. Misnested preview arguments are rejected before rendering.

The new routing and track/file tools use flat argument objects and small enums.
They validate fields specific to an operation before saving. Outputs report actual
state, allowing the next call to use returned paths and IDs rather than infer them.
Instrument automation readback supplies labels as well as numeric ranges, so a
model can discover a waveform choice instead of guessing its number.

Track creation now requires an explicit `kind` enum, and empty clip creation
requires a nonempty `name`. Missing values produce corrective errors. A real
Ornith run had silently accepted the previous defaults, creating an instrument
named TrialBus and a clip named melody while claiming to have met the requested
bus type and Manual name. The instructions also require verification of names
and track types, not just note and mixer values.

The shared catalog prevents transport registration and schema drift. It does not
prove that every public session method is exposed, nor that a model will follow a
valid schema. Those are separate coverage and behavioral questions.

## Audio review and fallback

The controller model does not acquire hearing merely because a WAV path appears in
a tool result. `listen` sends the WAV bytes to the configured critic and returns its
observations as evidence for the controller. A comparison should use the same
range before and after a targeted change. A successful upload establishes delivery
and a response, not the accuracy of the critic's judgments.

If the critic is unavailable or lacks audio input, the result must retain that
failure. The preview remains available for the user, while `analyze` can still
report levels and `analyze_music` can inspect written notes. Those measurements
cannot be relabeled as a listening assessment. Guide vocals, stale takes and
missing instruments remain relevant even when the critic responds successfully.

Audio import also distinguishes success in decoding from success in copying: the
session can retain an external source when its project-folder copy fails. The tool
reports that condition rather than saying the project is self-contained.

## Remaining interface gaps

These are audit findings, not evidence that the new workflows failed. Exposing
every session method without controlling schema size would create a different
usability problem for the target models.

| Area | Remaining gap and consequence |
| --- | --- |
| Existing-note transformations | Session commands can transpose, move, quantize, resize and change velocities in place. The tool surface principally removes/adds notes or copies entire clips. Remove/add can discard a note's lyrics, phonemes or ornaments. |
| Audio-clip editing | Audio import creates playable tracks, but `edit_clip` addresses note clips, and `describe` does not number audio clips for that tool. Trims, fades, source tempo and audio-clip movement need a distinct addressable interface. |
| Vocal correction | Voice selection, lyrics and singing are exposed; per-note phoneme overrides, phoneme timing, scoop/fall/vibrato and frame controls remain session-only. |
| SoundFont libraries | Built-in instruments and shipped GM sounds are discoverable. Importing a custom SoundFont and selecting its bank/preset need additional tools. |
| Hosted plugins | The session supports discovery and loading of CLAP/VST3 plugins. Model tools can inspect/control some already-loaded parameters, but cannot discover and insert arbitrary hosted instruments/effects. |
| Live transport and recording | The tool contract opens a fresh headless session per call. Live playback, device monitoring and recording need a persistent-session protocol, not just wrappers around current methods. |

## Verification status

Regression tests added in this work check persisted state and failure behavior:
empty creation followed by manual notes; MIDI timing/clock round trips; copied audio
reopened after the original source is removed; no-clobber exports; routing cycles
and duplicate sends; mute/solo preservation; static parameter changes alongside
existing automation; schema examples; rejection of guessed or misnested fields;
and meter-sensitive clip positioning. Catalog equality tests compare the MCP and
rig registrations with the same shared definitions.

`cargo test --workspace` passed, including doc tests, in
`target/workspace-model-final.log`. Two pre-existing Windows test dependencies
encountered during verification were repaired in their fixtures: the drum gesture
test resets a panel layout persisted by an earlier test, and a portrait HTTP mock
explicitly puts accepted sockets in blocking mode before its existing read timeout.
Neither repair changes application behavior. `cargo clippy --workspace --all-targets`
also passed (`target/listening-final-clippy.log`); its dependency future-compatibility notice
concerns `proc-macro-error2`, not a new source warning.

The Rust checks include 57 toolbox tests and both transport
catalog comparisons. Documentation also builds with `RUSTDOCFLAGS=-D warnings`
(`target/listening-final-docs.log`). After adding the actual 4096-token Ollama response
cap and truncation handling, all 31 agent tests passed
(`target/agent-output-cap-tests.log`). The final concise-listening-prompt build also
passed all affected session, toolbox, MCP and agent tests, including their doc tests
(`target/listening-prompt-tests.log`). The final output-limit error regression also
passed all five audio-review tests (`target/listening-limit-tests.log`). The
optional Python audio service passes 34
pytest tests and Ruff; these exercise actual WAV decoding, HTTP requests, input
ordering, revision pinning, truncation, and GPU-cache cleanup without model weights.

A further integration test,
`listening_after_a_saved_edit_submits_distinct_audio_and_the_immutable_before`,
drives the complete listening workflow against a scripted HTTP critic. It renders
a directly authored note, submits the actual WAV, saves a -12 dB fader change, then
submits both the original and revised WAVs. Assertions check the number and bytes
of the audio payloads, the original file's immutability, the expected amplitude
ratio `10^(-12/20)`, distinct preview paths and unchanged notes. The critic's replies
are explicitly scripted; this test checks transport and editing, not listening
quality. It passed in the complete workspace run.

Reproduce these checks from the repository root:

```powershell
cargo test --workspace
cargo test -p auris-toolbox listening_after_a_saved_edit_submits_distinct_audio_and_the_immutable_before
cargo clippy --workspace --all-targets
```

## Model evaluation

Controller trials use local Ollama 0.33.3, a 32768-token context, thinking off,
at most 16 turns and a 600-second timeout per provider/wire response. Both
installed target models advertised native tool support in `/api/show` and passed a small typed-function
probe. The harness runs a real MCP stdio connection or the actual rig agent and
checks the saved document rather than accepting the model's completion message.

The editing task renames the lead, sets its gain/pan, creates a real bus, and adds
a named two-bar manual clip with three exact notes while preserving the original
lead and bass material. The production task adds one compressor, connects the
bass sidechain, sets its threshold to -24 dB, and writes a linear lead gain lane
from -12 dB at quarter-note beat 0 to -3 dB at beat 8. Deterministic MCP controls
passed both tasks in `target/agent-tools/20260907-053425-d41dc9`.

The first editing results, from an intermediate build, are recorded in
`target/agent-tools/20260907-053558-34a440/summary.json`; the run directory also
contains prompts, wire events, before/after documents and executable hashes.

| Model | Transport | Saved-state verdict | Calls | Tool errors | Elapsed |
| --- | --- | --- | --- | --- | --- |
| `qwen3.8:27b` | MCP | Passed all editing checks | 9 | 0 | 331.8 s |
| `qwen3.8:27b` | rig | Passed all editing checks | 11 | 2, recovered | 313.2 s |
| `ornith-1.5:9b` | MCP | Passed all editing checks | 8 | 0 | 120.0 s |
| `ornith-1.5:9b` | rig | Failed; requested edit incomplete | 9 | 4 | 45.1 s |

The failed Ornith run supplied routing's `operation` as a tuple/array instead of a
string and repeated the error until the existing guard stopped it. Its bass
material remained preserved, but the requested renamed lead was absent. This is
an observed failure, not a successful task with an inconvenient final message.

The initial runs were not a controlled comparison of transport alone: the MCP
harness explicitly used temperature 0 and seed 71421, while rig inherited the
model preset's sampling temperature. In response, string-only `oneOf` branches
containing constant values are collapsed into an ordinary string `enum`, and rig's
Ollama request sets temperature 0 without changing the installed model or saved
server settings. Subsequent tuning uses Ornith; Qwen is reserved for the final
benchmark. The following saved-state results retain the intermediate failures.

Those earlier MCP runs also limited each response to 2048 output tokens, whereas
rig had no corresponding request cap. The final harness uses 4096 to match rig's
new output cap and treats `done_reason: "length"` as incomplete before executing
returned calls or accepting a final answer. Earlier artifacts retain their actual
settings; they are not retroactively described as a controlled sampling comparison.

| Build change and run directory under `target/agent-tools` | Transport/task | Verdict | Calls/errors | Elapsed |
| --- | --- | --- | --- | --- |
| String enums and temperature 0; `20260907-055700-9a028e` | Ornith rig editing | Failed: TrialBus was an instrument and the new clip was named melody | 10/0 | 44.4 s |
| Required track kind and clip name; `20260907-061617-db3e6c` | Ornith rig editing | Passed all checks | 10/0 | 35.9 s |
| Same build; `20260907-061617-db3e6c` | Ornith rig production | Failed: threshold remained unset; context guard stopped correction | 12/3 | 23.1 s |
| Same build; `20260907-061719-f7ce5b` | Ornith MCP production | Passed after accepting a valid constant threshold automation lane | 21/4 | 79.3 s |
| Object-union type hints and required effect-slot schema; `20260907-063943-d0e42a` | Ornith rig production | Failed: threshold saved, but sidechain and gain lane absent when context guard stopped correction | 12/2 | 38.1 s |
| Provider-aware context accounting; `20260907-070835-2a61de` | Ornith rig production | Passed all checks, including static threshold and bass sidechain | 19/1 | 61.2 s |

The zero-error editing failure was semantic: notes, timing and fader values were
correct, but omitted kind/name fields selected defaults and the model still
claimed completion. Requiring those fields resolved that case in the next run.

The MCP production verdict was initially too narrow: it required a static
threshold parameter. The actual document contained a correctly targeted
`threshold_db` automation lane with both endpoints at -24 dB. A lane holds its
nearest endpoint before and after its written range, so this sets the threshold
throughout the render interval. `reassessment.json` records this accepted
alternative; the original `result.json` and `summary.json` retain the initial
failure. The revised checker records static versus automated realization. It
still rejects the rig run with no threshold and preserves the exact gain-lane
requirements.

The object-hint rig retry used valid object operations and correctly included
`slot: 1` in `set_effect`. It then invented `effects.operation.action: "set"` and
omitted the sidechain slot. After a 3015-character `tool_help` response, the guard
stopped at an estimated 33578 tokens against the requested 32768. The previous
production attempt stopped at 33048. Both runs reported failure, retained saved
partial edits, and did not emit a false completion answer. That estimate counted
serialized JSON bytes divided by three plus a 4096-token output reserve; these
are guard cancellations, not observed Ollama context-limit errors. For comparison,
the different, completed MCP transcript used 22732 actual prompt tokens in its
last response. This does not establish that either rig transcript would fit.

Context accounting now estimates the content actually sent to the provider,
rather than repeatedly escaping text inside a serialized transcript. When the
provider reports actual input usage, a matching text-only request prefix supplies
the baseline and only newly appended content is estimated. Changed prefixes,
missing usage and opaque media retain a conservative fallback; the 4096-token
output reserve remains. The next 32K rig production run completed with one
recovered error and 22807 actual input tokens on its final request. Saved-state
checks confirmed the compressor, sidechain, static threshold, exact gain lane and
unchanged notes. This resolves the observed guard interruption in that trial;
one success is not a general reliability rate.

Thus both editing and production have demonstrated Ornith passes through both
doors across these builds. Each run's
`environment.json` preserves full executable SHA256 hashes and sampling settings;
results from different build stages must not be presented as one final benchmark.

After small-model tuning, Qwen ran the final Japanese production benchmark on
the latest binaries with 32768 context tokens, temperature 0, thinking off and a
4096-token response ceiling. Results are in
`target/agent-tools/20260907-081142-f83d4b`.

| Model | Transport | Task | Saved-state verdict | Calls | Tool errors | Elapsed |
| --- | --- | --- | --- | --- | --- | --- |
| `qwen3.8:27b` | MCP | Japanese production | Passed all 8 checks | 15 | 1, recovered | 389.0 s |
| `qwen3.8:27b` | rig | Japanese production | Passed all 8 checks | 13 | 0 | 336.5 s |

Both saved exactly one compressor with the bass sidechain, a static -24 dB
threshold, and the requested linear gain lane and curve, preserving the original
lead/bass notes and generated clips. In the MCP run, Qwen first sent an invented
`set_sidechain` operation to `automation`; after the validation error it consulted
`tool_help` and recovered with the correct `effects` sidechain operation. The rig
run completed without tool errors. These are actual saved-state passes for the
tested task, not a measured general reliability rate.

The final MCP executable SHA256 is
`485115E5A2F9F8536AEB4A22BAA3DB9C0BE3E79CEB8C719237DA4A9E421B19F5`;
the rig executable is
`1E15EF646E444F43AB016E5C7DA62EBD190C602A649F85304FE6680EF9F522C4`.
The model digest was
`22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643`.
Saved `/api/ps` snapshots reported Q4_K_M, 27.3B, 32768 context tokens,
18867667595 total bytes and 12237353778 VRAM bytes. This was a partially
GPU-resident run; its latency does not describe a fully GPU-resident deployment.

Two Japanese listening trials also ran on an intermediate build with the E4B
critic. They prescribed a -6 dB lead edit, rather than deriving an edit from a
musical diagnosis:

| Controller/transport | Run directory under `target/agent-tools` | Mechanical checks | Calls/errors | Elapsed |
| --- | --- | --- | --- | --- |
| Qwen rig | `20260907-060030-b93697` | Passed | 5/0 | 582.3 s |
| Ornith MCP | `20260907-061016-b84e8f` | Passed | 5/0 | 109.3 s |

Both submitted audio twice, saved the exact fader change without changing notes,
created distinct WAVs, and supplied the immutable first recording as `compare_to`.
Both critics said they could not hear the audio. The controllers reported that
limitation rather than inventing a successful auditory comparison. This validates
orchestration and honest handling of an unusable critique, not perceptual
improvement or critique-driven editing.

The harness can reproduce the editing cases without changing installed models or
application preferences:

```powershell
cargo build -p auris-mcp -p auris-agent
pwsh -File tools/eval/agent_tools.ps1 -Transport control
pwsh -File tools/eval/agent_tools.ps1 -ContextTokens 32768 -TimeoutSeconds 600

# Reserve this larger model for a final benchmark after small-model tuning.
pwsh -File tools/eval/agent_tools.ps1 -Models qwen3.8:27b -Scenario production -PromptLanguage ja

# Use a separately validated music critic for an unprescribed fader adjustment.
pwsh -File tools/eval/agent_tools.ps1 -Scenario critique -PromptLanguage ja
```

The default model is only `ornith-1.5:9b`; both transports and the editing and
production cases run by default. The `control` transport checks fixture construction
and document readback without a model. Each run writes a new directory under
`target/agent-tools`.

The separate `critique` case asks for a neutral balance assessment, one fader
adjustment selected from that assessment, readback, and another listen. Its prompt
does not reveal an expected track or gain. It permits abstention when the critique
does not justify an edit. Automated checks establish one persisted fader change,
unchanged remaining track state, and two distinct recordings; manual review must
still establish that the critique was accurate and actually supported the edit.

A separate Ornith rig session authored a complete chiptune project, Pixel
Lantern, with chip lead/chords, FM bass and drums. Its valid `compose` arguments
explicitly assigned 4+16+8+16+8=52 section bars despite a 16-bar test prompt; no
length field was silently ignored. The run then produced 18361 tokens of repeated
duration planning without exporting audio. That answer event was not accepted as
completion. The 30–40 second target was a demo constraint, not a user requirement,
so the longer authored piece was retained for the user and subsequent mix review.

A deterministic MCP call exported `first-draft-long.wav` under
`target/local-model-song/20260907-071049-pixel-lantern/creation-retry/PixelLantern`.
The project hash stayed unchanged. Independent WAV measurements found 115.178 s,
48 kHz stereo, peak -0.3 dBFS, RMS -18.02 dBFS and zero full-scale samples. The
model authored the musical specification; the evaluation driver performed this
export. The wire transcript and render verification preserve that distinction.

The first actual MusicFlamingo-assisted Ornith rig loop is retained in the song's
`mix-revision` directory (89.8 s, 5 calls, no tool errors). Both `listen` calls
submitted the real excerpt, but the reviewer claimed that no audio was available.
The controller abstained and exported the unchanged song. HTTP delivery and a
complete controller answer did not establish an auditory revision.

After simplifying the listening instructions and adding an explicit mono-input
notice, `mix-revision-retry` recorded another actual Ornith rig run (218.6 s,
8 calls, 2 errors). Its executable SHA256 is
`C6F023908CE884150CE52985B8DE5AAB1E926E3E27909FCF91A4067097FC3AD7`.
The first review described a synth lead, steady drums and subtle bass, suggesting
slightly more bass low-end presence. The controller recognized that this was a
timbral suggestion, not a necessary track-fader change under the trial's narrow
fader-only constraint, and kept the mix unchanged. It also explicitly rejected
the reviewer's stereo-placement claim because the reviewer received mono audio.

Both attempts at the second, two-excerpt `listen` returned an incomplete-review
error. The controller reported that no usable second observation existed. At this
stage the bridge split a 512-token combined limit between two independent
reviews; the incomplete responses require separate diagnosis. The first call's
130.9 s included a cold model load, so it is not a steady-state inference timing.

All three full exports (`first-draft-long.wav`, `revised-mix.wav`, and
`revised-mix-listened.wav`) are byte-identical, with SHA256
`AC2E262D47274D3E6EF7E9EED30D34095569530A967B44AE512DC93310F94775`.
The saved project also remains byte-identical to its baseline. These runs show
honest abstention and, after the prompt fix, a usable first musical description;
they do not yet establish a completed critique-driven edit-and-relisten loop.
Raw failures, final answers, saved-state comparisons and hashes are retained.

The subsequent `mix-revision-timbre` run allowed one supported local instrument
or effect adjustment, rather than imposing the evaluator's fader-only rule.
Ornith rig completed 8 calls without tool errors in 248.7 s, including a cold
critic load. The executable SHA256 was
`1E15EF646E444F43AB016E5C7DA62EBD190C602A649F85304FE6680EF9F522C4`.
After the same first critique, it read the bass instrument's actual metadata and
set its `level` from the default -9 dB to -6 dB. This raises the bass instrument's
output by 3 dB; it is not a frequency-selective EQ change. The saved document
differs at exactly `tracks[2].kind.instrument_state.params.level`. Every note,
clip, section, fader, effect, other track and master setting is unchanged. An
independent MCP readback confirmed -6 dB with no automation lane.

The revised `revised-mix-timbre.wav` is 115.178 s, 48 kHz stereo PCM24, peak
-0.3 dBFS and RMS -17.23 dBFS, with no full-scale samples. Its SHA256 is
`570936093D4B58F6D204CFCA177A682437203F79DA1780E5B73A757869ACE266`;
the original and both earlier no-op exports retain their original hashes.

User-facing copies are retained outside Cargo's disposable build directory in
`exports/PixelLantern-20260907`: `first-draft.wav`,
`bass-level-candidate.wav`, `PixelLantern.auris`, and `NOTES.md`. The audio and
project copies preserve the source hashes; the original evaluation evidence
remains under `target`.

The second `listen` now completed with 559 output tokens after the bridge gave
each independent review its own 512-token ceiling. This resolves the observed
truncation, but the review of the changed excerpt incorrectly described solo
acoustic piano. Ornith acknowledged that mistake and the mono/independent-review
limits, yet also wrote that the critic had confirmed improved bass presence.
That confirmation is unsupported by the raw review and is a controller reporting
failure. The actual critique-to-edit-to-relisten pipeline and persisted local
change are demonstrated; musical improvement and consistently reliable criticism
are not. The before/after recordings remain available for human assessment.

### Ollama audio API implementation

Ollama 0.33.3 implements audio input. Its OpenAI-compatible
`/v1/chat/completions` converter decodes `input_audio.data` as standard Base64 and
passes the original bytes through the native message's `Images` field; the
`format` property is not used by that converter. Native `/api/chat` represents
audio in JSON `images`, not a separate `audio` or `audios` field.
See the versioned [OpenAI converter](https://github.com/ollama/ollama/blob/v0.33.3/openai/openai.go#L571-L584)
and [native message type](https://github.com/ollama/ollama/blob/v0.33.3/api/types.go#L177-L190).

The [media detector](https://github.com/ollama/ollama/blob/v0.33.3/llm/media.go)
recognizes RIFF/WAVE bytes, and the
[runner adapter](https://github.com/ollama/ollama/blob/v0.33.3/llm/llama_server.go#L1493-L1510)
forwards media to llama-server. Its pinned llama.cpp version is
[b10760](https://github.com/ollama/ollama/blob/v0.33.3/LLAMA_CPP_VERSION); that
version's [audio decoder](https://github.com/ggml-org/llama.cpp/blob/b10760/tools/mtmd/mtmd-helper.cpp#L325-L365)
converts input to mono floating-point samples at the encoder's required rate.
The local server log also recorded Gemma4 audio-encoder initialization and
50-token audio batches. A narrow excerpt is retained at
`target/ollama-source-audit/encoder-log-excerpt.txt`.

Ollama's [integration tests](https://github.com/ollama/ollama/blob/v0.33.3/integration/audio_test.go)
cover native `images`, OpenAI `input_audio`, and transcription using a 16 kHz mono
spoken WAV fixture. These establish an implemented route and a useful speech
control; they do not establish reliable musical listening. The Auris request
matches the implemented Base64/WAV format. The cause of the local listening
failures below remains unproven.

### Actual audio-input controls

The installed Ollama version was 0.33.3, and `/api/show` advertised audio capability
for both `gemma4:e4b` and `gemma4:e2b`. That metadata did not establish useful
listening behavior. Exploratory E4B requests are saved under
`target/audio-interface-probe`; responses to actual tone/music input included
claims that no audio was available or that the model could not listen to files.

A first reproducible E2B control run is recorded in
`target/audio-input/20260907-055226-f5a4b2/summary.json`. It submitted four cases
with the same blinded instruction: a tone followed by silence, pure silence, a
real musical preview, and a no-audio control. The requests did not reveal the
filename or expected answer. Reasoning was disabled through the OpenAI-compatible
`input_audio` route. These requests put the audio part before the text part.

| Input | Reported input tokens | Observed response |
| --- | --- | --- |
| Tone then silence | 114 | Claimed no audio was supplied |
| Silence | 114 | Asked for an audio file |
| Music | 114 | Claimed no access to an audio file |
| No audio | 57 | Asked for an audio input |

The extra input tokens alone do not prove hearing. Later source inspection and
the local encoder log established that the audio bytes reached the audio encoder;
the route was not merely silently dropping the media. A known-speech positive
control then separated speech support from useful musical interpretation.

The official Ollama integration fixture says "Why is the sky blue?". Its original
32-bit PCM WAV and a conventional 16-bit PCM conversion were tested under an
exact-transcription instruction that did not reveal the expected words. E4B
repeated "lo" on the original native and audio-only OpenAI requests and on the
16-bit native request. Increasing the 16-bit fixture's level did not recover
the words. E2B correctly transcribed the same 16-bit file through native `images`.
Requests and responses, including `speech-pcm16-e2b-response.json`, are retained
under `target/audio-interface-probe`.

E2B also transcribed that fixture exactly through both tested OpenAI framings:
text before audio, and instructions in a system message with an audio-only user
message. The same blinded non-speech prompt was then applied to tone, silence,
music and no-audio controls in each framing. The speech control used its separate
exact-transcription instruction.

| Input | Text before audio | System instruction, audio-only user |
| --- | --- | --- |
| Known speech | Correct: "Why is the sky blue?" | Correct: "Why is the sky blue?" |
| Tone then silence | Could not determine the sounds | "no speech" |
| Silence | Claimed audio was not provided | "no speech" |
| Real chiptune music | Claimed audio was not provided | "no speech" |
| No audio | Requested an audio input | Claimed there were no audible sounds |

These runs are recorded in
`target/audio-input/20260907-062228-c298ac` (text first) and
`target/audio-input/20260907-062251-cbde9b` (system instruction). Speech requests
reported 105 input tokens; the tone/silence/music requests reported 114. No-audio
requests reported 57 and 62 respectively. The positive speech control establishes
working audio transport and speech recognition for E2B in this installation.
Neither framing produced a useful musical judgment or reliably distinguished the
non-speech controls. An absence-of-speech answer is not an assessment of musical
balance. The current experimental Gemma path therefore uses E2B and text-first
input, but autonomous musical correction remains unverified. This is not a claim
that all audio models or all Ollama audio input are unusable.

To repeat the blinded controls, supply an actual musical WAV returned by `preview`:

```powershell
$music = 'C:\absolute\path\preview.wav'
pwsh -File tools/eval/audio_input.ps1 -Model gemma4:e2b -AudioFile $music
pwsh -File tools/eval/audio_input.ps1 -Model gemma4:e4b -AudioFile $music

# Add a known spoken WAV as a separate positive control and compare framing.
$speech = 'C:\absolute\path\known-speech.wav'
pwsh -File tools/eval/audio_input.ps1 -AudioFile $music -SpeechFile $speech -PromptPlacement text-first
pwsh -File tools/eval/audio_input.ps1 -AudioFile $music -SpeechFile $speech -PromptPlacement system
```

The harness defaults to E2B and text-first input. It now also generates a
deterministic noise control. Each run records request bodies, response bodies,
endpoint/model metadata and source-file hashes under `target/audio-input`.

`-Provider openai -ApiUrl http://127.0.0.1:11435` selects an independently hosted
OpenAI-compatible critic without attempting Ollama metadata calls. Supplying
`-ContrastFile` alongside `-AudioFile` adds counterbalanced A/B and B/A comparisons
and an identical-audio A/A control. All three use the same neutral instruction;
the request reveals neither file names nor the edit or its expected direction.
`-PrepareOnly` saves the exact requests and generated controls without making any
network request.

Prepared requests in `target/audio-input/20260907-063634-f6d979` use the real
before/after -6 dB pair from the Japanese Qwen rig trial. Decoding and hashing the
request bodies confirmed that B/A reverses the exact recording order and A/A
duplicates the exact first recording. This checks the evaluation fixture, not a
reviewer's performance. A useful balance critic must identify the direction of an
audible change consistently when order reverses and avoid inventing differences
for A/A. Speech transcription, HTTP success, a fluent review and numerical
measurements cannot substitute for those perceptual checks.

The additional local `Qwen/Qwen2-Audio-7B-Instruct` backend was then tested at
revision `0a095220c30b7b31434169c3086508ef3ea5bf0a`, with NF4 language-model
quantization and a BF16 audio path on the AMD GPU. Its reported runtime was Torch
`2.13.0+rocm10.0.0`, HIP `7.15.26333`, and Transformers `5.15.1`.
`backend-health.json` in each run records this configuration. These observations
apply to this installation and precision, not every deployment of the model.

Complete text-first and audio-first suites are recorded in
`target/audio-input/20260907-065701-2d110e` and
`target/audio-input/20260907-065712-53bb87`. The bridge correctly rejected the
no-audio request with HTTP 400; that is server validation, not a model response.
The other cases reached inference.

| Control | Text-first result | Audio-first result |
| --- | --- | --- |
| Known speech | Answered why the sky is blue instead of transcribing | Returned "The sky is blue", not the recorded question |
| Tone / noise | Recognized a dial-like tone; called noise a vehicle engine | Recognized a dial-like tone and static noise |
| Silence | Invented a background sound effect | Invented background noise |
| Chiptune music | Described electronic keyboard/video-game-like instrumental music | Called it an unspecified sound effect |
| -6 dB A/B and B/A | Claimed identical balance in both orders, with invented G major and 146 BPM | Did not reverse the reported change; invented instruments absent from the fixture |
| A/A | Reported no difference, but retained invented musical details | Reported no balance difference, with invented instrumentation |

The fixture is C major at 120 BPM and contains synthesized lead and bass with a
reverb bus. Recognizing it as electronic music is useful positive evidence, but
these comparisons did not validate the direction of the fader change.

A stronger control used music versus generated noise under a neutral instruction
to describe A and B separately. The same exact WAVs were reversed for B/A and the
music WAV duplicated for A/A. `-PairsOnly` and `-ComparisonPrompt` reproduce this
bounded comparison without repeating the single-audio cases. Results are in
`target/audio-input/20260907-065913-397f21` (text first) and
`target/audio-input/20260907-065916-96bbd2` (audio first).

Both framings correctly called A music and B noise in the original order. Neither
handled the reversal correctly: text first called both recordings music, while
audio first retained A=music/B=noise despite the reversed bytes. Both invented a
silent or noisy second recording for identical A/A music. Thus coarse multi-audio
order binding also failed these controls. A lead-present/lead-muted trial was not
advanced after this failure.

A CPU-only check of the actual cached processor, recorded in
`target/audio-runtime/processor-check.log`, confirmed the complete binding for
these requests: two 2-second arrays produced two feature batches and two
50-token audio spans. Reversing the WAVs reversed the feature hashes exactly;
duplicating the WAV produced identical feature hashes. The bridge and processor
therefore preserved the tested order and duplication, despite the model's wrong
comparisons.

A partial unquantized BF16 baseline with CPU offload is retained in
`target/audio-runtime/bf16-controls/summary.json`. It also answered the speech
question instead of transcribing (151.1 s), classified the short music as an alarm
and siren (19.1 s), and invented a sine wave on silence (11.6 s). Music/noise A/B
was correct (6.8 s), but reversed B/A was incorrectly classified as two music
recordings (18.1 s). The A/A case was unfinished when the slow offloaded run was
stopped; `status.json` records that limit, and no A/A verdict is assigned. These
failures persisted without NF4, so they cannot be attributed solely to that
quantization. The short 2-second synthetic fixture still limits conclusions about
longer musical context; neither precision has established reliable criticism or
comparison on the tested material.

For a longer-context control, the complete Pixel Lantern project supplies bars
5–9, a 10.714-second excerpt with the lead active. A separate copy of the complete
project has only `lead.mixer.mute` changed; every note and other field is identical.
MCP rendered exactly the same range from both projects. The original document
remained unchanged, and the WAV hashes differ. Paths, hashes and preservation
checks are in the song's `listening-controls/controls.json`. These are prepared
controls, evaluated with MusicFlamingo below.

`nvidia/music-flamingo-2601-hf`, revision
`6b5be086d52f65a1e204cb0faf70bf54e2741ecd`, was tested in NF4/BF16 using the same
Torch/Transformers runtime. The bridge deliberately processes each excerpt in a
separate native model call and labels the resulting observations; it does not
claim that MusicFlamingo made a joint A/B comparison. The run is recorded in
`target/audio-input/20260907-072853-b5f4fd`, including runtime and GPU-memory
metadata. Resident GPU allocation after inference was approximately 7.24 GB.

The independent reviews were reproducible and input-dependent. Reversing the
original/muted inputs reversed the two review texts exactly; A/A produced
identical observations. The original was described as a prominent high-pitched
synth lead over drums and bass. With that track muted, the review described a
repetitive synth melody, consistent with the remaining chip chord part. This is
useful positive evidence of musical description under the longer context, rather
than the earlier order-insensitive joint answers.

Accuracy remains limited. Both versions were described as clean and balanced,
so these controls did not establish a necessary mix correction. The broader
single-music prompt incorrectly omitted percussion. The tone was described
plausibly and silence correctly produced no discernible sounds, but the generated
noise also incorrectly produced that answer. Any local revision should therefore
follow a concrete, supported musical critique, retain the first recording, and
report uncertainty rather than infer improvement from delivery or fluent wording.

The longer song exposed a framing failure in the actual `listen` path even though
the same WAV had produced musical descriptions in standalone controls. A bounded
four-condition comparison in `target/audio-input/20260907-074607-mf-framing`
submitted identical bytes (SHA256
`681689127084DE6AB89432855CFADD9953E3DF03C830F6F5B007EBD1F41CA380`),
temperature 0 and `max_tokens=512`, preserving the text-first ordinal header.
All full requests, responses and backend-health metadata are retained.

| System instruction | User review prompt | Result |
| --- | --- | --- |
| Original | Original long focus and wrapper | Reproduced "No audio is available" |
| Concise | Original long focus and wrapper | Claimed no audio was provided |
| Original | Concise instrumentation/balance question | Described synth lead, drums and bass; suggested more bass low-end presence |
| Concise | Concise instrumentation/balance question | Described synth lead, drums and bass; suggested more bass low-end presence |

The concise system was: "Review the supplied recordings. Describe audible musical
details, state uncertainty, and suggest a concrete local change only when
supported. You review audio; you do not edit the project." The concise user prompt
was: "Describe the instrumentation and balance in this recording. Suggest one
small mix adjustment only if the sound supports it, and explain why."

Changing only the system did not resolve the refusal; replacing the whole user
review prompt did in these cases. Since that replacement also simplified the
original focus, this does not identify a single offending sentence or guarantee
success for every future focus. The tested concise system and default focus were
adopted, preserving explicitly supplied questions without the long wrapper. The
subsequent actual rig trial reproduced the successful first description, while
the two-excerpt review remained incomplete as recorded above.

The two positive framing responses contradicted one another about stereo
placement. More fundamentally, the bridge supplies mono audio resampled to
16 kHz, so neither response can establish stereo width or panning. The bridge now
adds that input-format limitation to returned observations without changing the
model's inference prompt or rewriting its original claims. The controller's
rejection of unsupported stereo claims is positive evidence of handling this
limitation; it does not make the reviewer's other claims automatically accurate.

After the per-excerpt output-budget fix, the actual changed-song comparison
completed, but its current-excerpt observation hallucinated a solo piano
recording. Its 559-token combined response exceeded the former 512-token combined
budget, explaining why that earlier ceiling could truncate a comparable review.
Successful completion therefore solved a transport-level usability issue without
establishing perceptual correctness. The full observations and backend token/
finish-reason log are retained in the song's `mix-revision-timbre` directory.

Both actual `listen` calls used the same concise focus: the second call omitted
`focus` and selected that exact default. The independent-review bridge still adds
its own boundary instructions when splitting a pair. A final planned diagnostic
therefore submitted the changed excerpt as a single request using the exact
successful case-4 system, user text and `1. Current` label; an identical-prompt
baseline request was to follow. This attempt is retained in
`target/audio-input/20260907-082744-mf-final-singles`.

The cold backend crashed before returning the first review, at weight loading
0/830, with native exception `0xC0000005`; the first reported frame was
`torch_cpu.dll`'s `_local_scalar_dense_cpu`. A preceding ROCm architecture-detection
warning is also recorded, but its causal role is unproven. The HTTP connection was
reset and subsequent health requests were refused. The baseline request was not
sent and no retry was made. This diagnostic supplies no perceptual result and
does not distinguish pair framing from model perception. Earlier completed
reviews and tool trials remain valid observations of their respective runs;
this additional cold-start failure is retained as a separate runtime limitation.
