# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy==2.5.3", "scipy==1.18.1", "soundfile==0.14.0", "matplotlib>=3.10"]
# ///
"""Fit and compare the actual Rust choir against frozen real sung-vowel recordings."""

import argparse
import hashlib
import json
import os
import subprocess
from pathlib import Path

os.environ.setdefault("OPENBLAS_NUM_THREADS", "1")
os.environ.setdefault("OMP_NUM_THREADS", "1")

import numpy as np
import scipy
import soundfile as sf
from choir_reference import RATE, load_notes
from fetch_choir_reference import REFERENCES, cohort_lock
from physical_mel import configuration, distances, features, normalize
from prepare_physical_reference import manifest_digest
from scipy.optimize import differential_evolution
from scipy.signal import resample_poly

PERFORMANCE = {"ensemble": 0.0, "width": 0.0, "vibrato": 0.0}


def load_manifest(root):
    """Reject changes to notes, extraction, pitch guides or the singer split."""
    manifest = json.loads((root / "notes.json").read_text(encoding="utf-8"))
    expected = json.loads((REFERENCES / "choir-notes.json").read_text(encoding="utf-8"))
    if cohort_lock(manifest) != expected:
        raise ValueError("choir reference cohort differs from frozen extraction")
    return manifest


class Renderer:
    """Persistent worker; every note passes through the production instrument at 48 kHz."""

    def __init__(self, executable):
        self.executable = Path(executable).resolve()
        self.defaults = json.loads(subprocess.check_output([str(self.executable), "--describe"],
                                                         text=True))["choir"]
        self.process = subprocess.Popen([str(self.executable)], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE)

    def close(self):
        """Close the worker and report failed renderer exits."""
        self.process.stdin.close()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
        self.process.stdout.close()
        if self.process.returncode:
            raise RuntimeError(f"choir worker exited with {self.process.returncode}")

    def render(self, notes, params, seconds, calibration=None, rate=48000, guided=True):
        """Render reset notes in groups with the same vowel and fixed voice-size guide."""
        audio = np.empty((len(notes), round(seconds * rate)), dtype=np.float64)
        groups = sorted({(note["vowel"], note["voice_size"]) for note in notes})
        for vowel, size in groups:
            indices = [i for i, note in enumerate(notes)
                       if (note["vowel"], note["voice_size"]) == (vowel, size)]
            cohort = [{key: notes[i][key] for key in
                       (("pitch", "velocity", "tuning_cents", "bends") if guided
                        else ("pitch", "velocity"))} for i in indices]
            settings = (PERFORMANCE if guided else {}) | params | {"vowel": vowel, "voice_size": size}
            request = {"model": "choir", "notes": cohort, "seconds": seconds, "hold": seconds,
                       "params": settings, "sample_rate": rate}
            if calibration is not None:
                request["choir_calibration"] = calibration
            self.process.stdin.write((json.dumps(request, allow_nan=False) + "\n").encode())
            self.process.stdin.flush()
            header = self.process.stdout.read(4)
            if len(header) != 4:
                raise RuntimeError("choir renderer ended before PCM header")
            count = int.from_bytes(header, "little")
            expected = round(seconds * rate) * len(cohort)
            if count != expected:
                raise RuntimeError(f"wrong choir PCM count {count}, expected {expected}")
            raw = self.process.stdout.read(count * 4)
            if len(raw) != count * 4:
                raise RuntimeError("truncated choir PCM")
            audio[indices] = np.frombuffer(raw, dtype="<f4").reshape(len(cohort), -1)
        if not np.isfinite(audio).all():
            raise ValueError("non-finite choir audio")
        return resample_poly(audio, RATE, rate, axis=-1) if rate != RATE else audio


def summary(values):
    """Dimensionless mean/median/per-note acoustic distances."""
    return {"mean": float(np.mean(values)), "median": float(np.median(values)),
            "per_note": np.asarray(values).tolist()}


def provenance(manifest, renderers):
    """Describe source, feature definition and actual executable fingerprints."""
    return {"notes_sha256": manifest_digest(manifest), "metric": configuration(),
            "source": manifest["source"], "license": manifest["license"],
            "production_rate": 48000, "versions": {"numpy": np.__version__,
                "scipy": scipy.__version__, "soundfile": sf.__version__},
            "renderers": {key: {"sha256": hashlib.sha256(worker.executable.read_bytes()).hexdigest(),
                                "defaults": worker.defaults}
                          for key, worker in renderers.items()},
            "performance": "reference-only YIN bends at 20ms, ensemble/width/vibrato 0; same guides before/after"}


def fit(root, executable, iterations, seed):
    """Use training singers only; no validation PCM is loaded by this function."""
    manifest = load_manifest(root)
    notes, reference = load_notes(root, manifest, "train")
    target = features(reference)
    worker = Renderer(executable)
    initial_profile = json.loads((REFERENCES / "choir-mel-initial.json").read_text(encoding="utf-8"))
    initial_calibration = {key: initial_profile[key] for key in
                           ("areas", "radiation_hz", "output_gains")}
    try:
        baseline_audio = worker.render(notes, initial_profile["params"], manifest["seconds"],
                                       initial_calibration)
        baseline = distances(target, features(baseline_audio))
        areas = np.array(initial_profile["areas"])
        radiation = initial_profile["radiation_hz"]
        params = {key: initial_profile["params"][key] for key in ("tone", "breath", "attack")}
        history = []
        calls = 0

        def evaluate(settings, profiles, radiation_hz, cohort=notes, goal=target):
            nonlocal calls
            calls += 1
            audio = worker.render(cohort, settings, manifest["seconds"],
                                  {"areas": profiles.tolist(), "radiation_hz": float(radiation_hz)})
            loss = float(np.mean(distances(goal, features(audio))))
            if calls % 25 == 0:
                print(f"evaluation {calls}: loss {loss:.6f}", flush=True)
            return loss

        print(f"{len(notes)} training notes; baseline {baseline.mean():.6f}", flush=True)
        # Alternate a shared excitation/radiation fit with three bounded tract fits.
        for stage in range(2):
            bounds = [(0.0, 1.0), (0.0, 0.5), (0.01, 0.5), (np.log(30), np.log(4000))]
            initial = [params["tone"], params["breath"], params["attack"], np.log(radiation)]

            def source_objective(vector):
                settings = dict(zip(("tone", "breath", "attack"), map(float, vector[:3]), strict=True))
                return evaluate(settings, areas, np.exp(vector[3]))

            result = differential_evolution(source_objective, bounds, x0=initial,
                                            rng=seed + stage * 4, popsize=3, maxiter=iterations,
                                            polish=False, tol=1e-5)
            if result.fun < source_objective(initial):
                params = dict(zip(("tone", "breath", "attack"), map(float, result.x[:3]), strict=True))
                radiation = float(np.exp(result.x[3]))
            for vowel in range(3):
                indices = [i for i, note in enumerate(notes) if note["vowel"] == vowel]
                cohort = [notes[i] for i in indices]
                goal = [resolution[indices] for resolution in target]
                anchor = np.array(initial_profile["areas"][vowel])
                bounds = [(np.log(max(0.1, area / 3) / area), np.log(min(10, area * 3) / area))
                          for area in anchor[1:]]
                initial = np.log(areas[vowel, 1:] / anchor[1:])

                def tract_objective(vector, vowel=vowel, anchor=anchor, cohort=cohort, goal=goal,
                                    params=params, radiation=radiation):
                    candidate = areas.copy()
                    candidate[vowel, 1:] = anchor[1:] * np.exp(vector)
                    return evaluate(params, candidate, radiation, cohort, goal)

                result = differential_evolution(tract_objective, bounds, x0=initial,
                                                rng=seed + stage * 4 + vowel + 1,
                                                popsize=3, maxiter=iterations, polish=False, tol=1e-5)
                if result.fun < tract_objective(initial):
                    areas[vowel, 1:] = anchor[1:] * np.exp(result.x)
                print(f"stage {stage + 1} vowel {vowel}: {min(result.fun, tract_objective(initial)):.6f}",
                      flush=True)
            value = evaluate(params, areas, radiation)
            history.append({"stage": stage + 1, "loss": value, "params": params.copy(),
                            "areas": areas.tolist(), "radiation_hz": radiation})
            print(f"stage {stage + 1} all vowels: {value:.6f}", flush=True)
        final_audio = worker.render(notes, params, manifest["seconds"],
                                    {"areas": areas.tolist(), "radiation_hz": radiation})
        after = distances(target, features(final_audio))
        ratios = np.sqrt(np.mean(baseline_audio**2, axis=1) / np.mean(final_audio**2, axis=1))
        gains = [float(np.median(ratios[[note["vowel"] == vowel for note in notes]]))
                 for vowel in range(3)]
        return provenance(manifest, {"fit": worker}) | {
            "seed": seed, "iterations": iterations, "population_per_dimension": 3,
            "training_notes": [note["path"] for note in notes], "calls": calls,
            "params": params, "areas": areas.tolist(), "radiation_hz": radiation,
            "initial_profile_sha256": manifest_digest(initial_profile),
            "vowel_output_gains": gains,
            "level_policy": "preserve pre-calibration median training-note RMS per vowel",
            "train_before": summary(baseline), "train_after": summary(after), "history": history}
    finally:
        worker.close()


def compare(root, before_executable, after_executable, output, initial_profile=False):
    """Evaluate compiled factory constants, loading held-out singers only after fitting."""
    manifest = load_manifest(root)
    before, after = Renderer(before_executable), Renderer(after_executable)
    output.mkdir(parents=True, exist_ok=True)
    try:
        report = provenance(manifest, {"before": before, "after": after}) | {"splits": {}}
        baseline = json.loads((REFERENCES / "choir-mel-initial.json").read_text(encoding="utf-8")) \
            if initial_profile else {}
        calibration = {key: baseline[key] for key in ("areas", "radiation_hz", "output_gains")} \
            if initial_profile else None
        if initial_profile:
            report["before_profile_sha256"] = manifest_digest(baseline)
        for split in ("train", "validation"):
            notes, reference = load_notes(root, manifest, split)
            target = features(reference)
            initial = before.render(notes, baseline.get("params", {}), manifest["seconds"], calibration)
            final = after.render(notes, {}, manifest["seconds"])
            initial_losses = distances(target, features(initial))
            final_losses = distances(target, features(final))
            results = {"count": len(notes), "before": summary(initial_losses),
                       "after": summary(final_losses), "vowels": {}}
            for vowel, label in ((0, "oo"), (1, "ah"), (2, "ee")):
                indices = [i for i, note in enumerate(notes) if note["vowel"] == vowel]
                results["vowels"][label] = {"count": len(indices),
                    "before": summary(initial_losses[indices]), "after": summary(final_losses[indices])}
            results["notes"] = [note["path"] for note in notes]
            # Also check the normal ensemble with its own pitch motion; no recorded bend guide.
            unguided_before = before.render(notes, baseline.get("params", {}), manifest["seconds"],
                                           calibration, guided=False)
            unguided_after = after.render(notes, {}, manifest["seconds"], guided=False)
            results["factory_ensemble"] = {
                "before": summary(distances(target, features(unguided_before))),
                "after": summary(distances(target, features(unguided_after)))}
            report["splits"][split] = results
            print(f"{split} ({len(notes)}): {initial_losses.mean():.6f} -> {final_losses.mean():.6f}",
                  flush=True)
            if split == "validation":
                audition(output, notes, reference, initial, final)
        return report
    finally:
        before.close()
        after.close()


def audition(output, notes, reference, before, after):
    """Preselected held-out cases in reference/before/after order; common feature color scale."""
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    cases = []
    for singer in ("f2", "m3"):
        for vowel in (0, 1, 2):
            cases.append(next(i for i, note in enumerate(notes)
                              if note["singer"] == singer and note["vowel"] == vowel))
    rows = []
    figure, axes = plt.subplots(len(cases), 3, figsize=(10, 2 * len(cases)), constrained_layout=True)
    shared_maximum = max(float(features(audio[cases])[1].max()) for audio in (reference, before, after))
    for row, index in enumerate(cases):
        copies = normalize(np.array([reference[index], before[index], after[index]]))
        copies *= min(1, 0.85 / np.max(abs(copies)))
        fade = np.linspace(0, 1, round(RATE * 0.005))
        for column, samples in enumerate(copies):
            display = features(samples)[1][0]
            plot = axes[row, column].imshow(display, origin="lower", aspect="auto", vmin=0,
                                           vmax=shared_maximum, extent=(0, len(samples) / RATE, 0, 64))
            axes[row, column].set_title(f"{notes[index]['singer']} MIDI {notes[index]['pitch']} "
                                        f"{('oo', 'ah', 'ee')[int(notes[index]['vowel'])]} — "
                                        f"{('recorded', 'before', 'after')[column]}")
            samples[:len(fade)] *= fade
            samples[-len(fade):] *= fade[::-1]
            rows.extend((samples, np.zeros(round(RATE * 0.25))))
    axes[-1, 0].set_xlabel("Time (seconds)")
    axes[0, 0].set_ylabel("Mel band")
    figure.colorbar(plot, ax=axes, shrink=0.75, label="log(1 + 10000 × mel power)")
    figure.savefig(output / "held-out-log-mel.png", dpi=140)
    plt.close(figure)
    sf.write(output / "reference-before-after.wav", np.concatenate(rows), RATE, subtype="PCM_24")
    (output / "audition.json").write_text(json.dumps({"order": ["recorded", "before", "after"],
        "notes": [notes[index] for index in cases], "listening_only": "RMS 0.1, shared headroom, 5ms fades",
        "credit": "VocalSet: Julia Wilkins, Prem Seetharaman, Alison Wahl, Bryan Pardo; CC BY 4.0",
        "source": "https://doi.org/10.5281/zenodo.1442513"}, indent=2) + "\n", encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    fitting = commands.add_parser("fit")
    fitting.add_argument("reference", type=Path)
    fitting.add_argument("renderer", type=Path)
    fitting.add_argument("output", type=Path)
    fitting.add_argument("--iterations", type=int, default=6)
    fitting.add_argument("--seed", type=int, default=20261004)
    comparison = commands.add_parser("compare")
    comparison.add_argument("reference", type=Path)
    comparison.add_argument("before", type=Path)
    comparison.add_argument("after", type=Path)
    comparison.add_argument("output", type=Path)
    comparison.add_argument("--initial-profile", action="store_true",
                            help="replay the frozen pre-fit profile with a calibration-enabled before worker")
    args = parser.parse_args()
    if args.command == "fit":
        if not 1 <= args.iterations <= 100:
            parser.error("iterations must be between 1 and 100")
        report = fit(args.reference, args.renderer, args.iterations, args.seed)
        path = args.output
    else:
        report = compare(args.reference, args.before, args.after, args.output, args.initial_profile)
        path = args.output / "comparison.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(report, indent=2, allow_nan=False) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
