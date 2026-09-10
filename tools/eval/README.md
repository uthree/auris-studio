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
