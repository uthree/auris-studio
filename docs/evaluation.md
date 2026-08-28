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

## What the numbers are for

They are a regression detector and a coarse sieve, not a target. Two findings from the
literature are load-bearing here:

* Optimising a generator against a learned aesthetic score collapses its output diversity
  (SMART, arXiv:2504.16839). Nothing in this repository feeds these scores back into the
  composer, and nothing should without a diversity guard beside it.
* Objective metrics correlate weakly with human judgement across the board (survey,
  arXiv:2509.00051). Read several numbers together, never one alone, and let a pair of ears
  break every tie.
