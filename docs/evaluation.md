# Evaluating what the composer writes

Development measurements, none part of any release build. They exist for the same reason
every level and timing constant in this workspace was calibrated by rendering and measuring:
a change to a writer or a dial should be judged against numbers first and ears always.

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

```
uv run tools/eval/aesthetics.py --preset all --json before.json
# ...change something...
uv run tools/eval/aesthetics.py --preset all --baseline before.json
```

Runs Meta's Audiobox Aesthetics model (arXiv:2502.05139) over rendered audio — either WAVs
you point it at, or presets it composes and renders through `auris-cli` first. Four axes,
each 1–10, predicted from human ratings: **CE** content enjoyment, **CU** content usefulness,
**PC** production complexity (neither end is "better"), **PQ** production quality. Python is
managed entirely by `uv` through the script's inline metadata — there is no environment to
set up, and the first run downloads the model checkpoint into the Hugging Face cache.

Use `--cli path/to/auris` (or `auris.exe` on Windows) to render with an already-built
executable. This lets a baseline use an archived build while the working tree changes.
Scoring existing WAV directories needs no Rust build:

```
uv run tools/eval/aesthetics.py target/before --json before-aesthetics.json
uv run tools/eval/aesthetics.py target/after --baseline before-aesthetics.json --json after-aesthetics.json
```

The intended workflow is the baseline diff shown above: score before, change one thing,
score after, and treat any movement — up or down — as a reason to listen to the renders it
came from.

## Audio/text identity with LAION CLAP

```
uv run tools/eval/clap.py target/before --json before-clap.json
uv run tools/eval/clap.py target/after --baseline before-clap.json --json after-clap.json
```

This runs the [official LAION CLAP Python implementation](https://github.com/LAION-AI/CLAP)
on final WAVs, using its music + AudioSet `HTSAT-base` checkpoint with fusion disabled.
The public checkpoint is about 2.35 GB; it and the text-model dependencies are downloaded
to `target/composition-eval/models` by default. Audio stays local. The default device is
CPU; `--threads` defaults to four, and `--device cuda` selects a supported CUDA device.

The versioned `tools/eval/clap_prompts.json` contains two positive descriptions and two
contrasting descriptions per preset. Freeze these descriptions before examining A/B
scores. Filenames such as `jazz-trio.wav` or `jazz-trio-s101.wav` select that profile;
`--preset jazz-trio` explicitly selects it for other filenames. The prompts describe
style and instrumentation, not whether music is good or professionally composed.

The JSON retains each prompt's cosine in positive-then-contrast manifest order, each
excerpt's measurements, and the equal-weight mean across valid excerpts:

* **positive_cosine** is mean cosine similarity to the two intended descriptions.
* **contrast_cosine** is mean cosine similarity to the contrasting descriptions.
* **contrast_margin** is positive cosine minus contrast cosine. It is not a probability
  or an aesthetic score. Changing contrast prompts changes its meaning.

The default excerpts are the first, middle and last complete ten-second windows.
`--segments N` uniformly spaces more windows; one window uses the center. Channels are
averaged, then resampled to 48 kHz with a polyphase filter. Short files repeat complete
copies and zero-pad the remainder. Native CLAP clips samples to [-1, 1] and quantizes
to int16; the report records peaks and samples outside that range. There is no loudness
normalization. Silent windows, including audio quantized to silence, and nonfinite input
or embeddings are reported explicitly. Invalid windows never become artificial zero
similarities, and a run containing them exits unsuccessfully after saving its report.

Reports include checkpoint revision and verified SHA-256, pinned tokenizer revision and
hashes, package versions, preprocessing settings, prompts and WAV hashes. Baseline
comparison rejects incompatible measurement conditions and reports paired file deltas.
Compare the same preset and seed with the same SoundFont, render settings and audio
length; a report also flags changed excerpt positions. Keep per-preset paired results
alongside any overall mean, since improvements and regressions can cancel out.

CLAP samples local timbral and semantic identity. Three excerpts do not establish
full-song structure, memorable motifs, groove quality, or listening preference. Use it
alongside the symbolic ruler and Audiobox, then listen to the actual audio. An unchanged
file can be scored a second time against its baseline to check inference repeatability.

The evaluator's preprocessing and comparison tests need no model download:

```
uv run --with pytest --with numpy --with scipy --with soundfile pytest tools/eval/test_clap.py tools/eval/test_aesthetics.py
uv run --with ruff ruff check tools/eval/clap.py tools/eval/test_clap.py tools/eval/aesthetics.py
```

## The black-box tuner

```
uv run tools/eval/tune.py --preset all --trials 18 --out tune-results.json
```

Optuna's TPE searches a preset's *continuous* dials — humanize, dynamics, fill, variation,
the four mood numbers, tempo within ±6 %, and swing only where the preset already swings —
against Content Enjoyment averaged over two fixed seeds. Key, groove, progression, form and
roster never move: the search refines a genre, it does not escape one. The preset's own
dials are always trial zero, and the number to trust is the **held-out validation** printed
at the end: best-found versus current, on two seeds the search never saw. A candidate that
wins in search and loses there has learned the seeds, not the music.

The tool is a lead generator, not a judge. It does not edit `preset.rs`; adopting a winner
means listening to the renders it leaves in its workdir first, then changing the preset by
hand with the reason written down.

## What the numbers are for

They are a regression detector and a coarse sieve, not a target. Two findings from the
literature are load-bearing here:

* Optimising a generator against a learned aesthetic score collapses its output diversity
  (SMART, arXiv:2504.16839). Nothing in this repository feeds these scores back into the
  composer, and nothing should without a diversity guard beside it.
* Objective metrics correlate weakly with human judgement across the board (survey,
  arXiv:2509.00051). Read several numbers together, never one alone, and let a pair of ears
  break every tie.

## The singer's ruler

```
cd training && uv run python scripts/evaluate_host.py --voice voice.onnx \
    --checkpoint last.ckpt --data data/processed/jsut_song --json before.json
```

The same discipline pointed at the singing voices. A voice is trained and verified in
`training/`, in Python, and sung by `auris-singer`, in Rust, and the second is the one a
person hears: it chunks a long timeline, arranges frames into tokens, scales the energy and
runs its own copy of the runtime, none of which the training log sees. The script sings a
corpus's own curves through `auris sing-frames` — the frames-in door of the same session the
window uses — beside PyTorch singing the same curves, and beside the whole set sung as one
song so the chunking is in the picture; a second mode composes from lyrics and sings through
`auris sing`, the path a person walks. The metrics are the trainer's own, so a number in the
table means what `val/…` means in the training log. `training/doc/evaluation.md` is the
account, baseline diff and all.
