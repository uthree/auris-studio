# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12"]
# ///
"""Read a piano's tuning off its sound: fundamentals, heard pitch and octaves.

measure   read the worker's factory piano and, with --recorded, a Steinway beside it
prepare   download the hash-locked Steinway notes that --recorded reads

    uv run tools/eval/piano_stretch.py prepare target/piano-stretch
    uv run tools/eval/piano_stretch.py measure <worker> target/piano-stretch.json \\
      --recorded target/piano-stretch

Every figure is in cents. A fundamental and a heard pitch are measured from equal temperament:
at A4 = 440 Hz for the worker, and on its own A4 for the recorded piano, which was not tuned to
440. An octave is the lower note's partial minus the partial of the upper note it should meet:
below zero the two notes lie further apart than that pure octave, above zero closer, so a
negative 2:1 beside a positive 4:2 is an octave tuned between the two. Each octave is read on
four such pairs, the 2:1, 4:2, 6:3 and 8:4: the bass is tuned on the last two.

Each string's stiffness is read as well, off the line its partials make, with where that line
puts the first partial: a recorded bass note's fundamental lies under its own string's line.
"""

import argparse
import json
from pathlib import Path
from typing import NamedTuple

import numpy as np
import piano_register as register
import soundfile as sf
from fit_worker import Renderer

LOCK = Path(__file__).resolve().parent / "references/piano-stretch-captures.json"
RATE = 48000
VELOCITY = 0.7
SECONDS = 2.5
KEYS = (("A0", 21), ("B0", 23), ("C1", 24), ("A1", 33), ("C2", 36), ("A2", 45), ("C3", 48),
        ("C4", 60), ("A4", 69), ("C5", 72), ("C6", 84), ("C7", 96), ("C8", 108))
TOP = 108
# The pairs of partials an octave is read on, by the lower note's partial.
OCTAVES = (("octave_2_1", 2), ("octave_4_2", 4), ("octave_6_3", 6), ("octave_8_4", 8))
# The stretches of a note that are read, in seconds from its strike: its partials once the
# strike is over, and its period while the overtones that pull it are still there.
STEADY = (0.3, 2.3)
EARLY = (0.05, 0.55)
# Cents either side of a partial's strongest frequency that the strings of one key lie within.
UNISON = 25.0
# A partial this far below the note's strongest is not one an octave can be judged by.
FLOOR_DB = -60.0
# A recorded partial counts when it stands this far above what the same file holds once the
# note is over. The recordings carry a steady line at 8 kHz that otherwise reads as one.
NOISE_DB = 10.0
# The partials followed up one string to read its stiffness, and how faint one of them may be.
MOST = 40
FAINT_DB = -70.0
# A partial further than this from the line the others make is not the string's own. A real
# bass string has partials of another kind beside these, and its first few lie under the line.
OFF_LINE = 3.0
# Lags to the sample when a period is looked for: the top octave's is a dozen samples long.
FINE = 8
# A heard pitch that moves further than this with the weight given to the ends of the stretch
# it is read over is not one figure.
HEARD_TOLERANCE = 1.0


def hz(pitch):
    return 440.0 * 2 ** ((pitch - 69) / 12)


def channels(audio):
    """Samples as frames by channels: a render has one channel, a recording one for each microphone."""
    audio = np.asarray(audio, dtype=np.float64)
    return audio[:, None] if audio.ndim == 1 else audio


def cut(audio, stretch, rate):
    return audio[round(stretch[0] * rate):round(stretch[1] * rate)]


def spectrum(audio, rate):
    """The magnitude spectrum through a Hann window, on a grid far finer than its main lobe.

    A bass note's fundamental can lie forty decibels under its neighbours, and a bare
    rectangle lets them leak onto it: read that way, A0 comes out two cents from where a
    twelve-second stretch puts it. Channels add in power. Two microphones hold the strings of
    one key in different phase, and added as samples they cancel now one string and now
    another: the recorded C1 then moves three cents with the stretch that is read.
    """
    audio = channels(audio)
    size = 1 << int(np.ceil(np.log2(len(audio) * 32)))
    shaped = audio * np.hanning(len(audio))[:, None]
    return np.sqrt((abs(np.fft.rfft(shaped, size, axis=0)) ** 2).sum(axis=1)), rate / size


class Partial(NamedTuple):
    """One partial of a note: cents from where it was looked for, decibels below the note's
    strongest, and decibels above the noise of its recording (without bound for a render)."""

    cents: float
    level: float
    clear: float


def partial(magnitudes, resolution, target, noise=None, span=150.0):
    """The partial of a `spectrum` near `target`, or `None` where nothing peaks there.

    It is found as the strongest frequency within `span` cents of `target`, and lies at the
    middle of the power within `UNISON` cents of that. The strings of one key are not tuned
    exactly together, and which of them is strongest changes with the stretch and with the
    microphone: the middle of their power holds still where the strongest does not.
    """
    first = int(np.ceil(target * 2 ** (-span / 1200) / resolution))
    last = int(np.floor(target * 2 ** (span / 1200) / resolution))
    if first < 1 or last > len(magnitudes) - 2 or last <= first:
        return None
    peak = first + int(np.argmax(magnitudes[first:last + 1]))
    if peak in (first, last):
        return None
    a, b, c = magnitudes[peak - 1:peak + 2]
    centre = peak + 0.5 * (a - c) / (a - 2 * b + c)
    # As far below the strongest frequency as above it, so that one string alone reads where
    # it peaks; a bin counts for as much of itself as lies inside.
    reach = centre * (2 ** (min(UNISON, span) / 1200) - 1)
    low, high = max(centre - reach, 0.5), min(centre + reach, len(magnitudes) - 1.5)
    bins = np.arange(int(np.floor(low + 0.5)), int(np.ceil(high - 0.5)) + 1)
    lower, upper = np.maximum(bins - 0.5, low), np.minimum(bins + 0.5, high)
    weights = magnitudes[bins] ** 2 * (upper - lower)
    middle = float((0.5 * (lower + upper) * weights).sum() / weights.sum())
    floor = 0.0 if noise is None else float(noise[bins].max())
    clear = np.inf if floor == 0.0 else 20 * np.log10(b / floor)
    return Partial(float(1200 * np.log2(middle * resolution / target)),
                   float(20 * np.log10(b / magnitudes.max())), float(clear))


def heard_cents(audio, rate, pitch, weights=None, span=100.0):
    """Cents from equal temperament of the period the note repeats at, which is what is heard.

    The period is the lag at which the note best matches itself, each sample counting for
    its `weights` (evenly, if none are given). A lag is judged by the energy of the two
    stretches it lays side by side, weighted as their products are: fewer samples overlap at a
    longer lag, and a bare sum of products reads a low note a quarter of a cent sharp for it.
    Judged so, a note that holds its period reads the same under any weights. Channels add
    their products and their energies.
    """
    expected = hz(pitch)
    audio = channels(audio)
    audio = audio - audio.mean(axis=0)
    count = len(audio)
    weights = np.ones(count) if weights is None else np.asarray(weights, dtype=np.float64)
    size = 1 << int(np.ceil(np.log2(count * 2)))
    shape = np.fft.rfft(weights, size)
    products, leading, trailing = np.zeros(count * FINE), np.zeros(count), np.zeros(count)
    for channel in audio.T:
        # A power spectrum padded with zeros is the same autocorrelation at lags between samples.
        products += np.fft.irfft(abs(np.fft.rfft(channel * weights, size)) ** 2, size * FINE)[:count * FINE]
        energy = np.fft.rfft(weights * channel ** 2, size)
        leading += np.fft.irfft(np.conj(energy) * shape, size)[:count]
        trailing += np.fft.irfft(np.conj(shape) * energy, size)[:count]
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


class String(NamedTuple):
    """A string as its partials show it: its stiffness; the cents from equal temperament at
    which the line through its partials puts the first of them; the cents the first three lie
    from that line; and how many partials the line is drawn through, of how many followed."""

    stiffness: float
    cents: float
    low: list
    partials: tuple


def line(numbers, hertz):
    """The stiffness and the limp frequency of the string whose partials these are.

    Partial `n` of a stiff string lies at `n f sqrt(1 + B n^2)`, so `(f_n / n)^2` against `n^2`
    is a straight line, and its slope over its intercept is `B`.
    """
    slope, intercept = np.polyfit(numbers ** 2, (hertz / numbers) ** 2, 1)
    return slope / intercept, np.sqrt(max(intercept, 1e-12))


class Note:
    """A note from its strike on, and what its recording holds when no note sounds."""

    def __init__(self, audio, rate, noise=None):
        audio = channels(audio)
        steady = cut(audio, STEADY, rate)
        if len(steady) < round(STEADY[1] * rate) - round(STEADY[0] * rate):
            raise ValueError("note shorter than the stretch it is read over")
        self.rate = rate
        self.early = cut(audio, EARLY, rate)
        self.magnitudes, self.resolution = spectrum(steady, rate)
        self.noise = None if noise is None else spectrum(channels(noise)[-len(steady):], rate)[0]

    def partial(self, target, span=150.0):
        """Its partial near `target`, or `None` where none stands clear of the noise."""
        reading = partial(self.magnitudes, self.resolution, target, self.noise, span)
        return reading if reading is not None and reading.clear > NOISE_DB else None

    def string(self, pitch):
        """The string of this note at `pitch`, or `None` where fewer than three partials show.

        Each partial is looked for where the ones below it say it will be, no further off than
        two fifths of the way to the next. The line is then drawn through those that lie on it.
        """
        numbers, hertz = [], []
        stiffness, limp = 0.0, hz(pitch)
        for n in range(1, MOST + 1):
            target = n * limp * np.sqrt(1 + stiffness * n * n)
            if target > min(10_000.0, 0.45 * self.rate):
                break
            reading = self.partial(target, span=min(150.0, 480 * np.log2((n + 1) / n)))
            if reading is None or reading.level < FAINT_DB:
                continue
            numbers.append(float(n))
            hertz.append(target * 2 ** (reading.cents / 1200))
            if len(numbers) >= 3:
                stiffness, limp = line(np.array(numbers), np.array(hertz))
                stiffness = max(stiffness, 0.0)
        if len(numbers) < 3:
            return None
        numbers, hertz = np.array(numbers), np.array(hertz)

        def drawn(through):
            stiffness, limp = line(numbers[through], hertz[through])
            return stiffness, limp, 1200 * np.log2(hertz / (numbers * limp * np.sqrt(1 + max(stiffness, 0.0) * numbers ** 2)))

        on = np.ones(len(numbers), dtype=bool)
        for _ in range(4):
            stiffness, limp, off = drawn(on)
            spread = 1.4826 * np.median(abs(off[on] - np.median(off[on])))
            still = abs(off) <= max(OFF_LINE, 3 * spread)
            if still.sum() < 3 or (still == on).all():
                break
            on = still
        else:
            stiffness, limp, off = drawn(on)
        if stiffness <= 0:
            return None
        low = [float(off[numbers == n][0]) if n in numbers else None for n in (1.0, 2.0, 3.0)]
        first = 1200 * np.log2(limp * np.sqrt(1 + stiffness) / hz(pitch))
        return String(float(stiffness), float(first), low, (int(on.sum()), len(numbers)))


def octave(lower, upper, low_hz, n):
    """The lower note's `n`th partial minus the upper note's partial at the same place, in cents.

    `None` where either partial is missing or too weak to judge an octave by.
    """
    readings = [note.partial(low_hz * n) for note in (lower, upper)]
    if any(reading is None or reading.level < FLOOR_DB for reading in readings):
        return None
    return readings[0].cents - readings[1].cents


def measure(notes):
    """A row for each of `KEYS` from `notes` by pitch, empty where a note is not there."""
    rows = []
    for name, pitch in KEYS:
        note, above = notes.get(pitch), notes.get(pitch + 12)
        fundamental = note.partial(hz(pitch)) if note else None
        readings = list(heard_readings(note.early, note.rate, pitch)) if note else None
        settled = readings is not None and abs(readings[0] - readings[1]) <= HEARD_TOLERANCE
        string = note.string(pitch) if note else None
        row = {"key": name, "pitch": pitch, "fundamental": fundamental.cents if fundamental else None,
               "heard": readings[0] if settled else None, "heard_readings": readings,
               "stiffness": string.stiffness if string else None, "string": string.cents if string else None,
               "from_the_line": string.low if string else None,
               "string_partials": list(string.partials) if string else None}
        if pitch + 12 <= TOP:
            for label, n in OCTAVES:
                row[label] = octave(note, above, hz(pitch), n) if note and above else None
        rows.append(row)
    return rows


def onto_its_own_a4(rows):
    """Move rows measured from 440 Hz onto the piano's own A4, and say where that lay."""
    a4 = next(row["fundamental"] for row in rows if row["pitch"] == 69)
    if a4 is None:
        raise ValueError("no A4 to measure the other keys from")
    for row in rows:
        for label in ("fundamental", "heard", "string"):
            if row[label] is not None:
                row[label] -= a4
        if row["heard_readings"] is not None:
            row["heard_readings"] = [reading - a4 for reading in row["heard_readings"]]
    return a4


def played(renderer, rate=RATE):
    """The worker's factory piano at every key `measure` reads, by pitch."""
    pitches = sorted({pitch for _, pitch in KEYS} | {pitch + 12 for _, pitch in KEYS if pitch + 12 <= TOP})
    notes = [{"pitch": pitch, "velocity": VELOCITY} for pitch in pitches]
    audio = renderer.render("piano", notes, {}, SECONDS, SECONDS, rate=rate)
    return {pitch: Note(samples, rate) for pitch, samples in zip(pitches, audio, strict=True)}


def recorded(root, lock=LOCK):
    """The locked recordings under `root`, by pitch: each from where its note is struck, with
    its file's end as its noise."""
    notes = {}
    for capture in register.read_lock(lock):
        path = Path(root) / capture["path"]
        if register.digest(path) != capture["sha256"]:
            raise ValueError(f"reference hash mismatch: {path}")
        audio, rate = sf.read(path, always_2d=True)
        strike = register.onset(np.sqrt((audio ** 2).sum(axis=1)), rate)
        notes[capture["pitch"]] = Note(audio[strike:], rate, noise=audio)
    return notes


def shown(value):
    """A figure to a tenth of a cent, with no sign on a zero and a dash where there is none."""
    if value is None:
        return "-"
    text = f"{value:+.1f}"
    return "0.0" if text[1:] == "0.0" else text


def table(title, rows):
    print(f"{title}\nkey  fundamental   heard     2:1     4:2     6:3     8:4  string  stiffness")
    for row in rows:
        cells = "".join(f"{shown(row.get(key)):>8}" for key in ("heard", *(label for label, _ in OCTAVES), "string"))
        stiffness = "-" if row["stiffness"] is None else f"{row['stiffness'] * 1e4:.2f}e-4"
        print(f"{row['key']:<4}{shown(row['fundamental']):>12}{cells}{stiffness:>11}")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("prepare").add_argument("recordings", type=Path)
    reader = commands.add_parser("measure")
    reader.add_argument("renderer", type=Path)
    reader.add_argument("output", type=Path)
    reader.add_argument("--recorded", type=Path, metavar="RECORDINGS")
    args = parser.parse_args()
    if args.command == "prepare":
        register.prepare(args.recordings, LOCK)
        return
    renderer = Renderer(args.renderer.resolve())
    try:
        rows = measure(played(renderer))
    finally:
        renderer.close()
    missing = [row["key"] for row in rows if row["fundamental"] is None]
    if missing:
        raise SystemExit(f"no fundamental within 150 cents of equal temperament: {', '.join(missing)}")
    report = {"rate": RATE, "velocity": VELOCITY, "seconds": SECONDS,
              "method": f"partials: middle of the power within {UNISON:g} cents of the strongest frequency, in the "
                        f"Hann-windowed spectrum of {STEADY[0]:g}-{STEADY[1]:g} s from the strike, channels added "
                        f"in power; recorded partials more than {NOISE_DB:g} dB above the file's last "
                        f"{STEADY[1] - STEADY[0]:g} s only; "
                        f"heard pitch: normalized autocorrelation peak of {EARLY[0]:g}-{EARLY[1]:g} s, given where a "
                        f"Hann-weighted reading of the same stretch lies within {HEARD_TOLERANCE:g} cent; "
                        f"octaves: lower note's partial minus the upper note's, above {FLOOR_DB:g} dB only; "
                        f"strings: a line through (n^2, (f_n / n)^2) for up to {MOST} partials below 10 kHz, "
                        f"those more than {OFF_LINE:g} cents off it left out",
              "model": {"renderer_sha256": register.digest(args.renderer), "keys": rows}}
    table("Worker, from equal temperament at A4 = 440 Hz", rows)
    if args.recorded:
        rows = measure(recorded(args.recorded))
        a4 = onto_its_own_a4(rows)
        lock = json.loads(LOCK.read_text(encoding="utf-8"))
        report["recorded"] = {"source": lock["source"], "a4_cents_from_440": a4,
                              "references": {capture["path"]: capture["sha256"] for capture in lock["captures"]},
                              "keys": rows}
        table(f"\nRecorded, from equal temperament on its own A4 ({a4:+.1f} cents from 440 Hz)", rows)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
