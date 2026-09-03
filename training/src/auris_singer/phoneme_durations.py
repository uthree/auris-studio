"""Per-phoneme consonant widths, measured from an aligned corpus.

The model takes durations as an input, so something upstream has to decide how
many frames each phoneme gets. For the syllabic phonemes that is easy — they
stretch to fill the note. For the consonants leading into them it is not, and a
front-end with no better information has to guess.

A single flat guess is measurably wrong. Consonant length in sung Japanese
spans a factor of three by phoneme class: measured over the 110 hand-corrected
label files of the Namine Ritsu singing database (~38 000 consonant tokens),
the affricates and sibilants run 103-119 ms while the liquids and nasals run
36-51 ms. Averaged together they land near 60 ms, which is why one constant
looks reasonable and still starves every sibilant.

It matters because the model degrades sharply, not gracefully, when a consonant
comes in far under the length it was trained on. Measured against the same
recordings, the gain from getting this right is about half the spectral
distance for the sibilants and affricates:

===========  ==============  ================
phoneme      at 60 ms        at its own width
===========  ==============  ================
``ɕ``        0.97            0.46
``ts``       0.86            0.41
``s``        0.85            0.42
``tɕ``       0.78            0.41
===========  ==============  ================

Because the numbers are a property of the *corpus*, not of the architecture,
they travel with the model: :func:`summarize` builds the block that
``export.py`` writes into the ONNX metadata, and the front-end reads it back
rather than hard-coding a table of its own. ``doc/inference.md`` documents the
wire format for consumers.
"""

from __future__ import annotations

import statistics
from collections.abc import Iterable, Sequence
from pathlib import Path

from auris_singer.text.ipa import PAU, SIL

__all__ = [
    "DEFAULT_SECONDS",
    "MIN_SAMPLES",
    "METADATA_FIELD",
    "STRETCHED",
    "measure",
    "measure_dataset",
    "summarize",
    "summarize_speaker",
]

#: Width for a consonant the table does not name.
#:
#: The pooled median over all non-syllabic consonants is 63 ms, so this is also
#: what a corpus-wide average would give.
DEFAULT_SECONDS = 0.060

#: Fewest occurrences a phoneme needs before its median is worth shipping.
#:
#: The Ritsu corpus measures ``ɸʲ`` five times and ``ḁ`` twice; a median of
#: five samples is not a measurement, and the default serves those better.
MIN_SAMPLES = 90

#: Key under which the block lives in the exported metadata.
METADATA_FIELD = "phoneme_durations"

#: Phonemes a front-end stretches to fill a note instead of giving them a fixed
#: slot: the vowels proper, the moraic nasal, and the glottal stop standing for
#: a sokuon sung on a note of its own.
#:
#: They are excluded from the table because their length is a property of the
#: note, not of the phoneme — shipping a median for ``ɴ`` would invite a
#: consumer to pin it to 150 ms and swallow the note it was given. The devoiced
#: vowels are deliberately *not* here: they are whispered between voiceless
#: neighbours and behave like consonants, taking a slot of their own.
STRETCHED: frozenset[str] = frozenset(
    {
        "a", "i", "u", "e", "o", "ɯ", "ɨ", "ə", "ɛ", "ɔ", "æ", "ʌ", "ɑ", "ɒ",
        "ʊ", "ɪ", "y", "ø", "œ", "ɐ",
        "ɴ", "ʔ",
    }
)

#: Symbols that mark a boundary rather than a sound.
_BOUNDARY: frozenset[str] = frozenset({SIL, PAU, "sil", "pau"})


def measure(
    utterances: Iterable[Sequence[tuple[float, float, str]]],
) -> dict[str, list[float]]:
    """Collect the duration of every countable consonant, keyed by IPA symbol.

    Args:
        utterances: one sequence of ``(start, end, ipa_symbol)`` per utterance,
            in seconds and in time order. Symbols must already be IPA; a
            corpus in another phoneme set is mapped by its preparation script.

    Only **medial** consonants are counted — those whose predecessor is a sound
    rather than a boundary. The distinction is not cosmetic for plosives: the
    label span of an intervocalic ``k`` includes its closure and runs 91 ms,
    while phrase-initially there is no closure to measure and the same label
    covers 24 ms. A sung phrase puts nearly every consonant between two vowels,
    so the medial figure is the one a front-end wants. Continuants barely move
    between the two contexts (``ɕ`` is 112 ms against 110 ms), so the choice
    only really decides the plosives.
    """
    out: dict[str, list[float]] = {}
    for utterance in utterances:
        previous = SIL
        for start, end, symbol in utterance:
            initial = previous in _BOUNDARY
            previous = symbol
            if symbol in _BOUNDARY or symbol in STRETCHED or initial:
                continue
            if end > start:
                out.setdefault(symbol, []).append(end - start)
    return out


def measure_dataset(root: str | Path) -> dict[str, dict[str, list[float]]]:
    """:func:`measure` over a preprocessed dataset that stored labelled durations, one
    mapping per speaker — the shape :func:`summarize` takes.

    The frames the preprocessor stored are read back as seconds at the dataset's hop, so
    a width is quantised to the hop; a median of hundreds of them is not.
    """
    import json

    import numpy as np

    from auris_singer.data.dataset import read_metadata
    from auris_singer.text.ipa import PhonemeTable

    root = Path(root)
    table = PhonemeTable.load(root / "phonemes.json")
    audio = json.loads((root / "audio_config.json").read_text(encoding="utf-8"))
    hop = float(audio["hop_length"]) / float(audio["sample_rate"])
    by_speaker: dict[str, list[list[tuple[float, float, str]]]] = {}
    for record in read_metadata(root):
        if not record.get("has_durations"):
            continue
        with np.load(root / record["path"]) as data:
            symbols = table.decode(data["phonemes"].astype(np.int64).tolist())
            frames = data["durations"].astype(np.int64).tolist()
        timed, at = [], 0.0
        for symbol, count in zip(symbols, frames):
            timed.append((at, at + count * hop, symbol))
            at += count * hop
        by_speaker.setdefault(str(record["speaker"]), []).append(timed)
    return {speaker: measure(utterances) for speaker, utterances in by_speaker.items()}


def summarize(
    by_speaker: dict[str, dict[str, list[float]]],
    measured_from: str,
    min_samples: int = MIN_SAMPLES,
    default: float = DEFAULT_SECONDS,
) -> dict[str, object]:
    """The block the exporter ships: one table per speaker, under ``speakers``.

    Consonant length is the singer's, so a model trained on several corpora carries one
    table for each, keyed by the speaker's name — the name the preprocessing config gave
    the source, which is the name the model's speaker map carries. The host copies the
    chosen speaker's table into the document. Each table is :func:`summarize_speaker`'s.
    """
    return {
        "unit": "seconds",
        "measured_from": measured_from,
        "speakers": {
            speaker: summarize_speaker(durations, min_samples=min_samples, default=default)
            for speaker, durations in sorted(by_speaker.items())
        },
    }


def summarize_speaker(
    durations: dict[str, list[float]],
    min_samples: int = MIN_SAMPLES,
    default: float = DEFAULT_SECONDS,
) -> dict[str, object]:
    """Turn one speaker's raw per-phoneme durations into their table.

    A phoneme earns an entry when it was seen at least ``min_samples`` times
    **and** its median exceeds ``default``. The second condition is the
    surprising one, and it is there because the table is consumed by a model
    rather than by a phonetician: sweeping the duration of each consonant
    against the real recordings, the sibilants and affricates improve steeply
    up to their measured width, while shortening the liquids to theirs makes
    the output *worse* — ``ɾ``'s 36 ms median cost 20 % against leaving it at
    60 ms, because the model's own preference bottoms out around 50 ms and is
    nearly flat above it. Only lengthening is supported by the evidence, so
    only lengthening is shipped, and everything else keeps the default.

    Args:
        durations: the mapping :func:`measure` returns.
        min_samples: fewest occurrences before a median is trusted.
        default: the width a consumer uses for anything not named.

    Returns:
        The speaker's table. ``seconds`` is the table proper; ``counts`` carries
        the sample size behind each entry so a consumer can apply a stricter
        threshold than this one without re-measuring.
    """
    seconds: dict[str, float] = {}
    counts: dict[str, int] = {}
    for symbol, values in durations.items():
        if len(values) < min_samples:
            continue
        median = round(statistics.median(values), 3)
        if median <= default:
            continue
        seconds[symbol] = median
        counts[symbol] = len(values)

    order = sorted(seconds, key=lambda s: (-seconds[s], s))
    return {
        "default": default,
        "seconds": {s: seconds[s] for s in order},
        "counts": {s: counts[s] for s in order},
    }
