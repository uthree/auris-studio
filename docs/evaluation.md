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

The [physical-instrument measurements](physical-copy-synthesis.md) compare the piano, guitar,
strings, reeds, whistles and hammered strings with frozen cohorts of real recordings, through
the actual Rust renderer: temporal mel-spectrogram distance on held-out pitches and dynamics,
and per-partial decay, register level and soundboard knock across a Steinway's keyboard.
This acoustic distance complements the symbolic and learned song measurements below.
The drum kit, the guitar amplifier and the choir are fitted to recordings as well, each on its
training notes only and evaluated on held-out ones.
The [drum-kit calibration](physical-pack.md) fits seven kit families to 151 real drum hits,
with separate attack/body/tail loss and acoustic-role guards for every snare/tom register.
The [guitar-amplifier experiment](electric-guitar.md) adds an oversampled amplifier and a
paired clean/pedal recording benchmark with pitch-grouped splits.
The [choir calibration](choir-copy-synthesis.md) fits the native vocal-tract ensemble to
real sustained vowels, with 24 training notes and 80 notes from separate held-out singers.
The [piano stretch tuning](piano-stretch-tuning.md) is how the piano's fundamentals leave
equal temperament: the rule; the fundamentals, heard pitch, octaves and string stiffness
measured at thirteen keys beside the same reading of a recorded Steinway; and the tests that
hold them.

## Native instrument upper-midrange voicing

The 2026-10-19 upper-midrange study follows the
[snare and ride correction](physical-pack.md#targeted-snare-and-ride-voicing).
It screens the native instrument families, additional pulse/saw/FM lead patches and
unmodulated controls. The worker uses the actual instrument implementation at 44.1/48/96 kHz
and velocities 0.35/0.65/1, with 1.2 seconds held in each 1.8-second capture.

Pulse and saw leads receive a Q=0.9 section at 3.2 kHz, cutting at most 3 dB. This runs per
voice before its amplitude envelope. The wet amount falls linearly from full at a 1 kHz
fundamental to zero at 2 kHz, with 3 ms smoothing for bends, glides and timbre changes. FM
applies up to 3 dB while its modulation envelope is active; an unmodulated sine is unchanged.
The preview voice reduces its 2.6 kHz formant by up to 20% with the same fundamental
protection, retaining the two lower formants. This does not change the physical choir's vocal
tract.

The following medians describe the paired probe windows whose fundamental is below 1.4 kHz
and whose original 2–5 kHz power exceeds 1% of audible power. Attack is 0–150 ms; sustain is
150–1200 ms. The values are absolute band-power deltas, without gain normalization.

| Sound | Attack 2–5 kHz | Sustain 2–5 kHz |
| --- | ---: | ---: |
| Pulse lead, width 0.125 | -2.21 dB | -2.21 dB |
| Saw lead | -2.25 dB | -2.25 dB |
| Factory FM | -2.11 dB | — |
| Preview voice | -1.36 dB | -1.37 dB |

The report also retains deltas after audible-power matching, to distinguish spectral
changes from overall attenuation. Hann-periodogram normalization excludes frequencies
below 30 Hz, so a narrow pulse's DC offset cannot hide its upper harmonics. All tested
notes at MIDI 96 and above match the baseline sample for sample. The choir, complete drum kit
and legacy noise drum also remain identical, as do sine/triangle and zero-index FM controls.

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

### Line form, entries and predictability

```
uv run tools/eval/melody_form.py MySong/MySong.auris --json form.json
uv run tools/eval/melody_form.py runs/ --track Vocal
```

Cuts the foreground (the first singer track, or `--track`) into lines at the lines of the song-wide
lyric annotation, at clip boundaries and at rests of a beat or more; a looped clip is read once
for every pass. A line still longer than
four bars is cut again at its longest rest of half a beat or more, so a verse breathed in eighth
rests under one annotation is not counted as one line. A line belongs to the section most of its
notes start in, which keeps a line entering with a pickup in its own section. Each line gets a letter
in its section's form (the same onsets and pitches share a letter; the same onsets under other
pitches add a prime), the beat it enters on and whether that is a pickup, its syncopated and
anticipated notes, the spacing of its onsets, the scale degree and chord it ends on, Huron's
contour type, and an IDyOM-style short-term information content: a PPM model over (interval,
inter-onset interval) symbols that learns only from the song so far, so an exact repeat falls
towards zero bits a note. Sections add their chord loop and main drum bar, and strummed tracks
report the spread their strum stage gives each chord.

Flags mark extremes only: every line of a section identical, nine lines in ten entering on one
beat, no line entering in the second half of a bar, no syncopated note at all, every line one note count, one onset spacing for most notes, no line
ending on the tonic, several lines in a row predictable from the song so far, three sections
sharing one drum bar, a chorus looping the verse's progression from another chord, and a guitar
strummed over less than 10 ms (below every shipped preset) or not at all. The song whose
friends' feedback prompted the script raises all of them
([plan](song-feedback-improvement-plan-2026-11-09.ja.md)). None is a target, an absent flag is
not a judgement, and the model knows nothing about a genre's conventions.

### How conventional a melody is against a corpus

```
uv run tools/eval/melody_corpus.py train corpus/ --out model.json --name "corpus name"
uv run tools/eval/melody_corpus.py score MySong/MySong.auris --model model.json
uv run tools/eval/melody_corpus.py lines corpus/
```

The long-term half of the same IDyOM-style model. `train` counts a PPM model over the same
(interval, inter-onset interval) symbols from a folder of MusicXML or MIDI melodies, and
cross-validates the corpus in five folds. `score` gives each line of a song its information
content under the frozen model, and the song's mean as a z-score against the corpus's own
cross-validated songs. Low means the melody is made of the corpus's commonplaces; it is never a
target in either direction.

Two corpora were tried. PJS (100 short songs written for phoneme balance, CC BY-SA 4.0) is
Japanese but nothing like a fast rock song, and every trial song scored above it. 620 lead
sheets with lyrics tagged pop or rock from PDMX's conflict-free, deduplicated subset (CC0 and
public domain; 189,217 notes; cross-validated 5.57 ± 1.65 bits a note) are the better reference:
fourteen trial songs fell between -0.6 and +1.3 sd of it. The PDMX melody is the first non-drum
track's top line, which is a piano's right hand where a score has no lead part. Both models are
kept on the machine that trained them; a model inherits its corpus's licence.

`lines` cuts each MIDI file's sung part into lines by `melody_form.py`'s rule and reports where
a corpus's lines enter the bar; the two entry warnings take their thresholds from it. It counts
the files whose first melodic track is named as a voice (`--any-part` counts the rest too) and
that hold eight lines or more, which is what the warnings need. In the 282 such PDMX songs:

| Per song | Median | 90th percentile | 95th percentile | Songs at the warning |
| --- | --- | --- | --- | --- |
| Share of lines at the commonest entry point | 45% | 75% | 87% | 3.2% reach 90% |
| Share of lines entering in the second half of a bar | 44% | 82% | 91% | 4.6% have none |
| Share of onset spacings taken by the commonest one | 52% | 75% | 82% | 10% reach 75% |

Each entry warning therefore fires on one corpus song in twenty to thirty. The note-value
warning sits at the 90th percentile and fires on one in ten: at the 95th, four songs composed in
a row all finished at 76-78%, just under it. With `--any-part`, 518 songs give 3.9%, 5.8% and
11% for the three rows.

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

Whole songs have two more scorers beside SongEval. `audiobox.py` runs Meta Audiobox
Aesthetics (CE, CU, PC and PQ, 1-10). `song_aesthetics.py` runs the multi-stem song aesthetics
model of Lv et al. (Overall, Singing, Melody, Arrange and AudioQ, 0-100). Each song is separated
with Demucs `htdemucs`, cut into 10-second windows every 5 seconds, and embedded with MuQ. Windows
with a silent vocal or a silent accompaniment are skipped. Both write the same report format as
`songeval.py`. Against a set of commercial songs, only the second model kept rendered songs apart:
every rendered vocal song scored below the lowest commercial one. That held at the commercial
songs' own loudness, which is about six decibels above a -14 LUFS render (see below). Audiobox PQ
rates clean renders above commercial masters. On an instrumental the separator hears the lead as
a voice, so the multi-stem model's score there means nothing.

### What the learned scorers were trained on

None of these models was trained on Japanese songs, so none of them can say whether a Japanese
lyric is natural, how well its prosody fits, or how its moras are laid on the notes. Those
questions are for the symbolic checks: `melody_shape`, the lyric fit and the score linter.

| Scorer | Training data | Japanese |
| --- | --- | --- |
| SongEval | 2,399 songs, English and Chinese, mostly AI-generated | none |
| Multi-stem SAE (Lv et al.) | the SongEval set, plus 1,569 Chinese songs, mostly made by people, scored on its five axes by four experts | none |
| Audiobox Aesthetics | 97k clips of speech, sound and music in roughly equal shares; the music sources are not named, and the speech is mostly English | not stated |
| TuneJury | 17.5k preference pairs from AIME, SongEval, MusicPrefs and Music Arena | not stated |
| MuQ `MuQ-large-msd-iter`, the encoder SongEval and SAE embed with | the Million Song Dataset | no breakdown published |

Use them on a Japanese vocal song this way:

* SongEval catches only gross audio regressions there.
* SAE's Arrange and AudioQ axes read the accompaniment and the mix. Its Singing and Melody axes
  carry the four experts' taste for Chinese pop.
* On an instrumental, SAE has nothing to score, so SongEval is the main learned reference, read
  alongside Audiobox.

### Render the same way before comparing

* **Sample rate.** The workshop's `audition.wav` is a 22.05 kHz preview. On 「限界線を蹴り破れ」
  and on the 「限界線を蹴り飛ばせ」 candidate, auditions scored about three points lower on SAE
  Overall than 48 kHz renders of the same projects, and five to six lower on Singing and AudioQ. It also narrowed the gap between two songs: 3.4 points of
  Arrange between two auditions became 1.0 between their 48 kHz renders.
  * Score `auris render` output, never auditions.
* **Loudness.** SAE rises with loudness.
  * Raising the master limiter's input by 4 dB, from -14.0 to -10.7 LUFS, added 2.0 to 2.6
    points of Overall and about 2 of AudioQ on both songs tried.
  * Gain-matching the commercial set from about -8 LUFS down to -14 LUFS took 2.2 points off
    its Overall mean.
  * Compare at matched integrated loudness, and normalise reference songs to the renders'
    target rather than scoring their masters as they are.
* **What else moves it.** At matched loudness, mix and performance changes left SAE where it
  was. Each was tried on three songs and moved Melody, Arrange and Singing by under half a point:
  limiting harder and turning the result down, a glue compressor on the master, brighter
  guitars, a vocal delay, drums and bass up 2 dB, and vibrato on every note of half a second or
  more. Overall wandered by up to two points, and in both directions on the same change. Singing
  the take again with the same seed moved Singing by 0.1 at most and AudioQ by 0.4. Read SAE as telling songs
  apart. It cannot steer a mix.
* **What it does not see.** SAE scores windows of the song and averages them, so it cannot tell
  whether the song is finished. A composer given only the user's request and the tools
  (`AURIS_TRIAL_GUIDANCE=bare`) delivered four 21–43 second pieces of four tracks, with no
  effects. Gain-matched to -14 LUFS, they averaged 73.4 Overall and 81.1 Arrange. Two finished
  three-minute songs on the same request averaged 70.4 and 81.0. Audiobox did put the finished
  songs higher: PQ 7.2 against 6.8, CE 6.7 against 5.9. Check length, form and the mix before
  reading any learned score.

`songeval.py` and `song_aesthetics.py` take `--device cuda:0`, which a ROCm build of PyTorch also
answers. Audiobox takes the GPU whenever one is visible. On a GPU, scores match the CPU's to
0.001. Under ROCm 7.14 on gfx1201 the memory-efficient and flash attention kernels fail, so the
adapters turn both off. `song_aesthetics.py --precision bf16` is about 1.6 times faster but moves
scores by up to 0.8; fp16 turns MuQ's output into NaN.

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
