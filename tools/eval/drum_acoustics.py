# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy==2.5.3", "scipy==1.18.1", "soundfile==0.14.0"]
# ///
"""Measure the drum kit's per-band tables from the training captures, development only.

Writes `crates/auris-synth/src/drumkit/acoustics.rs`: for every pad, the -30 dB decay time of
each octave band (which sets both the modal and the diffuse losses), the time each band takes to
bloom, the level of the diffuse field in each band, and for a cymbal how much of each band is
resolved lines and how many of them there are. The levels need the kit itself: given
`--worker`, the current kit is rendered on the same training hits and every level is moved by
the difference between the captures' band shares and the kit's, so a few rebuild-and-rerun
rounds converge on the captures' spectrum. Only the training split is read.
"""

import argparse
import json
import re
from pathlib import Path

import numpy as np
import soundfile as sf
from fit_worker import Renderer
from physical_mel import RATE
from physical_pack import analysis_audio, load
from scipy.ndimage import median_filter
from scipy.signal import butter, find_peaks, sosfilt

BANDS = (63, 125, 250, 500, 1000, 2000, 4000, 8000, 16000)
MEASURED = len(BANDS) - 1  # the 24 kHz captures end inside the 16 kHz band
HOP = RATE // 200
# Shares are taken over the attack and the body together: both carry the drum's colour.
SHARE_SECONDS = 0.25
PADS = {"Kick": ("kick", None), "Snare": ("snare", None), "ClosedHat": ("closed_hat", None),
        "OpenHat": ("open_hat", None), "Crash": ("crash", None), "Ride": ("ride", None),
        "TomLow": ("tom", 41), "TomHigh": ("tom", 50)}
METAL = {"ClosedHat", "OpenHat", "Crash", "Ride"}
# The head's own modes carry a membrane's lowest bands; diffuse noise there would replace the
# pitched thump with a rumble. Bands below this many fundamentals stay at `MODAL_CEILING_DB`.
FUNDAMENTAL = {"Kick": 42.0, "Snare": 185.0, "TomLow": 63.0, "TomHigh": 135.0}
MODAL_REACH = 2.5
MODAL_CEILING_DB = -30.0
# Where a cymbal's tail is read for its lines: late enough that the strike has gone, short
# enough that the band is still well above the floor. A closed hat has no tail to read. The
# ride's lines are not modelled yet: they made its first 50 ms further from the captures.
LINE_WINDOWS = {"OpenHat": (0.05, 0.55), "Crash": (0.1, 1.1)}
# Below this the captures' lines are room and the other drums, not the cymbal.
LINE_FLOOR_HZ = 250
# A line counts when it stands this far above the local noise mean, at least this far apart.
LINE_HEIGHT_DB = 12
LINE_SPACING_HZ = 4
# Counted lines are the strong ones; as many again, weaker, sit in what reads as the floor.
LINE_SURPLUS = 1.5
# A band keeps at most this many lines, and a hit at most `MAX_LINES`: the cost is per line
# per sample. A band given fewer than it has puts their energy in fewer, stronger lines.
LINES_PER_BAND = 96
MAX_LINES = 384
LANES = 8


def band(audio, center):
    low, high = center / np.sqrt(2), min(center * np.sqrt(2), RATE * 0.49)
    return sosfilt(butter(3, [low, high], fs=RATE, btype="bandpass", output="sos"), audio, axis=-1)


def envelope_db(audio):
    count = audio.shape[-1] // HOP
    power = np.mean(audio[..., :count * HOP].reshape(*audio.shape[:-1], count, HOP) ** 2, axis=-1)
    return 10 * np.log10(power + 1e-14)


def decay(env):
    """Seconds the band takes to fall 30 dB below its peak.

    Read where it happens, not extrapolated from the first slope: a cymbal falls fast and then
    rings on, and how long it rings is what one exponential has to stand for. A band still above
    -30 dB at the end of the excerpt is extrapolated along its whole fall.
    """
    smooth = 10 * np.log10(np.convolve(10 ** (env / 10), np.ones(4) / 4, mode="same") + 1e-14)
    peak = int(np.argmax(smooth))
    after = smooth[peak:] - smooth[peak]
    below = np.flatnonzero(after < -30)
    if len(below):
        return min(6.0, below[0] / 200)
    drop = max(-after[-1], 1.0)
    return min(6.0, len(after) / 200 * 30 / drop)


def shares(audio):
    """Each band's share of the first quarter second, in dB, per hit."""
    stop = int(SHARE_SECONDS * RATE)
    power = np.array([np.mean(band(audio[:, :stop], c) ** 2, axis=-1) for c in BANDS[:MEASURED]])
    return 10 * np.log10(power / power.sum(axis=0) + 1e-12).T


def measure(audio):
    audio = analysis_audio(audio)
    envs = [envelope_db(band(audio, c)) for c in BANDS[:MEASURED]]
    t30 = np.array([[decay(e) for e in env] for env in envs])
    peaks = np.array([[np.argmax(e) / 200 for e in env] for env in envs])
    return np.median(shares(audio), axis=0), np.median(t30, axis=1), np.median(peaks, axis=1)


def reliable(share, t30):
    """A band a hundredth of the drum's energy cannot be timed: borrow the nearest audible one."""
    audible = np.flatnonzero(share > -30)
    return np.array([t30[audible[np.argmin(abs(audible - index))]] for index in range(len(t30))])


def bloom(peak, t30, metal):
    """Rise time constant r such that (1 - e^(-t/r)) e^(-t/tau) peaks at the measured time."""
    if not metal:
        return np.zeros_like(peak)
    result = []
    for at, decay_time in zip(peak, t30, strict=True):
        tau = decay_time / (1.5 * np.log(10))
        if at < 0.02:
            result.append(0.0)
            continue
        low, high = 1e-4, at
        for _ in range(60):
            middle = (low + high) / 2
            if middle * np.log(1 + tau / middle) < at:
                low = middle
            else:
                high = middle
        result.append(round(float(middle), 4))
    return np.array(result)


def extend(values):
    return np.array([*values, values[-1]])


def top_octave(root, notes):
    """How far the 16 kHz octave sits above the 8 kHz one, read from the original recordings.

    The prepared captures are at 24 kHz and stop inside the top band, so it is measured on the
    archive's own files, over the same first quarter second.
    """
    differences = []
    for note in notes:
        audio, rate = sf.read(root / "drums" / note["source"], always_2d=True)
        start = round(note["offset_seconds"] * rate)
        audio = audio.mean(axis=1)[start:start + round(SHARE_SECONDS * rate)]
        power = []
        for center in BANDS[-2:]:
            high = min(center * np.sqrt(2), rate * 0.49)
            sos = butter(3, [center / np.sqrt(2), high], fs=rate, btype="bandpass", output="sos")
            power.append(np.mean(sosfilt(sos, audio) ** 2))
        differences.append(10 * np.log10(power[1] / power[0]))
    return float(np.median(differences))


def measure_lines(root, notes, window):
    """Per band: the share of the tail's energy in resolved lines and how many lines to model.

    Read on the archive's own 48 kHz files. The local noise mean is a running median 60 Hz wide
    divided by ln 2 (the median of an exponential); whatever stands above it is line energy. The
    count is the lines `LINE_HEIGHT_DB` above that mean, scaled by `LINE_SURPLUS`, rounded up to
    whole SIMD lanes and capped per band and in total. Also returns the spread of the lines' levels in dB.
    """
    shares, densities, spreads = [], [], []
    for note in notes:
        audio, rate = sf.read(root / "drums" / note["source"], always_2d=True)
        if rate != 48000:
            raise ValueError(f"{note['source']}: {rate} Hz")
        start = round(note["offset_seconds"] * rate)
        audio = audio.mean(axis=1)[start + round(window[0] * rate):start + round(window[1] * rate)]
        power = np.abs(np.fft.rfft(audio * np.hanning(len(audio)))) ** 2
        hz = np.fft.rfftfreq(len(audio), 1 / rate)
        resolution = rate / len(audio)
        floor = median_filter(power, size=max(5, int(60 / resolution) | 1)) / np.log(2)
        share, density = np.zeros(len(BANDS)), np.zeros(len(BANDS))
        for index, center in enumerate(BANDS):
            if center < LINE_FLOOR_HZ:
                continue
            chosen = (hz >= center / np.sqrt(2)) & (hz < min(center * np.sqrt(2), rate * 0.48))
            share[index] = max(0.0, 1 - floor[chosen].sum() / power[chosen].sum())
            height = 10 * np.log10(power[chosen] / floor[chosen])
            peaks, _ = find_peaks(height, height=LINE_HEIGHT_DB,
                                  distance=max(1, round(LINE_SPACING_HZ / resolution)))
            density[index] = len(peaks) / (chosen.sum() * resolution)
            if len(peaks) > 2:
                spreads.append(np.std(10 * np.log10(power[chosen][peaks])))
        shares.append(share)
        densities.append(density)
    share, density = np.median(shares, axis=0), np.median(densities, axis=0)
    width = np.array([min(c * np.sqrt(2), 24000 * 0.96) - c / np.sqrt(2) for c in BANDS])
    wanted = LINE_SURPLUS * density * width
    kept = np.minimum(wanted, LINES_PER_BAND)
    if kept.sum() > MAX_LINES:
        kept *= MAX_LINES / kept.sum()
    count = np.ceil(kept / LANES).astype(int) * LANES
    while count.sum() > MAX_LINES:
        count[np.argmax(count - kept)] -= LANES
    return share, count, float(np.median(spreads))


def previous_levels(path):
    text = path.read_text(encoding="utf-8") if path.exists() else ""
    levels = {}
    for name in PADS:
        match = re.search(rf"pub\(super\) const {name.upper()}: Acoustics = Acoustics \{{.*?field_db: \[([^\]]*)\]", text, re.DOTALL)
        if match:
            levels[name] = np.array([float(value) for value in match[1].split(",") if value.strip()])
    return levels


def render_kit(worker, notes):
    return worker.render("drums", notes, {}, 3, 3)


def rust(tables):
    lines = ["//! Per-band decay, bloom and diffuse-field level of every pad, measured on the training",
             "//! captures. Generated by `tools/eval/drum_acoustics.py`; do not edit by hand.", "",
             "/// Centre of each octave band, in hertz.",
             f"pub(super) const BAND_HZ: [f32; {len(BANDS)}] = [{', '.join(f'{c:.1f}' for c in BANDS)}];", "",
             "/// One pad's measured bands.", "#[derive(Clone, Copy, Debug)]", "pub(super) struct Acoustics {",
             "    /// Seconds for each band to fall 30 dB; the modes and the diffuse field share it.",
             f"    pub(super) t30: [f32; {len(BANDS)}],",
             "    /// Rise time constant of each band, in seconds: the cascade that lets a cymbal bloom.",
             f"    pub(super) bloom: [f32; {len(BANDS)}],",
             "    /// Level of the diffuse field in each band, in decibels: lines and noise together.",
             f"    pub(super) field_db: [f32; {len(BANDS)}],",
             "    /// Share of each band's energy in resolved lines rather than noise.",
             f"    pub(super) line_share: [f32; {len(BANDS)}],",
             "    /// Lines modelled in each band, a whole number of SIMD lanes.",
             f"    pub(super) lines: [u16; {len(BANDS)}],",
             "    /// Standard deviation of the lines' levels, in decibels.",
             "    pub(super) line_spread_db: f32,", "}", ""]
    for name, (t30, rise, level, (share, count, spread)) in tables.items():
        fmt = lambda values: ", ".join(f"{value:.4f}" for value in values)
        lines += [f"pub(super) const {name.upper()}: Acoustics = Acoustics {{",
                  f"    t30: [{fmt(t30)}],", f"    bloom: [{fmt(rise)}],", f"    field_db: [{fmt(level)}],",
                  f"    line_share: [{fmt(share)}],", f"    lines: [{', '.join(str(int(c)) for c in count)}],",
                  f"    line_spread_db: {spread:.4f},", "};", ""]
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--worker", type=Path, help="render the current kit and correct the levels")
    parser.add_argument("--step", type=float, default=0.8, help="fraction of the level error to remove")
    parser.add_argument("--pad", action="append", choices=PADS,
                        help="correct only these pads' levels; the others keep theirs")
    args = parser.parse_args()
    manifest = json.loads((args.reference / "notes.json").read_text(encoding="utf-8"))
    previous = previous_levels(args.output)
    worker = Renderer(args.worker.resolve()) if args.worker else None
    tables = {}
    try:
        for name, (family, pitch) in PADS.items():
            notes, reference = load(args.reference, manifest, family, "train")
            if pitch is not None:
                keep = [index for index, note in enumerate(notes) if note["pitch"] == pitch]
                notes, reference = [notes[index] for index in keep], reference[keep]
            share, t30, peak = measure(reference)
            t30 = reliable(share, t30)
            level = previous.get(name)
            if level is not None and args.pad and name not in args.pad:
                pass
            elif level is None or worker is None:
                level = extend(share)
            else:
                kit = shares(analysis_audio(render_kit(worker, notes)))
                error = share - np.median(kit, axis=0)
                level = np.clip(level + args.step * extend(error), -80, 30)
                print(name, "mean |share error| dB", round(float(np.mean(abs(error))), 2), flush=True)
            level = np.array(level, dtype=float)
            level[-1] = level[-2] + top_octave(args.reference, notes)
            if name in FUNDAMENTAL:
                modal = np.array(BANDS) < FUNDAMENTAL[name] * MODAL_REACH
                level = np.where(modal, np.minimum(level, MODAL_CEILING_DB), level)
            if name in LINE_WINDOWS:
                found = measure_lines(args.reference, notes, LINE_WINDOWS[name])
                print(name, "line share", np.round(found[0], 2), "lines", found[1].tolist(), flush=True)
            else:
                found = (np.zeros(len(BANDS)), np.zeros(len(BANDS), dtype=int), 0.0)
            tables[name] = (extend(t30), extend(bloom(peak, t30, name in METAL)), level, found)
    finally:
        if worker:
            worker.close()
    args.output.write_text(rust(tables), encoding="utf-8")


if __name__ == "__main__":
    main()
