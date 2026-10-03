# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12"]
# ///
"""Extract pitch-checked, onset-aligned notes from the bounded Iowa cohort."""

import argparse
import hashlib
import json
from itertools import pairwise
from math import gcd
from pathlib import Path

import numpy as np
import soundfile as sf
from scipy.io import wavfile
from scipy.ndimage import median_filter
from scipy.signal import resample_poly, stft

RATE = 24000
SECONDS = 1.0


def manifest_digest(manifest):
    """Hash canonical JSON, independent of indentation and platform line endings."""
    canonical = json.dumps(manifest, sort_keys=True, separators=(",", ":"), ensure_ascii=True)
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()


def write_note(path, clip):
    """Write deterministic float32 WAV without libsndfile's timestamped PEAK chunk."""
    wavfile.write(path, RATE, np.asarray(clip, dtype="<f4"))


def regions(mask):
    edges = np.diff(np.r_[False, mask, False].astype(int))
    return list(zip(np.flatnonzero(edges == 1), np.flatnonzero(edges == -1), strict=True))


def pitch_regions(audio, pitches, minimum_seconds=1.0):
    """Locate stable runs of the requested duration; reject missing or repeated notes."""
    _, times, spectrum = stft(audio, RATE, nperseg=8192, noverlap=8192 - 240)
    power = abs(spectrum)
    frequencies = np.arange(power.shape[0]) * RATE / 8192
    scores = []
    for pitch in pitches:
        f0 = 440 * 2 ** ((pitch - 69) / 12)
        score = np.zeros(power.shape[1])
        for harmonic in range(1, 7):
            hz = f0 * harmonic
            # Played intonation can exceed a quarter tone; use the known scale order too.
            select = abs(frequencies - hz) < max(4, hz * 0.035)
            score += power[select].max(axis=0) / harmonic
        scores.append(score)
    scores = np.array(scores)
    winners = median_filter(np.argmax(scores, axis=0), size=21)
    level = np.sqrt(np.mean(power**2, axis=0))
    active = level > level.max() * 0.008
    found = []
    for index, pitch in enumerate(pitches):
        spans = [(start, end) for start, end in regions((winners == index) & active)
                 if times[end - 1] - times[start] > minimum_seconds]
        if not spans:
            raise ValueError(f"no stable region for MIDI {pitch}")
        # The decaying tail can fragment into shorter runs. Only the longest run is used.
        start, end = max(spans, key=lambda span: span[1] - span[0])
        found.append((float(times[start]), float(times[end - 1])))
    if any(a[0] >= b[0] for a, b in pairwise(found)):
        raise ValueError("pitch regions are not an ascending scale")
    return found


def onset(audio, start, end, backtrack_seconds=0.2):
    """Backtrack within the fixed limit from a stable pitch to its local 2% RMS onset."""
    begin = max(0, round((start - backtrack_seconds) * RATE))
    stop = min(len(audio), round(min(end, start + 0.6) * RATE))
    hop = 120
    x = audio[begin:stop]
    energy = np.sqrt(np.mean(x[:len(x) // hop * hop].reshape(-1, hop)**2, axis=1))
    if not len(energy) or energy.max() < 1e-6:
        raise ValueError("silent reference note")
    above = np.flatnonzero(energy > energy.max() * 0.02)
    return max(0, begin + int(above[0]) * hop - hop)


def tuning_cents(clip, pitch):
    f0 = 440 * 2 ** ((pitch - 69) / 12)
    audio = clip[round(0.2 * RATE):round(1.0 * RATE)]
    power = abs(np.fft.rfft(audio * np.hanning(len(audio))))
    frequencies = np.fft.rfftfreq(len(audio), 1 / RATE)
    estimates = []
    weights = []
    for harmonic in (1, 2, 3):
        bins = np.flatnonzero(abs(frequencies - f0 * harmonic) < f0 * harmonic * 0.045)
        index = bins[np.argmax(power[bins])]
        if power[index] < power.max() * 0.005:
            continue
        # Parabolic interpolation of the log spectrum reduces FFT-bin quantization.
        a, b, c = np.log(power[index - 1:index + 2] + 1e-15)
        offset = 0.5 * (a - c) / (a - 2 * b + c)
        estimates.append((index + np.clip(offset, -0.5, 0.5)) * RATE / len(audio) / harmonic)
        weights.append(power[index] / harmonic)
    if not weights:
        raise ValueError(f"no audible harmonics for MIDI {pitch}")
    order = np.argsort(estimates)
    cumulative = np.cumsum(np.array(weights)[order])
    frequency = np.array(estimates)[order][np.searchsorted(cumulative, cumulative[-1] * 0.5)]
    cents = float(1200 * np.log2(frequency / f0))
    if abs(cents) > 75:
        raise ValueError(f"reference intonation exceeds 75 cents for MIDI {pitch}: {cents:.1f}")
    return cents


def prepare(root, lock=None):
    source = json.loads((root / "captures.json").read_text(encoding="utf-8"))
    destination = root / "notes"
    destination.mkdir(exist_ok=True)
    notes = []
    for capture in source["captures"]:
        path = root / capture["path"]
        if hashlib.sha256(path.read_bytes()).hexdigest() != capture["sha256"]:
            raise ValueError(f"reference hash mismatch: {path}")
        audio, rate = sf.read(path, always_2d=True)
        audio = audio.mean(axis=1)
        divisor = gcd(rate, RATE)
        audio = resample_poly(audio, RATE // divisor, rate // divisor)
        pitches = capture["pitches"]
        spans = pitch_regions(audio, pitches)
        for pitch, (start, end) in zip(pitches, spans, strict=True):
            offset = onset(audio, start, end)
            clip = audio[offset:offset + round(SECONDS * RATE)]
            if len(clip) != round(SECONDS * RATE) or end - offset / RATE < SECONDS:
                raise ValueError(f"short note: {path.name}, MIDI {pitch}, stable {start:.3f}..{end:.3f}, onset {offset / RATE:.3f}")
            # pp is a held-out dynamic; the named pitches are held-out at all dynamics.
            held_pitch = pitch in (52, 60, 67) if capture["model"] == "piano" else pitch % 3 == 1
            split = "validation" if held_pitch or capture["velocity"] == 0.35 else "train"
            name = f"{capture['model']}-{pitch}-{round(capture['velocity'] * 100)}.wav"
            output = destination / name
            write_note(output, clip)
            notes.append({"model": capture["model"], "pitch": pitch,
                          "velocity": capture["velocity"], "split": split,
                          "tuning_cents": tuning_cents(clip, pitch),
                          "path": f"notes/{name}", "source": capture["path"],
                          "offset_seconds": offset / RATE, "stable_end_seconds": end,
                          "sha256": hashlib.sha256(output.read_bytes()).hexdigest()})
        print(f"{path.name}: {len(pitches)} notes", flush=True)
    manifest = {"source": source["source"], "creator": source["creator"], "terms": source["terms"],
                "rate": RATE, "seconds": SECONDS, "hold": SECONDS, "notes": notes,
                "captures_sha256": manifest_digest(source)}
    if lock is not None:
        expected = json.loads(lock.read_text(encoding="utf-8"))
        if manifest != expected:
            raise ValueError("prepared cohort differs from lock; check extraction and dependency versions")
    (root / "notes.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--lock", type=Path, help="verify exact extraction and split against a frozen manifest")
    args = parser.parse_args()
    prepare(args.directory, args.lock)
