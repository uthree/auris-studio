# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12"]
# ///
"""Fetch and freeze small, openly licensed folk-instrument reference cohorts.

The dulcimer source is a real recording by iternetcone on Freesound.  The
public preview is used because the original WAV download requires a login;
the manifest says so explicitly and preserves the preview hash.  The tin
whistle source is the public, raw Wikimedia WAV of a D-major scale.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
from itertools import pairwise
from pathlib import Path
from urllib.request import Request, urlopen

import numpy as np
import soundfile as sf
from scipy.io import wavfile
from scipy.signal import resample_poly

RATE = 24_000
SECONDS = 3.0
SOURCE_RATE = 48_000
PREVIEW_USER = 5_132_876
WHISTLE_URL = "https://upload.wikimedia.org/wikipedia/commons/6/65/Whistle.wav"
WHISTLE_PAGE = "https://commons.wikimedia.org/wiki/File:Whistle.wav"
FREESOUND_PACK = "https://freesound.org/people/iternetcone/packs/19445/"
CC_BY = "https://creativecommons.org/licenses/by/4.0/"
CC_BY_SA = "https://creativecommons.org/licenses/by-sa/4.0/"
# Commons exposes no uploader/author field for this file; its primary API
# metadata provides ESMUC as the credit. Preserve that distinction in the
# manifest instead of presenting the credit as a named author.
WHISTLE_AUTHOR = "author not machine-readable; credit: Escola Superior de Música de Catalunya (ESMUC)"
DULCIMER_AUTHOR = "iternetcone"

# Distinct notes confirmed by a broad FFT on the public previews.  342916
# has an audible ghost note and is deliberately excluded from the cohort.
# Harmonic spacing and autocorrelation distinguish the weak fundamentals of
# C4/D4/E4 from the separately recorded High C5/High D5 notes.
DULCIMER = {
    342911: ("C-sharp5", 73),
    342912: ("C4", 60),
    # The file is named B.wav. Harmonic spacing identifies B4 (493.9 Hz);
    # octave labels are based on the partial sequence rather than filenames.
    342913: ("B4", 71),
    342914: ("A4", 69),
    342915: ("F-sharp4", 66),
    342917: ("E4", 64),
    342918: ("D4", 62),
    342919: ("High-C5", 72),
    342921: ("High-D5", 74),
    342920: ("G4", 67),
}
EXCLUDED_DULCIMER = {
    342916: "F.wav: audible ghost note in public preview",
}
WHISTLE_PITCHES = (74, 76, 78, 79, 81, 83, 85, 86)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def download(url: str, path: Path) -> None:
    request = Request(url, headers={"User-Agent": "auris-studio-evaluation/1"})
    with urlopen(request, timeout=60) as stream:
        data = stream.read(50_000_001)
    if len(data) > 50_000_000:
        raise ValueError(f"oversized source: {url}")
    path.write_bytes(data)


def expected_frequency(pitch: int) -> float:
    return 440.0 * 2.0 ** ((pitch - 69) / 12.0)


def tuning_cents(clip: np.ndarray, source_rate: int, pitch: int) -> float:
    expected = expected_frequency(pitch)
    sample = np.asarray(clip, dtype=np.float64)
    sample = sample[int(.2 * len(sample)) : int(.8 * len(sample))]
    spectrum = abs(np.fft.rfft(sample * np.hanning(len(sample))))
    frequencies = np.fft.rfftfreq(len(sample), 1 / source_rate)
    estimates: list[float] = []
    weights: list[float] = []
    for harmonic in (1, 2, 3, 4, 5):
        band = np.flatnonzero(abs(frequencies - expected * harmonic) < expected * harmonic * .045)
        if len(band) == 0:
            continue
        index = int(band[np.argmax(spectrum[band])])
        offset = 0.0
        if 0 < index < len(spectrum) - 1:
            left, center, right = np.log(np.maximum(spectrum[index - 1 : index + 2], 1e-20))
            denominator = left - 2 * center + right
            if abs(denominator) > 1e-12:
                offset = float(.5 * (left - right) / denominator)
        estimates.append(float((frequencies[index] + offset * frequencies[1]) / harmonic))
        weights.append(float(spectrum[index] / harmonic))
    if not weights or max(weights) < spectrum.max() * .005:
        raise ValueError(f"no fundamental for MIDI {pitch}")
    order = np.argsort(estimates)
    actual = np.asarray(estimates)[order][
        np.searchsorted(np.cumsum(np.asarray(weights)[order]), np.sum(weights) * .5)
    ]
    cents = float(1200 * np.log2(actual / expected))
    if abs(cents) > 75:
        raise ValueError(f"unexpected pitch {pitch}: {cents:.1f} cents")
    return cents


def prepare_note(audio: np.ndarray, start: float, end: float, source_rate: int) -> np.ndarray:
    onset = max(0, round(max(0.0, start - .005) * source_rate))
    finish = min(len(audio), round((end + .25) * source_rate))
    clip = np.asarray(audio[onset:finish], dtype=np.float64)
    clip = np.pad(clip, (0, max(0, round(SECONDS * source_rate) - len(clip))))
    clip = resample_poly(clip, RATE, source_rate)
    clip = clip[: round(SECONDS * RATE)]
    return np.pad(clip, (0, max(0, round(SECONDS * RATE) - len(clip)))).astype("<f4")


def prepare_steady_note(audio: np.ndarray, start: float, end: float, source_rate: int) -> np.ndarray:
    """Keep only the settled middle of a continuous scale note.

    This deliberately does not claim to contain an independent whistle attack
    or release.  The primary whistle metric compares the steady middle only.
    """
    onset = round((start + .15) * source_rate)
    finish = round(max(start + .15, end - .10) * source_rate)
    clip = np.asarray(audio[onset:finish], dtype=np.float64)
    clip = resample_poly(clip, RATE, source_rate)
    clip = clip[: round(SECONDS * RATE)]
    return np.pad(clip, (0, max(0, round(SECONDS * RATE) - len(clip)))).astype("<f4")


def convert_preview(path: Path, ffmpeg: str) -> tuple[np.ndarray, int]:
    output = path.with_suffix(".decoded.wav")
    subprocess.run(
        [ffmpeg, "-hide_banner", "-loglevel", "error", "-y", "-i", str(path), str(output)],
        check=True,
    )
    audio, rate = sf.read(output, always_2d=False)
    output.unlink()
    if np.asarray(audio).ndim > 1:
        audio = np.asarray(audio).mean(axis=1)
    return np.asarray(audio, dtype=np.float64), rate


def stable_segments(audio: np.ndarray, rate: int, pitches: tuple[int, ...]) -> list[tuple[float, float]]:
    """Find stable sections of the ordered, ascending D-major source scale."""
    hop = max(1, round(rate * .02))
    window = min(round(rate * .08), len(audio))
    levels = np.array([
        np.sqrt(np.mean(audio[start : start + window] ** 2))
        for start in range(0, len(audio) - window + 1, hop)
    ])
    times: list[float] = []
    labels: list[int] = []
    for frame, start in enumerate(range(0, len(audio) - window + 1, hop)):
        if levels[frame] < levels.max() * .05:
            labels.append(-1)
            times.append(start / rate)
            continue
        clip = audio[start : start + window]
        spectrum = abs(np.fft.rfft(clip * np.hanning(window)))
        frequencies = np.fft.rfftfreq(window, 1 / rate)
        # The first partial is the strongest component in this recording.
        # Tracking its frequency avoids mistaking a strong upper harmonic for
        # the next scale degree.
        band = np.flatnonzero((frequencies >= 400) & (frequencies <= 1400))
        dominant = frequencies[band[np.argmax(spectrum[band])]] if len(band) else 0.0
        distances = np.abs(np.asarray([expected_frequency(p) for p in pitches]) - dominant)
        labels.append(int(np.argmin(distances)) if dominant and distances.min() < dominant * .08 else -1)
        times.append(start / rate)
    spans: list[tuple[float, float]] = []
    for index, pitch in enumerate(pitches):
        candidates = np.flatnonzero(np.asarray(labels) == index)
        runs = []
        for run in np.split(candidates, np.flatnonzero(np.diff(candidates) > 1) + 1):
            if len(run):
                runs.append(run)
        frames = max(runs, key=len) if runs else np.array([], dtype=int)
        if len(frames) < 3:
            raise ValueError(f"missing stable tin-whistle pitch {pitch}")
        start = times[frames[0]] + .04
        end = times[frames[-1]] + window / rate - .04
        if end - start < .12:
            raise ValueError(f"tin-whistle region too short for MIDI {pitch}")
        spans.append((start, end))
    starts = [s[0] for s in spans]
    if any(a >= b for a, b in pairwise(starts)):
        raise ValueError("tin-whistle pitch trajectory is not ascending")
    # The FFT window spans a little beyond each label transition.  Put each
    # boundary halfway between adjacent stable estimates so neighboring notes
    # cannot contaminate one another after the fixed onset lookback.
    for index in range(len(spans) - 1):
        boundary = (spans[index][1] + spans[index + 1][0]) * .5
        spans[index] = (spans[index][0], boundary)
        spans[index + 1] = (boundary, spans[index + 1][1])
    return spans


def write_note(root: Path, note_id: str, audio: np.ndarray) -> tuple[str, str]:
    path = root / "notes" / f"{note_id}.wav"
    path.parent.mkdir(parents=True, exist_ok=True)
    wavfile.write(path, RATE, audio)
    return path.relative_to(root).as_posix(), sha256(path)


def source_entry(root: Path, model: str, source: str, source_url: str, source_sha: str,
                 license_name: str, license_url: str, pitch: int, name: str,
                 prepared_audio: np.ndarray, tuning: float, start: float, end: float,
                 media: str, source_page: str, author: str) -> dict:
    note_id = f"{model}-{pitch}-{name.lower().replace('-', '').replace(' ', '_')}"
    path, prepared_sha = write_note(root, note_id, prepared_audio)
    return {
        "id": note_id, "model": model, "pitch": pitch,
        "split": "train" if pitch % 2 == 0 else "validation",
        "tuning_cents": tuning, "source_start_seconds": start,
        "source_end_seconds": end, "hold_seconds": max(.1, end - start),
        "source": source, "source_url": source_url,
        "source_sha256": source_sha, "prepared_sha256": prepared_sha,
        "path": path, "media": media, "license": license_name,
        "license_url": license_url, "source_page": source_page, "author": author,
        "pitch_method": "harmonic-spacing with normalized autocorrelation octave audit",
    }


def fetch(root: Path, *, ffmpeg: str = "ffmpeg", lock: Path | None = None,
          offline: bool = False) -> None:
    root.mkdir(parents=True, exist_ok=True)
    source_dir = root / "sources"
    source_dir.mkdir(exist_ok=True)
    whistle_path = source_dir / "Whistle.wav"
    if not whistle_path.exists():
        if offline:
            raise ValueError(f"missing cached source: {whistle_path}")
        download(WHISTLE_URL, whistle_path)
    whistle, whistle_rate = sf.read(whistle_path, always_2d=False)
    if whistle_rate != SOURCE_RATE:
        raise ValueError(f"unexpected tin-whistle rate: {whistle_rate}")
    whistle = np.asarray(whistle, dtype=np.float64)
    if whistle.ndim > 1:
        whistle = whistle.mean(axis=1)
    notes = []
    spans = stable_segments(whistle, whistle_rate, WHISTLE_PITCHES)
    for pitch, (start, end) in zip(WHISTLE_PITCHES, spans, strict=True):
        clip = whistle[round(start * whistle_rate) : round(end * whistle_rate)]
        tuning = tuning_cents(clip, whistle_rate, pitch)
        notes.append(source_entry(root, "tin_whistle", "Whistle.wav", WHISTLE_URL,
                                  sha256(whistle_path), "CC BY-SA 4.0", CC_BY_SA,
                                  pitch, f"d_major_{pitch}", prepare_steady_note(whistle, start, end, whistle_rate),
                                  tuning, start + .15, end - .10, "original_wav_steady_excerpt",
                                  WHISTLE_PAGE, WHISTLE_AUTHOR))
    for sound_id, (name, pitch) in DULCIMER.items():
        path = source_dir / f"dulcimer-{sound_id}.mp3"
        url = f"https://cdn.freesound.org/previews/342/{sound_id}_{PREVIEW_USER}-hq.mp3"
        if not path.exists():
            if offline:
                raise ValueError(f"missing cached source: {path}")
            download(url, path)
        audio, rate = convert_preview(path, ffmpeg)
        tuning = tuning_cents(audio, rate, pitch)
        notes.append(source_entry(root, "hammered_dulcimer", path.name, url, sha256(path),
                                  "CC BY 4.0", CC_BY, pitch, name,
                                  prepare_note(audio, .0, len(audio) / rate, rate), tuning,
                                  .0, len(audio) / rate, "public_hq_mp3_preview_transcoded_to_wav",
                                  f"https://freesound.org/people/iternetcone/sounds/{sound_id}/", DULCIMER_AUTHOR))
    manifest = {
        "source": "Wikimedia Commons Whistle.wav and Freesound Multi-sampled Hammered Dulcimer",
        "source_pages": [WHISTLE_PAGE, FREESOUND_PACK],
        "preparation": "mono mix; dulcimer source full preview; tin-whistle settled middle start + 150 ms and end - 100 ms; zero pad to 3 seconds; polyphase resample to 24 kHz",
        "rate": RATE, "seconds": SECONDS,
        "notes": notes,
        "excluded": EXCLUDED_DULCIMER,
        "limitations": [
            "Tin whistle is one continuous D-major scale, not independent isolated takes.",
            "Tin-whistle notes are steady excerpts; attack/release are excluded from the primary metric.",
            "Dulcimer entries use public HQ MP3 previews because original WAV downloads require Freesound login.",
            "No pitch appears in both splits; C4/D4 and High C5/High D5 are distinct octaves.",
        ],
    }
    encoded = json.dumps(manifest, indent=2, sort_keys=True) + "\n"
    if lock is not None and json.loads(lock.read_text(encoding="utf-8")) != json.loads(encoded):
        raise ValueError("prepared folk cohort differs from frozen manifest")
    (root / "notes.json").write_text(encoded, encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--lock", type=Path)
    parser.add_argument("--offline", action="store_true")
    args = parser.parse_args()
    fetch(args.directory, ffmpeg=args.ffmpeg, lock=args.lock, offline=args.offline)
