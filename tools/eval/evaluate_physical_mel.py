# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12", "matplotlib>=3.8"]
# ///
"""Compare two actual Rust builds against the frozen real-instrument cohort."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
import soundfile as sf
from fit_worker import Renderer
from physical_mel import RATE, configuration, distances, features, normalize
from prepare_physical_reference import load_notes, manifest_digest
from scipy.signal import resample_poly

MODELS = ("piano", "guitar", "violin")


def summary(values):
    return {"mean": float(np.mean(values)), "median": float(np.median(values)),
            "per_note": [float(value) for value in values]}


def evaluate(root, before, after, output, production_rate=RATE):
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    manifest = json.loads((root / "notes.json").read_text(encoding="utf-8"))
    if production_rate not in (RATE, 48000):
        raise ValueError("unsupported production sample rate")
    if manifest["rate"] != RATE:
        raise ValueError("unsupported cohort sample rate")
    renderers = [Renderer(before.resolve()), Renderer(after.resolve())]
    report = {"metric": configuration(), "reference_sha256": manifest_digest(manifest),
              "renderers": [{"sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                             "defaults": renderer.defaults} for path, renderer in zip((before, after), renderers, strict=True)],
              "production_rate": production_rate, "models": {}}
    output.mkdir(parents=True, exist_ok=True)
    audition, order = [], []
    try:
        for model in MODELS:
            report["models"][model] = {}
            for split in ("train", "validation"):
                notes, reference = load_notes(root, manifest, model, split)
                target = features(reference)
                rendered = [renderer.render(model, notes, {}, manifest["seconds"], manifest["hold"],
                                            rate=production_rate) for renderer in renderers]
                if production_rate != RATE:
                    # Both builds play at the production rate; the measure stays at 24 kHz.
                    rendered = [resample_poly(samples, RATE, production_rate, axis=-1) for samples in rendered]
                result = [distances(target, features(audio)) for audio in rendered]
                report["models"][model][split] = {
                    "notes": notes, "before": summary(result[0]), "after": summary(result[1]),
                    "relative_reduction": float(1 - np.mean(result[1]) / np.mean(result[0])),
                    "improved_notes": int(np.sum(result[1] < result[0])),
                    "peak_before": float(np.max(abs(rendered[0]))), "peak_after": float(np.max(abs(rendered[1]))),
                }
                print(f"{model} {split} ({len(notes)}): {result[0].mean():.6f} -> {result[1].mean():.6f}", flush=True)
                if split != "validation":
                    continue
                # A predetermined mf held-out pitch nearest the middle register, not the best score.
                center = {"piano": 60, "guitar": 55, "violin": 67}[model]
                index = min((i for i, note in enumerate(notes) if note["velocity"] == 0.65),
                            key=lambda i: abs(notes[i]["pitch"] - center))
                real, initial, final = reference[index], rendered[0][index], rendered[1][index]
                normalized = [normalize(audio)[0] for audio in (real, initial, final)]
                common_gain = min(0.5, 0.9 / max(np.max(abs(audio)) for audio in normalized))
                for label, samples in zip(("reference", "before", "after"), normalized, strict=True):
                    preview = samples * common_gain
                    # Only audition cuts are faded; feature/loss measurements use untouched PCM.
                    preview[:120] *= np.linspace(0, 1, 120)
                    preview[-120:] *= np.linspace(1, 0, 120)
                    audition.extend((preview, np.zeros(6000)))
                    order.append({"model": model, "pitch": notes[index]["pitch"], "label": label,
                                  "tuning_cents": notes[index]["tuning_cents"], "loss":
                                  None if label == "reference" else float(result[("before", "after").index(label)][index])})
                    sf.write(output / f"{model}-{label}.wav", preview, RATE, subtype="PCM_24")
                figure, axes = plt.subplots(3, 1, figsize=(9, 7), sharex=True, sharey=True)
                maximum = max(features(samples)[1].max() for samples in (real, initial, final))
                for axis, samples, label in zip(axes, (real, initial, final), ("Recorded", "Before", "After"), strict=True):
                    axis.imshow(features(samples)[1][0], origin="lower", aspect="auto",
                                extent=(0, manifest["seconds"], 0, 64), vmin=0, vmax=maximum)
                    axis.set_ylabel(f"{label}\nMel band")
                axes[0].set_title(f"{model.title()}, held-out MIDI {notes[index]['pitch']}, mf")
                axes[-1].set_xlabel("Seconds from onset")
                figure.tight_layout()
                figure.savefig(output / f"{model}-mel.png", dpi=150)
                plt.close(figure)
        sf.write(output / "reference-before-after.wav", np.concatenate(audition), RATE, subtype="PCM_24")
        report["audition_order"] = order
        report["audition_boundary_fade_ms"] = 5
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    finally:
        for renderer in renderers:
            renderer.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--production-rate", type=int, default=RATE, choices=(RATE, 48000))
    args = parser.parse_args()
    evaluate(args.reference, args.before, args.after, args.output, args.production_rate)
