# Music analysis

[日本語](music-analysis.ja.md)

Auris estimates chords from written notes. The command uses Rust signal processing and
fixed templates on the CPU, without learned weights or a GPU.

## Desktop controls

The **Analysis** menu collects chord estimation and drum measurement.
**Analyze Chords across Tracks** analyzes the project; **Analyze Written Chords** analyzes
the selected note track. Known percussion and muted material are excluded. The default
resolution is one quarter-note beat. A background worker reports progress and can be
cancelled. **Analysis → Analysis Results…** reopens its panel.

**View Analysis Draft** shows chord alternatives and their scores. Previews show up to
256 intervals; JSON output contains the full result. Times are zero-based quarter-note
beats.

Nothing changes until an acceptance control is used. **Apply Recognized Chords** replaces
recognized intervals and clears silent intervals in the harmony timeline. Unknown intervals
and outside harmony are preserved. Candidates can also be accepted individually.

Each acceptance is one undo step. Any document edit invalidates the report. Reports are
transient and add no project format fields.

## CLI and model tools

Commands print JSON. Beat arguments are zero-based quarter-note beats.

```sh
auris analyze-chords song.auris --track Piano --from-beat 0 --to-beat 32 --window-beats 1
auris analyze-chords song.auris --apply
```

`--track` also accepts `id:number`; omitting it selects all pitched note tracks.
`--apply` accepts written harmony and saves a checkpoint. MCP and agent tools expose
`analyze_chords` through the same session command. Scores measure template agreement,
not calibrated probabilities.

## Algorithm and practical limits

Analysis accumulates duration-weighted pitch classes on a fixed grid, caps octave and
unison doubling, and considers sustained bass. It searches twelve roots for major, minor,
diminished, augmented, suspended, seventh and sixth templates. A transition penalty
discourages unnecessary changes. Four alternatives are retained; weak or tied
interpretations remain unknown. Grid boundaries limit change timing. Requests are limited
to 16,384 chord intervals.

## Verification

```sh
cargo test -p auris-analysis
cargo test -p auris-session --lib recognition
cargo test -p auris-gpui --bins music_analysis
```

Numeric fixtures cover chord spelling, ambiguity/silence, loops, inversions and
cancellation. Session and window tests cover explicit acceptance, stale reports and Undo.
