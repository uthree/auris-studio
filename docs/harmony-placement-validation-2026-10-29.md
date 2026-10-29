# Agent harmony placement

Whitespace separates chords within one chart bar. Passing `I V vi IV` to an
eight-bar harmony edit therefore put all four chords in each bar. The data was
already dense before the harmony lane rendered it.

`edit_harmony` now accepts explicit bar arrays:

```json
{"start_bar":1,"bars":8,"chords":["I","V","vi","IV"]}
```

Each entry is one bar; this four-bar progression repeats twice. For intentional
within-bar changes, use entries such as `"I V"`. Explicit pipe-delimited strings,
single chords and catalogue references remain supported. Bare multi-chord strings
are rejected with correction examples before key, chord or section edits begin.
Empty arrays and entries that embed additional bars are rejected too.

There is no one-chord-per-bar requirement. For example,
`["I","IV V","vi","ii V"]` mixes one and two chords per bar. Chords inside
each entry share the bar equally, so in 4/4 the two changes start on beats 1 and
3. The shared prompt and tool help use this mixed example and ask the agent to
choose harmonic rhythm for the phrase.

The response reports the input `chart_bars` and actual `chord_changes` in the
requested range. The shared prompt asks the agent to check these before generating
backing. Harmony edits preserve existing notes; repositioning harmony in an existing
song and regenerating any backing remain explicit commands.

`inspect_audio` previously reported only the harmony at bar starts. It now also
returns bounded `harmony_events`, including the in-force harmony at the range
start and all subsequent chord/key changes within the range. Events include exact
ticks, bar/beat, sub-beat ticks and range-relative seconds. The response reports
the full event count and whether its 128-event list is truncated. Snapshot tests
cover intra-bar changes, carried harmony, modulation, boundaries and truncation.

Regression tests cover one chord per bar at bar 25, repeated progressions,
intentional subdivisions, catalogue charts, invalid input, preserved notes and
Undo/Redo. Existing meter/grid/boundary restoration tests still run.

A real Codex (`gpt-6.1-sol`) panel trial used
`tools/eval/references/harmony-placement-v1.json`. It placed `I V vi IV` twice using
an eight-entry bar array. The captured score has eight chord starts at ticks
0, 3840, 7680, 11520, 15360, 19200, 23040 and 26880, then restores the previous
empty harmony at tick 30720. No notes or exports were requested. Local frozen
runtime, prompts, transcript and score snapshots are under
`target/harmony-placement-2026-10-29/codex/`.

Both model turns completed with zero tool errors, and the before/after track data
is identical. Validation also passed 4,767 workspace unit/doc tests with
`cargo test --workspace --exclude auris-gpu -- --test-threads=4`,
`cargo clippy --workspace --all-targets` and `cargo fmt --all --check`.

## Full composition with mixed harmonic rhythm

The subsequent `mixed-harmonic-rhythm-v1.json` trial used the updated shared
prompt and tools with real Codex (`gpt-6.1-sol`). It requested a 16-bar, 120 BPM,
4/4 C-major instrumental with lead, keyboard, bass and restrained drums. Its
specified chart mixes eight single-chord bars with eight two-chord bars:

```text
I  | IV V | vi | ii V | I    | IV V   | vi IV | V |
IV | V vi | ii | V    | IV V | iii vi | ii V  | I |
```

Both model turns completed in approximately 320 seconds total with zero tool
errors. The 24 stored chord positions match the requested chart exactly, including
all eight beat-3 changes. An independent score audit checked all 24 positions
against the actual notes: keyboard attacks contain the requested triad and bass
attacks contain its root at each boundary. The score has 182 keyboard, 84 lead,
63 bass and 185 drum notes (514 total), all within the requested clips.

The agent used `inspect_audio` with the new event data; its first four-bar report
includes both F at bar 2 beat 1 and G at bar 2 beat 3. The original score was
preserved during the delivery turn.

The actual exports were independently parsed:

- WAV: 48 kHz stereo, 24-bit PCM, 1,536,000 frames / 32.000 seconds.
- MIDI: 960 ticks per quarter, 514 note-on events, 120 BPM and end tick 61,440,
  including the ending rest (32 seconds).
- Encoded WAV measurements: true peak -7.77 dBTP, -24.33 LUFS, zero full-scale
  samples and ending RMS approximately -158.52 dBFS. `verify_render` passed.
- WAV SHA-256: `5d1fd9a07d9fda87f87a3d7165b5fefda1eec8e29f954e8289a59de7eff562a0`.

The final snapshot was copied to an isolated project folder and saved using the
CLI's `collect` command. Reopening preserved notes and harmony; a fresh CLI render
with `--no-tail` produced a byte-identical WAV. The user's existing song was not
modified. This is one successful technical composition trial, not a measured
failure-rate improvement or a human listening evaluation.

The frozen runtime, source diff, prompt, transcripts, snapshots and original
exports are under `target/mixed-harmonic-rhythm-2026-10-29/codex/`. The independent
audit is `target/mixed-harmonic-rhythm-2026-10-29/placement-audit.json`; the reopened
project and render are in its sibling `delivery/` directory. The reproducible
brief is `tools/eval/references/mixed-harmonic-rhythm-v1.json`.

After the tool changes, 4,769 normal workspace unit/doc tests and 30 serial GPU
tests passed, along with `cargo clippy --workspace --all-targets` and
`cargo fmt --all -- --check`. The inspection regression failed on the old
bar-start-only response before the implementation was changed.
