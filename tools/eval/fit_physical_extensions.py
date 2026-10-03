# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy==2.5.3", "scipy==1.18.1", "soundfile==0.14.0", "matplotlib>=3.8"]
# ///
"""Training-only physical extensions and frozen, paired real-recording evaluation."""

import argparse
import hashlib
import json
import os
from pathlib import Path

os.environ.setdefault("OPENBLAS_NUM_THREADS", "1")
os.environ.setdefault("OMP_NUM_THREADS", "1")

import numpy as np
import scipy
import soundfile as sf
from fit_physical_mel import Renderer
from fit_physical_trajectories import Corpus
from physical_extensions import (
    MODE_HZ,
    MODE_Q,
    REGISTER_BOUNDS,
    render_extension,
    resonant_body,
)
from physical_mel import RATE, configuration, distances, features, normalize
from physical_trajectory import metrics
from prepare_physical_reference import manifest_digest


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def coordinate(objective, initial, bounds, fractions):
    """Fixed local search including the unchanged starting model, on training only."""
    vector = np.array(initial, dtype=float)
    best = objective(vector)
    calls = 1
    for fraction in fractions:
        for index, (low, high) in enumerate(bounds):
            options = []
            for direction in (-1, 1):
                candidate = vector.copy()
                candidate[index] = np.clip(candidate[index] + direction * fraction * (high - low), low, high)
                if np.array_equal(candidate, vector):
                    continue
                options.append((objective(candidate), candidate))
                calls += 1
            if options:
                value, candidate = min(options, key=lambda pair: pair[0])
                if value < best:
                    best, vector = value, candidate
        print("local", fraction, "calls", calls, "loss", best, flush=True)
    return vector.tolist(), best


def fit_model(corpus, renderer, model):
    baseline = corpus.render(renderer, {})
    baseline_loss = corpus.loss(baseline)
    print(model, "baseline", baseline_loss, flush=True)
    # Precompute the six causal basis responses once; candidate search changes gains only.
    bases = []
    for group, audio in zip(corpus.groups, baseline, strict=True):
        bases.append(np.array([resonant_body(audio, np.eye(len(MODE_HZ))[index],
                         group["notes"], corpus.response(renderer, {})) - audio
                               for index in range(len(MODE_HZ))]))

    def body_loss(gains):
        audio = [dry + np.einsum("m,mnt->nt", gains, basis)
                 for dry, basis in zip(baseline, bases, strict=True)]
        return corpus.loss(audio) + 0.001 * np.mean(np.array(gains)**2)

    modes, _ = coordinate(body_loss, [0] * len(MODE_HZ), [(-1.5, 1.5)] * len(MODE_HZ), (0.15, 0.06, 0.02))
    modes = [round(value, 6) for value in modes]

    def register_loss(slopes):
        return corpus.loss(render_extension(corpus, renderer, model, {"register": slopes}))

    register, _ = coordinate(register_loss, [0, 0, 0], REGISTER_BOUNDS, (0.25, 0.10, 0.04))
    register = [round(value, 6) for value in register]
    profile = {"modes": modes, "register": register, "bow_exponent": 1.0, "gesture": 0.0}
    if model == "violin":
        exponent, _ = coordinate(lambda x: corpus.loss(render_extension(
            corpus, renderer, model, {"bow_exponent": x[0]})), [1], [(0.5, 2.0)], (0.20, 0.08, 0.03))
        profile["bow_exponent"] = round(exponent[0], 6)
        # This ablation receives pitch guides but no recorded amplitude guide on either side.
        automatic, _ = coordinate(lambda x: corpus.loss(render_extension(
            corpus, renderer, model, {"gesture": x[0]}, automatic=True)),
            [0], [(0, 0.7)], (0.25, 0.10, 0.04))
        profile["gesture"] = round(automatic[0], 6)
    losses = {"before": baseline_loss}
    for variant in ("body", "register", "combined"):
        losses[variant] = corpus.loss(render_extension(corpus, renderer, model, variant_profile(profile, variant)))
    return {"profile": profile, "training_loss": losses, "counts": corpus.counts}


def variant_profile(profile, variant):
    if variant == "body":
        return {"modes": profile["modes"]}
    if variant == "register":
        return {"register": profile["register"]}
    if variant == "bow":
        return {"bow_exponent": profile["bow_exponent"]}
    return profile


def rows(corpus, audio):
    result = []
    for group, synth in zip(corpus.groups, audio, strict=True):
        generated = features(synth)
        for index, (note, real, candidate) in enumerate(zip(group["notes"], group["reference"], synth, strict=True)):
            target = [feature[index:index + 1] for feature in group["target"]]
            measure = metrics(real, candidate, note, target=target)
            if group["dataset"] == "short":
                measure["mel"] = float(distances(target, [feature[index:index + 1] for feature in generated])[0])
            result.append({"id": note["id"], "source": note.get("source", note["id"]),
                           "dataset": note["dataset"], "pitch": note["pitch"],
                           "velocity": note["velocity"], **measure})
    return result


def paired_summary(before, after):
    """Paired bootstrap clusters overlapping excerpts by original source capture."""
    if [row["id"] for row in before] != [row["id"] for row in after]:
        raise ValueError("unpaired evaluation rows")
    groups = {}
    for a, b in zip(before, after, strict=True):
        groups.setdefault(a["source"], []).append(b["mel"] - a["mel"])
    clusters = list(groups.values())
    rng = np.random.default_rng(20261003)
    boot = [float(np.mean([value for index in rng.integers(0, len(clusters), len(clusters))
                          for value in clusters[index]])) for _ in range(2000)]
    a = np.array([row["mel"] for row in before])
    b = np.array([row["mel"] for row in after])
    return {"count": len(a), "source_clusters": len(clusters), "before": float(a.mean()),
            "after": float(b.mean()), "relative_reduction": float(1 - b.mean() / a.mean()),
            "improved": int(np.sum(b < a)), "worse": int(np.sum(b > a)),
            "mean_delta_ci95": np.quantile(boot, [0.025, 0.975]).tolist(),
            "envelope_before_db": float(np.mean([row["envelope_db"] for row in before])),
            "envelope_after_db": float(np.mean([row["envelope_db"] for row in after]))}


def evaluate(corpus, renderer, model, profile, rate, candidate=None, audition=None):
    before = corpus.render(renderer, {}, rate=rate)
    before_rows = rows(corpus, before)
    result = {"before": {"weighted_mel": corpus.loss(before), "notes": before_rows}, "variants": {}}
    result["dataset_weights"] = corpus.weights
    result["fixed_training_profile"] = profile
    variants = {name: render_extension(corpus, renderer, model, variant_profile(profile, name), rate)
                for name in ("body", "register", "combined")}
    if model == "violin":
        variants["bow"] = render_extension(corpus, renderer, model, variant_profile(profile, "bow"), rate)
        fixed = render_extension(corpus, renderer, model, {}, rate, automatic=True)
        auto = render_extension(corpus, renderer, model, {"gesture": profile["gesture"]}, rate, automatic=True)
        result["automatic_bow"] = paired_summary(rows(corpus, fixed), rows(corpus, auto))
    if candidate:
        variants["production"] = corpus.render(candidate, {}, rate=rate)
    for name, audio in variants.items():
        candidate_rows = rows(corpus, audio)
        datasets = {}
        for dataset in corpus.weights:
            datasets[dataset] = paired_summary([row for row in before_rows if row["dataset"] == dataset],
                                               [row for row in candidate_rows if row["dataset"] == dataset])
        result["variants"][name] = {"weighted_mel": corpus.loss(audio), "by_dataset": datasets, "notes": candidate_rows}
        print(model, rate, name, result["variants"][name]["weighted_mel"], flush=True)
    if audition:
        # Select by metadata only, with all candidates in the same predetermined case.
        group_index = next(i for i, g in enumerate(corpus.groups) if g["dataset"] == "iowa" and g["seconds"] == 3.0)
        group = corpus.groups[group_index]
        index = min((i for i, n in enumerate(group["notes"]) if n["velocity"] == 0.65),
                    key=lambda i: (abs(group["notes"][i]["pitch"] - {"piano": 60, "guitar": 55, "violin": 67}[model]),
                                   group["notes"][i]["pitch"]))
        sounds = [group["reference"][index], before[group_index][index]]
        listening = {"production": variants["production"]} if candidate else variants
        sounds += [audio[group_index][index] for audio in listening.values()]
        normalized = [normalize(sound)[0] for sound in sounds]
        gain = min(1.0, 0.85 / max(np.max(abs(sound)) for sound in normalized))
        montage = []
        for sound in normalized:
            sound = sound.copy() * gain
            fade = np.linspace(0, 1, 120)
            sound[:120] *= fade
            sound[-120:] *= fade[::-1]
            montage.extend((sound, np.zeros(6000)))
        sf.write(audition / f"{model}-{rate}.wav", np.concatenate(montage), RATE, subtype="FLOAT")
        result["audition"] = {"note": group["notes"][index]["id"], "order": ["recording", "before", *listening],
                              "rms": 0.1, "common_gain": gain}
        if candidate:
            import matplotlib
            matplotlib.use("Agg")
            import matplotlib.pyplot as plt

            spectra = [features(sound)[1][0] for sound in sounds]
            figure, axes = plt.subplots(1, 3, figsize=(11, 3.3), sharey=True, layout="constrained")
            vmax = max(np.max(spectrum) for spectrum in spectra)
            for axis, spectrum, title in zip(axes, spectra, ("Recording", "Before", "After"), strict=True):
                plot = axis.imshow(spectrum, origin="lower", aspect="auto", extent=(0, 3, 0, 64),
                                   vmin=0, vmax=vmax, cmap="magma")
                axis.set_title(title)
                axis.set_xlabel("Time (s)")
            axes[0].set_ylabel("Mel band")
            figure.colorbar(plot, ax=axes, label="log(1 + 10000 * mel power)", shrink=0.85)
            figure.suptitle(f"{model.title()} | {group['notes'][index]['id']} | whole-note RMS matched")
            figure.savefig(audition / f"{model}-{rate}.png", dpi=160)
            plt.close(figure)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    parser.add_argument("short_reference", type=Path)
    parser.add_argument("renderer", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--fit", action="store_true")
    parser.add_argument("--profiles", type=Path)
    parser.add_argument("--candidate", type=Path)
    parser.add_argument("--rate", type=int, choices=(24000, 48000), default=24000)
    parser.add_argument("--audition", type=Path)
    args = parser.parse_args()
    if args.output.exists():
        parser.error("output exists; preserve previous measurements")
    manifest = json.loads((args.reference / "trajectories.json").read_text(encoding="utf-8"))
    provenance = {"reference_sha256": manifest_digest(manifest),
                  "short_reference_sha256": digest(args.short_reference / "notes.json"),
                  "renderer_sha256": digest(args.renderer)}
    if args.fit == bool(args.profiles):
        parser.error("choose training-only --fit or evaluation --profiles")
    if args.fit and (args.rate != RATE or args.candidate or args.audition):
        parser.error("fit at 24 kHz; candidate and audition are validation options")
    report = provenance | {"metric": configuration(), "rate": args.rate,
             "mode_hz": MODE_HZ, "mode_q": MODE_Q, "register_bounds": REGISTER_BOUNDS,
             "versions": {"numpy": np.__version__, "scipy": scipy.__version__, "soundfile": sf.__version__}, "models": {}}
    report["temporal_metric"] = {"phase_weights": {"attack": 0.15, "early": 0.15, "late": 0.4,
                                  "transition": 0.2, "release": 0.1},
                                  "empty_phases": "omit and renormalize; no artificial excerpt release",
                                  "dataset_weights": {"short": 0.3, "iowa": 0.5, "tu-note": 0.2},
                                  "missing_datasets": "renormalize nonempty cohorts",
                                  "short_loss": "equal-weight first 150 ms attack and remainder"}
    report["evaluation_policy"] = {"split": "train" if args.fit else "validation",
            "pitch_expression_guides": "identical supplied guides, no validation parameter fitting",
            "bootstrap": "2000 paired source-cluster draws, seed 20261003",
            "automatic_bow": "unweighted paired notes, no recorded expression on either side"}
    saved = json.loads(args.profiles.read_text()) if args.profiles else None
    if saved and any(saved[key] != value for key, value in provenance.items()):
        raise ValueError("training provenance differs from evaluation inputs")
    if saved:
        report["training_sha256"] = digest(args.profiles)
    renderer = Renderer(args.renderer.resolve())
    candidate = None
    if args.candidate:
        report["candidate_sha256"] = digest(args.candidate)
    if args.audition:
        args.audition.mkdir(parents=True, exist_ok=True)
    try:
        if renderer.performance.get("expression_exponent", 1.0) != 1.0 or any(
                gain != 0 for modes in renderer.radiation.values() for _, _, gain in modes):
            raise ValueError("extensions require the preserved pre-extension baseline renderer")
        candidate = Renderer(args.candidate.resolve()) if args.candidate else None
        for model in ("piano", "guitar", "violin"):
            corpus = Corpus(args.reference, manifest, args.short_reference, model,
                            "train" if args.fit else "validation")
            report["models"][model] = (fit_model(corpus, renderer, model) if args.fit else
                 evaluate(corpus, renderer, model, saved["models"][model]["profile"],
                          args.rate, candidate, args.audition))
    finally:
        renderer.close()
        if candidate:
            candidate.close()
    args.output.write_text(json.dumps(report, indent=2, allow_nan=False) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
