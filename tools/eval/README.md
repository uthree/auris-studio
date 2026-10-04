# Composition evaluation

Development-only measurements and preset tuning. See [the evaluation guide](../../docs/evaluation.md).

```sh
uv run tools/eval/music.py --preset all --seeds 3 --json before.json
uv run tools/eval/music.py target/after --baseline before.json --json after.json
uv run tools/eval/tune.py --preset all --trials 18 --out tune-results.json
```
