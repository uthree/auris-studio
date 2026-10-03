"""Fixed sung-vowel extraction and audited metadata for choir copy synthesis."""

import hashlib
from math import gcd
from pathlib import Path

import numpy as np
import soundfile as sf
from prepare_physical_reference import regions, write_note
from scipy.ndimage import median_filter
from scipy.signal import correlate, resample_poly

RATE = 24000
SECONDS = 1.2
VOWELS = {"u": 0.0, "a": 1.0, "i": 2.0}
TRAIN_SINGERS = ("f1", "f5", "m1", "m2")
# Voice types come from VocalSet's readme-anon.txt, not from the fitting objective.
VOICE_SIZE = {"f1": 0.15, "f2": 0.15, "f3": 0.15, "f4": 0.15, "f5": 0.4,
              "f6": 0.15, "f7": 0.15, "f8": 0.4, "f9": 0.15, "m1": 0.75,
              "m2": 0.6, "m3": 0.6, "m4": 0.95, "m5": 0.75, "m6": 0.75,
              "m7": 0.6, "m8": 0.95, "m9": 0.4, "m10": 0.85, "m11": 0.6}


def fundamental(frame):
    """YIN difference on equal-length windows; return frequency and periodicity error."""
    frame = np.asarray(frame, dtype=np.float64)
    if frame.shape != (4096,) or not np.isfinite(frame).all():
        raise ValueError("pitch analysis requires 4096 finite samples")
    frame = frame - frame.mean()
    window = 2048
    maximum = RATE // 50
    energy = np.r_[0.0, np.cumsum(frame**2)]
    correlation = correlate(frame, frame[:window], mode="valid", method="fft")[:maximum + 1]
    difference = np.maximum(0, energy[window] + energy[window:window + maximum + 1]
                            - energy[:maximum + 1] - 2 * correlation)
    lags = np.arange(1, maximum + 1)
    normalized = difference[1:] * lags / np.maximum(np.cumsum(difference[1:]), 1e-20)
    minimum = RATE // 1100
    candidates = np.flatnonzero((normalized[1:-1] < normalized[:-2])
                               & (normalized[1:-1] <= normalized[2:])) + 1
    candidates = candidates[candidates >= minimum - 1]
    if not len(candidates) or np.mean(frame**2) < 1e-10:
        return float("nan"), 1.0
    acceptable = candidates[normalized[candidates] < 0.18]
    index = acceptable[0] if len(acceptable) else candidates[np.argmin(normalized[candidates])]
    # A strong second harmonic can meet YIN's threshold at half the true period.
    # Accept a longer period only when its difference is lower AND the spectrum
    # contains an audible odd harmonic of that lower fundamental. This avoids
    # confusing ordinary integer-lag quantization with octave evidence.
    spectrum = abs(np.fft.rfft(frame * np.hanning(len(frame))))
    frequencies = np.fft.rfftfreq(len(frame), 1 / RATE)
    for _ in range(2):
        nearby = candidates[abs(candidates + 1 - 2 * (index + 1)) < (index + 1) * 0.08]
        if not len(nearby):
            break
        longer = nearby[np.argmin(normalized[nearby])]
        half = RATE / (2 * (index + 1))
        odd_energy = max(spectrum[abs(frequencies - harmonic * half)
                                 < max(RATE / 4096 * 1.5, harmonic * half * 0.035)].max()
                         for harmonic in (1, 3))
        if normalized[longer] >= normalized[index] * 0.8 or odd_energy < spectrum.max() * 0.06:
            break
        index = longer
    a, b, c = normalized[index - 1:index + 2]
    curvature = a - 2 * b + c
    offset = np.clip(0.5 * (a - c) / curvature, -0.5, 0.5) if curvature > 1e-15 else 0
    return RATE / (index + 1 + offset), float(b)


def extract(audio):
    """Select the first two stable sung notes, preserving their natural onset envelope."""
    hop = 240
    centers = np.arange(2048, len(audio) - 2048, hop)
    measured = np.array([fundamental(audio[center - 2048:center + 2048]) for center in centers])
    pitches = 69 + 12 * np.log2(measured[:, 0] / 440)
    voiced = np.isfinite(pitches) & (measured[:, 1] < 0.22)
    local_rms = np.array([np.sqrt(np.mean(audio[c - 120:c + 120]**2)) for c in centers])
    voiced &= local_rms > local_rms.max() * 0.025
    # Fill brief periodicity gaps without merging breaths between sung notes.
    voiced = median_filter(voiced, size=11)
    spans = []
    # The supplied long-tone exercise uses C and F across registers. Broad nearest-note
    # regions allow natural vibrato to cross a semitone boundary without splitting a note.
    anchors = np.array(sorted({*range(36, 85, 12), *range(41, 85, 12)}))
    smooth_pitch = median_filter(np.nan_to_num(pitches, nan=0), size=11)
    nearest = anchors[np.argmin(abs(smooth_pitch[:, None] - anchors), axis=1)]
    for anchor in anchors:
        for begin, end in regions(voiced & (nearest == anchor)):
            start, stop = centers[begin] / RATE, centers[end - 1] / RATE
            valid = np.isfinite(pitches[begin:end]) & (measured[begin:end, 1] < 0.22)
            if stop - start < SECONDS or not valid.any():
                continue
            pitch = round(float(np.median(pitches[begin:end][valid])))
            if abs(pitch - anchor) <= 1:
                spans.append((start, stop, pitch, begin, end))
    spans.sort()
    selected = []
    for start, stop, pitch, begin, end in spans:
        if selected and start < selected[-1]["stable_end_seconds"] + 0.3:
            continue
        search_start = max(0, round((start - 0.25) * RATE))
        search_stop = round(min(stop, start + 0.4) * RATE)
        data = audio[search_start:search_stop]
        energy = np.sqrt(np.mean(data[:len(data) // 120 * 120].reshape(-1, 120)**2, axis=1))
        above = np.flatnonzero(energy >= energy.max() * 0.10)
        offset = max(0, search_start + int(above[0]) * 120 - 120)
        if stop - offset / RATE < SECONDS:
            continue
        # Estimate intonation only from the reference; the same guide drives before/after.
        in_clip = (centers[begin:end] >= offset + round(0.2 * RATE)) & (
            centers[begin:end] < offset + round(SECONDS * RATE))
        reliable = in_clip & np.isfinite(pitches[begin:end]) & (measured[begin:end, 1] < 0.22)
        cents = float(np.median((pitches[begin:end][reliable] - pitch) * 100))
        if not np.isfinite(cents) or abs(cents) > 50:
            continue
        guide_times = centers[begin:end][reliable] / RATE - offset / RATE
        guide_pitch = median_filter(pitches[begin:end][reliable], size=3)
        if np.max(abs(guide_pitch - pitch)) > 2.0:
            continue
        bends = [{"seconds": float(t), "value": float(p - pitch)} for t, p in
                 zip(np.arange(0, SECONDS, 0.02),
                     np.interp(np.arange(0, SECONDS, 0.02), guide_times, guide_pitch), strict=True)]
        selected.append({"pitch": pitch, "tuning_cents": 0.0, "measured_cents": cents,
                         "bends": bends, "offset_seconds": offset / RATE,
                         "stable_end_seconds": float(stop)})
        if len(selected) == 2:
            break
    return selected


def prepare(root, captures):
    """Prepare fixed 1.2-second notes; training and validation singers never overlap."""
    notes = []
    (root / "notes").mkdir(exist_ok=True)
    for capture in captures:
        if not capture["path"].endswith(".wav"):
            continue
        path = root / capture["path"]
        if hashlib.sha256(path.read_bytes()).hexdigest() != capture["sha256"]:
            raise ValueError(f"capture hash mismatch: {path}")
        audio, rate = sf.read(path)
        if audio.ndim != 1 or not np.isfinite(audio).all():
            raise ValueError(f"expected finite mono recording: {path}")
        divisor = gcd(rate, RATE)
        audio = resample_poly(audio, RATE // divisor, rate // divisor)
        singer, vowel = path.stem.split("_")[0], path.stem[-1]
        selected = extract(audio)
        if not selected:
            raise ValueError(f"no reliable sustained note: {path}")
        for index, note in enumerate(selected):
            offset = round(note["offset_seconds"] * RATE)
            clip = audio[offset:offset + round(SECONDS * RATE)]
            name = f"notes/{path.stem}-{index}.wav"
            write_note(root / name, clip)
            notes.append(note | {"path": name, "source": capture["path"], "singer": singer,
                                 "vowel": VOWELS[vowel], "voice_size": VOICE_SIZE[singer],
                                 "velocity": 0.75,
                                 "split": "train" if singer in TRAIN_SINGERS else "validation",
                                 "sha256": hashlib.sha256((root / name).read_bytes()).hexdigest()})
        print(f"{path.name}: {[note['pitch'] for note in selected]}", flush=True)
    return {"rate": RATE, "seconds": SECONDS, "hold": SECONDS, "notes": notes,
            "source": "https://zenodo.org/records/1442513", "license": "CC-BY-4.0",
            "training_singers": list(TRAIN_SINGERS),
            "split_policy": "four voice types for fitting; all sixteen other singers held out",
            "extraction": "up to first two reliable stable notes, 10% local RMS onset, reference-only 20ms YIN pitch guide"}


def load_notes(root, manifest, split, vowel=None):
    """Read only the requested split, rejecting changed audio or invalid prepared notes."""
    notes = [note for note in manifest["notes"] if note["split"] == split
             and (vowel is None or note["vowel"] == vowel)]
    if not notes:
        raise ValueError("empty choir reference split")
    samples = []
    for note in notes:
        path = Path(root) / note["path"]
        if hashlib.sha256(path.read_bytes()).hexdigest() != note["sha256"]:
            raise ValueError(f"prepared note hash mismatch: {path}")
        audio, rate = sf.read(path)
        if rate != RATE or audio.ndim != 1 or len(audio) != round(manifest["seconds"] * RATE):
            raise ValueError(f"invalid prepared note: {path}")
        samples.append(audio)
    return notes, np.array(samples)
