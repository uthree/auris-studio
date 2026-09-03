#!/usr/bin/env python
"""Measure how loud each consonant is against the vowel after it, from a labelled dataset.

A score front-end writes one energy per frame — the note's velocity — and a plateau is wrong
for the consonants: on JSUT-song a voiceless plosive or fricative sits twenty-odd decibels
under the vowel it leads into, and a model asked for a /k/ at the vowel's level has never
heard one. This reads a *preprocessed* dataset that stored labelled durations (see
``doc/preprocessing.md``), measures every consonant's median RMS against the next vowel's,
and writes the JSON block that ``scripts/export_onnx.py --phoneme-levels`` embeds into the
model, so the numbers travel with the voice they were measured from.

Example:
    uv run python scripts/measure_phoneme_levels.py --data data/processed/jsut_song_lab \\
        --measured-from 'JSUT-song, HTS full-context labels, 27 songs' \\
        --output data/raw/jsut_song_levels.json
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from auris_singer.phoneme_levels import MIN_SAMPLES, measure_dataset, summarize, write  # noqa: E402
from auris_singer.text.ipa import phoneme_class  # noqa: E402


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--data", required=True, type=Path, help="a preprocessed dataset with labelled durations")
    parser.add_argument("--output", required=True, type=Path, help="output .json path")
    parser.add_argument("--measured-from", required=True, help="free text naming the corpus and label set")
    parser.add_argument("--min-samples", type=int, default=MIN_SAMPLES)
    args = parser.parse_args()

    levels = measure_dataset(args.data)
    if not levels:
        sys.exit(f"{args.data} holds no labelled durations; preprocess with a duration_dir first")
    block = summarize(levels, args.measured_from, min_samples=args.min_samples)
    write(block, args.output)
    for speaker, table in block["speakers"].items():
        print(f"{speaker}: {len(table['db'])} phonemes measured, default {table['default']:+.1f} dB:")
        for symbol, db in table["db"].items():
            print(f"  {symbol:4s} {db:+6.1f} dB  n={table['counts'][symbol]:4d}  {phoneme_class(symbol)}")
    print(f"wrote {args.output}")


if __name__ == "__main__":
    main()
