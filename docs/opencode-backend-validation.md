# OpenCode backend validation

The 2026-10-24 trials use the installed OpenCode 2.1.3 CLI, actual model inference,
the real GPUI window harness, live authenticated MCP and the normal session and
renderer. All songs and external files are generated scratch fixtures. The
window harness needs no display or audio device. No composer writer, instrument
constant or DSP algorithm changed.

## Repairs found through inference

- Actual V2 inference requested the native `shell` tool, which the V1-only
  transport rejected. Detect the CLI major version before launch and generate
  the current schema instead of relying on legacy configuration fields: V2
  `agents`/`system`/`permissions`, V1 `agent`/`prompt`/`permission`/`tools`.
  Recognize the V2 `shell` name.
- Keep V2 MCP tools directly available with `codemode: false`. Pass its token
  through a child environment variable referenced from the private configuration.
  Align the process and ACP project directories, wait for an authenticated tool
  catalogue request before inference, and explain the `auris_live_` name prefix.
  This prevents a mere successful `session/new` from being mistaken for a usable
  tool connection.
- ACP tool notifications can start with only `cwd`, and their display title can
  change. Prefer the stable `name`, retain argument updates, and emit the call
  when execution starts or finishes rather than recording the initial partial
  input as the complete command.

The shared MCP tests check catalogue readiness and cancellation. Scripted ACP
regressions cover both schema families, stable names, argument accumulation,
model selection, native allow/deny decisions, reviewer isolation and memory.
Windows version probes use the same hidden-console behavior as the ACP child.
V1 and Windows were not run against an installed CLI during these macOS trials.

## Actual trials

Canonical edits, arrangement, breathing and worker lifecycle use
`opencode/deepseek-v4.1-flash-free`. The strict composition trial uses
`opencode/glm-5.3-flash-free`, and image references use
`opencode/nemotron-3.5-super-vl-free`. The audio critic is the already installed
local Ollama `gemma4:e2b` at `127.0.0.1:11434`.

| Trial | Outcome | Evidence |
| --- | --- | --- |
| Canonical unsaved edits and exports | Passed, 52.26 s | Exact note/mixer/CC changes; untouched notes and CC7 preserved; one Undo and Redo; MIDI round trip retained 3 notes; 48 kHz stereo 24-bit WAV, 96,000 frames; no full-scale samples |
| Arrangement, performance, automation and SFX | Passed, 100.29 s | Revoice/strum, scoped automation, imported SFX and riser; source notes, maps and unrelated pad preserved; independent readback, paired audio and one Undo/Redo |
| Melody breathing | Passed, 82.40 s | Exact phrase-ending repair, protected references preserved; before/after stem and mix inspection; independent readback and one Undo/Redo |
| Native permissions, text, history and Stop | Passed, 39.43 s | Denied file absent, approved file exact; text tag recalled after worker replacement; unsupported compaction reported explicitly; pending command cancelled and worker ended within 5 s |
| Image and audio reference delivery | Passed, 11.57 s | Actual vision model identified a red fixture; reported 2.0 seconds and `audio_sent: false` from local audio metadata |
| GLM composition and A/B listening | Passed, 243.77 s | 8 bars, 3 tracks, 159 notes; Intro/Chorus; two actual audio submissions, a bass edit, a separate mixer read and matching-range comparison; verified MIDI and 24-bit WAV |
| DeepSeek composition and A/B listening | Strict harness failed, 132.77 s | Same 159-note song and successful audio submissions/exports, but the model skipped the separate mixer read before comparison |

Arrangement and breathing included respectively two and one rejected argument
attempts. The model corrected them and passed the state-preservation assertions.
They are recorded as recovered errors, not hidden successes.

The GLM song uses Chiptune, Physical Bass and the polyphonic Physical Drums kit.
Drum roles were assigned before generation. Each clip uses `generate_phrase`
with an explicit seed (101, 102, 103); the model orchestrates the existing
composer through the live tools. The resulting clips contain 44, 20 and 95 notes.
The exported MIDI independently parses to 159 Note On events. The 16-second,
48 kHz stereo WAV has 768,000 frames, 24-bit PCM in a WAVE_FORMAT_EXTENSIBLE
container, -17.749 LUFS and zero full-scale samples. `verify_render` returned PASS.
Its SHA-256 is
`7a4a61bd0b46e8132f1b2bf0b01891cc2bfe757cb6df4bc8a9654f625745deac`.

The first audition is bars 1–4 at revision 46. The model lowered the bass from
-12 to -15 dB, independently read the mixer, then compared the same bars at
revision 47 using the retained audition ID. Both encoded excerpts used 0 dB
protective attenuation. Their peaks were -11.695 and -11.786 dBFS, and RMS values
were -20.984 and -20.991 dBFS. The current WAV hash differs from the retained
baseline. The host saved the final project after the agent completed.

The DeepSeek trial proves actual submission, editing and export, but it is not
counted as a strict listening-loop pass. The GLM request explicitly numbered
set-strip, separate mixer-read and comparison as sequential tool calls; a graph
inside a write acknowledgment does not count as an independent read.
The critic is fallible and its focus disclosed the intended bass change. These
trials establish tool execution and measured audio differences, not reliable
auditory discrimination, musical improvement or human speaker listening.

Final checks passed: 128 default `auris-agent` tests and 128 selected headless
GPUI agent/settings tests, with nine opt-in tests ignored in each default run;
actual inference tests were invoked separately. `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets` and `git diff --check` passed.

The machine-readable results are recorded in
[`opencode-backend-2026-10-24.json`](../tools/eval/references/opencode-backend-2026-10-24.json).
Raw transcripts, generated projects, MIDI and WAV files remain in the ignored
`.local-evaluations/opencode-2026-10-24/` directory.

## Automatic-review limitation

A separate session denies every tool for approval review. In actual trials,
OpenCode's free provider rejected that tool-free request with `auth_required`;
its diagnostic identifies a free-tier client restriction. Both DeepSeek and GLM
free endpoints behaved this way. The GUI returned to manual confirmation and
left the pending operation unexecuted. The test stopped rather than approving
itself. Automatic review remains unavailable for those free endpoints in this
environment. Use a provider/model that accepts tool-free review requests; any
provider credentials remain managed by OpenCode.

The completed composition/listening trial explicitly allows `edit_project.*`
for its disposable new document and requested fresh outputs. Native external
operations still use AutoReview. This fixture rule is recorded in the report;
production defaults are unchanged. The successful composition does not establish
a successful free-provider automatic review. No tool was enabled in the reviewer
to work around the provider's rule.

## Reproduction

The opt-in GPUI tests take `AURIS_TOOL_SURFACE_EVAL` pointing at a JSON file:

```json
{
  "backend": "opencode",
  "model": "opencode/deepseek-v4.1-flash-free",
  "output": "/absolute/path/to/a/new/evidence/directory",
  "timeout_seconds": 420
}
```

Run the exact tests in
`ui::agent_chat::tool_surface_eval` for canonical edits/exports, arrangement and
melody breathing. Arrangement and breathing also need
`AURIS_ARRANGEMENT_HARNESS=1`.

For full composition and A/B listening, configure `AURIS_CODEX_TRANSCRIPTION_EVAL`
with absolute `workspace`, fresh `output` and UTF-8 `prompt` paths, `backend:
"opencode"`, an available `model`, `effort: "default"`, `timeout_seconds: 600` and
`listening_required: true`. `allow_operations` defaults to empty; list explicit
fixture grants only when needed. Point `AURIS_AUDIO_URL` and `AURIS_AUDIO_MODEL`
at a real audio-capable critic, then run:

```sh
cargo test -p auris-gpui --bins -- --ignored --exact ui::agent_chat::transcription_eval::installed_cli_composes_and_relistens_in_the_window --nocapture
```

The independent worker test
`backends::opencode::evaluation::installed_opencode_checks_native_permissions_references_memory_and_stop`
uses `AURIS_OPENCODE_EVAL_MODEL` and a fresh absolute
`AURIS_OPENCODE_EVAL_OUTPUT`. It checks native denial/approval, text attachment
content, a replacement worker's conversation memory, explicit unsupported manual
compaction, and Stop during a pending permission without creating the file.

The separate ignored `installed_opencode_reads_image_and_audio_references` test
also takes `AURIS_OPENCODE_EVAL_AUDIO`, pointing at the 48 kHz, two-second WAV
from the canonical export fixture. Select a vision-capable controller. It checks
the generated image's colour, the audio duration and `audio_sent: false`: this
attachment route supplies local measurements, while `listen` sends actual WAV
bytes to a separate critic.
