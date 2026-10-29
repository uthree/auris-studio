# Shared composition prompt and draft auditions

The editor agent and conversational song workshop use the same resolved composition
prompt. At `cb86f9f8`, the tested standard profile was 6,973 UTF-8 bytes (973 whitespace-separated words),
compared with 29,612 bytes (4,254 words) at `ffe1a6ac`: approximately 76% smaller.
These are text sizes, not measured model token usage. Saved custom prompts remain
replacements; a blank prompt follows the current standard.

The prompt retains the production sequence, authored foreground, existing-material
protection, exact IDs/revisions/tick units, complete vocal form, mixer review and
current-render verification. Detailed operation and musical guidance is available
through tool discovery/help. Tool errors and musical polishing have separate budgets.

An initial Codex panel trial completed a 24-bar instrumental and exported a 48-second
WAV/MIDI. It made one incorrect `read_notes` call by confusing a track ID with a clip
ID, then corrected it. The final prompt explicitly says to retain returned clip IDs
and never infer addresses from creation order or counters.

A vocal trial exposed a UI crash: multiline written lyrics can become a singer clip's
name, but canvas labels passed them to GPUI's single-line shaping API. Single-line
display controls now flatten line separators for layout only. The document's original
lyrics and clip names remain intact. A GPUI regression covers this path using the
application's real view tree; picker layout checks cover Japanese text and CRLF/LF names.

## Draft auditions

Each workshop composer captures an immutable render job after playable notes exist,
at most once every ten seconds following edits. A separate bounded worker renders it
at the existing audition rate and shares the voice/render gate. Rendering does not
block the model's next edits. Cancellation joins this worker before final publication.

Draft audio has its own snapshot map and is not an adoptable `SongCandidate`. Playback
keeps the chosen buffer when newer audio arrives; **Play latest audio** starts the
newest snapshot from the beginning. The player uses the playing buffer's duration.
Rendering errors stay on the affected card and clear after a successful audition.
Only completed candidates can be selected or saved as finished songs.

The real Codex workshop trial completed an eight-bar score (16 seconds, 120 BPM),
with four draft auditions at approximately 278, 292, 320 and 329 seconds after launch;
completion took approximately 381 seconds. Its retained final audition is 17.73 seconds
including decay. This demonstrates intermediate publication before completion, not
an instantaneous first preview or a performance benchmark.

## Reproduction and evidence

Code validation passed: 4,763 workspace unit/doc tests with `cargo test --workspace
--exclude auris-gpu -- --test-threads=4`, plus 30 GPU tests run serially with
`cargo test -p auris-gpu -- --test-threads=1`. The final prompt was additionally checked
with all 188 normal agent tests. `cargo clippy --workspace --all-targets` and
`cargo fmt --all --check` passed. The mode-selection UI fixture uses a session without
automatic full-song balancing; balance behavior retains its dedicated tests.

The final instrumental panel trial completed both turns without tool errors and
exported a 48.000-second, stereo 48 kHz/24-bit WAV and a MIDI containing 603 notes.
The encoded WAV had zero full-scale samples and a measured true peak of -6.53 dBTP.
This is execution and numerical verification, not a listening verdict.

The 40-bar vocal task also completed both turns and exported a 96.000-second WAV
with zero full-scale samples (-12.26 dBTP). The score contains 187 singer notes and
313 piano notes, with the requested section ranges, 100 BPM and 4/4 retained. Installed
voices were absent in the isolated environment. The agent muted the unsynthesized
singer and explicitly delivered piano-only WAV/MIDI while retaining vocal notes/lyrics.
The MIDI contains the 313 piano notes, not the singer score.

This trial made eight recoverable tool errors: four attempts to derive kanji readings
without a dictionary and four attempts to rename lyric-derived singer clips. The
prompt now directs dictionary-free written-lyric/phoneme edits through `edit_notes`
and says not to rename singer clips. A separate short vocal trial checks this guidance;
the completed 40-bar trial is retained with its failures rather than presented as an
error-free run.

The focused eight-bar trial completed both turns and exported a 16.000-second WAV
with zero full-scale samples (-10.83 dBTP), plus MIDI containing 74 piano notes and
31 guide-melody notes. Its singer score retains the written kanji phrases and explicit
phonemes on all 31 notes. Neither missing-dictionary lyric errors nor singer-name
errors recurred. Two different recoverable calls failed (`tool_help` operation
selection and a lyric-repair template); both were corrected. Singing remains
unsynthesized, and the audio includes guide playback. These different tasks do not
establish a numerical change in failure rate.

Local evidence is under `target/composition-prompt-2026-10-29/`:

- `codex/`: initial instrumental result and the vocal crash, with frozen runtime,
  prompts, transcripts, score snapshots and output hashes.
- `workshop-codex/`: private composer report, draft revision/timing/peak measurements,
  saved editable project and final audition.
- `codex-refined/`: additional instrumental trial interrupted by the machine restart.
- `claude/`: authentication failed before composing because the OAuth session had
  expired; this is not a composition result.
- `final-instrumental/` and `final-vocal/`: independent trials with the final prompt
  before the dictionary guidance, plus the multiline display fix, using the existing
  frontier-composition-v1 manifest.
- `vocal-recovery/`: focused trial of the dictionary-free guidance using
  `tools/eval/references/vocal-dictionary-free-v1.json` and an explicit frozen prompt
  replacement in the same runtime as the 40-bar vocal trial.

The panel trials run through the real GPUI agent host with an isolated blank project
and explicit fixture-only bypass policy. Run `tools/eval/composition_quality.py run`
with the GPUI binary test executable, the manifest, `--backend codex`,
`--model gpt-6.1-sol`, `--repetitions 1`, `--timeout 900`, and
`--no-candidate-tools`. Use a new output folder per run. The private-composer trial is
the ignored `workshop::tests::installed_composer_finishes_music_and_publishes_audio_before_completion`
test, enabled with `AURIS_WORKSHOP_EVAL_OUTPUT` and `AURIS_WORKSHOP_EVAL_MODEL`.

Listening quality is unassessed. Notes, successful tools, non-silent output and peak
measurements do not establish an audible improvement. Independent generated trials
are not a controlled estimate of failure-rate improvement. The isolated vocal trial
must report unavailable singing resources explicitly; a guide voice is not a
verified synthesized vocal take.
