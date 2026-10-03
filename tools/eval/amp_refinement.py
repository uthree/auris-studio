# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12"]
# ///
"""Train on EGFxSet pitches, then compare actual native amp output in bounded chunks."""

import argparse
import hashlib
import json
import platform
from importlib.metadata import version
from pathlib import Path

import numpy as np
from electric_copy import amp_audio, input_files, load, measurements, paired
from fit_physical_mel import Renderer
from physical_mel import normalize
from physical_pack import errors, metric_configuration, pack_features
from scipy.optimize import differential_evolution, minimize_scalar
from scipy.signal import resample_poly

SEED = 20261003
BOUNDS = {"drive_db": (0, 48), "bass_db": (-12, 12), "mid_db": (-12, 12), "treble_db": (-12, 12)}


def aligned(root, manifest, split):
    """Verify both recordings and their identities before measuring a paired cohort."""
    notes, dry = load(root, manifest, "Clean", split)
    wet_notes, wet = load(root, manifest, "BluesDriver", split)
    a = [(row["pickup"], row["pitch"]) for row in notes]
    b = [(row["pickup"], row["pitch"]) for row in wet_notes]
    if a != b:
        raise ValueError("clean/wet pairs differ")
    return notes, dry, wet


def fit(root, manifest, iterations):
    notes, dry, wet = aligned(root, manifest, "train")
    dry = normalize(dry)
    targets = [pack_features(row[None, :]) for row in wet]
    calls = {"simple": 0, "amp": 0}

    def objective(params, simple=False):
        name = "simple" if simple else "amp"
        calls[name] += 1
        values = []
        for audio, target in zip(dry, targets, strict=True):
            candidate = (np.tanh(audio * 10 ** (params["drive_db"] / 20)) if simple
                         else amp_audio(audio[None, :], params)[0])
            values.append(float(errors(target, pack_features(candidate[None, :]))["mel"][0]))
        value = float(np.mean(values))
        if calls[name] % 10 == 0:
            print(f"{name} training evaluation {calls[name]}: {value:.6f}", flush=True)
        return value

    simple = minimize_scalar(lambda drive: objective({"drive_db": drive}, True),
                             bounds=(0, 48), method="bounded", options={"xatol": .01})
    flat = minimize_scalar(lambda drive: objective({"drive_db": drive, "bass_db": 0,
                                                   "mid_db": 0, "treble_db": 0}),
                           bounds=(0, 48), method="bounded", options={"xatol": .01})
    initial = [float(flat.x), 0, 0, 0]
    keys = list(BOUNDS)
    result = differential_evolution(
        lambda vector: objective(dict(zip(keys, map(float, vector), strict=True))),
        list(BOUNDS.values()), x0=initial, seed=SEED, maxiter=iterations,
        popsize=4, polish=False, tol=1e-4,
    )
    return {"parameters": dict(zip(keys, map(float, result.x), strict=True)),
            "objective": float(result.fun), "flat_initial": initial,
            "flat_objective": float(flat.fun), "seed": SEED, "iterations": iterations,
            "bounds": BOUNDS, "evaluations": calls,
            "simple": {"parameters": {"drive_db": float(simple.x)},
                       "objective": float(simple.fun), "bounds": {"drive_db": [0, 48]}},
            "training_ids": [note["id"] for note in notes],
            "objective_definition": "mean phase-weighted log-mel L1 on training pitches only"}


def compare(root, manifest, renderer, baseline, split, fitted, old, destination, chunk):
    notes, _, wet = aligned(root, manifest, split)
    result = {"amp": [], "simple": [], "old_amp": []}
    squared_error, squared_native, max_error = 0.0, 0.0, 0.0
    for start in range(0, len(notes), chunk):
        group, target = notes[start:start + chunk], wet[start:start + chunk]
        paths, dry = input_files(root, group, destination)
        variants = [
            ("amp", renderer, "amp", fitted["parameters"] | {"cabinet": 0, "output_db": 0}),
            ("simple", renderer, "distortion", fitted["simple"]["parameters"] |
             {"mode": 0, "mix": 1, "output_db": 0}),
            ("old_amp", baseline, "amp", old | {"cabinet": 0, "output_db": 0}),
        ]
        for label, worker, effect, parameters in variants:
            audio = worker.render("electric_guitar", group, {}, 3, 3, rate=48000,
                                  effects=[{"id": effect, "params": parameters}], inputs=paths)
            if label == "amp":
                surrogate = amp_audio(dry, fitted["parameters"], rate=48000)
                squared_error += float(np.sum((audio - surrogate) ** 2))
                squared_native += float(np.sum(audio ** 2))
                max_error = max(max_error, float(abs(audio - surrogate).max()))
            result[label].extend(measurements(group, target, resample_poly(audio, 1, 2, axis=-1)))
    return {"count": len(notes), "rows": result,
            "old_to_new": paired(result["old_amp"], result["amp"]),
            "simple_to_new": paired(result["simple"], result["amp"]),
            "surrogate_relative_rms_error": float(np.sqrt(squared_error / squared_native)),
            "surrogate_max_error": max_error}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path("target/electric-guitar/reference"))
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--iterations", type=int, default=4)
    parser.add_argument("--fit", type=Path, help="replay a retained training fit instead of searching")
    parser.add_argument("--chunk", type=int, default=8)
    args = parser.parse_args()
    if args.iterations < 0 or not 1 <= args.chunk <= 16:
        parser.error("invalid search iterations or chunk size")
    manifest = json.loads((args.root / "notes.json").read_text(encoding="utf-8"))
    fitted = (json.loads(args.fit.read_text(encoding="utf-8")) if args.fit
              else fit(args.root, manifest, args.iterations))
    args.out.parent.mkdir(parents=True, exist_ok=True)
    (args.out.parent / "amp-fit.json").write_text(json.dumps(fitted, indent=2) + "\n", encoding="utf-8")
    previous = Path(__file__).parent / "references/electric-amp-fit.json"
    old = json.loads(previous.read_text(encoding="utf-8"))["parameters"]
    report = {"source": manifest["source"], "license": manifest["license"],
              "reference_sha256": digest(args.root / "notes.json"),
              "renderer_sha256": digest(args.renderer), "baseline_renderer_sha256": digest(args.baseline),
              "previous_fit_sha256": digest(previous), "previous_parameters": old,
              "fit": fitted, "metrics": metric_configuration(),
              "versions": {"python": platform.python_version(),
                           **{name: version(name) for name in ("numpy", "scipy", "soundfile")}},
              "evaluation_sources_sha256": {name: digest(Path(__file__).with_name(name)) for name in
                  ("amp_refinement.py", "electric_copy.py", "physical_pack.py", "physical_mel.py",
                   "fit_physical_mel.py", "prepare_physical_reference.py")},
              "interpretation": "Held-out pitches within the same setup; descriptive after model development. Cabinet bypass; no recorded speaker target.",
              "splits": {}}
    renderer = Renderer(args.renderer)
    try:
        baseline = Renderer(args.baseline)
        try:
            for split in ("train", "validation"):
                result = compare(args.root, manifest, renderer, baseline, split, fitted, old,
                                 args.out.parent / "amp-inputs", args.chunk)
                report["splits"][split] = result
                print(split, json.dumps({key: result[key]["mel"] for key in
                                         ("old_to_new", "simple_to_new")}), flush=True)
        finally:
            baseline.close()
    finally:
        renderer.close()
    args.out.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
