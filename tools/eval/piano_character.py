# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12"]
# ///
"""Score the native piano against the recorded Steinway on what a note does, not on one take.

prepare   download the hash-locked Iowa notes into REFERENCE
stats     measure the recordings once and keep what was measured beside them
score     render every locked note through WORKER and score it against the recordings
compare   score two workers and print both

The other piano measures (`piano_register.py`, `evaluate_physical_mel.py`) compare a rendered
note with one recording frame by frame. That holds a model to the recorded piano's own beats,
whose timing is that one piano's and that one blow's, and it says nothing about what makes a
bass note sound like wire rather than a spring. The measures here are statistics of a note,
each read the same way from the recording and the render:

balance   octave bands against the note's total, at the blow, in the tone and later on
level     each note's level against the mezzo-forte notes' mean, so the keyboard and the
          range of touch are both in it and the overall gain is not
decay     how fast each octave band of partials falls in the first second and the next two
spread    how unevenly the partials of one note fall: the spread of their first-second decays
stages    how far the first partials fall in their first second and in the two after, the
          median over the note's partials of each: a unison's prompt sound and its
          aftersound, where a band's median slope averages the two into one
wobble    how far a partial swings about its own decay, the beats of a unison and its planes
metal     the energy in peaks that belong to no transverse partial (longitudinal modes and
          phantom partials), from A0 to A4
pulse     how strongly the bands from 500 Hz to 4 kHz pulse once a period, which is what a
          coil spring's dispersion does and a struck wire's does not, from A0 to E4

Every measure is in decibels; the score of each is the rms of model minus recording over every
note it applies to, divided by the rms the recordings reach against one another on it — each
note against the nearest recorded note of the same dynamic. A measure that one key of the
Steinway already misses its neighbour by ten decibels on is not ten times as telling as one it
misses by one, and on the raw decibels the beats of a unison, which the ear is quick to, were
outweighed by everything else. One is as far from the recording as its own neighbouring keys
are; the total is the mean. See docs/physical-copy-synthesis.md.
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
from scipy.ndimage import median_filter
from scipy.signal import butter, hilbert, resample_poly, sosfiltfilt

REPO = Path(__file__).resolve().parents[2]
LOCK = REPO / "tools/eval/references/piano-character-captures.json"
RATE = 32_000
SECONDS = 4.0
FFT = 4096
HOP = 256
FRAME_RATE = RATE / HOP
BANDS = [(125 * 2**i, 250 * 2**i) for i in range(7)]  # 125 Hz .. 16 kHz
WINDOWS = {"attack": (0.0, 0.08), "tone": (0.08, 0.6), "late": (0.6, 2.0)}
PULSE_BANDS = [(500, 1000), (1000, 2000), (2000, 4000)]
METRICS = ("balance", "level", "decay", "spread", "stages", "wobble", "metal", "pulse")
STAGE_PARTIALS = 12
# Where a partial's level is read for its two stages: the blow, a second on, three seconds on.
STAGE_TIMES = ((0.05, 0.15), (0.9, 1.1), (2.8, 3.2))
METAL_TOP = 69
PULSE_TOP = 64
PARTIALS = 24
TOP_HZ = 8000.0


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def hz(pitch):
    return 440.0 * 2.0 ** ((pitch - 69) / 12.0)


class Worker:
    """The `physical_fit_render` example, one note per request."""

    def __init__(self, executable):
        self.process = subprocess.Popen(
            [str(Path(executable).resolve())],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
        )

    def render(self, pitch, velocity, dials=None):
        request = {
            "model": "piano",
            "notes": [{"pitch": pitch, "velocity": velocity, "tuning_cents": 0.0}],
            "seconds": SECONDS + 0.5,
            "hold": SECONDS + 0.5,
            "params": {},
            "sample_rate": RATE,
        }
        if dials:
            request["dials"] = dials
        self.process.stdin.write((json.dumps(request) + "\n").encode())
        self.process.stdin.flush()
        (count,) = struct.unpack("<I", self.process.stdout.read(4))
        data = self.process.stdout.read(count * 4)
        return np.frombuffer(data, dtype="<f4").astype(np.float64)

    def close(self):
        self.process.stdin.close()
        self.process.wait(timeout=30)


def onset(x):
    hop = int(0.002 * RATE)
    energy = np.sqrt(np.convolve(x**2, np.ones(hop) / hop, mode="valid")[::hop])
    db = 20 * np.log10(energy + 1e-12)
    peak = int(np.argmax(db))
    i = peak
    while i > 0 and db[i] > db[peak] - 25:
        i -= 1
    return i * hop


def aligned(x):
    x = x[onset(x) :][: int(SECONDS * RATE)]
    return np.pad(x, (0, int(SECONDS * RATE) - len(x)))


def spectrum(x, a, b, pad=4):
    seg = x[int(a * RATE) : int(b * RATE)]
    n = 1 << int(np.ceil(np.log2(len(seg) * pad)))
    power = np.abs(np.fft.rfft(seg * np.hanning(len(seg)), n)) ** 2
    return np.fft.rfftfreq(n, 1 / RATE), power


def fit_series(f, s, f0):
    """Fundamental and inharmonicity of the stiff-string series the spectrum's peaks lie on.

    The search widens to more partials as the fit firms up, and then narrows its window round
    each predicted partial, so that a stretch first read off the low partials is not carried
    unchecked to the high ones."""
    b = 1e-4
    for count, width in (
        (6, 0.012),
        (12, 0.012),
        (24, 0.01),
        (48, 0.006),
        (64, 0.004),
        (64, 0.003),
    ):
        peaks = []
        for k in range(1, count + 1):
            p = k * f0 * np.sqrt(1 + b * k * k)
            if p > TOP_HZ:
                break
            w = width * p + 2
            m = (f > p - w) & (f < p + w)
            if m.any():
                i = np.argmax(np.where(m, s, 0))
                peaks.append((k, f[i], s[i]))
        if len(peaks) < 3:
            break
        ks = np.array([p[0] for p in peaks], float)
        fs = np.array([p[1] for p in peaks])
        weight = np.sqrt(np.array([p[2] for p in peaks]))
        design = np.vstack([np.ones_like(ks), ks**2]).T * weight[:, None]
        c, *_ = np.linalg.lstsq(design, (fs / ks) ** 2 * weight, rcond=None)
        if c[0] > 0:
            f0, b = np.sqrt(c[0]), min(max(c[1] / c[0], 0.0), 0.01)
    return f0, b


def series(f0, b, top=TOP_HZ):
    out = []
    k = 1
    while True:
        p = k * f0 * np.sqrt(1 + b * k * k)
        if p > top:
            return out
        out.append(p)
        k += 1


def stft(x):
    frames = 1 + (len(x) - FFT) // HOP
    index = np.arange(FFT)[None, :] + HOP * np.arange(frames)[:, None]
    return np.abs(np.fft.rfft(x[index] * np.hanning(FFT), axis=1)) ** 2


def line_fit(t, y):
    if len(t) < 4:
        return None
    return np.polyfit(t, y, 1)[0]


def noise_floor(x):
    """The recording's noise, bin by bin, in the power of one frame: what the quietest tenth of
    its frames hold. A decaying note leaves the end of its file to the noise, and some files
    have little silence before the blow."""
    return np.percentile(stft(x), 10, axis=0)


def measure(x, pitch, noise=None):
    """Every statistic of one note, as a dict of lists of numbers (NaN where it does not apply).
    `noise` is a recording's own noise, bin by bin: it sets the floor beneath each band's
    level, and a partial is followed only while it stands clear of it."""
    out = {}
    f0, b = fit_series(*spectrum(x, 0.1, 1.6), hz(pitch))
    partials = series(f0, b)[:PARTIALS]
    power = stft(x)
    freqs = np.fft.rfftfreq(FFT, 1 / RATE)
    times = (np.arange(power.shape[0]) * HOP + FFT / 2) / RATE
    floor = np.median(power, axis=1, keepdims=True) + 1e-20
    if noise is not None:
        floor = np.maximum(floor, noise[None, :])

    # balance: octave bands against the whole, in three windows
    balance, balance_floor = [], []
    for a, b_ in WINDOWS.values():
        rows = (times >= a) & (times < b_)
        total = power[rows].sum() + 1e-20
        for i, (lo, hi) in enumerate(BANDS):
            band = power[rows][:, (freqs >= lo) & (freqs < hi)].sum()
            balance.append(10 * np.log10(band / total + 1e-12))
            if noise is not None:
                held = noise[(freqs >= lo) & (freqs < hi)].sum() * rows.sum()
                balance_floor.append(10 * np.log10(held / total + 1e-12))
    out["balance"] = balance
    if noise is not None:
        out["balance_floor"] = balance_floor
    early = (times >= 0.05) & (times < 0.5)
    out["level"] = [10 * np.log10(power[early].sum() / early.sum() + 1e-20)]

    # each partial's envelope, where it stands clear of the frame's floor
    envelopes = []
    for p in partials:
        bins = np.abs(freqs - p) <= max(1.5 * RATE / FFT, 0.004 * p)
        env = power[:, bins].max(axis=1)
        clear = env > 30 * (
            floor[:, bins].max(axis=1) if floor.shape[1] > 1 else floor[:, 0]
        )
        envelopes.append((p, 10 * np.log10(env + 1e-20), clear))

    def decays(a, b_):
        rates = []
        for p, db, clear in envelopes:
            rows = (times >= a) & (times < b_) & clear
            rate = (
                line_fit(times[rows], db[rows])
                if rows.sum() >= 0.5 * ((times >= a) & (times < b_)).sum()
                else None
            )
            rates.append((p, rate))
        return rates

    first, later = decays(0.1, 1.0), decays(1.0, 3.0)
    decay = []
    for rates, span in ((first, 0.9), (later, 2.0)):
        for lo, hi in [(0, 250)] + BANDS[:6]:
            inside = [r for p, r in rates if lo <= p < hi and r is not None]
            decay.append(np.median(inside) * span if inside else np.nan)
    out["decay"] = decay
    spread_rates = [r * 0.9 for _, r in first[1:16] if r is not None]
    out["spread"] = (
        [np.subtract(*np.percentile(spread_rates, [75, 25]))]
        if len(spread_rates) >= 5
        else [np.nan]
    )

    # stages: each partial's fall over its first second and the two after, read from its power
    # over a fifth of a second at each end so that one beat's trough does not decide it
    stages = []
    for p, db, clear in envelopes[:STAGE_PARTIALS]:
        power_at = []
        for a, b_ in STAGE_TIMES:
            rows = (times >= a) & (times < b_)
            held = clear[rows].mean() >= 0.5 if rows.any() else False
            power_at.append(
                10 * np.log10(np.mean(10 ** (db[rows] / 10))) if held else np.nan
            )
        stages.append((power_at[0] - power_at[1], power_at[1] - power_at[2]))
    # The median over the partials: which partial meets a beat's trough at the moment it is
    # read is one piano's and one blow's, and would otherwise decide the measure.
    out["stages"] = [
        float(np.nanmedian(column)) if np.isfinite(column).sum() >= 3 else np.nan
        for column in np.array(stages, dtype=float).reshape(-1, 2).T
    ]

    swings = []
    for p, db, clear in envelopes[:16]:
        rows = (times >= 0.2) & (times < 3.0) & clear
        if rows.sum() < 0.6 * ((times >= 0.2) & (times < 3.0)).sum():
            continue
        t, y = times[rows], db[rows]
        swings.append(np.sqrt(np.mean((y - np.polyval(np.polyfit(t, y, 2), t)) ** 2)))
    out["wobble"] = [float(np.mean(swings))] if len(swings) >= 3 else [np.nan]

    # metal: peaks off the transverse series, against the peaks on it
    metal = np.nan
    if pitch <= METAL_TOP:
        f, s = spectrum(x, 0.05, 1.05)
        local = median_filter(s, size=int(60 / f[1]) | 1)
        peak = s > 10 * local
        band = (f > 1.5 * f0) & (f < TOP_HZ)
        on = np.zeros(len(f), bool)
        for p in series(f0, b):
            w = max(4.0, 0.004 * p)
            on |= (f > p - w) & (f < p + w)
        metal = 10 * np.log10(
            ((s * (peak & band & ~on)).sum() + 1e-30)
            / ((s * (peak & band & on)).sum() + 1e-30)
        )
    out["metal"] = [metal]

    # pulse: modulation of the band's envelope near the note's period
    pulse = [np.nan] * len(PULSE_BANDS)
    if pitch <= PULSE_TOP:
        seg = x[int(0.1 * RATE) : int(0.8 * RATE)]
        t = np.arange(len(seg)) / RATE
        for i, (lo, hi) in enumerate(PULSE_BANDS):
            sos = butter(4, [lo, hi], btype="band", fs=RATE, output="sos")
            e = np.abs(hilbert(sosfiltfilt(sos, seg))) ** 2
            e = e / (e.mean() + 1e-30)
            e = e / np.exp(np.polyval(np.polyfit(t, np.log(e + 1e-9), 1), t))
            e -= e.mean()
            best = 0.0
            for fm in np.linspace(0.97 * f0, 1.25 * f0, 60):
                best = max(
                    best, abs(np.sum(e * np.exp(-2j * np.pi * fm * t)) * 2 / len(e))
                )
            pulse[i] = 20 * np.log10(best + 1e-9)
    out["pulse"] = pulse
    out["inharmonicity"] = [b]
    return {k: [float(v) for v in vals] for k, vals in out.items()}


def read_lock(lock=LOCK):
    return json.loads(Path(lock).read_text(encoding="utf-8"))["captures"]


def load_recording(path):
    """The note from its blow, and the recording's noise."""
    audio, rate = sf.read(path, always_2d=True)
    g = gcd(rate, RATE)
    x = resample_poly(audio.mean(axis=1), RATE // g, rate // g)
    return aligned(x), noise_floor(x)


def prepare(root, lock=LOCK):
    root.mkdir(parents=True, exist_ok=True)
    for capture in read_lock(lock):
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


def recorded_stats(root):
    """What the recordings measure, kept beside them and redone when the lock changes."""
    cache = root / "piano-character-stats.json"
    key = digest(LOCK) + digest(Path(__file__))
    if cache.exists():
        kept = json.loads(cache.read_text(encoding="utf-8"))
        if kept.get("key") == key:
            return kept["notes"]
    notes = {}
    for capture in read_lock():
        path = root / capture["path"]
        if digest(path) != capture["sha256"]:
            raise ValueError(f"reference hash mismatch: {path}")
        x, noise = load_recording(path)
        notes[capture["path"]] = measure(x, capture["pitch"], noise)
        print("measured", capture["path"], flush=True)
    cache.write_text(json.dumps({"key": key, "notes": notes}) + "\n", encoding="utf-8")
    return notes


def relative_levels(stats, captures):
    """Each note's level against the mean of the mezzo-forte notes."""
    middle = np.mean(
        [stats[c["path"]]["level"][0] for c in captures if c["dynamic"] == "mf"]
    )
    return {c["path"]: stats[c["path"]]["level"][0] - middle for c in captures}


def neighbours(recorded, captures):
    """Each note's nearest recorded note of the same dynamic, standing in for a render: how far
    the Steinway stands from itself on every measure."""
    standing = {}
    for c in captures:
        others = [
            d
            for d in captures
            if d["dynamic"] == c["dynamic"] and d["path"] != c["path"]
        ]
        nearest = min(others, key=lambda d: (abs(d["pitch"] - c["pitch"]), d["pitch"]))
        standing[c["path"]] = recorded[nearest["path"]]
    return standing


def score_stats(recorded, rendered, captures, scale=None):
    """The rms of each measure over `captures`, divided by `scale` (the recordings' own, over
    every locked note, when it is not given), and their mean as the total."""
    if scale is None:
        every = read_lock()
        scale, _ = raw_scores(recorded, neighbours(recorded, every), every)
    scores, per_note = raw_scores(recorded, rendered, captures)
    for m in METRICS:
        scores[m] /= scale[m]
    scores["total"] = float(np.nanmean([scores[m] for m in METRICS]))
    return scores, per_note


def raw_scores(recorded, rendered, captures):
    errors = {metric: [] for metric in METRICS}
    per_note = {}
    rec_level, ren_level = (
        relative_levels(recorded, captures),
        relative_levels(rendered, captures),
    )
    for c in captures:
        name = c["path"]
        rec, ren = recorded[name], rendered[name]
        note = {}
        for metric in METRICS:
            if metric in ("metal", "pulse") and c["dynamic"] == "pp":
                # A pianissimo recording's peaks off the series and its envelope's pulses are
                # its noise's as much as its own.
                continue
            if metric == "level":
                diffs = [ren_level[name] - rec_level[name]]
            elif metric == "balance" and "balance_floor" in rec:
                # A band the recording holds within 10 dB of its own noise says nothing; in
                # the rest the render is heard through the same noise.
                diffs = [
                    10 * np.log10(10 ** (m / 10) + 10 ** (f / 10)) - r
                    for m, r, f in zip(ren[metric], rec[metric], rec["balance_floor"])
                    if r > f + 10
                ]
            else:
                diffs = [
                    m - r
                    for m, r in zip(ren[metric], rec[metric])
                    if np.isfinite(m) and np.isfinite(r)
                ]
            if diffs:
                errors[metric].extend(diffs)
                note[metric] = float(np.sqrt(np.mean(np.square(diffs))))
        per_note[name] = note
    scores = {
        m: float(np.sqrt(np.mean(np.square(e)))) if e else float("nan")
        for m, e in errors.items()
    }
    scores["total"] = float(np.nanmean([scores[m] for m in METRICS]))
    return scores, per_note


def render_stats(executable, captures, dials=None):
    worker = Worker(executable)
    stats = {}
    try:
        for c in captures:
            stats[c["path"]] = measure(
                aligned(worker.render(c["pitch"], c["velocity"], dials)), c["pitch"]
            )
    finally:
        worker.close()
    return stats


def score(root, executable, dials=None):
    captures = read_lock()
    recorded = recorded_stats(root)
    rendered = render_stats(executable, captures, dials)
    return score_stats(recorded, rendered, captures)


def show(label, scores):
    print(
        label + "  " + "  ".join(f"{m} {scores[m]:5.2f}" for m in METRICS + ("total",))
    )


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("reference", type=Path)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("prepare")
    commands.add_parser("stats")
    one = commands.add_parser("score")
    one.add_argument("worker", type=Path)
    one.add_argument(
        "--dials", default=None, help="calibration dials as JSON, {index: value}"
    )
    one.add_argument("--json", type=Path, default=None)
    two = commands.add_parser("compare")
    two.add_argument("before", type=Path)
    two.add_argument("after", type=Path)
    two.add_argument("--json", type=Path, default=None)
    args = parser.parse_args()
    if args.command == "prepare":
        prepare(args.reference)
    elif args.command == "stats":
        recorded_stats(args.reference)
    elif args.command == "score":
        dials = json.loads(args.dials) if args.dials else None
        scores, per_note = score(args.reference, args.worker, dials)
        show("score", scores)
        if args.json:
            args.json.write_text(
                json.dumps({"scores": scores, "notes": per_note}, indent=1) + "\n",
                encoding="utf-8",
            )
    else:
        report = {}
        for label, executable in (("before", args.before), ("after", args.after)):
            scores, per_note = score(args.reference, executable)
            show(f"{label:6s}", scores)
            report[label] = {
                "scores": scores,
                "notes": per_note,
                "sha256": digest(executable),
            }
        if args.json:
            args.json.write_text(json.dumps(report, indent=1) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
