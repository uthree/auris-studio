# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy==2.5.3", "scipy==1.18.1", "soundfile==0.14.0"]
# ///
"""Measure the drum kit's per-band tables from the training captures, development only.

Writes `crates/auris-synth/src/drumkit/acoustics.rs`: for every pad, the -30 dB decay time of
each octave band (which sets both the modal and the diffuse losses), the time each band takes to
bloom, and the level of the diffuse field in each band. The levels need the kit itself: given
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
from scipy.signal import butter, sosfilt

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
             "    /// Level of the diffuse field in each band, in decibels.",
             f"    pub(super) field_db: [f32; {len(BANDS)}],", "}", ""]
    for name, (t30, rise, level) in tables.items():
        fmt = lambda values: ", ".join(f"{value:.4f}" for value in values)
        lines += [f"pub(super) const {name.upper()}: Acoustics = Acoustics {{",
                  f"    t30: [{fmt(t30)}],", f"    bloom: [{fmt(rise)}],", f"    field_db: [{fmt(level)}],", "};", ""]
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--worker", type=Path, help="render the current kit and correct the levels")
    parser.add_argument("--step", type=float, default=0.8, help="fraction of the level error to remove")
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
            if level is None or worker is None:
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
            tables[name] = (extend(t30), extend(bloom(peak, t30, name in METAL)), level)
    finally:
        if worker:
            worker.close()
    args.output.write_text(rust(tables), encoding="utf-8")


if __name__ == "__main__":
    main()
