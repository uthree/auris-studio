# Learned music calibration, October 2026

The evaluator uses the pinned TuneJury and MuQ-MuLan protocol in
[evaluation.md](evaluation.md). This experiment measured all nine presets before
changing the composer and retained both successful and rejected candidates.
[The measurement artifact](data/learned-music-tuning-2026-10-04.json) contains model
and package versions, prompts, checkpoint hashes, renderer hashes, trial scores,
WAV hashes, window positions, symbolic measurements and adoption decisions.

## Method

The baseline renderer was built from `354af0f2724ae9c75b7e1d87500ed10fa9abde32`.
The initial baseline used each preset's default seed plus 101/102 and three
ten-second windows. All renders use float32 WAVs without an added effect tail,
and neither evaluator applies gain normalization.

The broad dial search ran eight Optuna trials per preset, including the original
setting, on seeds 101/102. Selection required nondecreasing TuneJury reward and
MuQ-MuLan positive cosine, followed by separate validation on 301/302. These
exploratory passes used one centered window. Key, progression, form and instrument
roster stayed fixed. The symbolic ruler was run before and after writer changes.

The writer experiment tested subdivision ranges 2 and 4 against the original 3.
Range 2 failed the existing melodic repetition guard and provided no useful
joint score improvement. Range 4 improved the chiptune cohort; broader changes
to rock and synthwave did not survive validation. The final writer therefore
changes only the Chiptune palette, reaches faster preparations earlier on the
density dial, and caps subdivisions at four per beat. Held arrivals and phrase
breaths remain part of the gesture.

A second search froze all dials except humanize and dynamics, with 12 trials each
for chiptune and game-loop under the calibrated writer. Both candidates passed
the separate centered-window validation. The resulting shipped settings are:

| Preset | Humanize | Dynamics |
| --- | ---: | ---: |
| chiptune | 0.384 | 0.829 |
| game-loop | 0.668 | 0.407 |

## Fresh comparison

Seeds 401/402 were used only after candidate selection. The final comparison
returns to **three** windows: first, middle and last. Each row below is a paired
mean change over these two seeds; cosine is an embedding similarity, not a
probability, and TuneJury reward is uncalibrated.

| Preset | TuneJury reward delta | MuQ-MuLan positive cosine delta |
| --- | ---: | ---: |
| chiptune | −0.002384 | +0.004446 |
| game-loop | +0.134973 | +0.008355 |
| Mean of updated presets | +0.066295 | +0.006401 |
| Mean of all nine presets | +0.014732 | +0.001422 |

The chiptune reward reduction is retained explicitly as a tradeoff in the shared
palette calibration; the fresh comparison does **not** show a reward gain for
that preset. The aggregate improves both objectives, predominantly through
game-loop. Two fresh seeds do not establish statistical significance or listener
preference, and individual windows and seeds can regress.

The broad jazz candidate passed centered validation but lost reward on both
fresh seeds under the three-window protocol: mean reward −0.040662 and positive
cosine +0.025173. Its preset changes were rejected and the original restored.
The other broad dial candidates failed joint validation or retained trial zero.

The final CLI re-rendered all 18 fresh cases. The four updated files matched
their already scored WAV hashes exactly. Pop-band, city-pop, rock, jazz-trio,
orchestral, synthwave and ambient matched the baseline WAVs byte for byte on both
seeds. Their paired changes are therefore zero; absolute fresh model scores are
omitted for these identical pairs rather than inferred again. Rejected jazz
renders and their editable projects remain separate from the final cases.

## Repeating the experiment

Archive the baseline CLI and its matching `resolved_dials` example before edits.
Keep a second CLI with only the writer calibration for the focused search:

```sh
uv run tools/eval/music.py --preset all --seeds 3 --cli baseline/auris --workdir baseline/renders --json baseline/music.json
uv run tools/eval/tune.py --preset all --trials 8 --segments 1 --cli baseline/auris --dials-resolver baseline/resolved_dials --workdir broad --out broad.json
uv run tools/eval/tune.py --preset chiptune --preset game-loop --dials humanize dynamics --trials 12 --segments 1 --cli writer/auris --dials-resolver baseline/resolved_dials --workdir focused --out focused.json
```

For fresh comparison, compose each archived renderer with explicit seeds 401 and
402, render with `--bit-depth 32 --no-tail`, then score the corresponding folders
with `music.py --segments 3`. Filenames such as `game-loop-s401.wav` select the
frozen prompt profile. Use `--baseline` to compare reports made under identical
model and preprocessing conditions. Local artifact paths in the JSON are relative
to the repository root; WAVs, projects and executables remain in `target/tuning`.
