# Composition evaluation

Development-only measurements. See [the evaluation guide](../../docs/evaluation.md).

```sh
uv run tools/eval/music.py target/before --json before.json
uv run tools/eval/music.py target/after --baseline before.json --json after.json
```

The writers under a tune are measured from their notes instead:

```sh
cargo run --release -p auris-compose --example dump_parts > takes.jsonl
uv run tools/eval/accompaniment_parts.py intrinsic takes.jsonl
```
