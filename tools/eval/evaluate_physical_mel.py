# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12", "matplotlib>=3.8"]
# ///
"""Compare two actual Rust builds against the frozen real-instrument cohort."""

import argparse
import hashlib
import json
from pathlib import Path

from fit_physical_mel import BOUNDS, Renderer, color, load_notes, summary
from physical_mel import RATE, configuration, distances, features, normalize
from prepare_physical_reference import manifest_digest
from scipy.signal import resample_poly


def evaluate(root, before, after, output, production_rate=RATE, calibration=None):
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    import numpy as np
    import soundfile as sf

    manifest = json.loads((root / "notes.json").read_text(encoding="utf-8"))
    fitted = json.loads(calibration.read_text(encoding="utf-8")) if calibration else None
    if production_rate not in (RATE, 48000) or production_rate != RATE and fitted is None:
        raise ValueError("48 kHz cross-check requires the frozen calibration report")
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
        for model in BOUNDS:
            report["models"][model] = {}
            for split in ("train", "validation"):
                notes, reference = load_notes(root, manifest, model, split)
                target = features(reference)
                rendered = [renderer.render(model, notes, {}, manifest["seconds"], manifest["hold"])
                            for renderer in renderers] if production_rate == RATE else []
                agreement = None
                if production_rate != RATE:
                    # This change only alters defaults, coloration and scalar normalization.
                    # Verify their reconstruction against the frozen worker before using the
                    # same unchanged oscillator/string algorithms for a 48 kHz cross-check.
                    previous_settings = fitted["defaults"][model] | {"body": 0.0}
                    gains = np.array(fitted["initial_gains_db"][model])
                    gain = fitted["models"][model]["output_gain"]
                    dry = renderers[1].render(model, notes, previous_settings,
                                             manifest["seconds"], manifest["hold"])
                    reconstructed = color(dry, gains) / gain
                    frozen = renderers[0].render(model, notes, {}, manifest["seconds"], manifest["hold"])
                    agreement = float(np.max(abs(reconstructed - frozen)))
                    if agreement > 2e-4:
                        raise ValueError(f"cannot reconstruct baseline: {agreement}")
                    old_dry = renderers[1].render(model, notes, previous_settings,
                        manifest["seconds"], manifest["hold"], rate=production_rate)
                    current = renderers[1].render(model, notes, {}, manifest["seconds"],
                                                  manifest["hold"], rate=production_rate)
                    rendered = [resample_poly(samples, RATE, production_rate, axis=-1)
                                for samples in (color(old_dry, gains, rate=production_rate) / gain, current)]
                result = [distances(target, features(audio)) for audio in rendered]
                report["models"][model][split] = {
                    "notes": notes, "before": summary(result[0]), "after": summary(result[1]),
                    "relative_reduction": float(1 - np.mean(result[1]) / np.mean(result[0])),
                    "improved_notes": int(np.sum(result[1] < result[0])),
                    "peak_before": float(np.max(abs(rendered[0]))), "peak_after": float(np.max(abs(rendered[1]))),
                    "baseline_reconstruction_max_error": agreement,
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
    parser.add_argument("--calibration", type=Path, help="required for the 48 kHz baseline reconstruction")
    args = parser.parse_args()
    evaluate(args.reference, args.before, args.after, args.output, args.production_rate, args.calibration)
