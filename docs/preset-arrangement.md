# Preset arrangement development

This note records the default-seed arrangement shape case measured on 2026-10-04. It describes how parts enter, leave, and thin out across the form; it is not a total quality score. The machine-readable record is [preset-arrangement-structure-2026-10-04.json](data/preset-arrangement-structure-2026-10-04.json).

The recorded measurements use the branch's General MIDI configuration before integration with the native physical-instrument presets. The integrated presets retain these section selections and note-writing changes, with every added part assigned to an internal physical instrument. The audio scores below describe the recorded SoundFont renders; the integrated native configuration has not been scored again.

The comparison uses the frozen `arrangement_measure` example. A track sounds in a bar when a written note duration overlaps that bar. An onset-free bar has no note start but may still be sounding because a note is held across its boundary; a silent bar has no sounding track. Section density is the arithmetic mean of the bar-level note onset counts. Drum voices share one instrument track, so `tracks` counts the kit once. Both runs use each preset's declared seed and no explicit `--seed` override. The baseline is `target/evaluation-baseline/arrangement.json`; the final candidate shape case is `target/evaluation-final/arrangement.json`, with symbolic data beside it. Their SHA256 values, binary and SoundFont provenance, frozen example hash, and baseline commit are recorded in the JSON artifact.

## What changed

The presets now select participating parts per section. Intro and verse passages are reduced, later sections bring in alternate leads or supporting textures, and outros retire instruments. The arrangement pass reads the union of all playing melody parts, so accompaniment makes room around overlapping foreground voices. A held coda inherits the final section's explicit part selection; an unrestricted final section still lets the full roster land. Lyric sections continue to bypass this offline pass so singer notes remain lyric-conditioned.

The measured shape changes are summarized below. `tracks` is the number of composed instrument tracks, `sections` and `bars` cover non-coda material, and `mean onsets/bar` is averaged across all non-coda bars. `peak/quiet` gives the largest onset count and the smallest nonzero onset count in a bar. `changed joins` counts section transitions with a non-empty added/removed participation set.

| preset | before tracks | after tracks | before mean | after mean | before peak/quiet | after peak/quiet | changed joins after |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| chiptune | 4 | 7 | 34.27 | 38.21 | 42 / 28 | 51 / 29 | 5 |
| game-loop | 4 | 6 | 31.12 | 35.00 | 34 / 27 | 38 / 31 | 1 |
| pop-band | 6 | 11 | 27.86 | 26.31 | 35 / 15 | 47 / 15 | 9 |
| city-pop | 6 | 10 | 51.17 | 52.17 | 76 / 39 | 76 / 36 | 5 |
| rock | 5 | 9 | 33.46 | 37.04 | 44 / 28 | 56 / 28 | 5 |
| jazz-trio | 4 | 4 | 25.81 | 25.81 | 34 / 21 | 34 / 21 | 1 |
| orchestral | 7 | 13 | 16.94 | 19.81 | 25 / 2 | 45 / 8 | 5 |
| synthwave | 6 | 11 | 25.38 | 27.33 | 33 / 9 | 41 / 9 | 5 |
| ambient | 4 | 4 | 5.90 | 5.50 | 12 / 2 | 12 / 2 | 3 |

The detailed per-section counts and mean onset densities are in the artifact. The full per-bar reports also list added and removed tracks at each join. `game-loop` remains a 16-bar verse/chorus loop. `jazz-trio` keeps its piano, bass and brushes, and `ambient` retains eight onset-free bars with no silent bars under the duration-based definition.

Chiptune adds an FM arpeggio and rotating chord voices; game-loop adds an alternate chord layer and an arpeggio. Pop-band adds a guitar lead, piano and guitar accompaniment, vibraphone and synth bass. City-pop adds trumpet stabs, electric grand piano, guitar and vibraphone arpeggios. Rock adds an alternate guitar phrase, clean guitar, piano and arpeggios. Orchestral adds oboe, clarinet, solo violin, trumpet, pizzicato and celesta. Synthwave adds an alternate lead, chord/pluck layers, a dark pad and sub bass.

Ambient varies participation within its existing roster: verse2 uses pad, glass and cello, and its coda uses pad. Jazz retains its original four-track trio behavior after the proposed change failed the fresh-seed check. Pop-band and ambient deliberately retire instruments at the end; the other presets retain their core melody and rhythm through the final landing.

## Learned measurements and selection

The [learned evaluator](evaluation.md#the-learned-ear) supplies two different signals: TuneJury's uncalibrated preference reward and MuQ-MuLan's agreement with fixed preset descriptions. MuQ's positive cosine measures identity; the contrast margin subtracts the mean contrast cosine. These values are not percentages or a general musical quality scale. The complete paired records, excerpt scores, model and input hashes are in [preset-arrangement-learning-2026-10-04.json](data/preset-arrangement-learning-2026-10-04.json).

The baseline is commit `9e2c5adee5ed1cc44c7992ce127698ee6f161d04`. Both sides use the same MuseScore General SoundFont, 48 kHz stereo float32 renders without tails, and the evaluator's fixed three ten-second windows spanning the first to last complete window. Scoring uses mono 24 kHz PCM, unchanged prompts and pinned checkpoints, CPU inference with four threads, and equal weighting of valid windows. All 27 final before/after pairs have three valid windows at matching positions. Renders that substituted instruments because the SoundFont was inaccessible were excluded. The final default WAVs were regenerated and checked by SHA256 against their scored sources; restored jazz WAVs match the baseline byte for byte.

Default seeds guided candidate selection. An expanded arrangement with reduced endings lowered both cohort means, so the final candidate keeps the characteristic melody and rhythm at most final landings and changes supporting parts within the song. The symbolic ruler was run before and after: chiptune and game-loop retain their lead and percussion measurements, while pop-band and synthwave's reduced sections lower the mean snare syncopation. Added leads are measured separately rather than treated as replacements for the old lead's metric row.

Seeds 501 and 502 were then scored without tuning the eight revised presets against them. Before the jazz rollback, this prospective check averaged **+0.035782 reward, +0.001648 positive cosine and +0.000897 contrast margin** across nine presets. Jazz's candidate lowered both reward and positive cosine on both fresh seeds, so its original performance was restored. The final fresh-seed results below include that adaptive rollback; they are not an untouched validation cohort. No further independent cohort or listener study was run.

Every value below is `after - before`. The default column uses one declared seed per preset; the fresh column averages seeds 501 and 502. The final row gives each preset equal weight.

| preset | default reward | default MuQ positive | default margin | fresh reward | fresh MuQ positive | fresh margin |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| chiptune | +0.145990 | +0.005260 | +0.007890 | +0.045032 | -0.007025 | -0.001233 |
| game-loop | +0.024273 | +0.008686 | +0.007240 | -0.113469 | +0.006194 | +0.017823 |
| pop-band | -0.000855 | +0.009905 | -0.027369 | +0.163613 | +0.022945 | -0.002574 |
| city-pop | -0.111352 | +0.015462 | +0.024304 | +0.037420 | +0.030587 | +0.032121 |
| rock | +0.201025 | -0.015531 | -0.008650 | +0.149722 | -0.018120 | -0.022694 |
| jazz-trio | 0.000000 | 0.000000 | 0.000000 | 0.000000 | 0.000000 | 0.000000 |
| orchestral | +0.049415 | +0.008533 | +0.006012 | +0.093047 | -0.004059 | -0.001701 |
| synthwave | -0.022277 | +0.003118 | +0.005082 | -0.024347 | -0.005170 | +0.001904 |
| ambient | +0.123114 | +0.031433 | +0.014990 | +0.104515 | -0.005092 | +0.001415 |
| mean | +0.045481 | +0.007430 | +0.003278 | +0.050615 | +0.002251 | +0.002784 |

The cohort means improve under these measurements, with material differences by preset and seed. Game-loop loses fresh-seed reward while gaining identity agreement; rock gains reward while losing MuQ agreement. Chiptune, orchestral and ambient also lose some fresh-seed positive cosine. Synthwave's fresh reward and positive cosine both decline slightly; its rotating lead, pad and bass sections are retained for the requested arrangement contrast, with that tradeoff recorded rather than claimed as a score improvement. Two fresh seeds and sparse excerpts do not establish statistical significance, listener preference, or whether a whole song stays interesting.

## Reproducing the measurements

Run `cargo run -p auris-compose --example measure` for the eight-seed symbolic ruler and `cargo run -p auris-compose --example arrangement_measure` for the declared-seed arrangement report. The latter accepts `--seed 501` to inspect another take. Freeze separate baseline and candidate CLI binaries before rendering, and use the same installed SoundFont directory through `AURIS_SOUNDFONTS`.

For a fresh case, compose with `auris compose --preset pop-band --seed 501 -o pop-band-s501.auris`, then render the generated `pop-band-s501/pop-band-s501.auris` with `--bit-depth 32 --no-tail`. Repeat for the other presets and seed 502. Score both WAV directories with the commands in [evaluation.md](evaluation.md#the-learned-ear), passing the first report as `--baseline` to the second. The evaluator rejects changed measurement conditions before comparing matching filenames. The recorded scores reused inference only when WAV hashes matched the original scored audio; the artifact identifies those sources and the renderers used.
