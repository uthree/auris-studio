# VOICEVOX singing: fixed-score composition comparison

2026-09-10. The previous improvement in Audiobox content enjoyment did **not** reproduce when
the same saved scores were sung by VOICEVOX. Across two paired seeds, enjoyment fell slightly
and production quality was nearly unchanged. CLAP more strongly matched singing descriptions
after replacing the built-in audition voice, but that is a change in voice identity, not proof
that the composer improved.

Credit for generated singing: **VOICEVOX:ずんだもん** (Zundamon, normal).

This follows the [rule-based composition report](composition-phrases-2026-09-10.md).
The [evidence JSON](composition-voicevox-2026-09-10.json) contains all individual scores,
means, paired deltas, model pins, prompts and audio/project hashes.

## Conditions

- Original saved before/after scores: the same `vocal-pop-s101` and `vocal-pop-s103` used in
  the first report. Before means pre-improvement composition; after means improved composition.
  No score was recomposed, transposed or edited for VOICEVOX.
- All four were copied to separate project folders and sung through the same existing Auris
  CLI executable (SHA-256 in the evidence), at synthesis seed **42**. Score-writing seeds remain
  **101/103**. The source checkout at evaluation was `fdf81be`; executable identity is recorded
  independently rather than inferred from that checkout.
- Local VOICEVOX Engine **0.25.2**, singing query style **6000** (Namine Ritsu teacher), decoding
  style **3003** (Zundamon / normal). Output configuration: **24 kHz**, **93.75 frames/s**.
  The complete connection JSON is recorded in the evidence.
- Same saved mixer, effects, accompaniment and MuseScore_General SoundFont as the original
  comparison. No manual gain adjustment or loudness normalization was added. Final mixes:
  **48 kHz stereo float32, 37.777771 seconds**, `--no-tail`.
- Each saved singer has a real VOICEVOX audio take and backend pitch contour. The four take
  assets are mono 24 kHz float32, about 35.019 seconds, and finite/non-silent. Auris imports
  these to the project sample rate and uses them for the final mix.

## Scores

Means weight the two complete files equally. CE = content enjoyment, CU = content usefulness,
PC = production complexity, PQ = production quality; Audiobox predicts these on 1-10 scales.
PC is not a metric to maximize. CLAP cosine and margin are similarity values, not ratings.

| Singer / score | CE | CU | PC | PQ | CLAP vocal cosine | CLAP vocal-minus-instrumental margin |
|---|---:|---:|---:|---:|---:|---:|
| builtin / before | 6.8870 | 7.8575 | 5.2220 | 8.0690 | 0.2854 | -0.1929 |
| builtin / after | 7.0810 | 7.9015 | 5.2875 | 8.1030 | 0.2699 | -0.2054 |
| **builtin / delta** | +0.1940 | +0.0440 | +0.0655 | +0.0340 | -0.0154 | -0.0124 |
| voicevox / before | 6.6090 | 7.5730 | 5.9330 | 8.1045 | 0.3365 | -0.0460 |
| voicevox / after | 6.5760 | 7.5765 | 6.0090 | 8.0925 | 0.3408 | -0.0538 |
| **voicevox / delta** | -0.0330 | +0.0035 | +0.0760 | -0.0120 | +0.0043 | -0.0078 |

VOICEVOX paired changes, improved score minus original score:

| Score seed | CE delta | CU delta | PC delta | PQ delta | CLAP cosine delta | CLAP margin delta |
|---|---:|---:|---:|---:|---:|---:|
| 101 | -0.0470 | -0.0130 | +0.1150 | +0.0180 | +0.0051 | -0.0202 |
| 103 | -0.0190 | +0.0200 | +0.0370 | -0.0420 | +0.0035 | +0.0045 |

The original built-in-vocal CE increase of **+0.1940** becomes **-0.0330** under VOICEVOX;
both VOICEVOX seeds have a lower CE prediction after the composition changes. PQ changes by
**-0.0120** on average, with one seed up and the other down. These small changes do not establish
a perceptible regression, but the earlier improvement is not robust across these two renderers.

CLAP vocal cosine rises under VOICEVOX, while its contrast margin changes inconsistently across
seeds and decreases on average. The margin remains negative even with confirmed sung audio:
the matched instrumental descriptions score higher. This is a prompt-dependent whole-mix
comparison, not a calibrated vocal detector. In both composition versions the VOICEVOX voice
is closer to the vocal descriptions than the built-in audition voice under the same prompts.
This does not show that Japanese lyrics are intelligible or that phrasing is preferable.

## Evaluators and validation

Audiobox Aesthetics **0.0.4**, checkpoint revision
`9b1dd8e5df9af7216e836a98974fe3b82c56ded6`, uses the unchanged existing scoring script:
ten-second windows over the full mix with duration weighting for the final partial window.
The built-in Audiobox scores are retained from the original run; only the four new VOICEVOX
mixes needed new Audiobox inference. Model/cache pins match the previous run.

Native LAION CLAP **1.1.7**, music + AudioSet HTSAT-base checkpoint revision
`b3708341862f581175dba5c356a4ebf74a9b6651`, uses the same pinned tokenizer and CPU packages
as the original report. The new [fixed vocal prompts](../../tools/eval/fixtures/vocal-clap-prompts.json)
were written before inference and used for **all eight mixes**, including fresh CLAP measurements
of the existing built-in audio. They contrast sung pop with instrumental pop; the previous
instrumental preset prompts are not reused or compared numerically.

Three ten-second excerpts per mix have identical start positions across all conditions.
All **24 excerpts** are valid and none has samples outside the native quantizer's [-1, 1]
range. Audio SHA-256 values match the CLAP reports and the VOICEVOX render manifest.

All four source projects are unchanged. Every lyric, phoneme sequence, note pitch, start and
length is preserved, including all **54 moras** and four vocal sections. Backing tracks and
mixer settings compare exactly. One vibrato-delay float per saved project changes by a single
floating-point ULP during Rust JSON parsing/serialization; these are recorded individually,
with a maximum two-ULP tolerance for float serialization and exact checks for integer note data.
This is unrelated to musical edits. Source project hashes remain unchanged.

No application code or scoring function changed for this follow-up. Actual renders, manifest
validation and hash/score checks passed. This is two score seeds with one VOICEVOX voice;
there was no human listening test, ASR intelligibility test or significance test. No parameters
were tuned against these scores.

## Reproduction and local artifacts

Copy the original four project folders before singing; `auris sing` saves the take into its
project. Recreate the connection JSON from the evidence, then run on each copied project:

```powershell
target/debug/auris.exe sing <copied-project.auris> --voice <zundamon.voicevox.json> --speaker "Zundamon / normal" --seed 42
target/debug/auris.exe render <copied-project.auris> -o <mix.wav> --bit-depth 32 --no-tail
uv run tools/eval/aesthetics.py <before-wav-folder> --json before-aesthetics.json
uv run tools/eval/aesthetics.py <after-wav-folder> --baseline before-aesthetics.json --json after-aesthetics.json
uv run tools/eval/clap.py <before-wav-folder> --prompts tools/eval/fixtures/vocal-clap-prompts.json --preset vocal-pop --json before-clap.json
uv run tools/eval/clap.py <after-wav-folder> --prompts tools/eval/fixtures/vocal-clap-prompts.json --preset vocal-pop --baseline before-clap.json --json after-clap.json
```

Use the package/model pins in the evidence for exact environment reproduction; this run used
the existing `target/composition-eval/.venv`, CPU inference with four threads and offline caches.
Large artifacts remain under ignored `target/composition-eval/voicevox/`: `before/` and `after/`
hold final mixes, `projects/` holds editable scores with their `Audio/Vocal 1.wav` takes, and
the root holds raw model reports, engine catalog/manifest, connection, runner and audit logs.
