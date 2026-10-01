# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12"]
# ///
"""Fit compact radiation coloration from matched reference and dry-note probes.

The sampled instrument includes excitation, strings and recording coloration. This fit is
a regularized spectral prior, not an identification of an isolated soundboard response.
Only the twelve fitted EQ gains are used in the instrument; no audio is shipped.
"""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
import soundfile as sf
from scipy.ndimage import gaussian_filter1d
from scipy.optimize import least_squares
from scipy.signal import freqz, stft

CENTERS = np.geomspace(90, 10000, 12)
GRID = np.geomspace(80, 12000, 256)


def envelope(paths):
    rows = []
    for path in paths:
        audio, rate = sf.read(path, always_2d=True)
        audio = audio.mean(axis=1)[int(rate * 0.01):int(rate * 0.75)]
        rms = np.sqrt(np.mean(audio**2))
        if rms < 1e-8:
            raise ValueError(f"silent probe: {path}")
        frequencies, _, spectrum = stft(audio / rms, rate, nperseg=4096)
        power = np.mean(abs(spectrum)**2, axis=1)
        # Average power across pitches first, then smooth across harmonic gaps.
        rows.append(np.interp(GRID, frequencies, power))
    return gaussian_filter1d(10 * np.log10(np.mean(rows, axis=0) + 1e-10), 8)


def response(gains):
    magnitude = np.zeros_like(GRID)
    for center, gain in zip(CENTERS, gains, strict=True):
        omega = 2 * np.pi * center / 48000
        alpha = np.sin(omega) / (2 * 0.9)
        a = 10**(gain / 40)
        b = [1 + alpha * a, -2 * np.cos(omega), 1 - alpha * a]
        denominator = [1 + alpha / a, -2 * np.cos(omega), 1 - alpha / a]
        _, h = freqz(b, denominator, worN=2 * np.pi * GRID / 48000)
        magnitude += 20 * np.log10(abs(h))
    return magnitude


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    parser.add_argument("dry", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    report = {"centers_hz": CENTERS.tolist(), "q": 0.9, "models": {}}
    for model in ("piano", "guitar", "violin"):
        reference = sorted(args.reference.glob(f"{model}-*.wav"))
        if len(reference) != 18:
            raise ValueError(f"expected eighteen {model} probes, got {len(reference)}")
        dry = [args.dry / path.name for path in reference]
        target = envelope(reference) - envelope(dry)
        target -= target.mean()
        target = np.clip(target, -12, 12)
        fit = least_squares(
            lambda gains, target=target: np.r_[
                response(gains) - target, gains * 1.2, np.diff(gains) * 1.5
            ],
            np.zeros(12), bounds=(-6, 6),
        )
        before = float(np.sqrt(np.mean(target**2)))
        after = float(np.sqrt(np.mean((response(fit.x) - target)**2)))
        report["models"][model] = {
            "gains_db": np.round(fit.x, 3).tolist(),
            "before_envelope_error_db": before,
            "after_envelope_error_db": after,
            "inputs": {
                str(path): hashlib.sha256(path.read_bytes()).hexdigest()
                for path in (*reference, *dry)
            },
        }
        print(f"{model}: envelope error {before:.2f} -> {after:.2f} dB")
        print(", ".join(f"{gain:.3f}" for gain in fit.x))
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
