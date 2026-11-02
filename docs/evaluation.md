# Evaluating what the composer writes

[日本語](evaluation.ja.md)

Development measurements, none part of any release build. They exist for the same reason
every level and timing constant in this workspace was calibrated by rendering and measuring:
a change to a writer or a dial should be judged against numbers first and ears always.

The [agent composition trials](composition-quality.md) exercise complete panel tasks,
preserve failures and provenance, and prepare blinded human listening comparisons.

The [local refinement loop](composition-refinement.md) connects those trials to
SongEval, bounded foreground edits and retained before/after audio. It records
per-axis evidence and validity checks without an automatic musical winner.

The [preset arrangement study](preset-arrangement.md) pairs these measurements with track
participation and density across the form, and records the default-seed selection, fresh-seed
check, and per-preset tradeoffs from the 2026-10-04 revision.

The [physical-instrument copy-synthesis loop](physical-copy-synthesis.md) measures temporal
mel-spectrogram distance against a frozen cohort of real piano, guitar and violin recordings.
It fits only training notes and evaluates held-out pitches and dynamics with the actual Rust
renderer. This acoustic distance complements the symbolic and learned song measurements below.
The [grand-piano calibration](grand-piano-calibration.md) fits three-second Steinway notes
and checks held-out pitches/dynamics at one, three and six seconds, at 24/48 kHz.
The [whole-keyboard piano voicing](grand-piano-register.md) measures per-partial two-stage decay,
register level and soundboard knock from C1 to C8, and checks itself on that cohort's held-out notes.
The [low-register correction](physical-low-register.md) measures bow-force continuity,
bass-note periodicity and piano partial stretch, with paired audition files.
The [piano stretch tuning](piano-stretch-tuning.md) measures each key's fundamental, heard
pitch and octave beats against equal temperament.
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

## Native instrument upper-midrange voicing

The 2026-10-19 upper-midrange study follows the
[snare and ride correction](physical-pack.md#targeted-snare-and-ride-voicing).
It screens all sixteen native instrument families, additional pulse/saw/FM lead patches,
unmodulated controls and the production pop/rock guitar-amp chains. The worker uses the
actual instrument implementation at 44.1/48/96 kHz and velocities 0.35/0.65/1: 2,295 paired
conditions, with 1.2 seconds held in each 1.8-second capture.

Piano, acoustic guitar, electric guitar, bowed strings, clarinet and pulse/saw leads receive
a Q=0.9 section at 3.2 kHz. Maximum cuts are respectively 3.5/2/3/3/2/3 dB. This runs per
voice before its amplitude envelope; the acoustic models' feedback and modal frequencies
are unchanged. The wet amount falls linearly from full at a 1 kHz fundamental to zero at
2 kHz, with 3 ms smoothing for bends, glides and timbre changes. FM applies up to 3 dB while
its modulation envelope is active; an unmodulated sine is unchanged. The preview voice
reduces its 2.6 kHz formant by up to 20% with the same fundamental protection, retaining
the two lower formants. This does not change the physical choir's vocal tract.

The following medians describe the paired probe windows whose fundamental is below 1.4 kHz
and whose original 2–5 kHz power exceeds 1% of audible power. Attack is 0–150 ms; sustain is
150–1200 ms. The values are absolute band-power deltas, without gain normalization.

| Sound | Attack 2–5 kHz | Sustain 2–5 kHz |
| --- | ---: | ---: |
| Piano | -2.15 dB | -2.20 dB |
| Acoustic guitar | -1.21 dB | -1.19 dB |
| Electric guitar DI | -1.84 dB | -1.81 dB |
| Bowed strings | -2.43 dB | -2.47 dB |
| Clarinet | -1.65 dB | -1.65 dB |
| Pulse lead, width 0.125 | -2.21 dB | -2.21 dB |
| Saw lead | -2.25 dB | -2.25 dB |
| Factory FM | -2.11 dB | — |
| Preview voice | -1.36 dB | -1.37 dB |
| Pop amplified guitar | -1.80 dB | -1.81 dB |
| Rock amplified guitar | -1.19 dB | -1.55 dB |

The report also retains deltas after audible-power matching, to distinguish spectral
changes from overall attenuation. Hann-periodogram normalization excludes frequencies
below 30 Hz, so a narrow pulse's DC offset cannot hide its upper harmonics. All tested
notes at MIDI 96 and above match the baseline sample for sample, including the amp chains.
The bass, bell, mallet, dulcimer, whistle, choir, complete drum kit and legacy noise drum
also remain identical, as do sine/triangle and zero-index FM controls.

`tools/eval/references/instrument-harshness.json` retains the cohort, hashes and results.
The symbolic ruler is unchanged. A frozen solo pulse-lead melody through `music.py` has
TuneJury delta -0.0278 and MuQ-MuLan positive-cosine delta -0.000028 under matching
conditions. These coarse diagnostics do not establish improved musical preference or
perceived timbre. Listening files compare common gain and a separately labeled AC-RMS-matched
candidate, without limiting. Keep the original worker before rebuilding the changed DSP:

```sh
uv run tools/eval/instrument_harshness.py <before-worker> <after-worker> target/instrument-harshness/fix
uv run --with pytest --with numpy --with scipy --with soundfile pytest tools/eval/test_instrument_harshness.py
uv run --with ruff ruff check tools/eval/instrument_harshness.py tools/eval/test_instrument_harshness.py
```

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

The [2026-10-26 local evaluator trial](local-music-evaluators-2026-10-26.ja.md)
ran SongEval and Meta Audiobox Aesthetics on four fixed-context vocal mixes,
with repeated-input and silence controls. Both ran on Windows CPU; neither is
shipped in the application or calibrated as an automatic selector. The retained
[results](../tools/eval/references/local-music-evaluators-2026-10-26.json) distinguish
successful local inference from the Antigravity CLI attempt, which did not
deliver raw audio and produced no valid music assessment.

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
