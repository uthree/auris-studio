# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12"]
# ///
"""Fit and audit long real notes and glissandi with the actual Rust instruments."""

import argparse
import hashlib
import json
import os
from collections import defaultdict
from pathlib import Path

os.environ.setdefault("OPENBLAS_NUM_THREADS", "1")
os.environ.setdefault("OMP_NUM_THREADS", "1")

import numpy as np
import scipy
import soundfile as sf
from fit_physical_mel import BOUNDS, Renderer, body_profiles, color, load_notes
from physical_mel import RATE, distances, features
from physical_trajectory import metrics, phase_masks, temporal_error
from prepare_physical_reference import manifest_digest
from scipy.optimize import differential_evolution, least_squares
from scipy.signal import lfilter, resample_poly

LONG_BOUNDS = {model: bounds.copy() for model, bounds in BOUNDS.items()}
LONG_BOUNDS["violin"]["release"] = (0.05, 1.5)
LONG_BOUNDS["violin"]["bow_response"] = (0.002, 0.06)


def performance_color(audio, gains, notes, rate=RATE, response=0.0):
    """Match Rust's body-before-output-expression order for moving CC11.

    Dynamic gain and a causal body filter do not commute. Undo only the dry
    renderer's output expression (the bow still received it), color, then restore
    that expression. Reference guides stay above zero so no hidden state is lost.
    """
    expression = np.ones_like(audio)
    for index, note in enumerate(notes):
        points = note.get("expression", [])
        if not points:
            continue
        frames = np.floor(np.array([p["seconds"] for p in points], dtype=np.float32)
                          * np.float32(rate) + 0.5).astype(int)
        values = np.r_[1.0, [point["value"] for point in points]]
        expression[index] = values[np.searchsorted(frames, np.arange(audio.shape[1]), side="right")]
        if response > 0:
            # The first point arrives before note-on, when Rust sets the initial
            # value directly. Subsequent points use the prepared one-pole response.
            step = float(np.float32(1 - np.exp(np.float32(-1 / (rate * response)))))
            pole = 1 - step
            expression[index], _ = lfilter([step], [1, -pole], expression[index],
                                           zi=[expression[index, 0] * pole])
    if expression.min() < 0.001:
        raise ValueError("zero expression cannot be inverted for the body surrogate")
    return color(audio / expression, gains, rate=rate) * expression


def compact(manifest):
    """Commit metadata and control hashes rather than thousands of control points."""
    notes = []
    for note in manifest["notes"]:
        item = {key: value for key, value in note.items() if key not in ("bends", "expression")}
        for key in ("bends", "expression"):
            if key in note:
                item[f"{key}_sha256"] = manifest_digest(note[key])
                item[f"{key}_points"] = len(note[key])
        notes.append(item)
    return manifest | {"notes": notes, "full_manifest_sha256": manifest_digest(manifest)}


class Corpus:
    def __init__(self, root, manifest, short_root, model, split, expression=True):
        grouped = defaultdict(list)
        for note in manifest["notes"]:
            if note["model"] != model or note["split"] != split:
                continue
            path = root / note["path"]
            audio = read_note(path, note, note["seconds"])
            if not expression:
                note = {key: value for key, value in note.items() if key != "expression"}
            grouped[(note["dataset"], note["seconds"])].append((note, audio))
        short_manifest = json.loads((short_root / "notes.json").read_text(encoding="utf-8"))
        notes, audio = load_notes(short_root, short_manifest, model, split)
        grouped[("short", 1.0)] = [(note | {"id": note["path"], "seconds": 1.0, "hold": 1.0,
                                           "dataset": "short"}, samples)
                                  for note, samples in zip(notes, audio, strict=True)]
        self.groups = []
        for (dataset, seconds), entries in grouped.items():
            notes, samples = zip(*entries, strict=True)
            reference = np.array(samples)
            self.groups.append({"dataset": dataset, "seconds": seconds, "notes": list(notes),
                                "reference": reference, "target": features(reference)})
        self.counts = {key: sum(len(group["notes"]) for group in self.groups if group["dataset"] == key)
                       for key in ("short", "iowa", "tu-note")}
        requested = {"short": 0.3, "iowa": 0.5, "tu-note": 0.2}
        total = sum(requested[key] for key, count in self.counts.items() if count)
        self.weights = {key: requested[key] / total for key, count in self.counts.items() if count}

    def render(self, renderer, params, gains=None, rate=RATE, output_gain=1.0):
        result = []
        for group in self.groups:
            audio = renderer.render(group["notes"][0]["model"], group["notes"],
                                    params | ({"body": 0.0} if gains is not None else {}),
                                    group["seconds"], group["seconds"], rate)
            if gains is not None:
                audio = performance_color(audio, gains, group["notes"], rate=rate,
                                          response=self.response(renderer, params))
            if rate == 48000:
                audio = resample_poly(audio, 1, 2, axis=-1)[:, :group["reference"].shape[1]]
            result.append(audio * output_gain)
        return result

    def response(self, renderer, params):
        model = self.groups[0]["notes"][0]["model"]
        if model != "violin":
            return 0.0
        return (renderer.performance["expression_response_fraction"]
                * params.get("bow_response", renderer.defaults[model].get("bow_response", 0.012)))

    def errors(self, audio):
        values = defaultdict(list)
        for group, candidate in zip(self.groups, audio, strict=True):
            synth = features(candidate)
            if group["dataset"] == "short":
                values["short"].extend(distances(group["target"], synth))
            else:
                for index, note in enumerate(group["notes"]):
                    real = [feature[index:index + 1] for feature in group["target"]]
                    generated = [feature[index:index + 1] for feature in synth]
                    error, _ = temporal_error(real, generated, note)
                    values[group["dataset"]].append(error)
        return {key: float(np.mean(errors)) for key, errors in values.items()}

    def loss(self, audio):
        errors = self.errors(audio)
        return sum(errors[key] * weight for key, weight in self.weights.items())

    def audit(self, audio):
        rows = []
        for group, candidate in zip(self.groups, audio, strict=True):
            for note, reference, synth in zip(group["notes"], group["reference"], candidate, strict=True):
                rows.append({"id": note["id"], "dataset": group["dataset"],
                             "seconds": note["seconds"], "hold": note["hold"],
                             "pitch": note["pitch"], "velocity": note["velocity"],
                             **metrics(reference, synth, note)})
        return {"mel_by_dataset": self.errors(audio), "weighted_mel": self.loss(audio), "notes": rows}


def read_note(path, note, seconds):
    if hashlib.sha256(path.read_bytes()).hexdigest() != note["sha256"]:
        raise ValueError(f"prepared trajectory hash mismatch: {path}")
    audio, rate = sf.read(path)
    if rate != RATE or audio.ndim != 1 or len(audio) != round(seconds * RATE):
        raise ValueError(f"invalid trajectory file: {path}")
    return audio


def fit(corpus, renderer, model, gains, iterations, seed, start=None, local=True, whole=False):
    keys = list(LONG_BOUNDS[model])
    defaults = renderer.defaults[model]
    params = {key: defaults[key] for key in keys}
    before = corpus.render(renderer, {})
    match = max(float(np.max(abs(a - b))) for a, b in zip(
        before, corpus.render(renderer, {}, gains), strict=True))
    if match > 2e-4:
        raise ValueError(f"radiation surrogate differs from Rust: {match}")
    baseline = corpus.errors(before)
    print(model, "train baseline", baseline, "counts", corpus.counts, flush=True)
    calls = 0
    if start:
        params = start["params"].copy()
        if set(params) != set(keys):
            raise ValueError("starting calibration has different parameter bounds")
        gains = np.array(start["gains_db"], dtype=float)

    def objective(vector):
        nonlocal calls
        calls += 1
        settings = dict(zip(keys, map(float, vector), strict=True))
        try:
            value = corpus.loss(corpus.render(renderer, settings, gains))
        except ValueError:
            value = 1000.0
        if calls % 20 == 0:
            print(model, "evaluation", calls, "loss", round(value, 6), flush=True)
        return value

    if not start:
        result = differential_evolution(objective, list(LONG_BOUNDS[model].values()),
                                        x0=[params[key] for key in keys], seed=seed,
                                        maxiter=iterations, popsize=4, polish=False, tol=1e-4)
        if result.fun < objective([params[key] for key in keys]):
            params = dict(zip(keys, map(float, result.x), strict=True))
    # The calibrated starting point is much better than most wide random draws.
    # Finish with a fixed local coordinate schedule, still using training only.
    vector = np.array([params[key] for key in keys])
    best = objective(vector)
    for fraction in (0.05, 0.02, 0.008) if local else ():
        for index, key in enumerate(keys):
            low, high = LONG_BOUNDS[model][key]
            candidates = []
            for direction in (-1, 1):
                candidate = vector.copy()
                candidate[index] = np.clip(vector[index] + direction * fraction * (high - low), low, high)
                candidates.append((objective(candidate), candidate))
            value, candidate = min(candidates, key=lambda result: result[0])
            if value < best:
                best, vector = value, candidate
    params = dict(zip(keys, map(float, vector), strict=True))
    dry = corpus.render(renderer, params | {"body": 0.0})
    anchor = gains.copy()

    def residual(candidate):
        rows = []
        for group, samples in zip(corpus.groups, dry, strict=True):
            generated = features(performance_color(samples, candidate, group["notes"],
                                                    response=corpus.response(renderer, params)))
            weight = np.sqrt(corpus.weights[group["dataset"]] / corpus.counts[group["dataset"]])
            for reference, synth in zip(group["target"], generated, strict=True):
                delta = reference - synth
                if whole:
                    rows.append(delta.ravel() * weight / np.sqrt(delta.shape[1] * delta.shape[2]))
                    continue
                for index, note in enumerate(group["notes"]):
                    masks, phase_weights = phase_masks(note, delta.shape[-1])
                    for phase, mask in masks.items():
                        rows.append(delta[index, :, mask].ravel() * weight
                                    * np.sqrt(phase_weights[phase] / (delta.shape[1] * mask.sum())))
        return np.r_[*rows, (candidate - anchor) * 0.018, np.diff(candidate) * 0.008]

    fitted = least_squares(residual, gains, bounds=(-9, 9), max_nfev=10, diff_step=0.002, ftol=0.01)
    after = [performance_color(samples, gains, group["notes"], response=corpus.response(renderer, params))
             for samples, group in zip(dry, corpus.groups, strict=True)]
    proposal = [performance_color(samples, fitted.x, group["notes"], response=corpus.response(renderer, params))
                for samples, group in zip(dry, corpus.groups, strict=True)]
    if corpus.loss(proposal) < corpus.loss(after):
        gains, after = fitted.x, proposal
    ratios = np.concatenate([np.sqrt(np.mean(a**2, axis=1) / np.mean(b**2, axis=1))
                             for a, b in zip(before, after, strict=True)])
    output_gain = float(np.median(ratios))
    print(model, "train fitted", corpus.errors(after), flush=True)
    return {"params": params, "gains_db": gains.tolist(), "output_gain": output_gain,
            "search_calls": calls, "surrogate_max_error": match,
            "before": corpus.audit(before), "after": corpus.audit(after),
            "counts": corpus.counts, "dataset_weights": corpus.weights}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    parser.add_argument("short_reference", type=Path)
    parser.add_argument("renderer", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--fit", action="store_true")
    parser.add_argument("--before", type=Path)
    parser.add_argument("--model", action="append", choices=tuple(BOUNDS))
    parser.add_argument("--iterations", type=int, default=6)
    parser.add_argument("--rate", type=int, choices=(24000, 48000), default=24000)
    parser.add_argument("--seed", type=int, default=20261001)
    parser.add_argument("--start-from", type=Path, help="training-only local continuation of a matching calibration")
    parser.add_argument("--profiles", type=Path, help="frozen body gains belonging to the supplied renderer")
    parser.add_argument("--no-local", action="store_true", help="skip local coordinate refinement")
    parser.add_argument("--whole-note-coloration", action="store_true", help="use a whole-note L2 coloration proposal")
    parser.add_argument("--constant-expression", action="store_true", help="evaluation ablation: omit the recorded expression guide")
    args = parser.parse_args()
    if args.fit and (args.before or args.rate != RATE or args.constant_expression) or not 1 <= args.iterations <= 100:
        parser.error("fit uses one 24 kHz renderer and 1..100 iterations")
    manifest = json.loads((args.reference / "trajectories.json").read_text(encoding="utf-8"))
    renderer = Renderer(args.renderer.resolve())
    before_renderer = Renderer(args.before.resolve()) if args.before else None
    report = {"reference_sha256": manifest_digest(manifest), "seed": args.seed,
              "iterations": args.iterations, "rate": args.rate,
              "renderer_sha256": hashlib.sha256(args.renderer.read_bytes()).hexdigest(),
              "before_renderer_sha256": hashlib.sha256(args.before.read_bytes()).hexdigest() if args.before else None,
              "versions": {"numpy": np.__version__, "scipy": scipy.__version__, "soundfile": sf.__version__},
              "defaults": renderer.defaults, "performance": renderer.performance,
              "expression_guide": not args.constant_expression, "local_refinement": not args.no_local,
              "coloration_proposal": "whole-note L2" if args.whole_note_coloration else "phase-balanced L2",
              "bounds": LONG_BOUNDS, "models": {}}
    report["temporal_metric"] = {
        "phase_weights": {"attack": 0.15, "early": 0.15, "late": 0.4, "transition": 0.2, "release": 0.1},
        "centers": "only feature centers strictly inside the excerpt",
        "release": "requires note-off before excerpt end",
        "short_regression": "original equal attack/sustain metric",
    }
    profiles = ({model: np.asarray(gains, dtype=float) for model, gains in
                 json.loads(args.profiles.read_text(encoding="utf-8")).items()}
                if args.profiles else body_profiles())
    report["initial_gains_db"] = {model: gains.tolist() for model, gains in profiles.items()}
    starting = json.loads(args.start_from.read_text(encoding="utf-8")) if args.start_from else None
    if starting and (starting["reference_sha256"] != manifest_digest(manifest)
                     or starting["renderer_sha256"] != report["renderer_sha256"]):
        raise ValueError("local continuation must use the same reference and renderer")
    try:
        for model in args.model or BOUNDS:
            if args.fit:
                train = Corpus(args.reference, manifest, args.short_reference, model, "train")
                fitted = fit(train, renderer, model, profiles[model], args.iterations, args.seed,
                             starting["models"][model] if starting else None,
                             local=not args.no_local, whole=args.whole_note_coloration)
                report["models"][model] = fitted
                args.output.write_text(json.dumps(report, indent=2, allow_nan=False) + "\n", encoding="utf-8")
                # Held-out recordings enter only after fitting has finished.
                validation = Corpus(args.reference, manifest, args.short_reference, model, "validation")
                initial = validation.render(renderer, {})
                final = validation.render(renderer, fitted["params"], np.array(fitted["gains_db"]),
                                          output_gain=fitted["output_gain"])
                fitted["validation_before"] = validation.audit(initial)
                fitted["validation_after"] = validation.audit(final)
                report["models"][model] = fitted
                print(model, "validation", validation.errors(initial), "->", validation.errors(final), flush=True)
            else:
                report["models"][model] = {}
                for split in ("train", "validation"):
                    corpus = Corpus(args.reference, manifest, args.short_reference, model, split,
                                    expression=not args.constant_expression)
                    final = corpus.render(renderer, {}, rate=args.rate)
                    initial = corpus.render(before_renderer or renderer, {}, rate=args.rate)
                    report["models"][model][split] = {"before": corpus.audit(initial), "after": corpus.audit(final)}
                    print(model, split, corpus.errors(initial), "->", corpus.errors(final), flush=True)
            args.output.write_text(json.dumps(report, indent=2, allow_nan=False) + "\n", encoding="utf-8")
    finally:
        renderer.close()
        if before_renderer:
            before_renderer.close()


if __name__ == "__main__":
    main()
