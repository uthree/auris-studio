# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12"]
# ///
"""Compare upper-midrange voicing across native instruments and lead patches."""

import argparse
import hashlib
import html
import json
from pathlib import Path

import numpy as np
import scipy
import soundfile as sf
from drum_harshness import delta_db
from fit_worker import Renderer
from scipy.signal import periodogram

RATES = (44100, 48000, 96000)
VELOCITIES = (.35, .65, 1.0)
UNCHANGED = {"bass", "bell", "mallet", "tin_whistle", "hammered_dulcimer", "choir",
             "drums", "noisedrum", "sine", "triangle", "fm_sine"}


def profiles():
    """Factory ranges, brighter lead patches, bypass controls and two production amp chains."""
    result = []
    for model, low, high in [("piano", 36, 100), ("guitar", 40, 100),
                             ("electric_guitar", 40, 100), ("bass", 28, 64),
                             ("bell", 60, 96), ("mallet", 48, 96), ("violin", 55, 100),
                             ("clarinet", 50, 100), ("tin_whistle", 74, 98),
                             ("hammered_dulcimer", 48, 84), ("choir", 48, 84),
                             ("vocal", 48, 100), ("chiptune", 48, 100), ("fm2", 48, 100)]:
        pitches = sorted(set(range(low, high + 1, 6)) | {high} |
                         {p for p in (72, 77, 84, 85, 87, 96, 100) if low <= p <= high})
        result.append({"name": model, "model": model, "params": {}, "pitches": pitches})
    for name, model, params in [
        ("pulse_lead", "chiptune", {"pulse_width": .125}),
        ("saw_lead", "chiptune", {"waveform": 2}),
        ("fm_lead", "fm2", {"ratio": 1, "mod_decay": .3}),
        ("sine", "chiptune", {"waveform": 0}),
        ("triangle", "chiptune", {"waveform": 3}),
        ("fm_sine", "fm2", {"index": 0}),
    ]:
        result.append({"name": name, "model": model, "params": params,
                       "pitches": [48, 60, 72, 77, 84, 85, 87, 96, 100]})
    result.extend([
        {"name": "drums", "model": "drums", "params": {}, "pitches": [36, 38, 42, 46, 49, 51, 47]},
        {"name": "noisedrum", "model": "noisedrum", "params": {}, "pitches": [48, 60, 72]},
    ])
    for name, pickup, amp in [
        ("amp_pop", .12, {"drive_db": 12, "bass_db": -2, "treble_db": 1.5, "cabinet": 1, "output_db": -6}),
        ("amp_rock", .064146, {"drive_db": 34, "bass_db": -3, "mid_db": 1.5, "treble_db": 2, "cabinet": 2, "output_db": -12}),
    ]:
        result.append({"name": name, "model": "electric_guitar", "params": {"pickup_position": pickup},
                       "pitches": [48, 60, 72, 77, 84, 85, 87, 96, 100],
                       "effects": [{"id": "amp", "params": amp}]})
    return result


def measure(audio, rate):
    """Absolute and AC-relative power; DC is excluded from audible-band normalization."""
    if audio.ndim != 1 or len(audio) < rate or not np.isfinite(audio).all():
        raise ValueError("expected at least one second of finite mono PCM")
    result = {"peak": float(np.max(np.abs(audio)))}
    for name, start, end in [("attack", 0, .15), ("sustain", .15, 1.2)]:
        x = audio[round(start * rate):round(end * rate)]
        frequencies, powers = periodogram(x, fs=rate, window="hann", detrend=False,
                                         scaling="spectrum")
        audible = float(powers[frequencies >= 30].sum())
        if audible < 1e-24:
            # A fully decayed percussion tail is a silence measurement, not a timbre ratio.
            result[name] = None
            continue
        bands = {label: float(powers[(frequencies >= low) & (frequencies < high)].sum())
                 for label, low, high in [("lower", 250, 1500), ("sharp", 2000, 5000), ("upper", 7000, 10000)]}
        result[name] = {"audible_power": audible, "band_power": bands,
                        "sharp_fraction": bands["sharp"] / audible}
    return result


def compare(before, after, rate):
    """Keep both absolute changes and changes after whole-window audible-power matching."""
    old, new = measure(before, rate), measure(after, rate)
    changes = {}
    for name in ("attack", "sustain"):
        a, b = old[name], new[name]
        if a is None or b is None:
            changes[name] = None
            continue
        level = delta_db(a["audible_power"], b["audible_power"])
        absolute = {key: delta_db(value, b["band_power"][key])
                    for key, value in a["band_power"].items()}
        changes[name] = {"level_db": level, "band_db": absolute,
                         "matched_sharp_db": absolute["sharp"] - level}
    return {"before": old, "after": new, "changes": changes,
            "max_sample_difference": float(np.max(np.abs(after - before)))}


def render_pair(renderers, spec, notes, rate):
    return [r.render(spec["model"], notes, spec["params"], 1.8, 1.2, rate,
                     effects=spec.get("effects", ())) for r in renderers]


def auditions(output, spec, pair, rows):
    """One shared-gain note sequence plus a separately labeled AC-RMS-matched candidate."""
    case_rows = [r for r in rows if r["profile"] == spec["name"] and r["rate"] == 48000]
    candidates = [i for i, row in enumerate(case_rows) if row["velocity"] == 1 and row["pitch"] < 89
                  and row["before"]["sustain"] is not None]
    if not candidates:
        return ""
    index = max(candidates, key=lambda i: case_rows[i]["before"]["sustain"]["sharp_fraction"])
    row = case_rows[index]
    old, new = pair[0][index], pair[1][index]
    ac_rms = lambda x: np.sqrt(np.mean((x - x.mean())**2))
    matched = new * (ac_rms(old) / ac_rms(new))
    variants = {"before": old, "after": new, "after-matched": matched}
    gain = min(1, .9 / max(np.max(np.abs(x)) for x in variants.values()))
    content = [f"<h2>{html.escape(spec['name'])} · MIDI {row['pitch']}</h2>", "<div class='players'>"]
    for label, x in variants.items():
        file = f"{spec['name']}-{label}.wav"
        sf.write(output / file, np.tile(np.pad(x * gain, (0, 9600)), 3), 48000, subtype="FLOAT")
        content.append(f"<div><p>{label}</p><audio controls preload='none' src='{file}'></audio></div>")
    content.append("</div>")
    return "".join(content)


def summarize(rows):
    """Compact data for changed and unchanged profiles, with high-register bypass checks."""
    result = {}
    for name in sorted({row["profile"] for row in rows}):
        cohort = [row for row in rows if row["profile"] == name]
        summary = {"conditions": len(cohort),
                   "max_sample_difference": max(row["max_sample_difference"] for row in cohort)}
        for stage in ("attack", "sustain"):
            active = [row for row in cohort if row["pitch"] < 89 and row["changes"][stage] is not None
                      and row["before"][stage]["sharp_fraction"] > .01]
            if not active:
                continue
            summary[stage] = {"conditions": len(active)}
            for key, values in {
                "sharp_db": [row["changes"][stage]["band_db"]["sharp"] for row in active],
                "matched_sharp_db": [row["changes"][stage]["matched_sharp_db"] for row in active],
                "level_db": [row["changes"][stage]["level_db"] for row in active],
            }.items():
                summary[stage][key] = {"median": float(np.median(values)), "min": min(values), "max": max(values)}
        result[name] = summary
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    renderers = [Renderer(path.resolve()) for path in (args.before, args.after)]
    rows, cards = [], []
    try:
        if {p["model"] for p in profiles()} != set(renderers[0].defaults):
            raise ValueError("profiles do not cover every instrument described by the worker")
        for spec in profiles():
            notes = [{"pitch": pitch, "velocity": velocity} for pitch in spec["pitches"] for velocity in VELOCITIES]
            for rate in RATES:
                pair = render_pair(renderers, spec, notes, rate)
                for note, old, new in zip(notes, *pair, strict=True):
                    row = {"profile": spec["name"], "rate": rate, **note, **compare(old, new, rate)}
                    if (spec["name"] in UNCHANGED or note["pitch"] >= 96) and not np.array_equal(old, new):
                        raise ValueError(f"bypass changed: {spec['name']}, pitch {note['pitch']}, rate {rate}")
                    rows.append(row)
                if rate == 48000 and spec["name"] not in UNCHANGED:
                    cards.append(auditions(args.output, spec, pair, rows))
            print("measured", spec["name"], flush=True)
    finally:
        for r in renderers:
            r.close()
    report = {"schema_version": 1, "baseline_commit": "bdd4192", "rate_hz": list(RATES),
              "velocities": list(VELOCITIES), "seconds": 1.8, "hold_seconds": 1.2,
              "policy": "Hann periodogram; 0-150 ms attack and 150-1200 ms sustain; AC normalization excludes below 30 Hz",
              "renderer_sha256": {label: hashlib.sha256(path.read_bytes()).hexdigest()
                                  for label, path in [("before", args.before), ("after", args.after)]},
              "numpy": np.__version__, "scipy": scipy.__version__, "profiles": profiles(),
              "unchanged_profiles": sorted(UNCHANGED), "summary": summarize(rows)}
    (args.output / "measurements.json").write_text(json.dumps(report | {"rows": rows}, indent=2) + "\n", encoding="utf-8")
    (args.output / "summary.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    (args.output / "report.html").write_text("""<!doctype html><html lang='en'><meta charset='utf-8'>
<meta name='viewport' content='width=device-width,initial-scale=1'><title>Instrument voicing comparison</title>
<style>body{font:16px/1.6 system-ui;max-width:1100px;margin:40px auto;padding:0 24px;background:#fafbfc;color:#202630}
.players{display:grid;grid-template-columns:repeat(auto-fit,minmax(260px,1fr));gap:16px}audio{width:100%}</style>
<h1>Instrument and lead voicing</h1><p>Three velocity-1 notes per file at two-second intervals.
Each profile selects its strongest held 2–5 kHz example with a fundamental below 1.4 kHz.
Before and after share a gain. After-matched additionally matches the original whole-note AC RMS;
DC is excluded. No limiter is used. These are diagnostic solo sounds, without musical backing.</p>
""" + "".join(cards) + "<p><a href='summary.json'>Summary</a> · <a href='measurements.json'>All measurements</a></p></html>", encoding="utf-8")
    print("Saved", args.output / "report.html")


if __name__ == "__main__":
    main()
