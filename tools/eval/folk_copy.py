# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12"]
# ///
"""Copy-synthesis evaluation for the frozen folk-instrument cohorts."""

import argparse
import hashlib
import json
import platform
from importlib.metadata import version
from pathlib import Path

import numpy as np
import soundfile as sf
from fit_physical_mel import Renderer
from physical_mel import normalize
from physical_pack import envelope, errors, metric_configuration, pack_features, t90
from scipy.optimize import differential_evolution
from scipy.signal import resample_poly

SEED = 20261003


def paired(before, after):
    """Bootstrap distinct pitches; a small single-instrument cohort is exploratory."""
    if [row["id"] for row in before] != [row["id"] for row in after]:
        raise ValueError("comparison cohorts differ")
    result = {}
    keys = [key for key in ("mel", "envelope_db", "t90_seconds") if key in before[0]]
    for key in keys:
        a, b = [np.asarray([row[key] for row in rows]) for rows in (before, after)]
        groups = [np.mean((b - a)[[row["pitch"] == pitch for row in before]])
                  for pitch in sorted({row["pitch"] for row in before})]
        rng = np.random.default_rng(SEED)
        ci = np.quantile(rng.choice(groups, (2000, len(groups))).mean(axis=1), [.025, .975])
        result[key] = {"before": float(a.mean()), "after": float(b.mean()),
                       "reduction_percent": float(100 * (1 - b.mean() / a.mean())) if a.mean() else None,
                       "paired_delta_ci95": ci.tolist()}
    return result


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes().replace(b"\r\n", b"\n")).hexdigest()


def load(root: Path, model: str):
    manifest = json.loads((root / "notes.json").read_text(encoding="utf-8"))
    notes = [dict(note) for note in manifest["notes"] if note["model"] == model]
    if not notes or len({note["id"] for note in notes}) != len(notes):
        raise ValueError(f"invalid or empty {model} cohort")
    train = {note["pitch"] for note in notes if note["split"] == "train"}
    validation = {note["pitch"] for note in notes if note["split"] == "validation"}
    if train & validation:
        raise ValueError(f"pitch leakage in {model}: {train & validation}")
    audio = []
    for note in notes:
        path = root / note["path"]
        if hashlib.sha256(path.read_bytes()).hexdigest() != note["prepared_sha256"]:
            raise ValueError(f"prepared reference hash changed: {path}")
        source_path = root / "sources" / note["source"]
        if hashlib.sha256(source_path.read_bytes()).hexdigest() != note["source_sha256"]:
            raise ValueError(f"source reference hash changed: {source_path}")
        x, rate = sf.read(path, always_2d=False)
        if rate != manifest["rate"] or np.asarray(x).shape != (round(manifest["seconds"] * rate),):
            raise ValueError(f"invalid prepared note: {path}")
        note["hold"] = min(manifest["seconds"] - .02, note["hold_seconds"])
        note["velocity"] = .65
        note["bends"] = []
        note["expression"] = []
        audio.append(x)
    return manifest, notes, np.asarray(audio)


def render(renderer, manifest, notes, params, model, rate):
    chunks = []
    for note in notes:
        hold = max(.2, min(manifest["seconds"] - .02, note["hold_seconds"]))
        rendered_note = dict(note)
        if model == "tin_whistle":
            hold = min(manifest["seconds"] - .02, .25 + min(1.0, note["hold_seconds"]))
        rendered_note["hold"] = hold
        audio = renderer.render(model, [rendered_note], params, manifest["seconds"], hold, rate=rate)
        if rate != manifest["rate"]:
            audio = resample_poly(audio, manifest["rate"], rate, axis=-1)
        chunks.append(audio)
    return np.concatenate(chunks)


def metric_rows(model, notes, reference, candidate):
    if model == "tin_whistle":
        # The source is a continuous scale. Compare settled timbre and level;
        # attack and release are deliberately excluded from its primary metric.
        mel, env = [], []
        for index, note in enumerate(notes):
            count = max(round(.1 * 24_000), min(round(1.0 * 24_000), round((note["hold_seconds"] - .02) * 24_000)))
            ref = normalize(reference[index : index + 1, :count])
            cand = normalize(candidate[index : index + 1, round(.2 * 24_000) : round(.2 * 24_000) + count])
            ref_features, cand_features = pack_features(ref), pack_features(cand)
            mel.append(float(np.mean([abs(a - b).mean() for a, b in zip(ref_features, cand_features, strict=True)])))
            env.append(float(abs(envelope(ref) - envelope(cand)).mean()))
        mel, env = np.asarray(mel), np.asarray(env)
    else:
        values = errors(pack_features(reference), pack_features(candidate))
        mel, env, tail = values["mel"], abs(envelope(reference) - envelope(candidate)).mean(axis=-1), abs(t90(reference) - t90(candidate))
    return [{"id": note["id"], "pitch": note["pitch"], "split": note["split"],
             "mel": float(mel[i]), "envelope_db": float(env[i]),
             **({"t90_seconds": float(tail[i])} if model != "tin_whistle" else {})}
            for i, note in enumerate(notes)]


def fit(renderer, manifest, notes, reference, model, bounds, initial):
    train = [i for i, note in enumerate(notes) if note["split"] == "train"]
    train_notes = [notes[i] for i in train]
    target = reference[train]
    keys = list(bounds)
    calls = 0

    def objective(vector):
        nonlocal calls
        calls += 1
        params = dict(initial)
        params.update(zip(keys, map(float, vector), strict=True))
        candidate = render(renderer, manifest, train_notes, params, model, 24_000)
        rows = metric_rows(model, train_notes, target, candidate)
        value = float(np.mean([row["mel"] for row in rows]))
        value += .006 * float(np.mean([row["envelope_db"] for row in rows]))
        if model != "tin_whistle":
            value += .15 * float(np.mean([row["t90_seconds"] for row in rows]))
        return value

    result = differential_evolution(objective, list(bounds.values()), x0=[initial[key] for key in keys],
                                    seed=SEED, maxiter=4, popsize=4, polish=False, tol=1e-4)
    return {"model": model, "initial_parameters": initial,
            "parameters": dict(initial, **dict(zip(keys, map(float, result.x), strict=True))),
            "objective": float(result.fun), "evaluations": calls, "seed": SEED,
            "bounds": bounds, "training_ids": [note["id"] for note in train_notes]}


def metrics(model):
    configuration = metric_configuration()
    if model == "tin_whistle":
        configuration.update({"loss": "steady log-mel L1, mean of resolutions",
            "normalization": "per settled excerpt RMS 0.1; no time warp",
            "window": "reference 0..min(1 s, hold - 20 ms); candidate 200 ms plus the same duration",
            "secondary": "20 ms RMS-envelope L1 in dB on the same settled excerpts",
            "primary": "settled timbre; attack and release are excluded"})
        configuration.pop("attack_frames")
        configuration.pop("phases_seconds")
    return configuration


def main(args):
    manifest, notes, reference = load(args.reference, args.model)
    calibration = json.loads(args.calibration.read_text(encoding="utf-8")) if args.calibration else None
    if calibration is not None:
        if calibration["model"] != args.model:
            raise ValueError("calibration model differs from the selected instrument")
        expected_ids = [note["id"] for note in notes if note["split"] == "train"]
        if calibration["reference_sha256"] != digest(args.reference / "notes.json"):
            raise ValueError("calibration reference differs from frozen folk cohort")
        if calibration["training_ids"] != expected_ids:
            raise ValueError("calibration training cohort differs from frozen folk cohort")
    renderer = Renderer(args.renderer)
    try:
        defaults = renderer.defaults[args.model]
        baseline_renderer = Renderer(args.baseline) if args.baseline else renderer
        try:
            initial = (json.loads(args.initial_params.read_text(encoding="utf-8")) if args.initial_params
                       else baseline_renderer.defaults[args.model])
            if set(initial) != set(defaults) or not all(np.isfinite(value) for value in initial.values()):
                raise ValueError("initial controls do not match the instrument's parameters")
            baseline = render(baseline_renderer, manifest, notes, initial, args.model, args.rate)
        finally:
            if baseline_renderer is not renderer:
                baseline_renderer.close()
        fit_report = None
        params = defaults
        if args.fit_bounds:
            fit_report = fit(renderer, manifest, notes, reference, args.model, json.loads(args.fit_bounds.read_text()), initial)
            fit_report["reference_sha256"] = digest(args.reference / "notes.json")
            fit_report["renderer_sha256"] = hashlib.sha256(args.renderer.read_bytes()).hexdigest()
            params = fit_report["parameters"]
        candidate = render(renderer, manifest, notes, params, args.model, args.rate)
        report = {"model": args.model, "source": manifest["source"], "reference_sha256": digest(args.reference / "notes.json"),
                  "renderer_sha256": hashlib.sha256(args.renderer.read_bytes()).hexdigest(),
                  "baseline_renderer_sha256": hashlib.sha256((args.baseline or args.renderer).read_bytes()).hexdigest(),
                  "native_rate": args.rate, "versions": {"python": platform.python_version(), **{n: version(n) for n in ("numpy", "scipy", "soundfile")}},
                  "evaluation_sources_sha256": {name: digest(Path(__file__).with_name(name)) for name in
                      ("folk_copy.py", "fetch_folk_reference.py", "physical_pack.py", "physical_mel.py", "fit_physical_mel.py", "electric_copy.py")},
                  "text_hash_encoding": "UTF-8 with CRLF normalized to LF",
                  "metrics": metrics(args.model),
                  "fit": fit_report,
                  "factory_parameters": initial,
                  "current_factory_parameters": defaults,
                  "calibration_sha256": digest(args.calibration) if args.calibration else None,
                  "calibration": calibration,
                  "factory": metric_rows(args.model, notes, reference, baseline),
                  "candidate": metric_rows(args.model, notes, reference, candidate)}
        for label in ("factory", "candidate"):
            report[f"{label}_mean_mel"] = float(np.mean([row["mel"] for row in report[label]]))
            for split in ("train", "validation"):
                selected = [row["mel"] for row in report[label] if row["split"] == split]
                report[f"{label}_{split}_mean_mel"] = float(np.mean(selected))
        for split in ("train", "validation"):
            before = [row for row in report["factory"] if row["split"] == split]
            after = [row for row in report["candidate"] if row["split"] == split]
            report[f"paired_{split}"] = paired(before, after)
        args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    finally:
        renderer.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    parser.add_argument("renderer", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--model", choices=("tin_whistle", "hammered_dulcimer"), required=True)
    parser.add_argument("--fit-bounds", type=Path)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--initial-params", type=Path, help="frozen initial controls for portable baseline/search reproduction")
    parser.add_argument("--calibration", type=Path, help="retained fit provenance JSON")
    parser.add_argument("--rate", type=int, choices=(24_000, 48_000), default=24_000)
    main(parser.parse_args())
