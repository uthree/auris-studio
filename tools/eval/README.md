# eval

Learned aesthetic scoring for rendered Auris audio, run entirely through `uv` — no
environment to set up:

```
uv run tools/eval/aesthetics.py --preset all --json before.json
uv run tools/eval/aesthetics.py --preset all --baseline before.json
```

See `docs/evaluation.md` for what the four axes mean and how these numbers are meant to be
used. Not part of any release build.

Pass `--cli path/to/auris.exe` to `aesthetics.py` to render with an archived or
already-built CLI. Passing WAV files or directories scores existing final renders.

CLAP measures audio/text identity against a frozen per-preset prompt manifest:

```
uv run tools/eval/clap.py target/before --json before-clap.json
uv run tools/eval/clap.py target/after --baseline before-clap.json --json after-clap.json
```

It uses LAION's native music `HTSAT-base` model locally, downloads public model
artifacts on first use, and records checkpoint/tokenizer hashes and preprocessing.
By default, three fixed ten-second excerpts cover the beginning, middle and end.
Cosine similarity and positive-minus-contrast margin describe prompt alignment;
neither is a probability or a musical-quality rating. Keep prompts, seeds, sound
sources and render settings fixed across comparisons. See
[the evaluation guide](../../docs/evaluation.md#audiotext-identity-with-laion-clap)
for details and model-free tests.

For a melody comparison over an unchanged saved backing:

```
uv run tools/eval/melody_ab.py --source before/before.auris --candidate candidate/candidate.auris --output audition/audition.auris --cli target/debug/auris --wav audition.wav
```

The saved audition replaces only instrumental lead notes and their recipe digest.
It preserves the source mixer, performance settings and other parts, validates matching
musical context, and records source/output hashes. See [the evaluation guide](../../docs/evaluation.md#holding-the-backing-fixed).

## Seed diversity and listening

`seed_diversity.py` renders a fixed preset/seed cohort through an existing CLI and prepares
equal-duration first-chorus excerpts with linear LUFS matching (FFmpeg required).
`seed_metrics.py` compares the written chorus rhythms and pitch intervals across seeds.
`seed_listening.py` creates a local, initially blinded A-H listening page with blank human
ratings, JSON import/export, and an optional reveal of seed/model measurements.

See [the measured seed comparison](../../docs/reviews/seed-diversity-2026-09-10.md) for
commands, conditions, results and the listening protocol. Full unmodified renders feed the
learned models; level-matched excerpts feed listening. Human ratings are never inferred.

## Melody continuity

`melody_continuity.py` measures where the written lead stops moving in its first
eight-bar chorus: midpoint inter-onset spans, held/resting coverage, the placement
of long notes and repeated early spans. Phrase-ending bars are explicit and
separate. These are descriptive diagnostics, not quality penalties.

`melody_continuity_ab.py` prepares the fixed diagnostic/reference/held-out cohort
in two stages, `baseline` and `candidate`. The candidate replaces only lead notes
and their digest in the old editable project, then renders using the frozen old
CLI. Both sides use linear -23 LUFS chorus excerpts; the experiment scores those
same excerpts with Audiobox and centered-window CLAP.

See [the continuity experiment](../../docs/reviews/melody-continuity-2026-09-10.md)
for reproduction commands, artifacts, paired results and limitations.

## Local model tool and audio checks

`agent_tools.ps1` tests saved project state through real MCP stdio and rig/Ollama
tool loops. Build `auris-mcp` and `auris-agent`, then run with PowerShell 7:

```powershell
cargo build -p auris-mcp -p auris-agent
pwsh -File tools/eval/agent_tools.ps1
```

The default uses `ornith-1.5:9b`, a 32K context, thinking off and a 4096-token
response cap. `-Transport control` checks the fixtures without inference;
`-Models qwen3.8:27b` selects the larger final benchmark. Every run retains
requests, replies, binary hashes and independent before/after document checks
under `target/agent-tools`.

`audio_input.ps1` submits actual WAVs and blinded speech, tone, noise, silence
and no-audio controls. `-AudioFile` adds music, and `-ContrastFile` adds A/B,
B/A and identical A/A requests. It records responses without declaring that a
successful upload proves hearing. The [interface trial](../../docs/reviews/local-model-interface-2026-09-07.md)
documents the tested configurations and observed failures; the
[local audio service](../audio-review/README.md) supplies an optional separate critic.
