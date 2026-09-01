#!/usr/bin/env python
"""How far a checkpoint's alignment search lands from a corpus's labels, by phoneme class.

Training recovers the phoneme-to-frame alignment by monotonic alignment search unless the
preprocessor stored labelled durations. Where it did, the two can be compared — and the
comparison is what decides whether a corpus's labels are worth using: on JSUT-song the
search gave ɕ two thirds of its labelled frames and ts under three fifths, one ɕ in three
no more than two frames, and put a phoneme boundary 100–170 ms off on average.

Example::

    uv run python scripts/compare_alignment.py --checkpoint runs/small/checkpoints/last.ckpt \\
        --data data/processed/jsut_song_lab
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

import numpy as np  # noqa: E402

from auris_singer.data.dataset import read_metadata  # noqa: E402
from auris_singer.text.ipa import SIBILANTS, phoneme_class  # noqa: E402


def alignment_table(rows: list[tuple[str, float, int]]) -> list[dict]:
    """Per class: how many phonemes, their labelled and searched frames, and how the two
    disagree. ``rows`` are ``(symbol, labelled_frames, searched_frames)``; a sibilant is its
    own class here, since it is the class the search shortchanges."""
    by: dict[str, list[tuple[float, int]]] = {}
    for symbol, labelled, searched in rows:
        cls = "sibilant" if symbol in SIBILANTS else phoneme_class(symbol)
        by.setdefault(cls, []).append((labelled, searched))
    table = []
    for cls, pairs in sorted(by.items(), key=lambda kv: -len(kv[1])):
        labelled = np.asarray([p[0] for p in pairs], dtype=np.float64)
        searched = np.asarray([p[1] for p in pairs], dtype=np.float64)
        table.append(
            {
                "class": cls,
                "count": len(pairs),
                "labelled_frames": float(labelled.mean()),
                "searched_frames": float(searched.mean()),
                "ratio": float(searched.mean() / labelled.mean()) if labelled.mean() else float("nan"),
                "searched_at_most_two": float((searched <= 2).mean()),
                "mean_abs_diff": float(np.abs(searched - labelled).mean()),
            }
        )
    return table


def symbol_table(rows: list[tuple[str, float, int]], at_least: int = 20) -> list[dict]:
    """The same, per obstruent symbol with at least ``at_least`` occurrences — the class
    means hide that the search treats /s/ and /ɕ/ very differently."""
    by: dict[str, list[tuple[float, int]]] = {}
    for symbol, labelled, searched in rows:
        if symbol in SIBILANTS or phoneme_class(symbol) in {"plosive", "affricate", "fricative"}:
            by.setdefault(symbol, []).append((labelled, searched))
    table = []
    for symbol, pairs in sorted(by.items(), key=lambda kv: -len(kv[1])):
        if len(pairs) < at_least:
            continue
        labelled = np.asarray([p[0] for p in pairs], dtype=np.float64)
        searched = np.asarray([p[1] for p in pairs], dtype=np.float64)
        table.append(
            {
                "class": symbol,
                "count": len(pairs),
                "labelled_frames": float(labelled.mean()),
                "searched_frames": float(searched.mean()),
                "ratio": float(searched.mean() / labelled.mean()) if labelled.mean() else float("nan"),
                "searched_at_most_two": float((searched <= 2).mean()),
                "mean_abs_diff": float(np.abs(searched - labelled).mean()),
            }
        )
    return table


def format_table(table: list[dict]) -> str:
    head = f"{'class':12s} {'n':>6s} {'labelled':>9s} {'searched':>9s} {'ratio':>6s} {'<=2':>5s} {'|diff|':>7s}"
    lines = [head, "-" * len(head)]
    for row in table:
        lines.append(
            f"{row['class']:12s} {row['count']:6d} {row['labelled_frames']:9.2f} "
            f"{row['searched_frames']:9.2f} {row['ratio']:6.2f} {row['searched_at_most_two']:5.2f} "
            f"{row['mean_abs_diff']:7.2f}"
        )
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--checkpoint", required=True, type=Path)
    parser.add_argument("--data", required=True, type=Path, help="a preprocessed dataset with labelled durations")
    parser.add_argument("--utterances", type=int, default=0, help="how many to compare (default: all)")
    parser.add_argument("--device", default="cpu")
    args = parser.parse_args()

    from auris_singer.host_eval import Aligner, Corpus

    corpus = Corpus(args.data)
    aligner = Aligner(args.checkpoint, device=args.device)
    records = [r for r in read_metadata(args.data) if r.get("has_durations")]
    if not records:
        sys.exit(f"{args.data} holds no labelled durations; preprocess with a duration_dir first")
    if args.utterances:
        records = records[: args.utterances]
    rows: list[tuple[str, float, int]] = []
    for record in records:
        phonemes, f0, energy, voiced, wav = corpus.load(record)
        labelled = corpus.durations(record)
        searched = aligner.durations(
            phonemes, wav, f0, energy, voiced, int(record["speaker_id"]),
            corpus.n_fft, corpus.hop_length, corpus.win_length,
        )
        rows.extend(zip(phonemes, labelled, searched))
    print(f"{len(records)} utterances, {len(rows)} phonemes")
    print(format_table(alignment_table(rows)))
    print()
    print(format_table(symbol_table(rows)))


if __name__ == "__main__":
    main()
