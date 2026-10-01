"""Fixed, gain-invariant temporal mel distance for copy synthesis (development only)."""

from functools import lru_cache

import numpy as np
from scipy.signal import stft

RATE = 24000
HOP = 240
WINDOWS = (512, 2048)
BANDS = 64
COMPRESSION = 10000.0


@lru_cache(maxsize=8)
def mel_bank(window, rate=RATE):
    """Unit-height triangular HTK mel bands from 30 Hz to 10 kHz."""
    mel = lambda hz: 2595 * np.log10(1 + hz / 700)
    edges = 700 * (10 ** (np.linspace(mel(30), mel(min(10000, rate / 2)), BANDS + 2) / 2595) - 1)
    frequencies = np.fft.rfftfreq(window, 1 / rate)
    left = (frequencies[None, :] - edges[:-2, None]) / np.diff(edges)[:-1, None]
    right = (edges[2:, None] - frequencies[None, :]) / np.diff(edges)[1:, None]
    bank = np.maximum(0, np.minimum(left, right))
    # Area normalization prevents wide high-frequency bands from dominating.
    return bank / np.maximum(bank.sum(axis=1, keepdims=True), 1e-12)


def normalize(audio):
    audio = np.asarray(audio, dtype=np.float64)
    if audio.ndim == 1:
        audio = audio[None, :]
    if audio.ndim != 2 or audio.shape[1] < max(WINDOWS) or not np.isfinite(audio).all():
        raise ValueError("expected finite mono notes with at least 2048 samples")
    rms = np.sqrt(np.mean(audio**2, axis=1, keepdims=True))
    if np.any(rms < 1e-8):
        raise ValueError("silent candidate or reference")
    return audio * (0.1 / rms)


def features(audio):
    """Normalize once per whole note; retain attack and decay at 10 ms resolution."""
    audio = normalize(audio)
    result = []
    for window in WINDOWS:
        _, _, spectrum = stft(audio, RATE, nperseg=window, noverlap=window - HOP,
                              boundary="zeros", padded=True, axis=-1)
        power = abs(spectrum)**2
        mel = mel_bank(window) @ power
        result.append(np.log1p(COMPRESSION * mel))
    return result


def distances(reference, candidate):
    """Equal-weight attack (first 150 ms) and sustain log-mel L1, per note."""
    errors = []
    for target, synth in zip(reference, candidate, strict=True):
        if target.shape != synth.shape:
            raise ValueError("reference and candidate shapes differ")
        difference = abs(target - synth)
        attack = difference[:, :, :15].mean(axis=(1, 2))
        sustain = difference[:, :, 15:].mean(axis=(1, 2))
        errors.append((attack + sustain) * 0.5)
    return np.mean(errors, axis=0)


def configuration():
    return {"rate": RATE, "windows": list(WINDOWS), "hop": HOP, "bands": BANDS,
            "fmin": 30, "fmax": 10000, "mel_scale": "HTK, area normalized",
            "normalization": "whole-note RMS 0.1, no frame normalization or time warp",
            "compression": COMPRESSION, "loss": "0.5 attack L1 + 0.5 sustain L1, mean of resolutions",
            "attack_frames": 15}
