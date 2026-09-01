# Evaluating what the composer writes

Two measuring instruments, neither part of any release build. Both exist for the same reason
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
set up, and the first run downloads the model checkpoint (~1 GB) into the Hugging Face cache.

The intended workflow is the baseline diff shown above: score before, change one thing,
score after, and treat any movement — up or down — as a reason to listen to the renders it
came from.

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
