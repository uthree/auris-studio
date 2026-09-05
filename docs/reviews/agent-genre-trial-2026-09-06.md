# Agent composition trials: vocals, EDM, and orchestra

Tested on Windows on 2026-09-06, against `fb0aafd` on
`codex/agent-composition-workflow`. This is a workflow review, not an aesthetic score.
The observations below distinguish model behavior, unavailable local assets, and missing
commands. No composer, DSP, or application implementation was changed for these trials.

## Method

The initial prompts asked the actual `auris-agent --json` executable, using the same tools and
JSON conversation protocol as Agent Panel, to compose three original pieces:

| Brief | Musical requirements |
| --- | --- |
| Vocal pop | A farewell at an evening station; 92 BPM, A minor; 4-bar intro, 8-bar verse, 8-bar chorus, 4-bar outro; Japanese lyrics, a singable melody, piano, bass, and drums; develop the verse motif in the chorus. |
| EDM | A highway at night; 128 BPM, F minor; intro, buildup, drop, break, outro; repeated synth motif, four-on-the-floor kick, opening filter, and kick-triggered sidechain compression. |
| Orchestra | Reaching a summit at dawn; 100 BPM, D minor with a brighter ending; strings, woodwinds, and brass passing a theme, low-string ostinato, timpani, changing articulation, and a crescendo. |

Each prompt required a saved project, inspection, a short preview, a full WAV, and an honest
account of substitutions and unmet requests. The runs used separate folders and no existing
user songs. Agent writes remained confined to their individual working directories.

The saved model setting was `gpt-oss:20b`, but the local server was stopped and that model was
not installed. The installed, tool-capable `ornith-1.5:9b` was used through an explicit flag;
saved preferences were not changed. Runs used a maximum of 24 model turns per brief, and the
application's existing five-minute deadline. This model choice limits conclusions about other models.

A separate control used actual MCP stdio JSON-RPC calls to create the same three briefs from
validated specifications, inspect notes and recipes, render audio, and retrieve preview blobs
through `resources/read`. These controls were authored by the reviewing agent, rather than
claimed as successful autonomous `auris-agent` runs. Quantitative audio measurements and note
inspection were used; no subjective listening score was assigned.

## Autonomous run outcomes

The default-context runs all failed before a tool call. With 32K context and the model's default
thinking behavior, all three runs reached the application's 300-second deadline without saving
a project. The diagnostic repeat kept the same briefs and model, disabled thinking through a
temporary local request proxy, and made the verified SoundFont available.

| Diagnostic repeat | Elapsed | Tool calls | Saved state and stopping condition |
| --- | ---: | ---: | --- |
| Vocal pop | 295.993 s | 26 | Saved a band and an empty singer clip; still 41 bars including the ending, with no sung notes or lyrics. Stopped at the trial's 24-model-turn limit after repeated empty `edit_notes` calls. |
| EDM | 130.312 s | 39 | Saved the requested section layout and generated previews. Filter/sidechain work remained incomplete; two tracks named as buses were actually empty instrument tracks because `kind` was omitted. Stopped at the trial's 24-model-turn limit. |
| Orchestra | 49.232 s | 17 | Saved the orchestral preset at 76 BPM in 3/4; the requested 100 BPM, 24-bar arrangement remained unfinished. Ollama returned HTTP 500: invalid tool-call arguments for `check_spec`, unexpected end of JSON input. |

One model turn can contain multiple tool calls. A trial ending with an error event is not a
successful composition, even though `auris-agent --json` remains alive and exits normally when
stdin closes. The 24-turn cap was a test setting, not the application's default. The provider's
malformed JSON is a model/provider failure, not evidence that the specification parser failed.
These outputs were independently reopened and measured through MCP under `agent-audit/`.

A no-op request also occurred with `set_level`. Other corrected errors included
numeric/array values in string-valued overrides, an unsupported `singer` part role, and a preview
start bar without its required range length. Validation correctly refused these requests. The
remaining improvement is making valid operation shapes easier to generate and preventing
repeated failures from consuming the entire conversation.

## Findings in implementation order

### P1: Verify effective model context before starting the tool loop

All three initial requests failed before the first tool call. The provider reported requests of
11,811, 11,791, and 11,805 tokens against an effective context of 4,096 tokens. Meanwhile,
`auris-agent models` advertised `context_length: 262144` for the same model. That is the model's
architectural capacity, not the server's active allocation.

Restarting only the trial's Ollama process with `OLLAMA_CONTEXT_LENGTH=32768` allowed the tool
loop to start. The application has no corresponding context-size control in `AgentPreferences`
or its Ollama request builder. It arms every tool before the first request.

Add an effective-context preflight and an explicit request setting; distinguish advertised
capacity from configured capacity in the panel. Reduce the initial tool vocabulary, or load
task-relevant groups, so discovery itself does not require a large context window.

Evidence: [agent builder and model discovery](../../crates/auris-agent/src/main.rs),
[shared settings](../../crates/auris-session/src/settings.rs), and the panel's model picker in
[agent_chat.rs](../../crates/auris-gpui/src/ui/agent_chat.rs).

### P1: A registered sampler must not be mistaken for a playable fallback

The no-font orchestral control composed 223 notes successfully. Its specification explicitly
selected the catalogued `auris.sampler.soundfont` instrument and GM programs. `compose`
reported that General MIDI was unavailable and that a stand-in would play. In fact, all five
instrument tracks were silent: both the full WAV and the preview measured -120 dBFS.

The no-font vocal control exposed the same issue: its accompaniment was silent while the
temporary `auris.synth.vocal` voice remained audible. An overall non-silent mix therefore does
not prove the requested instruments are working.

`Session::compose` treats a registered instrument ID as an available fallback even
when that ID is the sampler without a preset/font. Add a playable-asset check, select a real
fallback when one is promised, and return explicit per-track readiness/substitution status.
The catalog should distinguish an installed implementation from a usable sound library.

Control: the repository's declared MuseScore General font was downloaded into the isolated
trial assets directory and verified against its SHA-256 manifest. Repeating with
`AURIS_SOUNDFONTS` pointed there produced audible accompaniment and orchestra. The missing
installation is an environment condition; the misleading fallback and silent success are the
application defects.

Evidence: [composition application](../../crates/auris-session/src/session/compose.rs),
[library manifest and discovery](../../crates/auris-session/src/library.rs).

### P1: Preserve motif intent in clip recipes and local regeneration

The EDM specification supplied `motif = "0 0 4 3 0 2"`. Its drop lead contained 41 notes with
seed 97568. Calling `write_again` on that clip, with the same build, seed, harmony, and tempo,
produced 71 notes and a different opening contour. A checkpoint restored the original after
the probe.

This is not a cross-version reproducibility request: the local rewrite path constructs
`ScoreSettings` with an empty motif. The whole-song motif is retained in original specification
text but is absent from `ClipRecipe` and cannot be changed through `edit_recipe`.

Carry the relevant musical intent into recipes, including motif and authored rhythm where
applicable. Allow a motif to be read, reused, and intentionally varied in one clip. A global
motif already exists; the missing part is its survival through the editing workflow.

Evidence: [recipe fields](../../crates/auris-core/src/project/recipe.rs),
[phrase regeneration](../../crates/auris-compose/src/phrase.rs),
[score motif contract](../../crates/auris-compose/src/spec/mod.rs).

### P1: Expose effect insertion, routing, and parameter automation

The EDM control could create the beat, bass, synth motif, and section contrast. It could not
implement the requested production steps through the 38 advertised MCP tools:

- `set_effect` on the bass compressor returned `'bass' has no effects`.
- `set_effect` on the lead filter returned `'lead' has no effects`.
- There is no tool to insert either effect, connect the kick to a sidechain input, or write
  a cutoff envelope. `set_send` changes an existing send level; it is not a sidechain router.
- `section_gain` can offset or hold section volume, but does not represent a filter sweep or
  arbitrary parameter curve.

The session already has effect insertion, sidechain, and automation commands. Expose them in
the shared toolbox with parameter descriptors, bounded range edits, and readback. This would
also supply orchestral crescendos and changes in instrumental expression.

Evidence: [session mixer commands](../../crates/auris-session/src/session/mixer.rs),
[toolbox mixer tools](../../crates/auris-toolbox/src/lib.rs).

### P1: Make the singing pipeline discoverable and report its render state

Lyrics and sung-note placement worked in the MCP control: the Vocal track contained 61 lyric
notes across verse and chorus. `sing` returned `track 34 names no voice model; choose one first`
in the font-enabled run. The available tools cannot enumerate installed voices, supported
backends, and speakers before choosing one. Documentation search describes a GUI voice shelf,
but does not answer which voice is available on this machine.

The autonomous vocal run made three documentation searches for voice discovery and never
reached composition before its timeout. This is observed behavior for this model, not proof
that every model will make the same choices.

Add a voice catalog and readiness query shared with the GUI library. Inspect/export results
should distinguish a temporary synth voice, an unsung track, a current rendered take, and a
stale take. A successful WAV export with the guide voice should not be mistaken for a finished
vocal performance. A missing selected voice is established here; absence of every possible
voice asset on the user's machine was not assumed.

Evidence: [voice discovery](../../crates/auris-session/src/library.rs),
[singer commands](../../crates/auris-session/src/session/singer.rs),
[sing tool](../../crates/auris-toolbox/src/lib.rs).

### P2: Add an orchestration vocabulary beyond GM patch and note length

With the standard font available, strings, flute, horn, cello, and timpani all rendered.
Section membership, pitch register, pan, and gate can approximate the brief. However,
`articulation = "staccato"` was rejected by `check_spec`; the exposed tools have no articulation
mapping, keyswitch operation, or modulation/expression event editing. A shorter gate changes
note duration, not the selected playing technique of a library.

The root motif can coordinate lead contours, but a section-specific `motif` field was also
rejected. Clip duplication has no destination-track argument, so exact theme transfer requires
reading and re-entering individual notes. Add cross-track phrase reuse and transformations,
plus library-aware articulation/expression controls. Automatic range checks should describe
the chosen instrument's practical range rather than only the MIDI 0-127 bound.

### P2: Support completion and listening loops explicitly

The application imposes a five-minute deadline on an entire conversation turn, including all
model and tool calls. The vocal, EDM, and orchestral runs timed out after seven, eight, and ten
calls respectively, with no saved composition. Lengthy generation continued after documentation discovery; the
runtime does not expose a thinking budget for the selected Ollama model. A larger wall-clock
limit alone would not address repeated searches for unavailable capabilities.

Use separate idle and overall budgets, report accumulated work on timeout, and offer a
continuation from the saved state. A compact capability response would prevent the model from
searching general documentation to discover that a command is absent.

A diagnostic repeat used a temporary localhost proxy to set `think: false` and `num_ctx: 32768`
in Ollama requests, with the verified SoundFont available to the agent. This was an external
test harness setting, not a new application feature. In the vocal run the agent then saved a
band, inspected it, attempted to rewrite its overlong form, and added a singer track. It subsequently called
`edit_notes` repeatedly with identical arguments containing neither `add` nor `remove` despite
the tool explaining that one was needed. These requests also satisfy the advertised schema,
which makes both operation fields optional. Require an actual operation in the schema, provide
compact examples, and detect identical failing calls rather than exhausting a turn budget.

The new MCP preview resource worked in every successful control. Passing that WAV back through
the actual Agent Panel JSON protocol with Ollama returned the explicit unsupported-audio error.
Thus playable audio for the user and audio input for the model are separate capabilities.
Surface the latter before promising a listening loop, and attach previews automatically only
through a provider/model combination that can actually consume them.

## Control artifacts and reproduction

The validated control specifications are committed beside this report:
[vocal pop](agent-genre-trial-scores/vocal.asong),
[EDM](agent-genre-trial-scores/edm.asong), and
[orchestra](agent-genre-trial-scores/orchestra.asong). Pass a file's contents as `spec` to
`check_spec`, then `compose` with an absolute output path. Use `preview` with section `chorus`,
`drop`, or `finale`, respectively. These scores are control inputs, not exports of the local
model's partial drafts.

For the silent-fallback reproduction, set `AURIS_SOUNDFONTS` to an empty test directory before
starting MCP and compose the orchestral control. Inspect `analyze` with `per_track: true`.
For the motif probe, compose the EDM control, create a checkpoint, read lead clip 2 with
`notes`, call `write_again` on that clip without changing anything else, and read it again.
Restore the checkpoint afterward. Exact note counts are observations for the tested build,
not a requirement to preserve historical composer algorithms.

All output is under `target/genre-trial/` (intentionally excluded from Git):

- `agent-*/events.jsonl`: the original effective-4K context failures.
- `context-32k/agent-*/`: exact prompts, JSON events, and run timing with a larger context.
- `no-thinking/agent-*/`: diagnostic repeat outputs; these are partial autonomous drafts.
- `proxy-requests.jsonl`: request-setting metadata for the temporary local diagnostic proxy.
- `agent-audit/`: MCP verification and previews of the actual autonomous drafts.
- `discovery.json.results.json`: the live MCP tool schemas and documentation discovery.
- `controlled/`: specifications and initial MCP controls, including the silent orchestra.
- `with-fonts/`: vocal and orchestral projects with a verified sound library.
- `probes/`: local-regeneration before/after notes, effect failures, and specification errors.
- `audio-input-check.jsonl`: the real audio-input capability failure.

The MCP runner uses `initialize`, `notifications/initialized`, `tools/call`, and, for preview
retrieval, `resources/read`. The controlled specifications and full call records are retained
beside the projects. Each rewrite probe creates/restores a checkpoint. Downloaded assets are
kept outside the source tree and are not included in the review commit.

| Control | WAV length including tails | Integrated loudness | Sample peak | Qualification |
| --- | ---: | ---: | ---: | --- |
| Vocal pop with fonts | 66.824 s | -15.7 LUFS | -0.3 dBFS | Accompaniment and temporary vocal synth; no trained singing take. |
| EDM | 48.482 s | -20.9 LUFS | -0.3 dBFS | Beat and arrangement; no filter sweep or sidechain. |
| Orchestra with fonts | 61.607 s | -14.0 LUFS | -0.3 dBFS | GM instrumentation; articulation approximated through gate/part changes. |

The specifications request 24 bars; the default held-tonic ending adds a 25th bar. This is an
existing configurable ending policy, not an unexplained extra section. Preview ranges exclude
that ending and effect tails. Musical quality and external VST3/CLAP library compatibility
remain outside these functional measurements.

## Repository validation

`cargo fmt --all --check` and `cargo clippy --workspace --all-targets` passed. The workspace test
run passed 623 of 624 GPUI tests but hit a Windows socket `WouldBlock` in the existing VOICEVOX
mock test `the_track_picker_fetches_real_engine_names_and_persists_the_clicked_style`. That test
passed immediately when run alone; the remaining workspace crates and doctests then passed.
No test or application source was edited to obtain these results. Logs retain both the initial
failure and the retry. The temporary Ollama server and diagnostic proxy were stopped after the
trials, and the model was unloaded.
