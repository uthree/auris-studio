# Agent composition trials — 2026-11-02

[日本語](agent-composition-trials-2026-11-02.ja.md)

Two everyday requests were given to the agent through the real application host, with the
installed Codex backend and `gpt-6.1-sol`, on an isolated unsaved fixture under explicit
bypass. Both completed. What they exposed was not a wrong edit but three places where the
tools made the model work for an answer they already had.

The harness is the opt-in `codex_composes_and_revises_the_open_document` trial described in
[composition quality](composition-quality.md); these runs used ad-hoc configurations rather
than the frozen manifest, so they measure execution, not musical quality.

## The two requests

**A background loop for a cooking video**, from a blank project, in three turns: a bright
sixteen-bar loop at 110 BPM in C major with piano, bass and light drums; then quieter drums
and strings added across bars 9–16 without touching the existing notes; then a 24-bit WAV
and a MIDI file, with the length, silence and clipping checked.

**An accompaniment for a melody the user wrote**, from the eight-bar 6/8 fixture, in two
turns: guitar, bass and light percussion around a melody that must survive untouched; then
a report on whether the melody's notes, velocities and controllers are unchanged, and the
same WAV and MIDI delivery.

Both delivered. The strings landed exactly on bars 9–16 (tick 30720, eight bars long) and
the earlier parts were unchanged; the drum strip ended at −4 dB. The handwritten track came
back byte-identical, with its CC1 point at tick 333 intact, 6/8 and 90 BPM retained. The
mixes are 34.909 s and 16.002 s — sixteen bars each at their tempo — 48 kHz/24-bit, no
full-scale samples, true peak −9.50 and −18.51 dBTP. Neither trial saved the project, and
neither confused a track ID with a clip ID.

## What the trials cost, and why

| | Calls | Failed | Elapsed |
| --- | --- | --- | --- |
| A: cooking-video loop, three turns | 205 | 2, both recovered | 657 s |
| B: accompaniment, two turns | 357 | 0 | 552 s |
| B again, after the curve inventory | 92 | 0 | 479 s |

**258 of trial B's 357 calls read controller lanes that did not exist.** Asked to confirm
that the melody's controllers were unchanged, the model had no way to ask which lanes a clip
carries, so it read all of them — every controller number from 0 to 127, twice, plus the
bend — to find the single modulation lane that was there. Every call succeeded; the tool
surface simply had no shorter way to be certain.

`MidiClip::curves` has always known the answer, for the scheduler and the MIDI writer. It is
now in both reads: a `read_notes` clip page carries `authored_curves` when the clip has
lanes, and every `edit_clip_curve` read returns the same list, in the shape its own `curve`
argument takes. Repeating trial B, the model read one lane — controller 1 — and reported the
melody intact: 92 calls instead of 357, with the same delivery.

Trial A's two recoverable failures were both messages that withheld what they knew.
`tool_help` for `render`, whose arguments are plain fields rather than an operation, answered
`Unknown operation for render; choose from ` and then nothing; it now says the tool takes no
operation. A checkpoint named `完成：ひだまりキッチン16小節` was refused for its fullwidth
colon while the message listed only what was allowed, so the message now says that spaces and
punctuation are not, in both languages. The rule itself is unchanged: the name becomes a file
name.

## Left as an observation

Trial A needed somewhere to put the renders it measured along the way, and an unsaved project
has no folder of its own. It chose `~/Music/Auris Exports/` and left four WAVs and their
manifests there, about 33 MB, after the conversation was over. `preview` exists for exactly
this and writes under the document's preview folder, which an unsaved document does not have.
Nothing here is wrong, and no change is proposed: a working render needs a durable path, the
user's export folder is a reasonable guess, and where intermediate audio for an unsaved
project should live is a design decision rather than a defect.

## Reproduction

`cargo clippy --workspace --all-targets` and `cargo fmt --all --check` are clean. The
`auris-session` and `auris-toolbox` suites pass, including three new tests: the lane
inventory a curve read returns, a `read_notes` page both with and without lanes, and the help
message for a tool that takes no operation.
