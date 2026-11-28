# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12"]
# ///
"""Measure and fit the physical winds against University of Iowa MIS recordings.

The reference is each note's first ten harmonics, in decibels under its strongest, read
from the settled part of the note (0.25 to 0.85 s after its onset). The instrument plays the
same pitch at a velocity for each dynamic and is read the same way, from 0.3 to 0.8 s; the
distance is the mean absolute difference over the ten, with both floored at -60 dB.

    uv run tools/eval/wind_profile.py fetch target/wind-reference [Flute]
    uv run tools/eval/wind_profile.py measure target/wind-reference <worker>
    uv run tools/eval/wind_profile.py fit target/wind-reference <calibration-worker> Trumpet

`fit` needs a worker built with `--features physical-calibration`; it searches the dials of
the instrument's family in `auris_synth::physical::dials` on the lowest and highest note of
each capture and reports the distance on the middle one, which it was not shown.
"""

import argparse
import json
import math
import re
import sys
import time
from pathlib import Path
from urllib.parse import quote
from urllib.request import urlopen

import numpy as np
import soundfile as sf
from scipy.optimize import differential_evolution

from fit_worker import Renderer

BASE = "https://theremin.music.uiowa.edu/sound%20files/MIS/"
DYNAMICS = ("pp", "mf", "ff")
# Concert-pitch chromatic captures, without vibrato where there is a choice.
CAPTURES = {
    "Trumpet": [f"Brass/Bbtrumpet/Trumpet.novib.{d}.{s}.aiff" for d in DYNAMICS
                for s in ("C4B4", "C5B5")],
    "Horn": [f"Brass/frenchhorn/Horn.{d}.C4B4.aiff" for d in DYNAMICS],
    "TenorTrombone": [f"Brass/tenortrombone/TenorTrombone.{d}.C3B3.aiff" for d in DYNAMICS],
    "Tuba": [f"Brass/tuba/Tuba.{d}.C2B2.aiff" for d in DYNAMICS],
    "Flute": [f"Woodwinds/flute/Flute.nonvib.{d}.{s}.aiff" for d in DYNAMICS
              for s in ("B3B4", "C6B6" if d != "pp" else "C6Bb6")],
    "Oboe": [f"Woodwinds/oboe/Oboe.{d}.{s}.aiff" for d in DYNAMICS for s in ("C4B4", "C5B5")],
    "Bassoon": [f"Woodwinds/bassoon/Bassoon.{d}.{s}.aiff" for d in DYNAMICS
                for s in ("C2B2", "C3B3" if d != "ff" else "C3Bb3")],
}
# Which model plays each, and with what instrument parameter.
PLAYED = {"Trumpet": ("brass", 0), "Horn": ("brass", 1), "TenorTrombone": ("brass", 2),
          "Tuba": ("brass", 3), "Flute": ("tin_whistle", 1), "Oboe": ("clarinet", 1),
          "Bassoon": ("clarinet", 3)}
VELOCITY = {"pp": 0.3, "mf": 0.65, "ff": 1.0}
RATE = 48_000
# Dial number, lowest and highest value searched, for each model.
DIALS = {
    "brass": [(100, 200.0, 1800.0), (101, 1500.0, 12000.0), (102, 0.0, 8e-3), (103, 0.055, 0.3),
              (104, 0.2, 2.0), (108, 0.5, 4.0), (105, 0.4, 0.95), (106, 1.5, 10.0),
              (107, 0.5, 10.0)],
    "tin_whistle": [(113, -0.6, 0.6), (114, 1500.0, 12000.0), (115, -0.3, 0.8),
                    (116, 1.0, 4.0), (117, 1.0, 1.4), (118, 0.0, 1.5)],
    "clarinet": [(119, 250.0, 4000.0), (120, 0.15, 2.0), (121, 0.2, 1.5), (122, 0.1, 0.9),
                 (124, 1000.0, 12000.0), (125, 0.1, 0.8), (127, 0.15, 0.7)],
}
SEED = 20261128
NOTE = {"C": 0, "Db": 1, "D": 2, "Eb": 3, "E": 4, "F": 5, "Gb": 6, "G": 7, "Ab": 8, "A": 9,
        "Bb": 10, "B": 11}


def first_note(span):
    """The MIDI note a capture's span starts on: "B3B4" is 59."""
    name, octave = re.match(r"([A-G]b?)(\d)", span).groups()
    return 12 * (int(octave) + 1) + NOTE[name]


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
    # Forty decibels under the loudest, or ten over the room's hiss where that is higher —
    # but never closer than 25 under the loudest, which a capture with little silence between
    # its notes would otherwise have as its hiss.
    hiss = min(np.percentile(level, 10) * 3.2, level.max() * 10 ** (-25 / 20))
    sounding = level > max(level.max() * 10 ** (-40 / 20), hiss)
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


def identify(samples, rate, candidates):
    """Which of `candidates` a settled segment sounds.

    A capture's notes are not always all there or all whole — a quiet one is missed, a long
    one split — so a note is named by what it sounds and not only by where it falls in the
    file. Each candidate scores the harmonics it predicts against the points half-way between
    them, which keeps both its octave below (whose odd multiples fall between the true
    harmonics) and its octave above (whose half-way points are the true odd harmonics) from
    winning; the caller keeps the candidates to a few semitones, where nothing else can.
    """
    t = np.arange(len(samples)) / rate
    window = np.hanning(len(samples))

    def at(hz):
        return abs(np.sum(samples * window * np.exp(-2j * math.pi * hz * t)))

    def score(pitch):
        f = 440 * 2 ** ((pitch - 69) / 12)
        orders = [k for k in range(1, 9) if k * f < rate * 0.45]
        return (sum(at(k * f) for k in orders) - sum(at((k + 0.5) * f) for k in orders)) / len(
            orders)

    return max(candidates, key=score)


def fetch(root, only=None):
    root.mkdir(parents=True, exist_ok=True)
    profiles = root / "profiles.json"
    reference = json.loads(profiles.read_text(encoding="utf-8")) if profiles.exists() else {}
    for instrument, captures in CAPTURES.items():
        if only and instrument != only:
            continue
        for capture in captures:
            path = root / Path(capture).name
            if not path.exists():
                with urlopen(BASE + quote(capture)) as response:
                    path.write_bytes(response.read())
            audio, rate = sf.read(path, always_2d=True)
            audio = audio.mean(axis=1)
            dynamic, span = path.stem.split(".")[-2:]
            rows, last = [], None
            for start, end in notes(audio, rate):
                settled = audio[start + int(0.25 * rate): min(end, start + int(0.85 * rate))]
                if len(settled) < rate * 0.2:
                    continue
                # A chromatic capture climbs: each note is the last, if the segmentation split
                # it, or up to three semitones over it, if it missed some on the way.
                candidates = (range(first_note(span), first_note(span) + 3) if last is None
                              else range(last, last + 4))
                pitch = identify(settled, rate, candidates)
                if pitch == last:
                    continue
                last = pitch
                written = 440 * 2 ** ((pitch - 69) / 12)
                heard = nearest(settled, rate, written)
                if abs(1200 * math.log2(heard / written)) > 45:
                    # Off by the edge of the search: a note the segmentation missed or split.
                    continue
                rows.append({"pitch": pitch, "cents": 1200 * math.log2(heard / written),
                             "harmonics_db": [round(float(x), 1)
                                              for x in profile(settled, rate, heard)]})
            reference[f"{instrument}.{dynamic}.{span}"] = {"instrument": instrument,
                                                         "dynamic": dynamic, "notes": rows}
            print(f"{path.name}: {len(rows)} notes", file=sys.stderr)
    profiles.write_text(json.dumps(reference, indent=1), encoding="utf-8")


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
    model, choice = PLAYED[instrument]
    for dynamic, rows in by_dynamic.items():
        played = renderer.render(model, [{"pitch": r["pitch"], "velocity": VELOCITY[dynamic]}
                                         for r in rows],
                                 {"instrument": choice}, 1.0, 0.9, rate=RATE, dials=dials)
        for row, samples in zip(rows, played, strict=True):
            settled = np.asarray(samples)[int(0.3 * RATE):int(0.8 * RATE)]
            if not np.all(np.isfinite(settled)) or np.sqrt(np.mean(settled ** 2)) < 1e-5:
                errors.append(60.0)
                continue
            heard = np.maximum(profile(settled, RATE, 440 * 2 ** ((row["pitch"] - 69) / 12)), -60)
            recorded = np.maximum(np.array(row["harmonics_db"]), -60)
            errors.append(float(np.mean(np.abs(heard - recorded))))
            if verbose:
                print(f"  {instrument} {dynamic} {row['pitch']}: {errors[-1]:4.1f} dB  model "
                      f"{[round(float(x)) for x in heard]}  recorded "
                      f"{[round(x) for x in row['harmonics_db']]}")
    return float(np.mean(errors))


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("command", choices=("fetch", "measure", "fit"))
    parser.add_argument("root", type=Path)
    parser.add_argument("worker", type=Path, nargs="?")
    parser.add_argument("instrument", nargs="?", choices=tuple(PLAYED))
    parser.add_argument("--generations", type=int, default=30)
    args = parser.parse_args()
    if args.command == "fetch":
        # `fetch DIR [INSTRUMENT]`: the instrument stands where a worker would.
        fetch(args.root, args.instrument or (str(args.worker) if args.worker else None))
        return
    reference = json.loads((args.root / "profiles.json").read_text(encoding="utf-8"))
    renderer = Renderer(args.worker)
    try:
        if args.command == "measure":
            summary = {}
            for instrument in [args.instrument] if args.instrument else PLAYED:
                train, validation = cohort(reference, instrument)
                summary[instrument] = {
                    "fitted_on": distance(renderer, instrument, train),
                    "held_out": distance(renderer, instrument, validation, verbose=True)}
            print(json.dumps(summary, indent=1))
            return
        train, validation = cohort(reference, args.instrument)
        family = DIALS[PLAYED[args.instrument][0]]
        started = time.time()
        result = differential_evolution(
            lambda x: distance(renderer, args.instrument, train,
                               {d[0]: float(v) for d, v in zip(family, x, strict=True)}),
            [(d[1], d[2]) for d in family], seed=SEED, popsize=8, maxiter=args.generations,
            tol=1e-3, polish=False, updating="immediate", workers=1)
        dials = {d[0]: float(v) for d, v in zip(family, result.x, strict=True)}
        print(json.dumps({"instrument": args.instrument, "fitted_on": result.fun,
                          "held_out": distance(renderer, args.instrument, validation, dials),
                          "seconds": round(time.time() - started),
                          "dials": {k: round(v, 5) for k, v in dials.items()}}, indent=1))
    finally:
        renderer.close()


if __name__ == "__main__":
    main()
