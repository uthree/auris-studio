# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12"]
# ///
"""Measure copy synthesis of the physical clarinet against Iowa MIS notes."""

import argparse
import hashlib
import json
import platform
from importlib.metadata import version
from pathlib import Path

import numpy as np
import soundfile as sf
from electric_copy import paired
from fit_physical_mel import Renderer
from physical_pack import envelope, errors, metric_configuration, pack_features, t90
from scipy.optimize import differential_evolution
from scipy.signal import resample_poly

SEED = 20261003


def reference_digest(path):
    """Hash the UTF-8 manifest with LF endings so Git checkouts agree across hosts."""
    return hashlib.sha256(path.read_bytes().replace(b"\r\n", b"\n")).hexdigest()


def load(root):
    manifest = json.loads((root / "notes.json").read_text(encoding="utf-8"))
    ids = [note["id"] for note in manifest["notes"]]
    if len(ids) != len(set(ids)):
        raise ValueError("duplicate note ids in frozen cohort")
    pitches = {split: {note["pitch"] for note in manifest["notes"] if note["split"] == split}
               for split in ("train", "validation")}
    if pitches["train"] & pitches["validation"]:
        raise ValueError("a pitch appears in both training and validation")
    rows = []
    for note in manifest["notes"]:
        path = root / note["path"]
        if hashlib.sha256(path.read_bytes()).hexdigest() != note["sha256"]:
            raise ValueError(f"reference hash changed: {path}")
        audio, rate = sf.read(path)
        if rate != manifest["rate"] or audio.shape != (round(manifest["seconds"] * rate),):
            raise ValueError(f"invalid prepared note: {path}")
        row = dict(note)
        row["tuning_cents"] = note["tuning_cents"]
        row["velocity"] = {"pp": .35, "mf": .65, "ff": .95}[note["dynamic"]]
        row["hold"] = min(manifest["seconds"] - 0.02, note["hold_seconds"])
        row["bends"] = []
        row["expression"] = []
        rows.append((row, audio))
    return manifest, rows


def render(renderer, manifest, rows, params, model="clarinet", rate=24_000):
    notes = [row for row, _ in rows]
    chunks = []
    for start in range(0, len(notes), 16):
        audio = renderer.render(model, notes[start:start + 16], params, manifest["seconds"],
                                manifest["hold"], rate=rate)
        if rate != manifest["rate"]:
            audio = resample_poly(audio, manifest["rate"], rate, axis=-1)
        chunks.append(audio)
    return np.concatenate(chunks)


def metric_rows(rows, reference, candidate):
    if len(rows) > 16:
        return [row for start in range(0, len(rows), 16) for row in
                metric_rows(rows[start:start + 16], reference[start:start + 16], candidate[start:start + 16])]
    values = errors(pack_features(reference), pack_features(candidate))
    values["envelope_db"] = abs(envelope(reference) - envelope(candidate)).mean(axis=-1)
    values["t90_seconds"] = abs(t90(reference) - t90(candidate))
    return [{"id": row["id"], "pitch": row["pitch"], "dynamic": row["dynamic"],
             **{key: float(array[index]) for key, array in values.items()}}
            for index, (row, _) in enumerate(rows)]


def fit(renderer, manifest, rows, defaults, bounds):
    train = [(row, audio) for row, audio in rows if row["split"] == "train"]
    reference = np.array([audio for _, audio in train])
    target = pack_features(reference)
    target_env, target_t90 = envelope(reference), t90(reference)
    keys = list(bounds)
    calls = 0

    def objective(vector):
        nonlocal calls
        calls += 1
        params = dict(defaults)
        params.update(zip(keys, map(float, vector), strict=True))
        candidate = render(renderer, manifest, train, params)
        value = float(errors(target, pack_features(candidate))["mel"].mean())
        value += .006 * float(abs(target_env - envelope(candidate)).mean())
        value += .15 * float(abs(target_t90 - t90(candidate)).mean())
        if calls % 20 == 0:
            print(f"clarinet evaluation {calls}: {value:.6f}", flush=True)
        return value

    result = differential_evolution(objective, list(bounds.values()), x0=[defaults[k] for k in keys],
                                    seed=SEED, maxiter=4, popsize=4, polish=False, tol=1e-4)
    return {"parameters": {**defaults, **dict(zip(keys, map(float, result.x), strict=True))},
            "objective": float(result.fun), "evaluations": calls, "seed": SEED,
            "bounds": bounds, "training_ids": [row["id"] for row, _ in train]}


def main(args):
    root = args.reference
    manifest, rows = load(root)
    calibration = None
    if args.calibration:
        calibration = json.loads(args.calibration.read_text(encoding="utf-8"))
        training_ids = [row["id"] for row, _ in rows if row["split"] == "train"]
        if calibration["training_ids"] != training_ids:
            raise ValueError("calibration training cohort differs from the frozen reference")
        if calibration["reference_sha256"] != reference_digest(root / "notes.json"):
            raise ValueError("calibration reference hash differs from the frozen reference")
    renderer = Renderer(args.renderer)
    try:
        defaults = renderer.defaults[args.model]
        baseline_renderer = Renderer(args.baseline) if args.baseline else renderer
        try:
            baseline_defaults = baseline_renderer.defaults[args.model]
            baseline = render(baseline_renderer, manifest, rows, baseline_defaults, args.model, args.rate)
        finally:
            if baseline_renderer is not renderer:
                baseline_renderer.close()
        params = json.loads(args.params.read_text(encoding="utf-8")) if args.params else defaults
        fit_report = None
        if args.fit_bounds:
            bounds = json.loads(args.fit_bounds.read_text(encoding="utf-8"))
            fit_report = fit(renderer, manifest, rows, defaults, bounds)
            params = fit_report["parameters"]
        candidate = render(renderer, manifest, rows, params, args.model, args.rate)
        report = {"source": manifest["source"], "license": manifest["license"],
                  "reference_sha256": reference_digest(root / "notes.json"),
                  "reference_hash_encoding": "UTF-8 with CRLF normalized to LF",
                  "renderer_sha256": hashlib.sha256(Path(args.renderer).read_bytes()).hexdigest(),
                  "baseline_renderer_sha256": hashlib.sha256(Path(args.baseline or args.renderer).read_bytes()).hexdigest(),
                  "native_rate": args.rate, "metrics": metric_configuration(),
                  "versions": {"python": platform.python_version(),
                               **{name: version(name) for name in ("numpy", "scipy", "soundfile")}},
                  "evaluation_sources_sha256": {
                      name: hashlib.sha256(Path(__file__).with_name(name).read_bytes()).hexdigest()
                      for name in ("clarinet_copy.py", "fetch_clarinet_reference.py", "electric_copy.py",
                                   "physical_pack.py", "physical_mel.py", "fit_physical_mel.py")},
                  "factory_parameters": baseline_defaults, "current_factory_parameters": defaults,
                  "calibration_sha256": (hashlib.sha256(args.calibration.read_bytes()).hexdigest()
                                         if args.calibration else None),
                  "fit": fit_report or ({"parameters": params, "source": str(args.params)}
                                         if args.params else None),
                  "factory": metric_rows(rows, np.array([audio for _, audio in rows]), baseline),
                  "candidate": metric_rows(rows, np.array([audio for _, audio in rows]), candidate)}
        report["factory_mean_mel"] = float(np.mean([r["mel"] for r in report["factory"]]))
        report["candidate_mean_mel"] = float(np.mean([r["mel"] for r in report["candidate"]]))
        for split in ("train", "validation"):
            ids = {row["id"] for row, _ in rows if row["split"] == split}
            for name in ("factory", "candidate"):
                selected = [row["mel"] for row in report[name] if row["id"] in ids]
                report[f"{name}_{split}_mean_mel"] = float(np.mean(selected))
            before = [row for row in report["factory"] if row["id"] in ids]
            after = [row for row in report["candidate"] if row["id"] in ids]
            report[f"paired_{split}"] = paired(before, after)
        args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    finally:
        renderer.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    parser.add_argument("renderer", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--model", default="clarinet")
    parser.add_argument("--fit-bounds", type=Path)
    parser.add_argument("--params", type=Path)
    parser.add_argument("--calibration", type=Path, help="retained training-search provenance; does not override factory controls")
    parser.add_argument("--baseline", type=Path, help="archived initial clarinet worker for a before/after comparison")
    parser.add_argument("--rate", type=int, choices=(24000, 48000), default=24000)
    main(parser.parse_args())
