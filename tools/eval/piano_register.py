# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12"]
# ///
"""Measure the native piano across its whole keyboard against Steinway notes from C1 to C8.

prepare   download the hash-locked Iowa notes into REFERENCE
evaluate  compare two workers' factory pianos note by note

Every measurement is relative within one note except `level`, which compares a note with the
same-dynamic C4 through offsets frozen at the start, and `knock`, which goes through the same
offsets. See docs/physical-copy-synthesis.md.
"""

import argparse
import hashlib
import json
import struct
import subprocess
from math import gcd
from pathlib import Path
from urllib.request import urlopen

import numpy as np
import soundfile as sf
from scipy.signal import resample_poly

REPO = Path(__file__).resolve().parents[2]
LOCK = REPO / "tools/eval/references/piano-register-captures.json"
RATE = 24_000
HOP = 600  # 25 ms
SECONDS = 8.0
T0 = 0.1
KNOCK_EDGES = ((40, 80), (80, 160), (160, 320), (320, 640), (640, 1280), (1280, 2560))
KNOCK_TIMES = (0.02, 0.06, 0.12)


def hz(pitch):
    return 440.0 * 2 ** ((pitch - 69) / 12)


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


class Worker:
    """A running `physical_fit_render`: JSON lines in, length-prefixed float32 PCM out."""

    def __init__(self, executable):
        self.process = subprocess.Popen(
            [str(executable)], stdin=subprocess.PIPE, stdout=subprocess.PIPE
        )

    def render(self, request):
        self.process.stdin.write((json.dumps(request) + "\n").encode())
        self.process.stdin.flush()
        header = self.process.stdout.read(4)
        if len(header) != 4:
            raise RuntimeError("worker closed before its PCM header")
        (count,) = struct.unpack("<I", header)
        data = self.process.stdout.read(count * 4)
        return np.frombuffer(data, dtype="<f4").astype(np.float64)

    def close(self):
        self.process.stdin.close()
        self.process.wait()


def onset(x):
    """Start of the rise into the loudest 2 ms frame, 25 dB below it."""
    hop = int(0.002 * RATE)
    energy = np.sqrt(np.convolve(x**2, np.ones(hop) / hop, mode="valid")[::hop])
    db = 20 * np.log10(energy + 1e-12)
    peak = int(np.argmax(db))
    index = peak
    while index > 0 and db[index] > db[peak] - 25:
        index -= 1
    return index * hop


def early_level(x):
    """Mean power from 50 to 500 ms after onset, in dB."""
    segment = x[int(0.05 * RATE) : int(0.5 * RATE)]
    return float(10 * np.log10(np.mean(segment**2) + 1e-20))


def stft(x, size):
    count = (len(x) - size) // HOP + 1
    frames = np.lib.stride_tricks.sliding_window_view(x, size)[::HOP][:count]
    window = np.hanning(size)
    return np.abs(np.fft.rfft(frames * window, axis=1)) * 2 / window.sum()


def find_partials(x, f0, count):
    """Measured partial frequencies, each search centred on the previous stretch."""
    segment = x[int(0.1 * RATE) : int(1.1 * RATE)]
    spectrum = np.abs(np.fft.rfft(segment * np.hanning(len(segment)), 1 << 19))
    freqs = np.fft.rfftfreq(1 << 19, 1 / RATE)
    found, ratio = [], 1.0
    for n in range(1, count + 1):
        target = n * f0 * ratio
        if target > 9000:
            break
        mask = (freqs > target * 0.985) & (freqs < target * 1.015)
        frequency = freqs[mask][np.argmax(spectrum[mask])]
        found.append(float(frequency))
        ratio = frequency / (n * f0)
    return found


def window_size(f0):
    return int(min(8192, 1 << int(np.ceil(np.log2(max(1024, 5 * RATE / f0))))))


def trajectories(x, freqs, size):
    """Partial levels in dB every 25 ms, the strongest bin within 0.6% of each frequency."""
    spectrum = stft(x, size)
    levels = []
    for frequency in freqs:
        low = max(1, int(frequency * 0.994 * size / RATE))
        high = max(low + 1, int(np.ceil(frequency * 1.006 * size / RATE)) + 1)
        levels.append(20 * np.log10(spectrum[:, low:high].max(axis=1) + 1e-12))
    times = (np.arange(spectrum.shape[0]) * HOP + size / 2) / RATE
    return times, np.array(levels)


def broadband(x):
    frames = np.lib.stride_tricks.sliding_window_view(x, 1200)[::HOP]
    return 10 * np.log10(np.mean(frames**2, axis=1) + 1e-20)


def band_power(x, start):
    size = 1024
    segment = x[start : start + size]
    if len(segment) < size:
        return np.full(len(KNOCK_EDGES), 1e-20)
    spectrum = np.abs(np.fft.rfft(segment * np.hanning(size))) ** 2
    freqs = np.fft.rfftfreq(size, 1 / RATE)
    return np.array(
        [spectrum[(freqs >= a) & (freqs < b)].sum() for a, b in KNOCK_EDGES]
    )


def knock_bands(x, f0, floor=None):
    """Absolute band powers in dB below 0.6 f0 at 20, 60 and 120 ms.

    With a floor, power less than 3 dB above it is unmeasurable and reported as None.
    """
    bands = []
    for seconds in KNOCK_TIMES:
        power = band_power(x, max(0, int(seconds * RATE) - 512))
        for index, (_, high) in enumerate(KNOCK_EDGES):
            if high > 0.6 * f0:
                bands.append(None)
            elif floor is None:
                bands.append(float(10 * np.log10(power[index] + 1e-20)))
            elif power[index] - floor[index] < floor[index]:
                bands.append(None)
            else:
                bands.append(float(10 * np.log10(power[index] - floor[index])))
    return bands


def with_floor(levels, floor):
    """A level heard through a noise floor at the given relative level."""
    return 10 * np.log10(10 ** (levels / 10) + 10 ** (floor / 10))


class Note:
    """One recorded note's measurements, at the analysis rate."""

    def __init__(self, path, capture):
        self.pitch = capture["pitch"]
        self.velocity = capture["velocity"]
        self.dynamic = capture["dynamic"]
        self.label = f"{self.dynamic}.{Path(path).name.split('.')[2]}"
        self.tuning = 0.0
        audio, rate = sf.read(path, always_2d=True)
        divisor = gcd(rate, RATE)
        x = resample_poly(audio.mean(axis=1), RATE // divisor, rate // divisor)
        start = onset(x)
        tail = x[-int(1.5 * RATE) :]
        self.level = early_level(x[start:])
        x = x[start : start + int(SECONDS * RATE)]
        self.f0 = hz(self.pitch)
        self.freqs = find_partials(x, self.f0, 12)
        self.size = window_size(self.freqs[0])
        self.times, self.levels = trajectories(x, self.freqs, self.size)
        self.floor = np.median(trajectories(tail, self.freqs, self.size)[1], axis=1)
        self.i0 = int(np.searchsorted(self.times, T0))
        self.broadband = broadband(x)
        self.broadband_floor = float(np.median(broadband(tail)))
        knock_floor = np.mean(
            [band_power(tail, i) for i in range(0, len(tail) - 1024, 1024)], axis=0
        )
        self.knock = knock_bands(x, self.freqs[0], knock_floor)
        start_levels = self.levels[:, self.i0]
        top = start_levels.max()
        # Partials worth measuring stand well clear of their floor and of the strongest one.
        self.use = [
            n
            for n in range(len(self.freqs))
            if start_levels[n] > self.floor[n] + 15 and start_levels[n] > top - 45
        ]


def note_losses(note, y, offset):
    """Mean absolute dB distances of one rendered note from its recording."""
    level = early_level(y[onset(y) :])
    y = y[onset(y) :][: int(SECONDS * RATE)]
    found = find_partials(y, note.f0, len(note.freqs))
    freqs = (found + note.freqs[len(found) :])[: len(note.freqs)]
    levels = trajectories(y, freqs, note.size)[1]
    frames = min(levels.shape[1], note.levels.shape[1])
    i0 = note.i0
    losses = {}
    long_enough = frames > i0 + 80
    if long_enough:
        # Decay shape: each partial relative to its own level at T0, through the same floor.
        error = weight = 0.0
        top = note.levels[note.use, i0].max()
        for n in note.use:
            recorded = note.levels[n, i0:frames] - note.levels[n, i0]
            floor = note.floor[n] - note.levels[n, i0]
            rendered = with_floor(levels[n, i0:frames] - levels[n, i0], floor)
            share = 10 ** ((note.levels[n, i0] - top) / 40)
            error += share * np.mean(np.abs(np.maximum(recorded, floor) - rendered))
            weight += share
        losses["decay"] = error / max(weight, 1e-9)
        envelope = broadband(y)
        count = min(len(envelope), len(note.broadband))
        recorded = note.broadband[i0:count] - note.broadband[i0]
        floor = note.broadband_floor - note.broadband[i0]
        rendered = with_floor(envelope[i0:count] - envelope[i0], floor)
        losses["envelope"] = float(
            np.mean(np.abs(np.maximum(recorded, floor) - rendered))
        )
    # Spectral balance: partial levels over 50-400 ms relative to their total.
    window = slice(max(1, i0 - 2), i0 + 12)
    recorded = note.levels[:, window].mean(axis=1)
    rendered = levels[:, window].mean(axis=1)
    recorded_total = 10 * np.log10(np.sum(10 ** (recorded[note.use] / 10)))
    rendered_total = 10 * np.log10(np.sum(10 ** (rendered[note.use] / 10)))
    differences = []
    for n in note.use:
        target = recorded[n] - recorded_total
        value = max(rendered[n] - rendered_total, target - 30)
        differences.append(abs(target - value) * (1.0 if target > -40 else 0.5))
    losses["spectrum"] = float(np.mean(differences))
    if offset is not None:
        losses["level"] = abs(note.level - (level + offset))
        knock = knock_bands(y, note.f0)
        pairs = [
            abs(a - (b + offset))
            for a, b in zip(note.knock, knock)
            if a is not None and b is not None
        ]
        losses["knock"] = float(np.mean(pairs)) if pairs else 0.0
    return losses


class Evaluator:
    def __init__(self, notes, worker):
        self.notes = notes
        self.worker = worker
        self.offsets = {}

    def render(self, notes):
        length = SECONDS + 0.2
        request = {
            "model": "piano",
            "notes": [
                {"pitch": n.pitch, "velocity": n.velocity, "tuning_cents": n.tuning}
                for n in notes
            ],
            "seconds": length,
            "hold": length,
            "params": {},
            "sample_rate": RATE,
        }
        pcm = self.worker.render(request)
        length = len(pcm) // len(notes)
        return [pcm[i * length : (i + 1) * length] for i in range(len(notes))]

    def freeze_levels(self):
        """Per-dynamic offsets from rendered to recorded level, taken at C4."""
        references = [n for n in self.notes if n.pitch == 60]
        for note, y in zip(references, self.render(references)):
            self.offsets[note.dynamic] = note.level - early_level(y[onset(y) :])

    def losses(self, notes):
        return {
            note.label: note_losses(note, y, self.offsets.get(note.dynamic))
            for note, y in zip(notes, self.render(notes))
        }


def read_lock():
    return json.loads(LOCK.read_text(encoding="utf-8"))["captures"]


def load_notes(root):
    notes = []
    for capture in read_lock():
        path = root / capture["path"]
        if digest(path) != capture["sha256"]:
            raise ValueError(f"reference hash mismatch: {path}")
        # The lock clears `fit` on the two captures that lie in their own noise floor.
        if capture["fit"]:
            notes.append(Note(path, capture))
    return notes


def prepare(root):
    root.mkdir(parents=True, exist_ok=True)
    for capture in read_lock():
        path = root / capture["path"]
        if not path.exists():
            with urlopen(capture["url"], timeout=120) as response:
                raw = response.read(40_000_001)
            if len(raw) > 40_000_000 or raw[:4] != b"FORM":
                raise ValueError(f"invalid AIFF: {path.name}")
            path.write_bytes(raw)
        if digest(path) != capture["sha256"]:
            raise ValueError(f"reference hash mismatch: {path}")
        print(path.name, flush=True)


def evaluate(root, before, after, output):
    notes = load_notes(root)
    report = {
        "before_sha256": digest(before),
        "after_sha256": digest(after),
        "references": {c["path"]: c["sha256"] for c in read_lock()},
        "notes": {},
    }
    means = {}
    for label, executable in (("before", before), ("after", after)):
        evaluator = Evaluator(notes, Worker(executable))
        evaluator.freeze_levels()
        losses = evaluator.losses(notes)
        evaluator.worker.close()
        for note, parts in losses.items():
            report["notes"].setdefault(note, {})[label] = parts
        keys = sorted({k for parts in losses.values() for k in parts})
        means[label] = {
            k: float(np.mean([p[k] for p in losses.values() if k in p])) for k in keys
        }
    report["means"] = means
    for key in means["before"]:
        print(
            f"{key:9s} {means['before'][key]:7.2f} dB -> {means['after'][key]:7.2f} dB"
        )
    output.write_text(json.dumps(report, indent=1) + "\n", encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("prepare")
    evaluator = commands.add_parser("evaluate")
    evaluator.add_argument("before", type=Path)
    evaluator.add_argument("after", type=Path)
    evaluator.add_argument("output", type=Path)
    args = parser.parse_args()
    if args.command == "prepare":
        prepare(args.reference)
    else:
        evaluate(args.reference, args.before, args.after, args.output)


if __name__ == "__main__":
    main()
