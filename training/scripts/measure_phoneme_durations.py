#!/usr/bin/env python
"""Measure per-phoneme consonant widths from an aligned corpus.

Some singing databases ship hand-corrected phoneme alignments next to the
audio. Training does not use them — the phoneme-to-frame alignment is recovered
by monotonic alignment search — but they answer a question training never asks:
how long *is* each consonant when this singer sings. A score front-end has to
choose that before it can turn a note into frames, and one flat constant for
every consonant is measurably wrong (see
``src/auris_singer/phoneme_durations.py``).

This script reads the labels, converts them to IPA, and writes the JSON block
that ``scripts/export_onnx.py --phoneme-durations`` embeds into the model, so
the numbers travel with the voice they were measured from.

Both shipped label formats are read, detected per file:

* **mono** — ``start end phoneme`` per line, as the Namine Ritsu database ships;
* **HTS full-context** — as JSUT-song ships, with the phoneme between ``-`` and
  ``+`` of the third field.

Times are in 100 ns units in both.

Example:
    uv run python scripts/measure_phoneme_durations.py \
        --label-dir 'data/raw/namine_ritsu_v2/「波音リツ」歌声データベースVer2.0.2/DATABASE' \
        --measured-from 'Namine Ritsu singing DB Ver2.0.2, mono labels, 110 songs' \
        --output data/raw/namine_ritsu_durations.json
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from prepare_jsut_song import PHONEME_PATTERN, TIME_UNIT  # noqa: E402
from prepare_namine_ritsu import ENUNU_TO_OPENJTALK  # noqa: E402

from auris_singer.phoneme_durations import (  # noqa: E402
    DEFAULT_SECONDS,
    MIN_SAMPLES,
    measure,
    measure_dataset,
    summarize,
)
from auris_singer.text.ipa import DEFAULT_PHONEME_TABLE  # noqa: E402
from auris_singer.text.japanese import OPENJTALK_TO_IPA  # noqa: E402


def read_timed_phonemes(path: Path) -> list[tuple[float, float, str]]:
    """Read one label file as ``(start, end, symbol)``, mono or HTS.

    The format is decided per line rather than per file: the third field of an
    HTS label always carries the phoneme between ``-`` and ``+``, and a mono
    label's third field never does.
    """
    out: list[tuple[float, float, str]] = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        parts = line.split(maxsplit=2)
        if len(parts) < 3:
            continue
        match = PHONEME_PATTERN.search(parts[2])
        symbol = match.group(1) if match else parts[2].split()[0]
        try:
            start, end = int(parts[0]) * TIME_UNIT, int(parts[1]) * TIME_UNIT
        except ValueError:
            continue
        out.append((start, end, ENUNU_TO_OPENJTALK.get(symbol, symbol)))
    return out


def to_ipa(utterance: list[tuple[float, float, str]]) -> list[tuple[float, float, str]]:
    """Map an utterance's symbols to IPA, keeping unmapped ones in place.

    An unmapped symbol stays under its original name rather than being dropped:
    dropping it would make the phoneme after it look phrase-initial, and
    ``main`` reports whatever survives outside the phoneme table instead of
    letting it slip into the shipped file.
    """
    return [(s, e, OPENJTALK_TO_IPA.get(sym, sym)) for s, e, sym in utterance]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument(
        "--data", type=Path, help="a preprocessed dataset with labelled durations: every speaker at once"
    )
    source.add_argument("--label-dir", type=Path, help="label files, searched recursively: one speaker")
    parser.add_argument("--speaker", help="the speaker the label files belong to, as the model names it")
    parser.add_argument("--output", required=True, type=Path, help="output .json path")
    parser.add_argument(
        "--measured-from",
        default="",
        help="note on the corpus and label set, stored verbatim in the block",
    )
    parser.add_argument("--min-samples", type=int, default=MIN_SAMPLES)
    parser.add_argument("--default-seconds", type=float, default=DEFAULT_SECONDS)
    args = parser.parse_args()

    if args.data is not None:
        by_speaker = measure_dataset(args.data)
        if not by_speaker:
            raise SystemExit(f"{args.data} holds no labelled durations; preprocess with a duration_dir first")
        measured_from = args.measured_from or f"the labelled durations under {args.data}"
    else:
        if not args.speaker:
            parser.error("--label-dir needs --speaker: whose consonants these are")
        paths = sorted(args.label_dir.rglob("*.lab"))
        if not paths:
            raise SystemExit(f"no .lab files under {args.label_dir}")
        by_speaker = {args.speaker: measure(to_ipa(read_timed_phonemes(path)) for path in paths)}
        measured_from = args.measured_from or f"{len(paths)} label files under {args.label_dir}"
    for speaker, raw in by_speaker.items():
        unknown = sorted(set(raw) - set(DEFAULT_PHONEME_TABLE.symbols))
        if unknown:
            print(f"warning: {speaker}: symbols outside the phoneme table, not shipped: {unknown}")
        by_speaker[speaker] = {k: v for k, v in raw.items() if k in DEFAULT_PHONEME_TABLE}
    block = summarize(
        by_speaker,
        measured_from=measured_from,
        min_samples=args.min_samples,
        default=args.default_seconds,
    )
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(block, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    for speaker, table in block["speakers"].items():
        print(f"{speaker}: {len(table['seconds'])} phonemes shipped, default {table['default'] * 1000:.0f} ms:")
        for symbol, seconds in table["seconds"].items():
            print(f"  {symbol:4s} {seconds * 1000:5.0f} ms  n={table['counts'][symbol]:5d}")
    print(f"wrote {args.output}")


if __name__ == "__main__":
    main()
