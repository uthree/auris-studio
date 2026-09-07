# eval

Learned aesthetic scoring for rendered Auris audio, run entirely through `uv` — no
environment to set up:

```
uv run tools/eval/aesthetics.py --preset all --json before.json
uv run tools/eval/aesthetics.py --preset all --baseline before.json
```

See `docs/evaluation.md` for what the four axes mean and how these numbers are meant to be
used. Not part of any release build.

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
