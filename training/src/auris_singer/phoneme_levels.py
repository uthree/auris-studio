"""How loud each consonant is, relative to the vowel it leads into.

Energy reaches the decoder through the excitation, and the frames a score front-end writes
carry one energy per frame: the note's velocity, shaped by a short attack and release. That
is a plateau, and a plateau is wrong for the consonants. Measured on JSUT-song, a voiceless
plosive or fricative sits twenty-odd decibels below the vowel after it, a voiced one six to
nine, an approximant three; a front-end that gives the /k/ the vowel's level asks the model
for a consonant it has never heard at that loudness, and gets none. On the labelled corpus,
putting the vowel's level on every consonant cost the phoneme error rate 0.25 → 0.56, and
putting these medians back recovered 0.35 — most of the loss, and as much as a per-phoneme
table gets over one number per class.

The numbers are a property of the corpus a voice was trained on, not of the architecture, so
they travel **with the model**, under the optional ``phoneme_levels`` key of the metadata
JSON, exactly as the consonant widths do:

.. code-block:: json

    {"phoneme_levels": {"unit": "db", "speakers": {"<speaker>": {"default": -12.0,
                        "db": {"k": -22.6, "s": -20.6, "n": -5.9},
                        "counts": {"k": 282, "s": 142, "n": 200},
                        "measured_from": "JSUT-song, HTS labels, 27 songs"}}

The rule for a consumer: a phoneme's energy is scaled by ``10 ** (db[phoneme] / 20)`` where
the table has it and by ``10 ** (default / 20)`` for a consonant it does not, and the vowels
and the other syllabics are left at the note's level.

Measured from a *preprocessed* dataset rather than from raw labels, because it needs the
frame energies the preprocessor computed, and the labelled durations it stored say which
frames are whose.
"""

from __future__ import annotations

import json
from collections.abc import Iterable
from pathlib import Path

import numpy as np

from auris_singer.text.ipa import SPECIAL_SYMBOLS, phoneme_class

__all__ = [
    "METADATA_FIELD",
    "MIN_SAMPLES",
    "ENERGY_FLOOR",
    "measure",
    "measure_dataset",
    "summarize",
]

#: Key under which the block lives in the exported metadata.
METADATA_FIELD = "phoneme_levels"

#: Fewest occurrences a phoneme needs before its median level is worth shipping.
#:
#: Levels are steadier than durations — a plosive is twenty decibels down whoever sang it —
#: so the bar is lower than the width table's ninety; below twenty the median is still a
#: handful of readings and the class default serves better.
MIN_SAMPLES = 20

#: Frames whose RMS is below this are not a reading of anything but the noise floor.
ENERGY_FLOOR = 1e-4


def measure(
    utterances: Iterable[tuple[list[str], list[int], np.ndarray]],
) -> dict[str, list[float]]:
    """Collect every consonant's level against the next vowel, in dB, keyed by symbol.

    Args:
        utterances: per utterance, ``(phonemes, durations, energy)`` — IPA symbols, frames
            per symbol, and the per-frame linear RMS the preprocessor stored.

    A consonant's level is the ratio of its median RMS to the median RMS of the first vowel
    after it, so a quiet phrase and a loud one give the same number. The vowels themselves
    are the reference and are not measured; the specials are boundaries, not sounds. A
    consonant with no vowel after it in the utterance, or one whose frames are at the noise
    floor, contributes nothing.
    """
    out: dict[str, list[float]] = {}
    for phonemes, durations, energy in utterances:
        energy = np.asarray(energy, dtype=np.float64)
        spans = []
        at = 0
        for symbol, frames in zip(phonemes, durations):
            spans.append((symbol, at, at + int(frames)))
            at += int(frames)
        for index, (symbol, start, end) in enumerate(spans):
            if symbol in SPECIAL_SYMBOLS or phoneme_class(symbol) in {"vowel", "special"}:
                continue
            vowel = next(
                ((s, e) for q, s, e in spans[index + 1 :] if phoneme_class(q) == "vowel"), None
            )
            if vowel is None or end <= start:
                continue
            mine = float(np.median(energy[start:end]))
            theirs = float(np.median(energy[vowel[0] : vowel[1]]))
            if mine <= ENERGY_FLOOR or theirs <= ENERGY_FLOOR:
                continue
            out.setdefault(symbol, []).append(20.0 * np.log10(mine / theirs))
    return out


def measure_dataset(root: str | Path) -> dict[str, dict[str, list[float]]]:
    """:func:`measure` over a preprocessed dataset that stored labelled durations, one
    mapping per speaker — the shape :func:`summarize` takes."""
    from auris_singer.data.dataset import read_metadata
    from auris_singer.text.ipa import PhonemeTable

    root = Path(root)
    table = PhonemeTable.load(root / "phonemes.json")
    by_speaker: dict[str, list[tuple[list[str], list[int], np.ndarray]]] = {}
    for record in read_metadata(root):
        if not record.get("has_durations"):
            continue
        with np.load(root / record["path"]) as data:
            by_speaker.setdefault(str(record["speaker"]), []).append(
                (
                    table.decode(data["phonemes"].astype(np.int64).tolist()),
                    data["durations"].astype(np.int64).tolist(),
                    data["energy"].astype(np.float32),
                )
            )
    return {speaker: measure(utterances) for speaker, utterances in by_speaker.items()}


def summarize(
    by_speaker: dict[str, dict[str, list[float]]],
    measured_from: str,
    min_samples: int = MIN_SAMPLES,
) -> dict[str, object]:
    """The block the exporter ships: one table per speaker, under ``speakers``, keyed by
    the speaker's name as the model's speaker map carries it — the widths' shape, for the
    same reason: how far under the vowel a singer puts a consonant is the singer's."""
    return {
        "unit": "db",
        "measured_from": measured_from,
        "speakers": {
            speaker: summarize_speaker(levels, min_samples=min_samples)
            for speaker, levels in sorted(by_speaker.items())
        },
    }


def summarize_speaker(levels: dict[str, list[float]], min_samples: int = MIN_SAMPLES) -> dict[str, object]:
    """Turn one speaker's raw per-phoneme levels into their table.

    A phoneme earns an entry when it was seen at least ``min_samples`` times; ``default`` is
    the median over every measured reading, which is what a consonant the table does not
    name should be assumed to be — a consonant, quieter than a vowel — rather than 0 dB,
    which would be the plateau this table exists to correct.
    """
    shipped = {
        symbol: float(round(float(np.median(values)), 1))
        for symbol, values in levels.items()
        if len(values) >= min_samples
    }
    pooled = [v for values in levels.values() for v in values]
    default = float(round(float(np.median(pooled)), 1)) if pooled else 0.0
    return {
        "default": default,
        "db": dict(sorted(shipped.items(), key=lambda item: item[1])),
        "counts": {symbol: len(levels[symbol]) for symbol in shipped},
    }


def write(block: dict[str, object], path: str | Path) -> None:
    Path(path).write_text(json.dumps(block, ensure_ascii=False, indent=2), encoding="utf-8")
