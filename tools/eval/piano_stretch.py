# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11"]
# ///
"""Read the physical piano's tuning off its own sound: fundamentals, heard pitch and octaves.

    uv run tools/eval/piano_stretch.py <worker> target/piano-stretch.json

Every figure is in cents. A fundamental and a heard pitch are measured from equal temperament
at A4 = 440 Hz. An octave is the lower note's partial minus the partial of the upper note it
should meet: below zero the two notes lie further apart than that pure octave, above zero
closer, so a negative 2:1 beside a positive 4:2 is an octave tuned between the two.
"""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from fit_worker import Renderer

RATE = 48000
VELOCITY = 0.7
SECONDS = 2.5
KEYS = (("A0", 21), ("C1", 24), ("A1", 33), ("C2", 36), ("A2", 45), ("C3", 48),
        ("C4", 60), ("A4", 69), ("C5", 72), ("C6", 84), ("C7", 96), ("C8", 108))
TOP = 108
# The stretches of a note that are read, in seconds: its partials once the strike is over, and
# its period while the overtones that pull it are still there.
STEADY = (0.3, 2.3)
EARLY = (0.05, 0.55)
# A partial this far below the note's strongest is not one an octave can be judged by.
FLOOR_DB = -60.0
# Lags to the sample when a period is looked for: the top octave's is a dozen samples long.
FINE = 8
# A heard pitch that moves further than this with the weight given to the ends of the stretch
# it is read over is not one figure.
HEARD_TOLERANCE = 1.0


def hz(pitch):
    return 440.0 * 2 ** ((pitch - 69) / 12)


def spectrum(audio, rate):
    """The magnitude spectrum through a Hann window, on a grid far finer than its main lobe.

    A bass note's fundamental can lie forty decibels under its neighbours, and a bare
    rectangle lets them leak onto it: read that way, A0 comes out two cents from where a
    twelve-second stretch puts it.
    """
    size = 1 << int(np.ceil(np.log2(len(audio) * 32)))
    return abs(np.fft.rfft(audio * np.hanning(len(audio)), size)), rate / size


def partial(magnitudes, resolution, target, span=150.0):
    """Where the strongest frequency within `span` cents of `target` lies, and how strong.

    Returns cents from `target` and decibels below the strongest partial of the note, or
    `None` where nothing peaks inside the span.
    """
    first = int(np.ceil(target * 2 ** (-span / 1200) / resolution))
    last = int(np.floor(target * 2 ** (span / 1200) / resolution))
    if first < 1 or last > len(magnitudes) - 2 or last <= first:
        return None
    peak = first + int(np.argmax(magnitudes[first:last + 1]))
    if peak in (first, last):
        return None
    a, b, c = magnitudes[peak - 1:peak + 2]
    frequency = (peak + 0.5 * (a - c) / (a - 2 * b + c)) * resolution
    return float(1200 * np.log2(frequency / target)), float(20 * np.log10(b / magnitudes.max()))


def heard_cents(audio, rate, pitch, weights=None, span=100.0):
    """Cents from equal temperament of the period the note repeats at, which is what is heard.

    The period is the lag at which the note best matches itself, each sample counting for
    its `weights` (evenly, if none are given). A lag is judged by the energy of the two
    stretches it lays side by side, weighted as their products are: fewer samples overlap at a
    longer lag, and a bare sum of products reads a low note a quarter of a cent sharp for it.
    Judged so, a note that holds its period reads the same under any weights.
    """
    expected = hz(pitch)
    audio = np.asarray(audio, dtype=np.float64)
    audio = audio - audio.mean()
    count = len(audio)
    weights = np.ones(count) if weights is None else np.asarray(weights, dtype=np.float64)
    size = 1 << int(np.ceil(np.log2(count * 2)))
    # A power spectrum padded with zeros is the same autocorrelation at lags between samples.
    products = np.fft.irfft(abs(np.fft.rfft(audio * weights, size)) ** 2, size * FINE)[:count * FINE]
    shape = np.fft.rfft(weights, size)
    energy = np.fft.rfft(weights * audio ** 2, size)
    leading = np.fft.irfft(np.conj(energy) * shape, size)[:count]
    trailing = np.fft.irfft(np.conj(shape) * energy, size)[:count]
    whole = np.arange(count)
    lags = np.arange(count * FINE) / FINE
    overlap = np.interp(lags, whole, leading) * np.interp(lags, whole, trailing)
    correlation = products / np.sqrt(np.maximum(overlap, 1e-30))
    first = max(1, int(np.floor(FINE * rate / (expected * 2 ** (span / 1200)))))
    last = int(np.ceil(FINE * rate / (expected * 2 ** (-span / 1200))))
    if last >= count * FINE // 2:
        raise ValueError("note too short for its own period")
    lag = first + int(np.argmax(correlation[first:last + 1]))
    a, b, c = correlation[lag - 1:lag + 2]
    curvature = a - 2 * b + c
    offset = float(np.clip(0.5 * (a - c) / curvature, -0.5, 0.5)) if curvature < 0 else 0.0
    return float(1200 * np.log2(FINE * rate / (lag + offset) / expected))


def heard_readings(audio, rate, pitch):
    """The heard pitch read twice: every sample counting alike, and weighted by a Hann window.

    The window favours the middle of the stretch. A note that holds its period reads the same
    both ways; one that dies away within the stretch, as the top octave does, has a period
    that moves as its overtones go, and the two readings part.
    """
    return heard_cents(audio, rate, pitch), heard_cents(audio, rate, pitch, np.hanning(len(audio)))


def octave(lower, upper, low_hz, n):
    """The lower note's `n`th partial minus the upper note's partial at the same place, in cents.

    `lower` and `upper` are `spectrum` results. `None` where either partial is missing or too
    weak to judge an octave by.
    """
    readings = [partial(*note, low_hz * n) for note in (lower, upper)]
    if any(reading is None or reading[1] < FLOOR_DB for reading in readings):
        return None
    return readings[0][0] - readings[1][0]


def cut(samples, stretch, rate):
    return samples[round(stretch[0] * rate):round(stretch[1] * rate)]


def measure(renderer, rate=RATE):
    pitches = sorted({pitch for _, pitch in KEYS} | {pitch + 12 for _, pitch in KEYS if pitch + 12 <= TOP})
    notes = [{"pitch": pitch, "velocity": VELOCITY} for pitch in pitches]
    audio = dict(zip(pitches, renderer.render("piano", notes, {}, SECONDS, SECONDS, rate=rate), strict=True))
    steady = {pitch: spectrum(cut(samples, STEADY, rate), rate) for pitch, samples in audio.items()}
    rows = []
    for name, pitch in KEYS:
        fundamental = partial(*steady[pitch], hz(pitch))
        if fundamental is None:
            raise ValueError(f"{name} has no fundamental within 150 cents of equal temperament")
        readings = heard_readings(cut(audio[pitch], EARLY, rate), rate, pitch)
        settled = abs(readings[0] - readings[1]) <= HEARD_TOLERANCE
        row = {"key": name, "pitch": pitch, "fundamental": fundamental[0],
               "heard": readings[0] if settled else None, "heard_readings": list(readings)}
        if pitch + 12 <= TOP:
            row["octave_2_1"] = octave(steady[pitch], steady[pitch + 12], hz(pitch), 2)
            row["octave_4_2"] = octave(steady[pitch], steady[pitch + 12], hz(pitch), 4)
        rows.append(row)
    return rows


def shown(value):
    """A figure to a tenth of a cent, with no sign on a zero and a dash where there is none."""
    if value is None:
        return "-"
    text = f"{value:+.1f}"
    return "0.0" if text[1:] == "0.0" else text


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("renderer", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    renderer = Renderer(args.renderer.resolve())
    try:
        rows = measure(renderer)
    finally:
        renderer.close()
    report = {"renderer_sha256": hashlib.sha256(args.renderer.read_bytes()).hexdigest(),
              "rate": RATE, "velocity": VELOCITY, "seconds": SECONDS,
              "method": f"partials: strongest frequency in the Hann-windowed spectrum of {STEADY[0]:g}-{STEADY[1]:g} s; "
                        f"heard pitch: normalized autocorrelation peak of {EARLY[0]:g}-{EARLY[1]:g} s, given where a "
                        f"Hann-weighted reading of the same stretch lies within {HEARD_TOLERANCE:g} cent; "
                        f"octaves: lower note's partial minus the upper note's, above {FLOOR_DB:g} dB only",
              "keys": rows}
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print("key  fundamental   heard     2:1     4:2")
    for row in rows:
        cells = "".join(f"{shown(row.get(key)):>8}" for key in ("heard", "octave_2_1", "octave_4_2"))
        print(f"{row['key']:<4}{shown(row['fundamental']):>12}{cells}")


if __name__ == "__main__":
    main()
