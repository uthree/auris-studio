# Evaluating what the composer writes

Development measurements, none part of any release build. They exist for the same reason
every level and timing constant in this workspace was calibrated by rendering and measuring:
a change to a writer or a dial should be judged against numbers first and ears always.

The [preset arrangement study](preset-arrangement.md) pairs these measurements with track
participation and density across the form, and records the default-seed selection, fresh-seed
check, and per-preset tradeoffs from the 2026-10-04 revision.

The [physical-instrument copy-synthesis loop](physical-copy-synthesis.md) measures temporal
mel-spectrogram distance against a frozen cohort of real piano, guitar and violin recordings.
It fits only training notes and evaluates held-out pitches and dynamics with the actual Rust
renderer. This acoustic distance complements the symbolic and learned song measurements below.
The [long-note/trajectory extension](physical-trajectories.md) adds 3/6-second notes, real
violin glissandi, recorded performance guides and separate late-sustain/transition/release losses.
The [physical-model experiments](physical-extensions.md) add causal radiation modes,
register/gesture ablations, paired confidence intervals and synchronized piano-pedal references.
The [bass/bar/drum calibration](physical-pack.md) adds 391 real-recording excerpts, separate
attack/body/tail loss and acoustic-role guards for every snare/tom register.
The [electric-guitar experiment](electric-guitar.md) adds independent pickup DI, an
oversampled amplifier and a paired clean/pedal recording benchmark with pitch-grouped splits.
The [folk-instrument experiment](folk-physical-instruments.md) adds hammered dulcimer strikes
and settled tin-whistle scale excerpts, with distinct-pitch validation and frozen source hashes.
The [choir calibration](choir-copy-synthesis.md) fits the native vocal-tract ensemble to
real sustained vowels, with 24 training notes and 80 notes from separate held-out singers.

## The symbolic ruler

```
cargo run -p auris-compose --example measure
```

Prints one row per preset and part, averaged over eight seeds with swing forced straight, so
the table reads the *pattern* the composer chose rather than where the feel later nudged it:

* **sync/bar** — Longuet-Higgins–Lee syncopation on the grid's metric hierarchy, per bar.
  Zero is four-on-the-floor; a backbeat is about 3; the classic last-sixteenth anticipation
  is 4. The groove studies (Witek et al. 2014) put pleasure and the urge to move at the
  *middle* of this scale — an inverted U — so the number is a dial to aim, not a score to
  maximise.
* **pc-bits** — pitch-class entropy of the tune, duration- and velocity-weighted. A part
  evenly over its scale sits at log₂7 ≈ 2.81; near zero is a drone, past the ceiling is a
  line that has lost its key.
* **steps% / mean-int** — how much of the melody moves stepwise, and its mean interval in
  semitones. The corpus reference used while tuning the melody writer was 68 % stepwise.

The functions behind the table are `auris_compose::metrics`, public and unit-tested, so a
future command or test can read the same numbers the example prints.

The optional [composition search](composition-search.md) uses a separate, explicit symbolic
target: written note events per bar. It maximizes negative absolute distance from that target,
with parameter bounds and a fixed composition seed. Moving closer to a requested density is
a measurable arrangement change; it does not establish that the song sounds better.

## Inspecting the saved score

```
uv run tools/eval/structure.py before/ --json before-structure.json
uv run tools/eval/structure.py after/ --baseline before-structure.json --json after-structure.json
```

Reads ordinary notes from saved `.auris` projects, including actual lyric-bearing singer
clips. It measures within-clip bar rhythm repetition/variety, contiguous monophonic leaps,
foreground/support onset collisions and sounding overlap, and minimum noncrossing chord
voice motion with separate entering/leaving voice counts. Chords, stabs and arpeggios count
as support; only chords and stabs count for voice motion. Silence, rests and clip boundaries
do not become artificial melodic leaps. A ratio with no observations is `null`.

The report also validates note bounds and finite data, saves source hashes, and pairs projects
by filename stem. Repetition, overlap and complexity have musical uses: none of these numbers
is a universal quality score. This reads written text, not performance transforms or audio;
muted clips/tracks are excluded from musical measures, while validation covers all stored
notes. It does not simulate loops, routing, solo or audio clips.

`tools/eval/fixtures/vocal-phrases.asong` supplies a fixed lyric probe with shared verses and
unvoiced closures. Use the same dictionary and sound assets for both renders. An instrument
audition of sung notes checks melody/rhythm, not the intelligibility of a learned singer.

## The learned ear

```sh
uv run tools/eval/music.py --preset all --seeds 3 --workdir target/before --json before.json
uv run tools/eval/music.py --preset all --seeds 3 --workdir target/after --baseline before.json --json after.json
```

Pass `--cli path/to/auris.exe` (or `auris` on macOS) to freeze a renderer, or
pass existing WAVs/directories instead of `--preset`. Both models are development
tools; rendering and inference stay local. The initial run fetches several GB of
public weights into `target/composition-eval/models` and a pinned copy of the
official TuneJury source beside them. Python and packages are managed by uv.
`--device cpu` is the default; `--device cuda` requires a CUDA-enabled PyTorch.

[TuneJury](https://github.com/yonghyunk1m/TuneJury) predicts a scalar pairwise
preference reward. Its primary released head uses frozen CLAP-Music and MERT
embeddings, in training order `[CLAP audio, MERT audio, CLAP text]`. For these
post-hoc genre captions the evaluator uses the author's recommended empty-prompt
protocol: the text branch is 512 zeros. Reward is uncalibrated, has no 1–10 range,
and is comparable only under the same checkpoint and preprocessing conditions.

[MuQ-MuLan](https://github.com/tencent-ailab/MuQ) evaluates agreement with the
versioned positive and contrast descriptions in `tools/eval/music_prompts.json`.
The model receives float32 mono **24 kHz** PCM and returns joint music/text
embeddings. Normalize embeddings explicitly before computing `positive_cosine`,
`contrast_cosine`, and their `contrast_margin`. Similarity describes preset
identity, not preference. Freeze prompts before comparing candidates.

Both models read the same deterministic ten-second windows: first, middle and
last by default, or one centered window with `--segments 1`. Channel averaging
precedes polyphase resampling to 24 kHz; short clips repeat whole copies and
zero-pad the remainder. No gain normalization is applied. TuneJury's internal
CLAP encoder resamples to 48 kHz and quantizes, so windows below its int16
resolution are rejected alongside silence and nonfinite inputs. A partial or
invalid measurement saves diagnostics and exits unsuccessfully.

Reports contain per-window and mean scores, WAV hashes, model/source revisions,
downloaded artifact hashes, package versions and prompts. Baseline comparison
rejects changed measurement conditions. Keep per-preset paired deltas and excerpt
positions alongside means. A changed tempo can move the centered excerpt: inspect
the retained full renders as well. An unchanged-file comparison checks inference
repeatability. Model weights are CC-BY-NC 4.0; their use here is offline research.

## Controlled melody listening

`melody_ab.py` transplants an instrumental lead into a frozen backing;
`melody_continuity_ab.py` freezes the renderer and makes level-matched chorus
excerpts. `melody_phrase_ab.py` compares baseline, pitch, rhythm and combined
variants with explicit artifact hashes and component-change guards. Existing
notes, backing, IDs, mixer and performance transforms remain in editable projects.
`structure.py`, `melody_continuity.py` and `seed_metrics.py` inspect written-score
rhythm, phrase continuity, overlap and voice motion independently of model scores.

Score **the listening excerpts** with `music.py --segments 1`. The four-condition
`melody_phrase_listening.py --scores scores.json` specification has schema_version
1 and a `variants` object. Each condition supplies `music: {path, sha256}` for its
evaluation report and `excerpt_sha256`, a complete label-to-WAV-hash map. Reports
must share model, preprocessing and prompt metadata. Omitting scores leaves cells
blank. `seed_listening.py --manifest manifest.json --scores music.json --output
listening.html` produces a shuffled seed comparison and verifies that scored hashes
match the listening excerpts. Both reports use local audio players and upload nothing.

## Preset tuning

```sh
uv run tools/eval/tune.py --preset all --trials 18 --out tune-results.json --workdir target/tuning
uv run tools/eval/tune.py --preset chiptune --dials humanize dynamics --trials 12 --out focused.json
```

Optuna TPE maximizes two separate objectives: TuneJury reward and MuQ-MuLan positive
cosine. The continuous dials are humanize, dynamics, fill, variation, mood, tempo
within ±6%, and swing within ±6 points only for already swung presets. Key, groove,
progression, form and instruments stay fixed. Trial zero uses the current preset;
training seeds are 101/102 and independent validation seeds are 301/302.
`--dials` confines an experiment to the named parameters; every other resolved
value, including tempo and swing, remains fixed.

Select a Pareto candidate whose two training objectives are no worse than trial
zero; prioritize reward among those candidates. Acceptance requires both validation
means to be no worse than current and at least one to improve. Every trial retains
its effective dial settings, editable projects, full renders, window positions,
PCM hashes and scores under distinct filenames. Results save after each preset.
An improvement on training alone is insufficient to update `preset.rs`.

The [October 2026 calibration](learned-music-tuning.md) records the shipped dials,
writer changes, fresh paired measurements and rejected candidates.

For writer changes, archive the original renderer, run the symbolic ruler and
`music.py` before editing, change one musical rule, then compare matching seeds
through the same instruments and render settings. Retain failed candidates and
regressions. Use fresh seeds for a final generalization check after model-guided
selection, since validation seeds used repeatedly become training evidence.

These models provide a measurable optimization objective, not a substitute for
listening. Check timbral identity, score diversity, note bounds, and per-case
regressions along with mean reward. Optimizing one reward can reward a repeated
texture or a specific renderer artifact instead of better musical development.

## Verify the measuring tools

```sh
uv run --python 3.11 --with pytest --with numpy --with scipy --with soundfile --with optuna pytest tools/eval/test_music.py tools/eval/test_render_audio.py tools/eval/test_tune.py
uv run --with ruff ruff check tools/eval/music.py tools/eval/learned_models.py tools/eval/render_audio.py tools/eval/tune.py
```

The contract tests require no model download. Real-model comparisons should use
the pinned evaluator environment and record all retained artifacts.
