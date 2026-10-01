# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12"]
# ///
"""Fit real physical-instrument parameters to a fixed real-recording mel cohort.

Uses the Rust instrument itself, a seeded bounded search, and training notes only.
The validation split is read once after fitting, never used by the optimizer.
"""

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
from pathlib import Path

# Small matrix products are faster and reproducible with a single BLAS thread.
os.environ.setdefault("OPENBLAS_NUM_THREADS", "1")
os.environ.setdefault("OMP_NUM_THREADS", "1")

import numpy as np
import scipy
import soundfile as sf
from physical_mel import RATE, configuration, distances, features
from prepare_physical_reference import manifest_digest
from scipy.optimize import differential_evolution, least_squares
from scipy.signal import sosfilt

REPO = Path(__file__).resolve().parents[2]
BOUNDS = {
    "piano": {"hardness": (0.1, 0.9), "position": (0.06, 0.35), "decay": (1, 12),
              "damping": (0, 0.5), "stiffness": (0, 0.0015)},
    "guitar": {"hardness": (0.1, 0.9), "position": (0.06, 0.35), "decay": (0.6, 10),
               "damping": (0, 0.5)},
    "violin": {"hardness": (0.1, 0.9), "position": (0.06, 0.35), "decay": (1, 12),
               "damping": (0, 0.5), "bow_pressure": (0.2, 0.85), "bow_speed": (0.2, 0.9)},
}
CENTERS = np.geomspace(90, 10000, 12)


def body_profiles(path=REPO / "crates/auris-synth/src/physical/body.rs"):
    text = path.read_text(encoding="utf-8")
    profiles = {}
    for model in BOUNDS:
        match = re.search(rf"Model::{model.title()} => Some\(\[(.*?)\]\)", text, re.DOTALL)
        if match is None:
            raise ValueError(f"missing body profile for {model}")
        gains = np.array([float(value) for value in match[1].split(",") if value.strip()])
        if gains.shape != (12,):
            raise ValueError("body profile parser must read twelve coefficients")
        profiles[model] = gains
    return profiles


def color(audio, gains, amount=0.65, rate=RATE):
    """Exact causal peaking-bank surrogate; verified against the Rust renderer."""
    sections = []
    for center, gain in zip(CENTERS, gains, strict=True):
        omega = 2 * np.pi * center / rate
        alpha = np.sin(omega) / (2 * 0.9)
        amplitude = 10 ** (gain / 40)
        a0 = 1 + alpha / amplitude
        sections.append(np.array([1 + alpha * amplitude, -2 * np.cos(omega),
                                  1 - alpha * amplitude, a0, -2 * np.cos(omega),
                                  1 - alpha / amplitude]) / a0)
    wet = sosfilt(np.array(sections), audio, axis=-1)
    return audio + amount * (wet - audio)


class Renderer:
    def __init__(self, executable):
        self.defaults = json.loads(subprocess.check_output([str(executable), "--describe"], text=True))
        self.process = subprocess.Popen([str(executable)], stdin=subprocess.PIPE, stdout=subprocess.PIPE)

    def close(self):
        self.process.stdin.close()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
        self.process.stdout.close()

    def render(self, model, notes, params, seconds, hold, rate=RATE):
        request = {"model": model, "notes": [{key: note[key] for key in
                    ("pitch", "velocity", "tuning_cents")} for note in notes],
                   "seconds": seconds, "hold": hold, "params": params}
        if rate != RATE:
            request["sample_rate"] = rate
        self.process.stdin.write((json.dumps(request, allow_nan=False) + "\n").encode())
        self.process.stdin.flush()
        header = self.process.stdout.read(4)
        if len(header) != 4:
            raise RuntimeError("Rust renderer terminated before PCM header")
        count = int.from_bytes(header, "little")
        expected = round(seconds * rate) * len(notes)
        if count != expected:
            raise RuntimeError(f"unexpected PCM length: {count}, expected {expected}")
        raw = self.process.stdout.read(count * 4)
        if len(raw) != count * 4:
            raise RuntimeError("truncated PCM from Rust renderer")
        return np.frombuffer(raw, dtype="<f4").reshape(len(notes), -1).astype(np.float64)


def load_notes(root, manifest, model, split):
    notes = [note for note in manifest["notes"] if note["model"] == model and note["split"] == split]
    if not notes:
        raise ValueError(f"empty {model} {split} cohort")
    audio = []
    for note in notes:
        path = root / note["path"]
        if hashlib.sha256(path.read_bytes()).hexdigest() != note["sha256"]:
            raise ValueError(f"note hash mismatch: {path}")
        samples, rate = sf.read(path)
        if rate != RATE or samples.ndim != 1 or len(samples) != round(manifest["seconds"] * RATE):
            raise ValueError(f"invalid prepared reference: {path}")
        audio.append(samples)
    return notes, np.array(audio)


def summary(values):
    return {"mean": float(np.mean(values)), "median": float(np.median(values)),
            "per_note": [float(value) for value in values]}


def fit_model(renderer, root, manifest, model, gains, iterations, seed):
    notes, reference = load_notes(root, manifest, model, "train")
    target = features(reference)
    defaults = renderer.defaults[model]
    keys = list(BOUNDS[model])
    bounds = list(BOUNDS[model].values())
    params = {key: defaults[key] for key in keys}
    history = []
    calls = 0

    def render(settings, dry=False, cohort=notes):
        return renderer.render(model, cohort, settings | ({"body": 0.0} if dry else {}),
                               manifest["seconds"], manifest["hold"])

    before_audio = render({})
    before = distances(target, features(before_audio))
    surrogate = color(render({}, dry=True), gains)
    agreement = float(np.max(abs(before_audio - surrogate)))
    if agreement > 2e-4:
        raise ValueError(f"Python/Rust radiation mismatch: {agreement}")
    print(f"{model}: {len(notes)} training notes, baseline {before.mean():.6f}, filter match {agreement:.2g}", flush=True)

    def objective(vector):
        nonlocal calls
        calls += 1
        settings = dict(zip(keys, map(float, vector), strict=True))
        synth = color(render(settings, dry=True), gains)
        try:
            value = float(np.mean(distances(target, features(synth))))
        except ValueError:
            value = 1e3
        if calls % 50 == 0:
            print(f"{model}: evaluation {calls}, loss {value:.6f}", flush=True)
        return value

    # Alternate excitation/loss and regularized coloration, both against temporal mel.
    for stage in range(2):
        result = differential_evolution(objective, bounds, x0=[params[key] for key in keys],
                                        seed=seed + stage, maxiter=iterations, popsize=5,
                                        polish=False, workers=1, tol=1e-4)
        current = np.array([params[key] for key in keys])
        if result.fun < objective(current):
            params = dict(zip(keys, map(float, result.x), strict=True))
        dry = render(params, dry=True)
        anchor = gains.copy()

        def residual(candidate, dry=dry, anchor=anchor):
            synth_features = features(color(dry, candidate))
            # Squared differences for the local fit; only keep it if primary L1 improves.
            rows = []
            for reference_features, candidate_features in zip(target, synth_features, strict=True):
                delta = reference_features - candidate_features
                rows.extend((delta[:, :, :15].ravel() / np.sqrt(delta[:, :, :15].size),
                             delta[:, :, 15:].ravel() / np.sqrt(delta[:, :, 15:].size)))
            return np.r_[*rows, (candidate - anchor) * 0.012, np.diff(candidate) * 0.01]

        fitted = least_squares(residual, gains, bounds=(-9, 9), max_nfev=16,
                               diff_step=0.002, ftol=0.005)
        previous_error = np.mean(distances(target, features(color(dry, gains))))
        candidate_error = np.mean(distances(target, features(color(dry, fitted.x))))
        if candidate_error < previous_error:
            gains = fitted.x
        error = float(np.mean(distances(target, features(color(dry, gains)))))
        history.append({"stage": stage, "loss": error, "params": params.copy(), "gains_db": gains.tolist()})
        print(f"{model}: stage {stage + 1} loss {error:.6f}", flush=True)

    after_audio = color(render(params, dry=True), gains)
    train_after = distances(target, features(after_audio))
    ratios = np.sqrt(np.mean(before_audio**2, axis=1) / np.mean(after_audio**2, axis=1))
    output_gain = float(np.median(ratios))
    after_audio *= output_gain
    # The held-out recordings are loaded only here, after all parameter selection.
    validation_notes, validation_reference = load_notes(root, manifest, model, "validation")
    validation_target = features(validation_reference)
    validation_before_audio = render({}, cohort=validation_notes)
    validation_after_audio = color(render(params, dry=True, cohort=validation_notes), gains) * output_gain
    validation_before = distances(validation_target, features(validation_before_audio))
    validation_after = distances(validation_target, features(validation_after_audio))
    output = root.parent / f"fit-{model}"
    output.mkdir(exist_ok=True)
    for label, cohort, recordings, before_samples, after_samples in (
        ("train", notes, reference, before_audio, after_audio),
        ("validation", validation_notes, validation_reference, validation_before_audio, validation_after_audio),
    ):
        for note, real, initial, final in zip(cohort, recordings, before_samples, after_samples, strict=True):
            stem = f"{label}-{note['pitch']}-{round(note['velocity'] * 100)}"
            for suffix, samples in (("reference", real), ("before", initial), ("after", final)):
                sf.write(output / f"{stem}-{suffix}.wav", samples, RATE, subtype="FLOAT")
    print(f"{model}: validation {validation_before.mean():.6f} -> {validation_after.mean():.6f}", flush=True)
    return {"params": params, "gains_db": gains.tolist(), "output_gain": output_gain,
            "search_calls": calls, "history": history,
            "surrogate_max_error": agreement, "level_policy": "multiply existing model normalization to preserve median training RMS",
            "training_notes": notes, "validation_notes": validation_notes,
            "train_before": summary(before), "train_after": summary(train_after),
            "validation_before": summary(validation_before), "validation_after": summary(validation_after)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    parser.add_argument("renderer", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--model", action="append", choices=tuple(BOUNDS))
    parser.add_argument("--iterations", type=int, default=8)
    parser.add_argument("--seed", type=int, default=20261001)
    parser.add_argument("--profiles", type=Path, help="frozen initial body gains for an archived renderer")
    args = parser.parse_args()
    if not 1 <= args.iterations <= 100:
        parser.error("iterations must be between 1 and 100")
    manifest = json.loads((args.reference / "notes.json").read_text(encoding="utf-8"))
    if manifest["rate"] != RATE:
        raise ValueError("renderer and reference sample rates differ")
    renderer = Renderer(args.renderer.resolve())
    profiles = ({model: np.asarray(gains, dtype=float) for model, gains in
                 json.loads(args.profiles.read_text(encoding="utf-8")).items()}
                if args.profiles else body_profiles())
    if set(profiles) != set(BOUNDS) or any(gains.shape != (12,) for gains in profiles.values()):
        raise ValueError("expected twelve initial body gains for each model")
    report = {"metric": configuration(), "seed": args.seed, "iterations": args.iterations,
              "versions": {"python": sys.version, "numpy": np.__version__, "scipy": scipy.__version__,
                           "soundfile": sf.__version__},
              "reference_sha256": manifest_digest(manifest),
              "renderer_sha256": hashlib.sha256(args.renderer.read_bytes()).hexdigest(),
              "defaults": renderer.defaults, "bounds": BOUNDS,
              "initial_gains_db": {model: gains.tolist() for model, gains in profiles.items()}, "models": {}}
    try:
        for model in args.model or BOUNDS:
            report["models"][model] = fit_model(renderer, args.reference, manifest, model,
                                                profiles[model], args.iterations, args.seed)
            args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    finally:
        renderer.close()


if __name__ == "__main__":
    main()
