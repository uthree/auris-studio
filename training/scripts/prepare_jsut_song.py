#!/usr/bin/env python
"""Turn JSUT-song into the wav + IPA-transcript layout the preprocessor expects.

`JSUT-song <https://sites.google.com/site/shinnosuketakamichi/publication/jsut-song>`_
ships 27 children's songs (48 kHz, one singer, ~25 minutes) together with
HTS full-context singing labels. Two things have to happen before it can be
used here:

* the recordings are 30-60 s long, far beyond a usable training clip, so they
  are split into phrases at the pauses the labels mark;
* the corpus has no plain-text transcripts, so the phoneme sequence is taken
  from the labels and written out as IPA — preprocess it with
  ``text.language: ipa``.

Label timings find the phrase boundaries, and are also written out as seconds
per phoneme (``dur/``), which the preprocessor stores when its source names a
``duration_dir``. Training then expands the phonemes by the labels instead of
by monotonic alignment search — which, measured on this corpus, gives ɕ two
thirds of its labelled frames, one ɕ in three no more than two frames, and
lands a boundary 100–170 ms off on average. Leave ``duration_dir`` out to
fall back to the search.

Loudness is normalized per **song**, not per phrase, so the dynamics between
phrases of one song survive; run the preprocessor with
``audio.peak_normalize: false`` to keep them.

Example:
    uv run python scripts/prepare_jsut_song.py \
        --wav-dir data/raw/jsut-song_ver1/child_song/wav \
        --label-dir data/raw/todai_child \
        --output data/raw/jsut_song
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

import numpy as np  # noqa: E402
import soundfile as sf  # noqa: E402

from auris_singer.text import SIL  # noqa: E402
from auris_singer.text.japanese import openjtalk_to_ipa  # noqa: E402

# HTS full-context labels encode the current phoneme between '-' and '+'.
PHONEME_PATTERN = re.compile(r"-([^+]+)\+")
# Label times are in 100 ns units.
TIME_UNIT = 1e-7
PAUSE_PHONEMES = {"pau", "sil"}
# OpenJTalk vowels, including the devoiced (uppercase) variants.
VOWEL_PHONEMES = {"a", "i", "u", "e", "o", "A", "I", "U", "E", "O"}


@dataclass
class Phoneme:
    start: float
    end: float
    symbol: str

    @property
    def duration(self) -> float:
        return self.end - self.start

    @property
    def is_pause(self) -> bool:
        return self.symbol in PAUSE_PHONEMES


def read_label(path: Path) -> list[Phoneme]:
    """Parse an HTS full-context label file into timed phonemes."""
    phonemes: list[Phoneme] = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        parts = line.split(maxsplit=2)
        if len(parts) < 3:
            continue
        match = PHONEME_PATTERN.search(parts[2])
        if match is None:
            continue
        phonemes.append(
            Phoneme(int(parts[0]) * TIME_UNIT, int(parts[1]) * TIME_UNIT, match.group(1))
        )
    return phonemes


def split_into_phrases(
    phonemes: list[Phoneme],
    min_pause: float,
    min_seconds: float,
    max_seconds: float,
) -> list[list[Phoneme]]:
    """Group phonemes into phrases, cutting at sufficiently long pauses.

    A cut is taken at a pause of at least ``min_pause`` seconds once the
    current phrase is at least ``min_seconds`` long, or unconditionally at any
    pause once it would otherwise exceed ``max_seconds``.

    The pause a cut lands on is kept by **both** neighbours, so every phrase
    starts and ends in silence. ``trim_edges`` then clips each side down, and
    the small overlap between adjacent clips is silence only.
    """
    phrases: list[list[Phoneme]] = []
    current: list[Phoneme] = []

    def flush(carry: Phoneme | None = None) -> None:
        # Drop phrases that ended up being nothing but silence.
        if current and any(not p.is_pause for p in current):
            phrases.append(list(current))
        current.clear()
        if carry is not None:
            current.append(carry)

    for phoneme in phonemes:
        current.append(phoneme)
        if not phoneme.is_pause:
            continue
        span = current[-1].end - current[0].start
        long_enough = span >= min_seconds and phoneme.duration >= min_pause
        if long_enough or span >= max_seconds:
            flush(carry=phoneme)
    flush()
    return phrases


def enforce_max_length(phrase: list[Phoneme], max_seconds: float) -> list[list[Phoneme]]:
    """Split a phrase that is still too long after pause-based segmentation.

    Some songs sustain a legato line for a minute with barely a pause, so a
    hard cut is unavoidable. Japanese is overwhelmingly CV, which makes the
    boundary before a consonant a syllable boundary — the least damaging place
    to cut. If no consonant onset is available in range, the boundary nearest
    the limit is used.
    """
    if phrase[-1].end - phrase[0].start <= max_seconds:
        return [phrase]

    limit = phrase[0].start + max_seconds
    earliest = phrase[0].start + max_seconds * 0.4
    candidates = [
        i
        for i in range(1, len(phrase))
        if earliest <= phrase[i].start <= limit
    ]
    if not candidates:
        # Every boundary is outside the window (one enormous phoneme); take the
        # first boundary past the lower bound so progress is still made.
        candidates = [i for i in range(1, len(phrase)) if phrase[i].start >= earliest]
    if not candidates:
        return [phrase]

    onsets = [
        i
        for i in candidates
        if phrase[i].symbol not in VOWEL_PHONEMES
        and phrase[i].symbol not in PAUSE_PHONEMES
    ]
    cut = onsets[-1] if onsets else candidates[-1]
    return [phrase[:cut]] + enforce_max_length(phrase[cut:], max_seconds)


def trim_edges(phrase: list[Phoneme], pad: float) -> tuple[float, float, list[str]]:
    """Clip leading/trailing silence to ``pad`` seconds and return the symbols."""
    start, end = phrase[0].start, phrase[-1].end
    symbols = [p.symbol for p in phrase]

    if phrase[0].is_pause and phrase[0].duration > pad:
        start = phrase[0].end - pad
    if phrase[-1].is_pause and phrase[-1].duration > pad:
        end = phrase[-1].start + pad
    return start, end, symbols


def to_durations(phrase: list[Phoneme], start: float, end: float) -> list[float]:
    """Seconds per transcript token, in the order ``to_ipa_line`` writes them.

    The edge pauses are as long as ``trim_edges`` left them, not as long as the label
    says. A symbol the IPA mapping drops leaves the transcript but not the audio, so its
    time goes to the token before it (or after it, at the very start), and the list lines
    up with the transcript token for token and sums to the clip. These are the alignment
    monotonic alignment search would otherwise have to guess, and for a consonant it
    guesses badly: on this corpus it gives ɕ two thirds of its labelled frames and one
    ɕ in three no more than two frames.
    """
    durations: list[float] = []
    carried = 0.0
    for index, phoneme in enumerate(phrase):
        first = max(phoneme.start, start) if index == 0 else phoneme.start
        last = min(phoneme.end, end) if index == len(phrase) - 1 else phoneme.end
        seconds = max(last - first, 0.0)
        if not openjtalk_to_ipa([phoneme.symbol]):
            if durations:
                durations[-1] += seconds
            else:
                carried += seconds
            continue
        durations.append(seconds + carried)
        carried = 0.0
    return durations


def to_ipa_line(symbols: list[str]) -> str:
    """Convert OpenJTalk phonemes to a space-separated IPA transcript.

    A boundary pause becomes ``<sil>`` rather than ``<pau>``. Phrases produced
    by a hard cut have no silence at that edge, and no ``<sil>`` is invented
    for them — claiming silence that is not in the audio would make alignment
    assign real speech frames to it.
    """
    ipa = openjtalk_to_ipa(symbols)
    if not ipa:
        return ""
    if symbols[0] in PAUSE_PHONEMES:
        ipa[0] = SIL
    if symbols[-1] in PAUSE_PHONEMES:
        ipa[-1] = SIL
    return " ".join(ipa)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--wav-dir", required=True, type=Path)
    parser.add_argument("--label-dir", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--min-pause", type=float, default=0.25)
    parser.add_argument("--min-seconds", type=float, default=2.0)
    parser.add_argument("--max-seconds", type=float, default=8.0)
    parser.add_argument(
        "--edge-pad",
        type=float,
        default=0.15,
        help="seconds of silence kept at each phrase boundary",
    )
    parser.add_argument("--peak", type=float, default=0.95)
    parser.add_argument(
        "--min-clip-seconds",
        type=float,
        default=0.8,
        help="clips shorter than this are dropped",
    )
    args = parser.parse_args()

    wav_out = args.output / "wav"
    text_out = args.output / "text"
    dur_out = args.output / "dur"
    wav_out.mkdir(parents=True, exist_ok=True)
    text_out.mkdir(parents=True, exist_ok=True)
    dur_out.mkdir(parents=True, exist_ok=True)

    n_phrases = 0
    total_seconds = 0.0
    skipped_without_label = 0

    for wav_path in sorted(args.wav_dir.glob("*.wav")):
        label_path = args.label_dir / f"{wav_path.stem}.lab"
        if not label_path.is_file():
            skipped_without_label += 1
            continue

        wav, sample_rate = sf.read(str(wav_path), dtype="float32", always_2d=True)
        wav = wav.mean(axis=1)
        # Normalize once per song so phrase-to-phrase dynamics are preserved.
        peak = float(np.abs(wav).max())
        if peak > 1e-6:
            wav = wav * (args.peak / peak)

        phonemes = read_label(label_path)
        grouped = split_into_phrases(
            phonemes, args.min_pause, args.min_seconds, args.max_seconds
        )
        phrases = [
            part for phrase in grouped for part in enforce_max_length(phrase, args.max_seconds)
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
            durations = to_durations(phrase, begin / sample_rate, finish / sample_rate)
            (dur_out / f"{name}.txt").write_text(
                " ".join(f"{d:.4f}" for d in durations), encoding="utf-8"
            )
            n_phrases += 1
            total_seconds += (finish - begin) / sample_rate

    if skipped_without_label:
        print(f"warning: {skipped_without_label} recording(s) had no label file")
    print(
        f"wrote {n_phrases} phrases ({total_seconds / 60:.1f} min) to {args.output}"
    )


if __name__ == "__main__":
    main()
