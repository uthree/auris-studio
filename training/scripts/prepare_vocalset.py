#!/usr/bin/env python
"""Turn VocalSet recordings into the generic ``wav + text`` corpus layout.

`VocalSet <https://zenodo.org/records/1193957>`_ is 20 professional singers
(11 male, 9 female) recorded at 44.1 kHz. It exists here for one reason: it is
the only freely scriptable corpus found that covers the **low pitch range**.
A Japanese singing corpus such as JSUT-song is a single soprano whose f0 almost
never drops below 200 Hz, so a model trained on it alone has never seen the
dense harmonic structure of a male voice and its behaviour down there is
untested.

What VocalSet is not: it is not Japanese, and outside the three excerpts the
singers perform scales, arpeggios and sustained tones on a **single vowel**.
These speakers therefore contribute no consonant training at all. That is an
accepted trade-off — the purpose is to give the decoder real low-f0 excitation
to filter, and the speaker embedding keeps the limited phoneme inventory from
leaking into the other speakers.

Only normal-phonation techniques are kept. The extended techniques
(``vocal_fry``, ``inhaled``, ``lip_trill``, ``trill``, ``trillo``) are excluded:
their glottal behaviour is aperiodic or absent, so a pitch tracker's output is
unreliable and an impulse-train excitation is the wrong model for them. This is
also why the prepared corpus bottoms out near 80 Hz rather than 50 — in this
corpus only vocal fry goes lower, and fry is exactly the phonation the source
model cannot represent.

Example:
    uv run python scripts/prepare_vocalset.py \\
        --source data/raw/VocalSet/FULL \\
        --output data/raw/vocalset
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

import numpy as np  # noqa: E402
import soundfile as sf  # noqa: E402
from scipy.signal import resample_poly  # noqa: E402

from auris_singer.text import SIL  # noqa: E402

#: Techniques with ordinary voiced phonation. Everything else in VocalSet is an
#: extended technique -- see the module docstring.
DEFAULT_TECHNIQUES: tuple[str, ...] = (
    "straight", "vibrato", "belt", "breathy", "forte", "pp", "messa",
    "slow_forte", "slow_piano", "fast_forte", "fast_piano",
)

#: The four male singers with the lowest median f0, measured with FCPE over a
#: random sample of the corpus. ``male9`` is a high tenor (median 330 Hz) and
#: adds nothing this recipe is for, so it is not in the default set.
DEFAULT_SPEAKERS: tuple[str, ...] = ("male8", "male11", "male3", "male1")

#: VocalSet file names end in the sung vowel, e.g. ``m1_arpeggios_straight_a``.
#: The singers are classically trained and the vowels are Italianate, so they
#: map onto the bare IPA vowels rather than any language-specific variant.
VOWEL_PATTERN = re.compile(r"_([aeiou])$")


def find_recordings(
    source: Path, speakers: set[str], techniques: set[str]
) -> list[tuple[str, str, Path]]:
    """Collect ``(speaker, vowel, path)`` for every recording worth keeping.

    VocalSet is laid out as ``FULL/<speaker>/<category>/<technique>/<file>.wav``.
    Files whose name does not end in a vowel are the sung excerpts, which carry
    real lyrics this recipe does not transcribe, and are skipped.
    """
    found: list[tuple[str, str, Path]] = []
    for path in sorted(source.glob("*/*/*/*.wav")):
        speaker, _category, technique = path.parts[-4:-1]
        if speaker not in speakers or technique not in techniques:
            continue
        match = VOWEL_PATTERN.search(path.stem.replace(" ", ""))
        if match is None:
            continue
        found.append((speaker, match.group(1), path))
    return found


def load_mono(path: Path, sample_rate: int) -> np.ndarray:
    """Read a recording as mono float32 at ``sample_rate``."""
    wav, source_rate = sf.read(path, dtype="float32", always_2d=True)
    wav = wav.mean(axis=1)
    if source_rate != sample_rate:
        gcd = np.gcd(source_rate, sample_rate)
        wav = resample_poly(wav, sample_rate // gcd, source_rate // gcd)
    return wav.astype(np.float32)


def find_sound_regions(
    wav: np.ndarray, sample_rate: int, hop: int, floor_db: float, min_silence: float
) -> list[tuple[int, int]]:
    """Split ``wav`` at silences of at least ``min_silence`` seconds.

    The threshold is relative to the loudest frame of this recording rather
    than absolute, because VocalSet's per-file levels vary with the technique
    (``pp`` sits far below ``forte``).

    Returns:
        ``(start, end)`` sample indices of each sounding region.
    """
    if len(wav) < hop:
        return []
    n_frames = len(wav) // hop
    frames = wav[: n_frames * hop].reshape(n_frames, hop)
    rms = np.sqrt((frames.astype(np.float64) ** 2).mean(axis=1) + 1e-12)
    loud = rms > rms.max() * (10.0 ** (floor_db / 20.0))
    if not loud.any():
        return []

    min_silent_frames = max(1, int(round(min_silence * sample_rate / hop)))
    regions: list[tuple[int, int]] = []
    start: int | None = None
    silence = 0
    for i, is_loud in enumerate(loud):
        if is_loud:
            if start is None:
                start = i
            silence = 0
        elif start is not None:
            silence += 1
            if silence >= min_silent_frames:
                regions.append((start * hop, (i - silence + 1) * hop))
                start = None
    if start is not None:
        # Close on the last sounding frame rather than the end of the file: a
        # trailing silence too short to split on would otherwise be swallowed
        # into the region, and the transcript would then not declare the <sil>
        # that is audibly there.
        last_loud = int(np.flatnonzero(loud)[-1])
        regions.append((start * hop, (last_loud + 1) * hop))
    return regions


def split_long(
    region: tuple[int, int], sample_rate: int, max_seconds: float
) -> list[tuple[int, int]]:
    """Cut a region into equal pieces no longer than ``max_seconds``.

    Sustained tones and slow scales run past any sensible batch length, and
    there is no silence inside them to cut at. Equal pieces at least avoid a
    short ragged tail.
    """
    start, end = region
    limit = int(max_seconds * sample_rate)
    length = end - start
    if length <= limit:
        return [region]
    pieces = int(np.ceil(length / limit))
    step = int(np.ceil(length / pieces))
    return [(s, min(s + step, end)) for s in range(start, end, step)]


def gain_to_match(clips: list[np.ndarray], target_dbfs: float, ceiling: float) -> float:
    """One gain per speaker that lands their overall RMS on ``target_dbfs``.

    VocalSet sits 8-14 dB below JSUT-song. Level matters here more than it
    normally would: frame energy is a conditioning input, so leaving the gap in
    would teach the model that the low-pitched speakers are also the quiet
    ones, and the low-f0 behaviour this corpus exists to test would only ever
    be seen at low energy.

    The gain is per speaker, not per clip -- VocalSet's ``pp`` and ``forte``
    takes are a deliberate dynamic contrast that per-clip normalization would
    erase. It is capped so the loudest sample stays under ``ceiling``.
    """
    total = sum(float((clip.astype(np.float64) ** 2).sum()) for clip in clips)
    samples = sum(len(clip) for clip in clips)
    rms = np.sqrt(total / max(samples, 1))
    if rms <= 0.0:
        return 1.0
    gain = float(10.0 ** (target_dbfs / 20.0) / rms)
    peak = max(float(np.abs(clip).max()) for clip in clips)
    return min(gain, ceiling / peak) if peak > 0.0 else gain


def durations(
    padded_start: int, start: int, end: int, padded_end: int, sample_rate: int
) -> list[float]:
    """Seconds per transcript token, the way the song corpora's labels give them.

    A clip is silence, one vowel, silence — the silence detection that cut it *is* its
    alignment, exact to a frame, so the preprocessor can store these as labelled durations
    and the clip joins the labelled batches instead of sending them to the search. Written
    in the order :func:`transcript` writes the tokens, and only for the edges it declares.
    """
    out = []
    if padded_start < start:
        out.append((start - padded_start) / sample_rate)
    out.append((end - start) / sample_rate)
    if padded_end > end:
        out.append((padded_end - end) / sample_rate)
    return out


def transcript(vowel: str, pad_start: bool, pad_end: bool) -> str:
    """The IPA line for a single-vowel clip.

    ``<sil>`` is written only for an edge that actually keeps some silence.
    Claiming silence that is not in the audio would make alignment hand real
    sung frames to the silence token.
    """
    return " ".join([*([SIL] if pad_start else []), vowel, *([SIL] if pad_end else [])])


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True, help="VocalSet FULL directory")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--speakers", nargs="+", default=list(DEFAULT_SPEAKERS))
    parser.add_argument("--techniques", nargs="+", default=list(DEFAULT_TECHNIQUES))
    parser.add_argument("--sample-rate", type=int, default=48_000)
    parser.add_argument("--hop", type=int, default=480, help="silence-detection frame size")
    parser.add_argument("--floor-db", type=float, default=-40.0,
                        help="silence threshold relative to the file's loudest frame")
    parser.add_argument("--split-silence", type=float, default=0.4,
                        help="internal silence long enough to split an utterance")
    parser.add_argument("--pad", type=float, default=0.1,
                        help="silence kept at each edge of a clip")
    parser.add_argument("--min-seconds", type=float, default=0.6)
    parser.add_argument("--max-seconds", type=float, default=10.0)
    parser.add_argument("--target-dbfs", type=float, default=-20.7,
                        help="per-speaker RMS to match; the default is JSUT-song's")
    parser.add_argument("--peak-ceiling", type=float, default=0.95)
    args = parser.parse_args()

    recordings = find_recordings(args.source, set(args.speakers), set(args.techniques))
    if not recordings:
        raise SystemExit(f"no recordings under {args.source} matched the filters")

    pad = int(args.pad * args.sample_rate)
    minimum = int(args.min_seconds * args.sample_rate)

    print(f"wrote clips to {args.output}")
    for speaker in args.speakers:
        names: list[str] = []
        lines: list[str] = []
        timings: list[list[float]] = []
        clips: list[np.ndarray] = []

        for _, vowel, path in [r for r in recordings if r[0] == speaker]:
            wav = load_mono(path, args.sample_rate)
            regions = find_sound_regions(
                wav, args.sample_rate, args.hop, args.floor_db, args.split_silence
            )
            spans = [c for r in regions for c in split_long(r, args.sample_rate, args.max_seconds)]

            for index, (start, end) in enumerate(spans):
                if end - start < minimum:
                    continue
                # Keep a little silence around the clip when the recording has
                # it; the transcript declares a <sil> for that edge and only then.
                padded_start, padded_end = max(0, start - pad), min(len(wav), end + pad)
                clip = wav[padded_start:padded_end]
                if float(np.abs(clip).max()) < 1e-4:
                    continue
                names.append(f"{path.stem.replace(' ', '')}_{index:02d}")
                lines.append(transcript(vowel, padded_start < start, padded_end > end))
                timings.append(durations(padded_start, start, end, padded_end, args.sample_rate))
                clips.append(clip)

        if not clips:
            print(f"  {speaker:<8} no clips matched the filters")
            continue

        gain = gain_to_match(clips, args.target_dbfs, args.peak_ceiling)
        wav_dir = args.output / speaker / "wav"
        text_dir = args.output / speaker / "text"
        dur_dir = args.output / speaker / "dur"
        wav_dir.mkdir(parents=True, exist_ok=True)
        text_dir.mkdir(parents=True, exist_ok=True)
        dur_dir.mkdir(parents=True, exist_ok=True)
        for name, line, timing, clip in zip(names, lines, timings, clips):
            sf.write(wav_dir / f"{name}.wav", clip * gain, args.sample_rate, subtype="PCM_16")
            (text_dir / f"{name}.txt").write_text(line + "\n", encoding="utf-8")
            (dur_dir / f"{name}.txt").write_text(
                " ".join(f"{d:.4f}" for d in timing) + "\n", encoding="utf-8"
            )

        seconds = sum(len(clip) for clip in clips) / args.sample_rate
        print(f"  {speaker:<8} {len(clips):4d} clips  {seconds / 60:5.1f} min  gain {gain:5.2f}x")


if __name__ == "__main__":
    main()
