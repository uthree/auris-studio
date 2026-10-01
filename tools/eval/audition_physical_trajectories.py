# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12", "matplotlib>=3.8"]
# ///
"""Plot fixed held-out violin examples and create a local before/after audition."""

import argparse
import json
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np
import soundfile as sf
from fit_physical_mel import Renderer
from fit_physical_trajectories import read_note
from physical_mel import RATE, features, normalize
from physical_trajectory import metrics, pitch_track
from scipy.signal import resample_poly


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--constant-before", action="store_true", help="audition the previous fixed bow beside recorded expression following")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    manifest = json.loads((args.reference / "trajectories.json").read_text(encoding="utf-8"))
    long = [note for note in manifest["notes"] if note["model"] == "violin"
            and note["dataset"] == "iowa" and note["split"] == "validation"
            and note["seconds"] == 3.0 and note["velocity"] == 0.65]
    selected = [min(long, key=lambda note: (abs(note["pitch"] - 67), note["id"]))]
    selected.extend(next(note for note in manifest["notes"] if note["id"] == identifier)
                    for identifier in ("tu-glissando-65", "tu-glissando-66"))
    before, after = Renderer(args.before.resolve()), Renderer(args.after.resolve())
    report = []
    audition = []
    try:
        for note in selected:
            reference = read_note(args.reference / note["path"], note, note["seconds"])
            renders = []
            for index, renderer in enumerate((before, after)):
                controls = ({key: value for key, value in note.items() if key != "expression"}
                            if index == 0 and args.constant_before else note)
                audio = renderer.render("violin", [controls], {}, note["seconds"], note["hold"], 48000)[0]
                renders.append(resample_poly(audio, 1, 2)[:len(reference)])
            samples = [reference, *renders]
            report.append({"id": note["id"], "before_expression_guide": not args.constant_before,
                           "after_expression_guide": True, "before": metrics(reference, renders[0], note),
                           "after": metrics(reference, renders[1], note)})
            mel = [features(audio)[1][0] for audio in samples]
            maximum = max(float(value.max()) for value in mel)
            figure, axes = plt.subplots(5, 1, figsize=(11, 10), layout="constrained")
            for axis, value, label in zip(axes[:3], mel, ("Real recording", "Before", "After"), strict=True):
                axis.imshow(value, origin="lower", aspect="auto", vmin=0, vmax=maximum,
                            extent=(0, note["seconds"], 0, 64))
                axis.set_ylabel(label + "\nmel bands")
            for audio, label in zip(samples, ("Real", "Before", "After"), strict=True):
                times, hz, voiced = pitch_track(audio, *note["pitch_bounds_hz"])
                axes[3].plot(times[voiced], hz[voiced], label=label, alpha=0.8)
                normalized = normalize(audio)[0]
                frames = len(normalized) // 480
                rms = np.sqrt(np.mean(normalized[:frames * 480].reshape(frames, 480)**2, axis=1))
                axes[4].plot(np.arange(frames) * 0.02, 20 * np.log10(rms + 1e-4), label=label)
            axes[3].set_ylabel("Fundamental (Hz)")
            axes[4].set_ylabel("RMS envelope (dB)")
            axes[4].set_xlabel("Time (seconds)")
            axes[3].legend()
            for axis in axes:
                axis.set_xlim(0, note["seconds"])
                axis.axvline(note["hold"], color="gray", linestyle=":")
                if "transition" in note:
                    axis.axvspan(*note["transition"], color="gray", alpha=0.15)
            figure.suptitle(note["id"] + "\nFixed held-out example, identical recorded pitch controls"
                            + ("\nBefore: constant bow; after: recorded expression" if args.constant_before else ""))
            figure.savefig(args.output / f"{note['id']}.png", dpi=140)
            plt.close(figure)
            # TU-Note is BY-ND: do not distribute modified excerpts or an edited
            # reference montage. The audition includes synthetic renders only there.
            preview = samples if note["dataset"] == "iowa" else renders
            for audio in preview:
                audio = normalize(audio)[0]
                audio *= min(0.6, 0.85 / max(abs(audio)))
                # Preview-only fades remove discontinuities at the excerpt boundary;
                # reference preparation and every reported metric use untouched PCM.
                ramp = np.linspace(0, 1, 120)
                audio[:120] *= ramp
                audio[-120:] *= ramp[::-1]
                audition.extend((audio, np.zeros(RATE // 3)))
        sf.write(args.output / "violin-long-and-glissando-before-after.wav", np.concatenate(audition), RATE, subtype="PCM_24")
        (args.output / "examples.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(report, indent=2), flush=True)
    finally:
        before.close()
        after.close()


if __name__ == "__main__":
    main()
