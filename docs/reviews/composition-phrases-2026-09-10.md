# Composition phrases: implementation and measured comparison

2026-09-10. The rule-based composer now shares phrase context, remembers melodic material,
connects chord voices, and shapes generated backing around the foreground. Written notes remain
editable. Measurements show lower chord voice motion and modest average Audiobox changes, with
genre-specific regressions and increased bar-rhythm repetition. No listening evaluation was
performed, and these results establish neither universal musical quality nor statistical confidence.

This compares baseline `19b8d21` with the implementation accompanying this report. The
[compact evidence JSON](composition-phrases-2026-09-10.json) identifies the measured audio,
projects, models and source reports by SHA-256. Integration validation is recorded below.

## Five implementation changes

1. **Shared phrase planning.** Harmony, melody and accompaniment use common multi-bar boundaries
   and statement, continuation, answer and release roles. Rhythmic styles start from four bars;
   ambient and orchestral styles allow eight. Uneven lengths are balanced, such as 5 = 3 + 2.
   A recognizable opening gesture returns with developed endings. Explicitly authored harmony
   remains the score's authority.
2. **Foreground-aware backing.** Generated chords, stabs and arpeggios can leave room under busy
   foreground notes and answer in a planned breath. Lyrics use the actual singer notes.
   Explicit rhythm patterns bypass the pass; muted or empty candidates are ignored, selection
   survives track reordering, and a rewritten clip cannot use its own old Lead as foreground.
   Copied responses stop at incompatible harmony and cannot fill erased harmony.
3. **Prepared tension and connected voices.** Suspensions are prepared before a chord change
   and resolve by step. Chord inversions consider individual voice movement and register;
   sustained parts retain common tones. These are written pitches and rhythms.
4. **Genre-specific writing.** Melodic moves, bass figures, comping rhythm, phrase span and vocal
   rhythm read a separate `writing_style` palette. Performance controls retain their independent
   role. Generic bass clips can also choose the fifth above or below their root anchors per
   phrase, so a new take has a voicing choice without changing the rhythmic figure.
5. **Prosodic vocal rhythm and hooks.** A bounded 24-candidate search uses mora contours,
   word/accent groups, style and seed to choose short/long cells, pickups, holds and breaths.
   Pitch phrases recall an initial gesture while retaining accent and harmony costs. Fixed
   section bars, every mora, unvoiced closures and exact shared melodies remain constraints;
   infeasible density is rejected.

`writing_style` selects score vocabulary and is persisted in generated clip recipes.
`performance` remains a separate playback setting. These changes produce ordinary `Note`
data; the existing piano roll, Undo and freeze workflows remain the editing surface.
Learned models below are development evaluators, not note or waveform generators.

## Controlled render conditions

- Nine presets × seeds **101 and 103**: 18 instrumental pairs, plus two separate vocal pairs.
- One shared **MuseScore_General v0.2** SoundFont, 215,614,036 bytes. Its SHA-256 is
  `ee51d2c4b1525e70f19a45909c4fd7a2e26d91d115fa89dbf5a6bc413d8b9bf3`.
  All 40 saved projects reference that same file.
- Stereo **48 kHz, 32-bit IEEE float WAV**, with export tail disabled by `--no-tail`.
  All 40 WAV headers were checked, including the extensible WAV float subtype.
  Normal session mixing remains in the render path; evaluator preprocessing adds no loudness normalization.
- Paired instrumental durations and CLAP excerpt positions match. Each CLAP file has three valid
  ten-second excerpts: first, middle and last complete windows. Across 108 sampled windows,
  no samples exceeded the native quantizer's [-1, 1] input range.
- The controlled vocal fixture uses the same compiled NAIST dictionary and sound assets on both
  sides. Its built-in vocal synthesis is an audition of the written melody/rhythm, not a test of
  learned singing quality or lyric intelligibility.

The final `game-loop` pair includes the external preset's explicit `writing_style = "chiptune"`.
An initial render had omitted it. Both affected current WAVs were rescored, the final all-18
reports were merged with an audit manifest, and the unchanged 16 labels retained their results.
All numbers below use that final merge.

## Evaluator provenance

**CLAP:** the [official LAION implementation](https://github.com/LAION-AI/CLAP), native
`laion-clap 1.1.7`, `HTSAT-base`, fusion disabled, RoBERTa text encoder. The checkpoint is
`lukewys/laion_clap/music_audioset_epoch_15_esc_90.14.pt`, revision
`b3708341862f581175dba5c356a4ebf74a9b6651`, SHA-256
`fae3e9c087f2909c28a09dc31c8dfcdacbc42ba44c70e972b58c1bd1caf6dedd`.
The tokenizer is pinned to `roberta-base` revision
`e2da8e2f811d1448a5b465c236feacd80ffbac7b`; tokenizer file hashes are in the evidence JSON.

CLAP uses two fixed intended descriptions and two contrasting descriptions per preset from
[`clap_prompts.json`](../../tools/eval/clap_prompts.json). Channels are averaged, resampled to
48 kHz if needed, then native CLAP clips and quantizes to int16. The positive cosine measures
description alignment; the margin subtracts the contrasting-description cosine. Neither is a
probability or an aesthetic rating. A fresh inference on the unchanged baseline
`jazz-trio-s101.wav` reproduced all three aggregate values exactly.

**Audiobox:** [Meta's Audiobox Aesthetics model](https://huggingface.co/facebook/audiobox-aesthetics),
package `0.0.4`, repository revision `9b1dd8e5df9af7216e836a98974fe3b82c56ded6`,
`model.safetensors` SHA-256
`a5a3c2412649cc2384ec525ffd5180ce6c4778f43bed6108e0a1303de04d014e`.
It predicts content enjoyment (**CE**), content usefulness (**CU**), production complexity
(**PC**) and production quality (**PQ**), on 1–10 scales. Complexity is not a scale to maximize.
The existing scoring function processes ten-second windows across the complete WAV, weighting
the final partial window by duration.

Both ran locally on CPU with four threads, PyTorch/torchaudio `2.6.0+cpu`.
CLAP also used transformers `4.57.6`, NumPy `1.26.4`, SciPy `1.17.1` and
soundfile `0.14.0`. Model, tokenizer, configuration and environment hashes are retained in
the evidence JSON. Large model files remain under `target/`.

## Instrumental measurements

Each genre row is the mean paired delta over two seeds; overall means weight all 18 files
equally. Positive signs mean an increase in that particular measurement.

| Preset | CE Δ | CU Δ | PC Δ | PQ Δ | CLAP positive cosine Δ | CLAP margin Δ |
|---|---:|---:|---:|---:|---:|---:|
| ambient | -0.1810 | -0.0885 | -0.0820 | -0.0785 | +0.0033 | +0.0234 |
| chiptune | +0.1015 | +0.0365 | +0.0070 | +0.0430 | -0.0059 | -0.0037 |
| city-pop | +0.0110 | +0.0170 | +0.0120 | +0.0100 | -0.0018 | -0.0071 |
| game-loop | +0.1560 | +0.0575 | -0.0060 | +0.0925 | -0.0347 | -0.0368 |
| jazz-trio | +0.0650 | +0.0480 | -0.0490 | +0.0475 | +0.0138 | +0.0337 |
| orchestral | +0.0300 | +0.0290 | -0.1355 | +0.1065 | +0.0104 | +0.0028 |
| pop-band | -0.0455 | -0.0240 | +0.0095 | -0.0430 | +0.0018 | -0.0209 |
| rock | +0.1190 | +0.1405 | -0.0740 | +0.1680 | +0.0022 | +0.0332 |
| synthwave | +0.0355 | +0.0360 | +0.0995 | +0.0260 | +0.0046 | +0.0063 |

| All 18 files | Before | Current | Mean paired Δ |
|---|---:|---:|---:|
| CE | 6.8599 | 6.8923 | +0.0324 |
| CU | 7.6759 | 7.7039 | +0.0280 |
| PC | 5.1289 | 5.1046 | -0.0243 |
| PQ | 7.8165 | 7.8578 | +0.0413 |
| CLAP positive cosine | 0.4329 | 0.4322 | -0.0007 |
| CLAP contrast margin | 0.3563 | 0.3598 | +0.0034 |

CE increased in **12/18** pairs and decreased in 6; PQ increased in **13/18** and decreased in 5.
Ambient and instrumental pop-band decreased on both seeds in CE and PQ. Rock increased on
both, while game-loop's Audiobox values increased but both CLAP identity measures decreased.
These observations should remain visible beside the small positive overall Audiobox means.

## Written-score structure and tradeoffs

The structural tool reads saved notes before performance transforms. Foreground includes Lead
and Vocal; support includes chords, stabs and arpeggios. Chord motion uses chords/stabs only
and finds minimum noncrossing voice matches, accounting separately for entering/leaving voices.
A large melodic leap is at least five semitones between contiguous monophonic notes; rests and
clip boundaries are excluded. Rhythm compares onset sets within complete, nonempty bars.

The following ratios use **percentage-point deltas**. Motion is semitones per matched voice.

| Preset | Voice motion before → current | Adjacent rhythm repetition Δ pp | Onset collision Δ pp | Sounding overlap Δ pp | Large leap fraction Δ pp |
|---|---:|---:|---:|---:|---:|
| ambient | — | +21.43 | +1.03 | +8.00 | -6.53 |
| chiptune | 4.04 → 1.67 | +1.32 | +0.88 | -1.24 | +1.74 |
| city-pop | 3.61 → 1.32 | +1.32 | -2.95 | -3.71 | +1.05 |
| game-loop | 4.00 → 1.54 | +7.14 | +5.17 | +0.32 | -0.72 |
| jazz-trio | 5.15 → 1.78 | +1.43 | -3.37 | -0.79 | +13.86 |
| orchestral | 4.12 → 1.52 | +21.43 | +4.03 | +5.15 | +2.02 |
| pop-band | 2.69 → 1.43 | +3.64 | -3.83 | -8.49 | +1.43 |
| rock | 4.30 → 1.76 | +3.95 | +6.47 | -1.27 | +1.01 |
| synthwave | — | +1.43 | -1.86 | -9.90 | -0.12 |

Across the 18 projects, adjacent bar-rhythm repetition **increased from 71.20% to 78.20%**;
the fraction of unique bar rhythms decreased from **28.94% to 26.12%**. This is not evidence
of reduced repetition. Reusing a phrase head can increase this measure, but the number does
not determine whether the repetition is memorable or monotonous.

Mean chord voice motion decreased from **3.988 to 1.574 semitones** across the 14 projects
with matching observations. Ambient and synthwave have no chords/stabs observations for this
measure, so they are omitted from that mean rather than counted as zero.

Mean support/foreground sounding overlap decreased from **68.63% to 67.31%**, while onset
collision increased from **35.88% to 36.50%**. Ambient, orchestral and game-loop overlap
increased; pop-band and synthwave overlap decreased more substantially. The mean contiguous
large-leap fraction increased from **7.11% to 8.64%**, including a larger increase for jazz-trio.
These are musical tradeoffs to audition, not a uniform structural win.

All 40 instrumental/vocal saved projects have zero invalid clips, invalid notes and nonfinite
values in the structural reports; their recorded project hashes match the files inspected.

## Separate vocal probe

The [controlled fixture](../../tools/eval/fixtures/vocal-phrases.asong) has four four-bar vocal
sections, with `verse2` sharing `verse`, plus the composition's instrumental ending.
Its final line is **あるいて いくよ** on both sides. The vocal contract audit confirms, for
both seeds and both versions:

- **54 moras**, all **four vocal sections**, and every note within its clip.
- Exact shared-verse note data, including timing, pitches and ornaments.
- One preserved unvoiced closure with valid closure handling.
- Three distinct note lengths in each baseline take, versus seven/eight in the current takes.
  Changing the seed changes the current vocal rhythm; it did not change the baseline rhythm.

The two vocal pairs were evaluated separately with Audiobox; the instrumental CLAP prompt set
was not applied to them.

| Vocal mean, two seeds | Before | Current | Mean paired Δ |
|---|---:|---:|---:|
| CE | 6.8870 | 7.0810 | +0.1940 |
| CU | 7.8575 | 7.9015 | +0.0440 |
| PC | 5.2220 | 5.2875 | +0.0655 |
| PQ | 8.0690 | 8.1030 | +0.0340 |

CE changed by **+0.259** for seed 101 and **+0.129** for seed 103. Across the complete fixture's
foreground/support score, sounding overlap moved **95.96% → 92.75%**, onset collision
**53.18% → 43.97%**, and chord voice motion **3.515 → 1.625 semitones**.
Adjacent rhythm repetition increased **33.33% → 37.50%**, while unique bar rhythms increased
**62.12% → 69.70%**; the metrics measure different aspects of recurrence and variety.

A separate production regression covers **あるいて ゆこう**. The dictionary can emit the
volitional auxiliary as a separate long-vowel `ー` node; parsing that node without the previous
vowel made the otherwise valid section unreadable. The reader now carries vowel context across
nodes while preserving mora and accent-group boundaries. The controlled A/B uses `いくよ`
because the baseline predates this fix; both comparison versions were rerendered with that same
text so a missing baseline section cannot inflate the measured change.

## Reproduction and limits

See [the evaluation guide](../evaluation.md) for complete options and interpretation. Existing
renders and saved projects can be measured without rebuilding the composer:

```powershell
uv run tools/eval/structure.py target/composition-eval/after --baseline target/composition-eval/before-structure.json --json target/composition-eval/after-structure.json
uv run tools/eval/aesthetics.py target/composition-eval/after --baseline target/composition-eval/before-aesthetics.json --json target/composition-eval/after-aesthetics.json
uv run tools/eval/clap.py target/composition-eval/after --baseline target/composition-eval/before-clap.json --json target/composition-eval/after-clap.json
```

The evidence JSON keeps audio/project hashes, package/model pins, paired deltas, genre means,
the vocal contract summary and hashes of the raw source reports. Large WAVs, full reports,
build logs and checkpoints remain in `target/composition-eval/`; they are not added to Git.

Two selected seeds per preset are a small descriptive comparison. There was no human listening
test, significance test or confidence interval. CLAP's three excerpts do not establish long-form
structure or hook quality, and Audiobox predictions do not establish human preference.
The vocal probe does not evaluate a learned singer's intelligibility. No evaluator scores were
used as an optimization objective in this change.

The [VOICEVOX follow-up](composition-voicevox-2026-09-10.md) sings these same saved vocal scores
with Zundamon and remeasures the final mixes. The original vocal CE increase does not reproduce
under that singer, so the built-in-vocal result above should not be generalized across renderers.

## Integration validation

- `cargo fmt --all --check`: passed.
- `cargo test --workspace --locked --offline -- --test-threads=1`: **3,263 passed,
  8 ignored**, including unit, integration and documentation tests. The Windows run used the
  complete ASIO/LLVM setup, the pinned SoundFont and the installed Japanese dictionary.
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`: passed.
- `git diff --check`: passed.
- Python evaluation tooling: **22 tests passed**; Ruff passed for the changed scripts.
- The separate CLI/dictionary probe for `あるいて ゆこう` produced all four vocal sections
  with the expected 14/13/14/13 mora counts. The controlled fixture independently verified
  all 54 moras, fixed section bounds, shared-verse recall and valid closure lengths.

Workspace and Python check logs are retained in `target/composition-eval/`. These checks and
the model scores are automated evidence; no human listening assessment was performed.
