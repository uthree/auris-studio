# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12"]
# ///
"""Compare factory drum voicing with frozen production renderers, development only."""

import argparse
import hashlib
import html
import json
from pathlib import Path

import numpy as np
import scipy
import soundfile as sf
from fit_worker import Renderer
from physical_pack import snare_fitness, snare_wire_ratio
from scipy.signal import periodogram

PADS = {36: "kick", 38: "snare", 42: "closed_hat", 46: "open_hat",
        49: "crash", 51: "ride", 41: "tom_low", 47: "tom_mid", 50: "tom_high"}
RATES = (44100, 48000, 96000)
VELOCITIES = (.35, .65, 1.0)


def band_power(audio, rate, low, high):
    """Mean-square power in an inclusive band, without gain normalization or detrending."""
    frequencies, power = periodogram(audio, fs=rate, window="boxcar", detrend=False,
                                    scaling="spectrum")
    return float(power[(frequencies >= low) & (frequencies <= high)].sum())


def measure(audio, rate):
    """Retain absolute attack power, plate-line power and energy duration."""
    if len(audio) < rate or not np.isfinite(audio).all():
        raise ValueError("expected at least one second of finite mono PCM")
    attack = audio[:round(rate * .05)]
    body = audio[round(rate * .05):round(rate * .25)]
    cumulative = np.cumsum(audio**2)
    if cumulative[-1] <= 1e-15:
        raise ValueError("silent drum probe")
    return {
        "peak": float(np.max(np.abs(audio))),
        "rms": float(np.sqrt(np.mean(audio**2))),
        "t90_seconds": float(np.searchsorted(cumulative, cumulative[-1] * .9) / rate),
        "attack_power": float(np.mean(attack**2)),
        "attack_bands": {name: band_power(attack, rate, low, high)
                         for name, low, high in [("wire_body", 700, 1500),
                                                 ("sharp", 2000, 5000),
                                                 ("sizzle", 7000, 10000)]},
        "body_lines": {str(hz): band_power(body, rate, hz - 15, hz + 15)
                       for hz in (2650, 12359.6)},
    }


def delta_db(before, after):
    """A power ratio in decibels; zero-power comparisons are undefined."""
    if before <= 0 or after <= 0:
        raise ValueError("expected positive powers")
    return float(10 * np.log10(after / before))


def compare(before, after, rate):
    old, new = measure(before, rate), measure(after, rate)
    return {"before": old, "after": new,
            "rms_delta_db": delta_db(old["rms"]**2, new["rms"]**2),
            "attack_band_delta_db": {key: delta_db(value, new["attack_bands"][key])
                                     for key, value in old["attack_bands"].items()},
            "body_line_delta_db": {key: delta_db(value, new["body_lines"][key])
                                   for key, value in old["body_lines"].items()},
            "max_sample_difference": float(np.max(np.abs(after - before)))}


def listening(output, sounds, rows):
    """Save common-gain and RMS-matched comparisons without limiting or normalization per hit."""
    cards = []
    for name, pair in sounds.items():
        matches = [row for row in rows if row["pad"] == name and row["rate"] == 48000
                   and row["velocity"] == 1]
        row = matches[0]
        rms_gain = row["before"]["rms"] / row["after"]["rms"]
        variants = {"before": pair[0], "after": pair[1], "after-matched": pair[1] * rms_gain}
        common = min(1, .9 / max(np.max(np.abs(x)) for x in variants.values()))
        cards.append(f"<h2>{name.title()}</h2><p>Attack 2–5 kHz: "
                     f"{row['attack_band_delta_db']['sharp']:+.2f} dB; whole-note RMS: "
                     f"{row['rms_delta_db']:+.2f} dB; 90%-energy time: "
                     f"{row['before']['t90_seconds']:.4f} → {row['after']['t90_seconds']:.4f} s.</p>"
                     "<div class='players'>")
        for label, audio in variants.items():
            sequence = np.zeros(48000 * 8)
            for start in (0, 2, 4, 6):
                stop = min(len(sequence), start * 48000 + len(audio))
                sequence[start * 48000:stop] += audio[:stop - start * 48000] * common
            file = f"{name}-{label}.wav"
            sf.write(output / file, sequence, 48000, subtype="PCM_24")
            cards.append(f"<div><p>{html.escape(label)}</p>"
                         f"<audio controls preload='none' src='{file}'></audio></div>")
        cards.append("</div>")
    (output / "report.html").write_text("""<!doctype html><html lang="en"><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Drum voicing comparison</title><style>
body{font:16px/1.6 system-ui;max-width:1100px;margin:40px auto;padding:0 24px;background:#fafbfc;color:#202630}
.players{display:grid;grid-template-columns:repeat(auto-fit,minmax(260px,1fr));gap:20px}audio{width:100%}
</style><h1>Snare and ride voicing comparison</h1>
<p>Four identical isolated strikes per file at two-second intervals, velocity 1, 48 kHz.
Before and after share one listening gain. After-matched additionally matches the original
whole-note RMS, so the comparison can be checked independently of the level reduction.
No limiter or peak normalization per hit is used. These files compare production DSP.</p>
""" + "".join(cards) + '<p><a href="metrics.json">Complete measurements and hashes</a></p></html>',
                                        encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    renderers = [Renderer(path.resolve()) for path in (args.before, args.after)]
    rows, sounds = [], {}
    notes = [{"pitch": pitch, "velocity": velocity} for pitch in PADS for velocity in VELOCITIES]
    try:
        for rate in RATES:
            pair = [renderer.render("drums", notes, {}, 3, 3, rate) for renderer in renderers]
            for note, before, after in zip(notes, *pair, strict=True):
                name = PADS[note["pitch"]]
                row = {"pad": name, "rate": rate, **note, **compare(before, after, rate)}
                if name not in ("snare", "ride") and not np.array_equal(before, after):
                    raise ValueError(f"unrelated pad changed: {name} at {rate}")
                if rate == 48000 and note["velocity"] == 1 and name in ("snare", "ride"):
                    sounds[name] = before, after
                    if name == "snare":
                        row["snare_fitness"] = [snare_fitness(x) for x in (before, after)]
                        row["snare_wire_ratio"] = [snare_wire_ratio(x) for x in (before, after)]
                rows.append(row)
    finally:
        for renderer in renderers:
            renderer.close()
    report = {"schema_version": 1, "rate_hz": list(RATES), "velocities": list(VELOCITIES),
              "duration_seconds": 3, "params": {},
              "policy": "absolute unnormalized power; 0-50 ms attack; 50-250 ms body; rectangular periodogram",
              "renderer_sha256": {label: hashlib.sha256(path.read_bytes()).hexdigest()
                                  for label, path in [("before", args.before), ("after", args.after)]},
              "numpy": np.__version__, "scipy": scipy.__version__, "rows": rows}
    (args.output / "metrics.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    listening(args.output, sounds, rows)
    for row in rows:
        if row["rate"] == 48000 and row["velocity"] == 1 and row["pad"] in ("snare", "ride"):
            print(row["pad"], "bands", row["attack_band_delta_db"], "lines", row["body_line_delta_db"],
                  "RMS delta", row["rms_delta_db"], "t90", row["before"]["t90_seconds"],
                  row["after"]["t90_seconds"], "wire ratio", row.get("snare_wire_ratio"))


if __name__ == "__main__":
    main()
