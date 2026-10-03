# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy==2.5.3", "scipy==1.18.1", "soundfile==0.14.0"]
# ///
"""Fit remaining physical models and drum families on disjoint real captures."""

import argparse
import hashlib
import json
import os
import sys
from pathlib import Path

os.environ.setdefault("OPENBLAS_NUM_THREADS", "1")
os.environ.setdefault("OMP_NUM_THREADS", "1")

import numpy as np
import scipy
import soundfile as sf
from fit_physical_extensions import coordinate, paired_summary
from fit_physical_mel import Renderer, color
from physical_pack import (
    FAMILIES,
    envelope,
    errors,
    load,
    metric_configuration,
    pack_features,
    rows,
    snare_fitness,
    snare_wire_ratio,
    tom_role_margin,
)
from prepare_physical_reference import RATE, manifest_digest
from scipy.signal import resample_poly

PHYSICAL_BOUNDS = {"hardness": (0.05, 0.95), "position": (0.05, 0.45), "decay": (0.3, 12), "damping": (0, 0.6)}
DRUM_BOUNDS = {"hardness": (0.05, 0.95), "position": (0.05, 0.95), "decay": (0.5, 2), "damping": (0, 0.8)}


def guard_drums(renderer, root, manifest, family, result):
    """Select on training captures while retaining existing acoustic role contracts."""
    notes, reference = load(root, manifest, family, "train")
    baseline = renderer.render("drums", notes, {}, 3, 3)
    target = pack_features(reference)
    baseline_envelope = abs(envelope(reference) - envelope(baseline)).mean()
    grid = (0, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 1)
    best = None
    keys = [38] if family == "snare" else [41, 43, 45, 47, 48, 50]

    def valid(probe, gains):
        audio = color(probe, gains, amount=1.0, rate=48000)
        if family == "snare":
            return snare_fitness(audio[0]) >= 0.65 and snare_wire_ratio(audio[0]) > 2.05
        return min(tom_role_margin(note) for note in audio) >= 0.05

    def probe_audio(settings):
        return renderer.render("drums", [{"pitch": key, "velocity": 1} for key in keys], settings, 3, 3, rate=48000)

    for variant, settings in (("defaults", {}), ("fitted", result["params"])):
        source = renderer.render("drums", notes, settings, 3, 3)
        probe = probe_audio(settings)
        for scale in grid:
            gains = np.array(result["gains_db"]) * scale
            candidate = color(source, gains, amount=1.0)
            envelope_error = abs(envelope(reference) - envelope(candidate)).mean()
            loss = float(errors(target, pack_features(candidate))["mel"].mean())
            if valid(probe, gains) and envelope_error <= baseline_envelope * 1.05 and (best is None or loss < best[0]):
                best = loss, variant, scale, candidate, settings
    if best is None:
        raise ValueError(f"no {family} candidate satisfies the acoustic contract")
    loss, variant, scale, candidate, settings = best
    source = renderer.render("drums", notes, settings, 3, 3)
    probe = probe_audio(settings)

    def constrained(gains):
        if not valid(probe, gains):
            return 1000.0
        audio = color(source, gains, amount=1.0)
        if abs(envelope(reference) - envelope(audio)).mean() > baseline_envelope * 1.05:
            return 1000.0
        return float(errors(target, pack_features(audio))["mel"].mean()) + 0.0001 * np.mean(np.array(gains)**2)

    gains, _ = coordinate(constrained, np.array(result["gains_db"]) * scale, [(-9, 9)] * 12, (0.125, 0.05, 0.02))
    gains = [round(gain, 6) for gain in gains]
    candidate = color(source, gains, amount=1.0)
    loss = float(errors(target, pack_features(candidate))["mel"].mean())
    return result | {"params": {key: settings.get(key, renderer.defaults["drums"][key]) for key in result["params"]},
                     "gains_db": gains,
                     "normalization": float(np.median(np.sqrt(np.mean(baseline**2, axis=-1) / np.mean(candidate**2, axis=-1)))),
                     "train_after": loss, "acceptance_guard": {"candidate": variant, "radiation_scale": scale,
                         "acoustic_contract": {"snare_fitness_min": 0.65, "wire_head_min_ratio": 2.05} if family == "snare" else {"tom_minus_kick_min": 0.05, "keys": keys},
                         "constrained_fractions": [0.125, 0.05, 0.02],
                         "training_envelope_max_ratio": 1.05, "grid": list(grid)}}


def fit(renderer, root, manifest, family):
    notes, reference = load(root, manifest, family, "train")
    model = notes[0]["model"]
    target = pack_features(reference)
    bounds = PHYSICAL_BOUNDS if model != "drums" else DRUM_BOUNDS
    keys = list(bounds)
    initial = [renderer.defaults[model][key] for key in keys]
    gains = np.zeros(12)

    def render(vector):
        return renderer.render(model, notes, dict(zip(keys, vector, strict=True)), 3, 3)

    def loss(audio):
        return float(errors(target, pack_features(audio))["mel"].mean())

    baseline = render(initial)
    print(family, len(notes), "baseline", loss(baseline), flush=True)
    params, _ = coordinate(lambda x: loss(render(x)), initial, list(bounds.values()), (0.18, 0.06))
    source = render(params)
    gains, _ = coordinate(lambda x: loss(color(source, x, amount=1.0)) + 0.0001 * np.mean(np.array(x)**2),
                          gains, [(-9, 9)] * 12, (0.25, 0.10))
    gains = [round(value, 6) for value in gains]
    params, _ = coordinate(lambda x: loss(color(render(x), gains, amount=1.0)), params,
                           list(bounds.values()), (0.03,))
    params = {key: round(value, 6) for key, value in zip(keys, params, strict=True)}
    candidate = color(renderer.render(model, notes, params, 3, 3), gains, amount=1.0)
    normalization = float(np.median(np.sqrt(np.mean(baseline**2, axis=-1) / np.mean(candidate**2, axis=-1))))
    return {"model": model, "params": params, "gains_db": gains, "normalization": normalization,
            "training_notes": len(notes), "train_before": float(loss(baseline)), "train_after": float(loss(candidate)),
            "bounds": bounds}


def evaluate(before, after, root, manifest, family, fit_result, rate, audition):
    notes, reference = load(root, manifest, family, "validation")
    model = notes[0]["model"]
    initial = before.render(model, notes, {}, 3, 3, rate=rate)
    predicted = color(before.render(model, notes, fit_result["params"], 3, 3, rate=rate),
                      fit_result["gains_db"], amount=1.0, rate=rate) * fit_result["normalization"]
    final = after.render(model, notes, {}, 3, 3, rate=rate) if after else predicted
    agreement = float(np.max(abs(final - predicted))) if after else None
    if agreement is not None and agreement > 2e-4:
        raise ValueError(f"implemented calibration differs from frozen fit: {family}, {rate}, {agreement}")
    # Analysis stays at 24 kHz; the instrument itself is also checked at 48 kHz.
    a, b = (resample_poly(initial, 1, 2, axis=-1), resample_poly(final, 1, 2, axis=-1)) if rate == 48000 else (initial, final)
    baseline_rows, final_rows = rows(notes, reference, a), rows(notes, reference, b)
    summary = paired_summary(baseline_rows, final_rows)
    for name in ("attack", "body", "tail", "t90_error_seconds"):
        summary[f"{name}_before"] = float(np.mean([row[name] for row in baseline_rows]))
        summary[f"{name}_after"] = float(np.mean([row[name] for row in final_rows]))
    if audition and rate == RATE:
        destination = audition / family
        destination.mkdir(parents=True, exist_ok=True)
        # Fixed first validation capture; same gain for before/after, reference RMS matched once.
        x, y = initial[0], final[0]
        ref = reference[0] * np.sqrt(np.mean(x*x) / np.mean(reference[0]**2))
        common = 0.85 / max(np.max(abs(ref)), np.max(abs(x)), np.max(abs(y)))
        for label, samples in (("reference", ref), ("before", x), ("after", y)):
            sf.write(destination / f"{label}.wav", samples * common, RATE, subtype="PCM_24")
    print(family, rate, summary["before"], "->", summary["after"], "match", agreement, flush=True)
    return {"summary": summary, "surrogate_max_error": agreement, "before": baseline_rows, "after": final_rows}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    parser.add_argument("renderer", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--fit", type=Path, help="frozen fit; evaluate instead of optimizing")
    parser.add_argument("--candidate", type=Path)
    parser.add_argument("--family", action="append", choices=FAMILIES)
    parser.add_argument("--audition", type=Path)
    args = parser.parse_args()
    manifest = json.loads((args.reference / "notes.json").read_text(encoding="utf-8"))
    before = Renderer(args.renderer.resolve())
    after = Renderer(args.candidate.resolve()) if args.candidate else None
    report = {"metric": metric_configuration(), "reference_sha256": manifest_digest(manifest),
              "renderer_sha256": hashlib.sha256(args.renderer.read_bytes()).hexdigest(),
              "versions": {"python": sys.version, "numpy": np.__version__, "scipy": scipy.__version__, "soundfile": sf.__version__},
              "defaults": before.defaults, "families": {}}
    if args.candidate:
        report["candidate_sha256"] = hashlib.sha256(args.candidate.read_bytes()).hexdigest()
    if args.fit:
        report["fit_sha256"] = manifest_digest(json.loads(args.fit.read_text(encoding="utf-8")))
    frozen = json.loads(args.fit.read_text(encoding="utf-8")) if args.fit else None
    if frozen and frozen["reference_sha256"] != report["reference_sha256"]:
        raise ValueError("frozen fit uses a different reference cohort")
    try:
        for family in args.family or FAMILIES:
            if frozen:
                result = frozen["families"][family]
                rates = (RATE, 48000) if after else (RATE,)
                report["families"][family] = {"fit": result, "evaluation": {str(rate): evaluate(
                    before, after, args.reference, manifest, family, result, rate, args.audition) for rate in rates}}
            else:
                result = fit(before, args.reference, manifest, family)
                report["families"][family] = guard_drums(before, args.reference, manifest, family, result) if family in ("snare", "tom") else result
            args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    finally:
        before.close()
        if after:
            after.close()


if __name__ == "__main__":
    main()
