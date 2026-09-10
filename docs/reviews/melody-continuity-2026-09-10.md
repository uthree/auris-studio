# Melody continuity — 2026-09-10

A busy melody could stop moving early in every bar: two sixteenths led immediately into a dotted-quarter target and an eighth rest. The identifying head preserved that placement, and phrase closure could stretch the same early note still further. The writer now allocates that approach against the following gesture in the whole bar. It redistributes existing attacks onto the meter's hierarchy and keeps their order, degrees, accents, initial pickup and note count. Late arrivals, sparse held openings and sustained palettes retain their freedom to hold. No random draw is added.

A closing answer that would fall back to a busy early preparation instead receives a later landing for its final degree. A half-beat anticipated landing near the final two beats remains valid. This closing branch can add a note; the approach allocator does not. Written patterns bypass the new allocator. Realized pitches can change when new timing meets harmonic or metric boundaries. All results remain ordinary editable notes in Auris projects.

## Paired experiment

The 11 cases were fixed before measuring the candidate. Diagnostics: Pop-band 102, Rock 105, City-pop 102. References: Pop-band 105 and Rock 107. Held-out: seeds 201 and 202 in all three genres. All cases are retained. The five previously heard complete projects/WAVs were copied with hashes verified; the six new baselines use the archived c6711de CLI. The new composer supplies only lead notes and their recipe digest to the old backing. Every candidate audio render uses the same old renderer, SoundFont, mixer and performance settings. Shared effects may respond to changed lead audio.

Open the local [11-pair listening report](../../target/melody-continuity/listening.html). It links the normalized excerpts and editable projects. First-chorus excerpts are eight bars, stereo 48 kHz float WAV, with 5 ms fades and linear gain only to -23 LUFS. Audiobox reads these same full excerpts; CLAP uses their centered ten seconds with frozen genre/instrument prompts (`--segments 1`). The model measurement therefore concerns the listening passage rather than earlier full-song scores. It is not a delayed hook-recall test or a whole-song quality evaluation.

## Written-score observations

The diagnostic counts a complete interval of at least two beats between attacks that starts in the first half and crosses beat three. It includes both held and resting time. Phrase-ending bars 4 and 8 are explicitly separate. This is a descriptive measurement, not a quality penalty.

| Case | Cohort | Chorus notes before / after | Non-ending early spans before / after |
| --- | --- | ---: | ---: |
| pop-band-s102 | diagnostic | 42 / 42 | 6 / 0 |
| pop-band-s105 | reference | 30 / 30 | 0 / 0 |
| rock-s105 | diagnostic | 38 / 38 | 0 / 0 |
| rock-s107 | reference | 58 / 58 | 0 / 0 |
| city-pop-s102 | diagnostic | 42 / 42 | 6 / 0 |
| rock-s201 | held-out | 57 / 57 | 0 / 0 |
| rock-s202 | held-out | 51 / 51 | 0 / 0 |
| city-pop-s201 | held-out | 41 / 41 | 0 / 0 |
| city-pop-s202 | held-out | 43 / 43 | 0 / 0 |
| pop-band-s201 | held-out | 43 / 43 | 0 / 0 |
| pop-band-s202 | held-out | 43 / 43 | 0 / 0 |

Pop-band 102 and City-pop 102 each change from six non-ending early spans to zero, while keeping 42 chorus notes. Pop-band 105 keeps its 30 notes and six late long notes; its normalized chorus PCM is exactly identical to the baseline. Seven cases retain their exact chorus note text even though another section of their full melody changes. The two diagnostic first-bar approaches move from sixteenth-grid onsets `[0,1,2,10,...]` to `[0,4,6,10,...]`: the central inter-onset interval becomes one beat instead of two. An intentional half-beat pickup rest remains. The held-out six had no qualifying early spans before the change, so this cohort does not establish generalization of that specific reduction; Rock 202 and Pop-band 202 do reduce their maximum complete within-bar interval from 1.5 to 1 beat.

## Learned audio measurements

CE predicts enjoyment, PQ production quality; CLAP measures prompt similarity. The algorithm was not selected by maximizing these values. Positive and negative deltas are both reasons to listen, and no new human preference result is claimed.

| Case | CE before → after | PQ before → after | CLAP before → after |
| --- | ---: | ---: | ---: |
| pop-band-s102 | 6.998 → 6.995 | 8.249 → 8.271 | 0.3196 → 0.3098 |
| pop-band-s105 | 6.820 → 6.820 | 8.112 → 8.112 | 0.3303 → 0.3303 |
| rock-s105 | 7.384 → 7.383 | 8.195 → 8.194 | 0.3585 → 0.3585 |
| rock-s107 | 7.410 → 7.410 | 8.125 → 8.125 | 0.3853 → 0.3853 |
| city-pop-s102 | 7.691 → 7.596 | 8.386 → 8.374 | 0.3366 → 0.3471 |
| rock-s201 | 7.357 → 7.357 | 8.113 → 8.113 | 0.3848 → 0.3848 |
| rock-s202 | 7.320 → 7.428 | 8.040 → 8.159 | 0.3559 → 0.3614 |
| city-pop-s201 | 7.768 → 7.768 | 8.411 → 8.411 | 0.3652 → 0.3652 |
| city-pop-s202 | 7.760 → 7.760 | 8.423 → 8.423 | 0.3346 → 0.3346 |
| pop-band-s201 | 6.997 → 6.997 | 8.248 → 8.248 | 0.2920 → 0.2920 |
| pop-band-s202 | 6.966 → 6.983 | 8.176 → 8.175 | 0.2971 → 0.3114 |

| Cohort | n | Mean CE delta | Mean PQ delta | Mean CLAP delta |
| --- | ---: | ---: | ---: | ---: |
| all | 11 | +0.0024 | +0.0115 | +0.0019 |
| diagnostic | 3 | -0.0330 | +0.0030 | +0.0002 |
| reference | 2 | +0.0000 | +0.0000 | +0.0000 |
| held-out | 6 | +0.0208 | +0.0197 | +0.0033 |

The mixed changes do not establish a perceptual improvement. In particular, removing the identified score pattern does not imply that an audio model must predict greater enjoyment. The companion JSON retains every pair, conditions, artifact hashes, first-bar timings, unchanged-chorus checks and model metadata.

## Reproduction and validation

```sh
uv run tools/eval/melody_continuity_ab.py baseline --out target/experiment/before --cli path/to/frozen-auris.exe --reference target/seed-diversity/manifest.json --assets target/composition-eval/assets --ffmpeg path/to/ffmpeg.exe
uv run tools/eval/melody_continuity_ab.py candidate --out target/experiment/after --cli path/to/candidate-auris.exe --before target/experiment/before/manifest.json
uv run tools/eval/melody_continuity.py --manifest target/experiment/before/manifest.json --json target/experiment/before/continuity.json
uv run tools/eval/melody_continuity.py --manifest target/experiment/after/manifest.json --json target/experiment/after/continuity.json
uv run tools/eval/aesthetics.py target/experiment/before/excerpts --json target/experiment/before/aesthetics.json
uv run tools/eval/aesthetics.py target/experiment/after/excerpts --baseline target/experiment/before/aesthetics.json --json target/experiment/after/aesthetics.json
uv run tools/eval/clap.py target/experiment/before/excerpts --segments 1 --json target/experiment/before/clap.json
uv run tools/eval/clap.py target/experiment/after/excerpts --segments 1 --baseline target/experiment/before/clap.json --json target/experiment/after/clap.json
```

Workspace Rust tests: 3,278 passed, 8 ignored. Denied-warning workspace/all-targets Clippy and formatting pass. Python evaluation tests: 106 passed. Ruff passes on the four new Python files; a directory-wide run reports four pre-existing findings in unchanged tune.py. The composer snapshots retain their harmony and note counts, with two digests updated for the intentional writer change.

Independent audit verifies 22 baseline/output projects, 11 newly composed candidate projects and 66 WAVs: all artifact hashes, unchanged non-melody content and backing, fixed renderer, matching chorus bounds, exact slicing/fades/linear gain and finite non-silent audio agree. Final excerpts measure -23.01 to -23.00 LUFS before and -23.00 after, with after true peaks -10.74 to -8.24 dBTP. No sample clipping was found. This is numerical audio evidence, not an assistant listening judgment.

The listening page passes 34 real-browser checks: all 22 audio players decode and play, exclusive playback and stop controls work, genre and model toggles work, and 1280/390-pixel layouts do not overflow. Playback was muted during automation and is not a listening judgment. The local page generator and browser evidence are retained under the experiment directory.

Detailed paired metadata: [JSON](melody-continuity-2026-09-10.json). Local raw evidence and audio are under `target/melody-continuity/`; they are excluded from Git.
