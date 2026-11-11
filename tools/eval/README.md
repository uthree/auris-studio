# Composition evaluation

Development-only measurements. See [the evaluation guide](../../docs/evaluation.md).

```sh
uv run tools/eval/music.py target/before --json before.json
uv run tools/eval/music.py target/after --baseline before.json --json after.json
```
