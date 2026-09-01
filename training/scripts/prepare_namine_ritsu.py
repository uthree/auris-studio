#!/usr/bin/env python
"""Turn the Namine Ritsu singing database into the preprocessor's layout.

The `official database <https://www.canon-voice.com/voicebanks/>`_ (Ver2)
ships one folder per song under ``DATABASE/``, each holding ``song.wav``
(44.1 kHz/16-bit mono, minutes long) and ``song.lab`` — a *mono* label file:
``start end phoneme`` per line in 100 ns units, Sinsy phonemes plus a few
ENUNU extensions. As with JSUT-song, the songs are split into phrases at
labeled pauses and the transcripts are written as IPA for
``text.language: ipa``; label timings are used only to find the boundaries.

The ENUNU extensions are normalized to their nearest Sinsy/OpenJTalk symbol
before conversion:

* ``GlottalStop`` and ``Edge`` mark a glottal(ized) attack on the following
  vowel — both become ``cl`` (the glottal stop ``ʔ``);
* ``br`` (breath, a handful of labels in one song) becomes ``pau``.

Example:
    uv run python scripts/prepare_namine_ritsu.py \
        --db-dir 'data/raw/namine_ritsu_v2/「波音リツ」歌声データベースVer2.0.2/DATABASE' \
        --output data/raw/namine_ritsu
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import numpy as np  # noqa: E402
import soundfile as sf  # noqa: E402
from prepare_jsut_song import (  # noqa: E402
    Phoneme,
    enforce_max_length,
    split_into_phrases,
    to_ipa_line,
    trim_edges,
)

from auris_singer.text.japanese import OPENJTALK_TO_IPA  # noqa: E402

TIME_UNIT = 1e-7

#: ENUNU-specific labels -> the Sinsy/OpenJTalk symbol they are nearest to.
ENUNU_TO_OPENJTALK = {
    "GlottalStop": "cl",
    "Edge": "cl",
    "br": "pau",
}


def read_mono_label(path: Path) -> list[Phoneme]:
    """Parse a mono label file (``start end phoneme``, 100 ns units)."""
    phonemes: list[Phoneme] = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        parts = line.split()
        if len(parts) != 3:
            continue
        symbol = ENUNU_TO_OPENJTALK.get(parts[2], parts[2])
        phonemes.append(Phoneme(int(parts[0]) * TIME_UNIT, int(parts[1]) * TIME_UNIT, symbol))
    return phonemes


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db-dir", required=True, type=Path, help="the DATABASE folder")
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--min-pause", type=float, default=0.25)
    parser.add_argument("--min-seconds", type=float, default=2.0)
    parser.add_argument("--max-seconds", type=float, default=8.0)
    parser.add_argument("--edge-pad", type=float, default=0.15)
    parser.add_argument("--peak", type=float, default=0.95)
    parser.add_argument("--min-clip-seconds", type=float, default=0.8)
    args = parser.parse_args()

    wav_out = args.output / "wav"
    text_out = args.output / "text"
    wav_out.mkdir(parents=True, exist_ok=True)
    text_out.mkdir(parents=True, exist_ok=True)

    n_phrases = 0
    total_seconds = 0.0
    skipped = 0
    unknown: set[str] = set()

    for song_dir in sorted(p for p in args.db_dir.iterdir() if p.is_dir()):
        wav_paths = sorted(song_dir.glob("*.wav"))
        if not wav_paths:
            continue
        for wav_path in wav_paths:
            label_path = wav_path.with_suffix(".lab")
            if not label_path.is_file():
                skipped += 1
                continue

            wav, sample_rate = sf.read(str(wav_path), dtype="float32", always_2d=True)
            wav = wav.mean(axis=1)
            # Per song, as in the JSUT recipe: phrase dynamics survive.
            peak = float(np.abs(wav).max())
            if peak > 1e-6:
                wav = wav * (args.peak / peak)

            phonemes = read_mono_label(label_path)
            # A symbol the mapping does not know would vanish from the
            # transcript while its sound stays in the audio — a silent
            # alignment poison, so it is at least said out loud.
            unknown |= {p.symbol for p in phonemes if p.symbol not in OPENJTALK_TO_IPA}
            grouped = split_into_phrases(
                phonemes, args.min_pause, args.min_seconds, args.max_seconds
            )
            phrases = [
                part
                for phrase in grouped
                for part in enforce_max_length(phrase, args.max_seconds)
            ]

            for index, phrase in enumerate(phrases):
                start, end, symbols = trim_edges(phrase, args.edge_pad)
                begin = max(int(start * sample_rate), 0)
                finish = min(int(end * sample_rate), wav.shape[0])
                if finish - begin < int(args.min_clip_seconds * sample_rate):
                    continue
                transcript = to_ipa_line(symbols)
                if not transcript:
                    continue

                name = f"{wav_path.stem}_{index:03d}"
                sf.write(wav_out / f"{name}.wav", wav[begin:finish], sample_rate)
                (text_out / f"{name}.txt").write_text(transcript, encoding="utf-8")
                n_phrases += 1
                total_seconds += (finish - begin) / sample_rate

    if skipped:
        print(f"warning: {skipped} wav(s) had no label file")
    if unknown:
        print(f"warning: unmapped label symbols dropped from transcripts: {sorted(unknown)}")
    print(f"wrote {n_phrases} phrases ({total_seconds / 60:.1f} min) to {args.output}")


if __name__ == "__main__":
    main()
