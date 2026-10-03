# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12"]
# ///
"""Check folk-model tuning against the strongest peak across the audible spectrum."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from fit_physical_mel import Renderer


def measure(audio, rate, pitch):
    """Report tuning, DC and fundamental dominance on a settled, held note."""
    x = np.asarray(audio, dtype=np.float64)[round(.35 * rate):]
    rms = float(np.sqrt(np.mean(x * x)))
    if rms < 1e-8:
        raise ValueError(f"silent MIDI {pitch} at {rate} Hz")
    spectrum = abs(np.fft.rfft((x - x.mean()) * np.hanning(len(x))))
    frequencies = np.fft.rfftfreq(len(x), 1 / rate)
    expected = 440 * 2 ** ((pitch - 69) / 12)
    region = np.flatnonzero((frequencies > expected * .8) & (frequencies < expected * 1.2))
    index = region[np.argmax(spectrum[region])]
    a, b, c = np.log(spectrum[index - 1:index + 2] + 1e-20)
    offset = np.clip(.5 * (a - c) / (a - 2 * b + c), -.5, .5)
    actual = (index + offset) * rate / len(x)
    audible = np.flatnonzero((frequencies > 20) & (frequencies < min(10_000, rate * .49)))
    strongest = audible[np.argmax(spectrum[audible])]
    return {"rms": rms, "dc_fraction": float(abs(x.mean()) / rms),
            "cents": float(1200 * np.log2(actual / expected)),
            "fundamental_peak_fraction": float(spectrum[index] / spectrum[strongest]),
            "strongest_hz": float(frequencies[strongest])}


def main(args):
    renderer = Renderer(args.renderer)
    rows = []
    try:
        for model, pitches in (("tin_whistle", range(74, 99)), ("hammered_dulcimer", range(48, 85))):
            for rate in (24_000, 48_000):
                notes = [{"pitch": pitch, "velocity": velocity, "tuning_cents": 0.0}
                         for pitch in pitches for velocity in (.35, .65, .95)]
                for start in range(0, len(notes), 12):
                    group = notes[start:start + 12]
                    audio = renderer.render(model, group, {}, 1.2, 1.2, rate=rate)
                    for note, x in zip(group, audio, strict=True):
                        rows.append({"model": model, "rate": rate, **note, **measure(x, rate, note["pitch"])})
    finally:
        renderer.close()
    summary = {}
    for model in ("tin_whistle", "hammered_dulcimer"):
        selected = [row for row in rows if row["model"] == model]
        summary[model] = {"conditions": len(selected),
                          "max_absolute_cents": max(abs(row["cents"]) for row in selected),
                          "min_fundamental_peak_fraction": min(row["fundamental_peak_fraction"] for row in selected),
                          "max_dc_fraction": max(row["dc_fraction"] for row in selected)}
    report = {"renderer_sha256": hashlib.sha256(args.renderer.read_bytes()).hexdigest(),
              "method": "1.2-s held notes; discard 350 ms; DC-subtracted Hann FFT; log-parabolic fundamental peak; strongest peak over 20 Hz..min(10 kHz, 0.49 fs)",
              "summary": summary, "notes": rows}
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("renderer", type=Path)
    parser.add_argument("output", type=Path)
    main(parser.parse_args())
