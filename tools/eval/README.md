# eval

Learned aesthetic scoring for rendered Auris audio, run entirely through `uv` — no
environment to set up:

```
uv run tools/eval/aesthetics.py --preset all --json before.json
uv run tools/eval/aesthetics.py --preset all --baseline before.json
```

See `docs/evaluation.md` for what the four axes mean and how these numbers are meant to be
used. Not part of any release build.
