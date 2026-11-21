# VOICEVOX pronunciation checks

[日本語](voicevox-pronunciation.ja.md)

The 2026-10-22 investigation uses VOICEVOX Engine 0.25.2, prediction style 6000,
and the adapter's 93.75 fps clock. Its baseline is commit `8025e4ab`.

Three adapter problems affected pronunciation:

- Extended kana were split into ordinary syllables. For example, `スィ` became
  `s u i` instead of `s i`, and `クォ` became `k u o` instead of `kw o`.
  The kana reader now recognizes the missing extended moras and Engine aliases.
  Across all 187 spellings, the baseline reader split 40 into extra syllables
  and did not recognize `ヶ`; all 41 readings are corrected.
- A predicted consonant can anticipate its vowel inside the preceding note.
  Applying that preceding note's gain could silence the incoming consonant;
  applying its pitch edits could also put the consonant at the wrong pitch.
  Predicted articulation ranges now take controls from their own note.
- A short note's vowel could receive zero frames, or a temporary query frame
  could be removed from the following consonant. The adapter reserves a vowel
  frame and recalculates acoustic curves without changing vowel onsets or the
  total duration. A one-frame note immediately before a consonant cannot hold
  both sounds on this grid; it is reported as a lyric-duration error.

## Inventory measurement

An individual-query sweep covered 163 primary mora spellings at MIDI 60. The
target mora lasts 24 frames and follows `ア`; the preceding note length and gap
vary as shown below. Both boundaries have 94 padding frames.

| Preceding note | Gap | Cases | Baseline phone mismatches | Fixed mismatches | Duration errors |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 24 | 0 | 163 | 35 | 0 | 0 |
| 24 | 2 | 163 | 35 | 0 | 0 |
| 24 | 8 | 163 | 35 | 0 | 0 |
| 4 | 0 | 163 | 68 | 0 | 0 |
| 2 | 0 | 163 | 36 | 0 | 0 |
| 1 | 0 | 163 | 156 | 0 | 156 |

All 822 representable cases preserve the Engine's intended non-pause phoneme
sequence. The other 156 are rejected before inference. The retained
[measurement](../tools/eval/references/voicevox-pronunciation.json) includes the
35 incorrect splits, conditions, Engine version and inventory-source hash.

The regression additionally covers all 187 spellings, including aliases, in
batched scores: five timing conditions are compared to queries sent directly
to the Engine, independently of Auris's kana parser. Every sung phoneme must
retain a positive duration. All 187 spellings also receive the one-frame
duration validation check.

These checks measure phoneme preservation, not perceived intelligibility for
every singer and pitch. A paired audio probe with 四国めたん / ノーマル keeps
the Engine's phonemes and predicted curves identical and changes only control
ownership: incoming `/k/` and `/s/` remain audible after a zero-velocity note.
Their waveform RMS over the predicted consonant windows rises by about 20 and
22 dB respectively; the intended incoming note velocity is 0.8 in both versions.
Numeric tests also check that the incoming pitch edit is used and the preceding
vowel keeps its own gain.

## Velocity and the Engine's volume

The 2026-10-23 investigation transcribed sung probes with Whisper (large-v3-turbo). The Engine's
`volume` is a decoder input, not a fader. Scaling it changes the spectrum of every phoneme even
after the level is normalised: across 24 phrases sung by 四国めたん / ノーマル, the per-phoneme
log-spectral RMS difference is about 0.5 at ×0.8, 0.9 at ×0.5 and 1.2 at ×0.3. Auris used to
send velocity × expression as that scale. At velocity 0.5 the character error rate of 四国めたん
and 波音リツ rose from about 10 % to 18.7 %, and the errors were substituted consonants: ば as ま,
ざ as だ, ひ as ぎ.

The adapter now sends the predicted volume unchanged and applies velocity × expression to the
decoded audio. The gain is interpolated between frame centres. Consonants that anticipate their
vowel still take the gain of their own note.

A same-take comparison isolates the change. It uses 72 queries (24 phrases × 3 takes,
四国めたん / ノーマル), and each one is decoded with its volume scaled and with its audio scaled.

| Level | Volume scaled (before) | Audio scaled (now) |
| ---: | ---: | ---: |
| 0.8 | 11.9 % | 11.4 % |
| 0.5 | 19.0 % | 11.7 % |

At 0.5, audio scaling was better on 20 takes and worse on 8. The errors it removed are consonant
substitutions (さしすせそ heard as しじせぞ, ざじずぜぞ as ざみでぞ, ぽすと as ぼすと). The
remaining error rate is the same as at full volume.

Two properties of Engine 0.25.2 limit these comparisons. `sing_frame_audio_query` predicts phoneme
durations deterministically, but its f0 and volume differ on every request, so two renders of the
same score are two takes. A fair comparison therefore synthesises the same query twice;
`frame_synthesis` is deterministic. Some confusions also occur without Auris: ぱ行 is heard as ば行
in most takes sung directly through the Engine.

## Long clips

VOICEVOX prediction and decoding requests are limited to twenty seconds, including
context and boundary rests. Longer clips are synthesized sequentially, preferring
rests and note boundaries for cuts. Each window includes up to one second of real
neighbouring music on either side. Sustained notes resume on their final sound when
the window begins inside them; a short crossfade joins decoded windows. Lyrics are
normalized before slicing, so multi-mora words and prolonged vowels retain their
meaning across cuts. Saved notes, total duration, expression controls and the displayed
pitch timeline keep their original positions. Cancellation is checked between prediction
and decoding stages and before each window.

## Re-singing after an edit

A rest of at least one second divides a score into phrases, and each phrase is sung on
its own with half a second of that rest on either side; the rest beyond is silence, ramped
over ten milliseconds at a phrase's inner edges. A phrase's request then holds its own notes
and nothing of its neighbours, so the loaded voice keeps every answer the Engine gave, keyed
by the request's complete input, and sings again only the phrases an edit touched. A phrase
moved in time without changing tempo is the same request. On a 93-second test song, one
changed lyric or pitch re-sings in about 1.2 seconds where the whole score took 8.

Reused audio is only ever reused for the Engine that sang it: before a render consults the
cache, the Engine's version, manifest and singer list are read again, and any change empties
it. The cache holds at most fifteen minutes of audio, dropping the least recently used.

The optional live regression
`a_running_voicevox_engine_sings_a_long_score_in_bounded_windows` exercises a score
longer than two minutes with a sustained syllable, multi-mora lyrics and pauses.

## Reproduction

With the Engine running, set `AURIS_VOICEVOX_TEST_URL` to its URL and run:

```sh
cargo test -p auris-vocal
cargo test -p auris-singer
```

The live inventory regression is
`a_running_engine_preserves_the_mora_inventory_across_timing_adaptation`.
Without the environment variable, Engine-dependent tests return without
contacting a service. Existing saved takes require an explicit re-render.

To check intelligibility, render the probes and transcribe them:

```sh
cargo run -p auris-singer --example voicevox_probe -- target/voicevox-probe 3002
uv run tools/eval/voicevox_intelligibility.py target/voicevox-probe
```

The probe writes each phrase through Auris at three velocities and once directly through the
Engine. The script reports the character error rate per variant against the written kana.
