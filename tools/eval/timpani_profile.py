# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12"]
# ///
"""Measure the physical timpani against single strokes from the VSCO-2 Community Edition.

A struck timpano is told from a plucked string less by its first few partials than by how
they change: which of them outlasts the others, and how much of the sound lies between them.
For each of four windows after the stroke (0-0.15, 0.15-0.5, 0.5-1.2 and 1.2-2.5 s) this
reads the share of the sound within 3.5 % of the note, the fifth, the octave and the tenth
over it, the share of everything else from 30 Hz to 2 kHz, and the window's level against
the first. The distance is the mean absolute difference, in decibels, between the model and
the mean of the recordings.

    uv run tools/eval/timpani_profile.py fetch target/timpani-reference
    uv run tools/eval/timpani_profile.py measure target/timpani-reference <worker>

The samples are CC0. Each drum's note was read from its recording: the partial with a fifth
and an octave over it.
"""

import argparse
import json
import math
import sys
from pathlib import Path
from urllib.request import urlopen

import numpy as np
import soundfile as sf
from fit_worker import Renderer

BASE = "https://raw.githubusercontent.com/sgossner/VSCO-2-CE/master/Percussion/Timpani/"
# The five drums' notes, in hertz, as their recordings sound them.
DRUMS = {"Timpani1": 89.8, "Timpani2": 121.5, "Timpani3": 143.0, "Timpani4": 169.0,
         "Timpani5": 190.4}
RATIOS = (1.0, 1.5, 2.0, 2.5)
WINDOWS = ((0.0, 0.15), (0.15, 0.5), (0.5, 1.2), (1.2, 2.5))
# The louder of the two dynamics every drum has; the model plays it at this velocity.
DYNAMIC = "v3"
VELOCITY = 0.85
RATE = 48_000


def name(drum):
    return f"{drum}_Hit_{DYNAMIC}_rr1_Sum.wav"


def balance(audio, rate, note):
    """One row a window: the five shares in dB of the window's sound, and its level."""
    onset = int(np.argmax(np.abs(audio) > 0.05 * np.abs(audio).max()))
    audio = audio[onset:]
    rows = []
    for start, end in WINDOWS:
        span = audio[int(start * rate):int(end * rate)]
        power = np.abs(np.fft.rfft(span * np.hanning(len(span)), 1 << 16)) ** 2
        hz = np.fft.rfftfreq(1 << 16, 1 / rate)
        band = (hz > 30) & (hz < 2000)
        total = power[band].sum()
        taken = np.zeros_like(band)
        shares = []
        for ratio in RATIOS:
            near = (hz > note * ratio * 0.965) & (hz < note * ratio * 1.035)
            shares.append(power[near].sum() / total)
            taken |= near
        shares.append(power[band & ~taken].sum() / total)
        rows.append([10 * math.log10(share + 1e-12) for share in shares]
                    + [10 * math.log10(total / len(span) + 1e-20)])
    rows = np.array(rows)
    rows[:, -1] -= rows[0, -1]
    return rows


def fetch(root):
    root.mkdir(parents=True, exist_ok=True)
    for drum in DRUMS:
        path = root / name(drum)
        if not path.exists():
            with urlopen(BASE + name(drum)) as response:
                path.write_bytes(response.read())
        print(path.name, file=sys.stderr)


def show(label, rows):
    print(label)
    for (start, end), row in zip(WINDOWS, rows, strict=True):
        print(f"  {start:4.2f}-{end:4.2f} s  note {row[0]:6.1f}  fifth {row[1]:6.1f}  "
              f"octave {row[2]:6.1f}  tenth {row[3]:6.1f}  rest {row[4]:6.1f}  level {row[5]:6.1f}")


def measure(root, worker):
    recorded = []
    for drum, note in DRUMS.items():
        audio, rate = sf.read(root / name(drum), always_2d=True)
        recorded.append(balance(audio.mean(axis=1), rate, note))
    recorded = np.mean(recorded, axis=0)
    renderer = Renderer(worker)
    try:
        played = []
        for note in DRUMS.values():
            pitch = round(69 + 12 * math.log2(note / 440))
            audio = renderer.render("timpani", [{"pitch": pitch, "velocity": VELOCITY}], {}, 3.0,
                                    0.2, rate=RATE)[0]
            played.append(balance(np.asarray(audio), RATE, 440 * 2 ** ((pitch - 69) / 12)))
    finally:
        renderer.close()
    played = np.mean(played, axis=0)
    show("recorded", recorded)
    show("model", played)
    # The first window's level is zero on both sides by definition.
    error = np.abs(played - recorded)
    print(json.dumps({"distance": float(np.mean(np.r_[error[:, :5].ravel(), error[1:, 5]]))}))


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("command", choices=("fetch", "measure"))
    parser.add_argument("root", type=Path)
    parser.add_argument("worker", type=Path, nargs="?")
    args = parser.parse_args()
    if args.command == "fetch":
        fetch(args.root)
    else:
        measure(args.root, args.worker)


if __name__ == "__main__":
    main()
