# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12"]
# ///
"""Measure and fit the physical brass against University of Iowa MIS recordings.

The reference is each note's first ten harmonics, in decibels under its strongest, read
from the settled part of the note (0.25 to 0.85 s after its onset). The instrument plays the
same pitch at a velocity for each dynamic and is read the same way, from 0.3 to 0.8 s; the
distance is the mean absolute difference over the ten, with both floored at -60 dB.

    uv run tools/eval/brass_profile.py fetch target/brass-reference
    uv run tools/eval/brass_profile.py measure target/brass-reference <worker>
    uv run tools/eval/brass_profile.py fit target/brass-reference <calibration-worker> Trumpet

`fit` needs a worker built with `--features physical-calibration`; it searches the brass
dials of `auris_synth::physical::dials` on the lowest and highest note of each capture and
reports the distance on the middle one, which it was not shown.
"""

import argparse
import json
import math
import sys
import time
from pathlib import Path
from urllib.parse import quote
from urllib.request import urlopen

import numpy as np
import soundfile as sf
from fit_worker import Renderer
from scipy.optimize import differential_evolution

BASE = "https://theremin.music.uiowa.edu/sound%20files/MIS/Brass/"
# Concert-pitch chromatic captures; the first MIDI note of each span.
CAPTURES = {
    "Trumpet": [f"Bbtrumpet/Trumpet.novib.{d}.{s}.aiff" for d in ("pp", "mf", "ff")
                for s in ("C4B4", "C5B5")],
    "Horn": [f"frenchhorn/Horn.{d}.C4B4.aiff" for d in ("pp", "mf", "ff")],
    "TenorTrombone": [f"tenortrombone/TenorTrombone.{d}.C3B3.aiff" for d in ("pp", "mf", "ff")],
    "Tuba": [f"tuba/Tuba.{d}.C2B2.aiff" for d in ("pp", "mf", "ff")],
}
FIRST = {"C4B4": 60, "C5B5": 72, "C3B3": 48, "C2B2": 36}
INSTRUMENT = {"Trumpet": 0, "Horn": 1, "TenorTrombone": 2, "Tuba": 3}
VELOCITY = {"pp": 0.3, "mf": 0.65, "ff": 1.0}
RATE = 48_000
# Dial number, lowest and highest value searched.
DIALS = [(100, 200.0, 1800.0), (101, 1500.0, 12000.0), (102, 0.0, 8e-3), (103, 0.055, 0.3),
         (104, 0.2, 2.0), (108, 0.5, 4.0), (105, 0.4, 0.95), (106, 1.5, 10.0), (107, 0.5, 10.0)]
SEED = 20261128


def harmonics(samples, rate, fundamental, count=16):
    t = np.arange(len(samples)) / rate
    window = np.hanning(len(samples))
    return np.array([abs(np.sum(samples * window * np.exp(-2j * math.pi * fundamental * k * t)))
                     for k in range(1, count + 1)]) * 2 / np.sum(window)


def profile(samples, rate, fundamental):
    """The first ten harmonics in dB under the strongest of sixteen."""
    level = harmonics(samples, rate, fundamental)
    return 20 * np.log10(level / level.max() + 1e-9)[:10]


def notes(audio, rate):
    """Onset and end of each note of a chromatic capture, from a 5 ms RMS envelope."""
    hop = int(rate * 0.005)
    count = len(audio) // hop
    level = np.sqrt(np.mean(audio[:count * hop].reshape(count, hop) ** 2, axis=1))
    sounding = level > level.max() * 10 ** (-40 / 20)
    edges = np.diff(np.r_[False, sounding, False].astype(int))
    merged = []
    for start, end in zip(np.flatnonzero(edges == 1), np.flatnonzero(edges == -1), strict=True):
        if merged and start - merged[-1][1] < 12:
            merged[-1][1] = end
        else:
            merged.append([start, end])
    return [(start * hop, end * hop) for start, end in merged if (end - start) * 0.005 > 0.3]


def nearest(samples, rate, fundamental):
    candidates = fundamental * 2 ** (np.arange(-50, 51) / 1200)
    t = np.arange(len(samples)) / rate
    window = np.hanning(len(samples))
    power = [abs(np.sum(samples * window * np.exp(-2j * math.pi * c * t))) for c in candidates]
    return candidates[int(np.argmax(power))]


def fetch(root):
    root.mkdir(parents=True, exist_ok=True)
    reference = {}
    for instrument, captures in CAPTURES.items():
        for capture in captures:
            path = root / Path(capture).name
            if not path.exists():
                with urlopen(BASE + quote(capture)) as response:
                    path.write_bytes(response.read())
            audio, rate = sf.read(path, always_2d=True)
            audio = audio.mean(axis=1)
            dynamic, span = path.stem.split(".")[-2:]
            rows = []
            for index, (start, end) in enumerate(notes(audio, rate)):
                pitch = FIRST[span] + index
                settled = audio[start + int(0.25 * rate): min(end, start + int(0.85 * rate))]
                if len(settled) < rate * 0.2:
                    continue
                written = 440 * 2 ** ((pitch - 69) / 12)
                heard = nearest(settled, rate, written)
                rows.append({"pitch": pitch, "cents": 1200 * math.log2(heard / written),
                             "harmonics_db": [round(float(x), 1)
                                              for x in profile(settled, rate, heard)]})
            reference[f"{instrument}.{dynamic}.{span}"] = {"instrument": instrument,
                                                         "dynamic": dynamic, "notes": rows}
            print(f"{path.name}: {len(rows)} notes", file=sys.stderr)
    (root / "profiles.json").write_text(json.dumps(reference, indent=1), encoding="utf-8")


def cohort(reference, instrument):
    """The lowest and highest note of each capture to fit on, the middle one to validate."""
    train, validation = [], []
    for capture in reference.values():
        if capture["instrument"] != instrument or not capture["notes"]:
            continue
        rows = capture["notes"]
        train += [(capture["dynamic"], row) for row in
                  ([rows[0], rows[-1]] if len(rows) > 2 else rows)]
        if len(rows) > 2:
            validation.append((capture["dynamic"], rows[len(rows) // 2]))
    return train, validation


def distance(renderer, instrument, notes, dials=None, verbose=False):
    errors = []
    by_dynamic = {}
    for dynamic, row in notes:
        by_dynamic.setdefault(dynamic, []).append(row)
    for dynamic, rows in by_dynamic.items():
        played = renderer.render("brass", [{"pitch": r["pitch"], "velocity": VELOCITY[dynamic]}
                                           for r in rows],
                                 {"instrument": INSTRUMENT[instrument]}, 1.0, 0.9, rate=RATE,
                                 dials=dials)
        for row, samples in zip(rows, played, strict=True):
            settled = np.asarray(samples)[int(0.3 * RATE):int(0.8 * RATE)]
            if not np.all(np.isfinite(settled)) or np.sqrt(np.mean(settled ** 2)) < 1e-5:
                errors.append(60.0)
                continue
            model = np.maximum(profile(settled, RATE, 440 * 2 ** ((row["pitch"] - 69) / 12)), -60)
            recorded = np.maximum(np.array(row["harmonics_db"]), -60)
            errors.append(float(np.mean(np.abs(model - recorded))))
            if verbose:
                print(f"  {instrument} {dynamic} {row['pitch']}: {errors[-1]:4.1f} dB  model "
                      f"{[round(float(x)) for x in model]}  recorded "
                      f"{[round(x) for x in row['harmonics_db']]}")
    return float(np.mean(errors))


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("command", choices=("fetch", "measure", "fit"))
    parser.add_argument("root", type=Path)
    parser.add_argument("worker", type=Path, nargs="?")
    parser.add_argument("instrument", nargs="?", choices=tuple(INSTRUMENT))
    parser.add_argument("--generations", type=int, default=30)
    args = parser.parse_args()
    if args.command == "fetch":
        fetch(args.root)
        return
    reference = json.loads((args.root / "profiles.json").read_text(encoding="utf-8"))
    renderer = Renderer(args.worker)
    try:
        if args.command == "measure":
            summary = {}
            for instrument in INSTRUMENT:
                train, validation = cohort(reference, instrument)
                summary[instrument] = {
                    "fitted_on": distance(renderer, instrument, train),
                    "held_out": distance(renderer, instrument, validation, verbose=True)}
            print(json.dumps(summary, indent=1))
            return
        train, validation = cohort(reference, args.instrument)
        started = time.time()
        result = differential_evolution(
            lambda x: distance(renderer, args.instrument, train,
                               {d[0]: float(v) for d, v in zip(DIALS, x, strict=True)}),
            [(d[1], d[2]) for d in DIALS], seed=SEED, popsize=8, maxiter=args.generations,
            tol=1e-3, polish=False, updating="immediate", workers=1)
        dials = {d[0]: float(v) for d, v in zip(DIALS, result.x, strict=True)}
        print(json.dumps({"instrument": args.instrument, "fitted_on": result.fun,
                          "held_out": distance(renderer, args.instrument, validation, dials),
                          "seconds": round(time.time() - started),
                          "dials": {k: round(v, 5) for k, v in dials.items()}}, indent=1))
    finally:
        renderer.close()


if __name__ == "__main__":
    main()
