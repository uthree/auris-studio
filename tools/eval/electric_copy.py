# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12"]
# ///
"""Fit electric DI and generic saturation on training pitches, then measure held-out notes."""

import argparse
import hashlib
import json
import os
import platform
from importlib.metadata import version
from pathlib import Path

os.environ.setdefault("OPENBLAS_NUM_THREADS", "1")
os.environ.setdefault("OMP_NUM_THREADS", "1")

import numpy as np
import soundfile as sf
from fit_physical_mel import Renderer
from physical_mel import normalize
from physical_pack import envelope, errors, metric_configuration, pack_features, t90
from prepare_physical_reference import tuning_cents
from scipy.optimize import differential_evolution, minimize_scalar
from scipy.signal import resample_poly, sosfilt, upfirdn

SEED = 20261003
DI_BOUNDS = {"hardness": (0.1, 1), "position": (0.05, 0.35), "decay": (1, 16), "decay_ratio": (0.25, 1),
             "damping": (0, 0.4), "tone": (1500, 9000), "pickup_q": (0.5, 2),
             "neck_position": (0.12, 0.25), "bridge_position": (0.025, 0.08)}


def load(root, manifest, effect, split):
    notes = [dict(note) for note in manifest["notes"] if note["effect"] == effect and note["split"] == split]
    audio = []
    for note in notes:
        path = root / note["path"]
        if hashlib.sha256(path.read_bytes()).hexdigest() != note["sha256"]:
            raise ValueError("reference hash changed")
        x, rate = sf.read(path)
        if rate != 24000 or x.shape != (72000,):
            raise ValueError("unexpected prepared note")
        note["tuning_cents"] = tuning_cents(x, note["pitch"])
        audio.append(x)
    return notes, np.array(audio)


def di_audio(renderer, notes, params, rate=24000, model="electric_guitar"):
    result = []
    order = []
    for pickup in ("Neck", "Bridge"):
        indices = [index for index, note in enumerate(notes) if note["pickup"] == pickup]
        if not indices:
            continue
        group = [notes[index] for index in indices]
        controls = {key: value for key, value in params.items() if key not in ("neck_position", "bridge_position")}
        if model == "electric_guitar":
            controls["pickup_position"] = params[pickup.lower() + "_position"]
        result.append(renderer.render(model, group, controls, 3, 3, rate=rate))
        order.extend(indices)
    if len(order) != len(notes) or not notes:
        raise ValueError("empty or unknown pickup cohort")
    return np.concatenate(result)[np.argsort(order)]


def measurements(notes, reference, candidate):
    values = errors(pack_features(reference), pack_features(candidate))
    values["envelope_db"] = abs(envelope(reference) - envelope(candidate)).mean(axis=-1)
    values["t90_seconds"] = abs(t90(reference) - t90(candidate))
    return [{"id": note["id"], "pitch": note["pitch"], "pickup": note["pickup"],
             **{key: float(array[index]) for key, array in values.items()}}
            for index, note in enumerate(notes)]


def paired(before, after):
    if [row["id"] for row in before] != [row["id"] for row in after]:
        raise ValueError("comparison cohorts differ")
    report = {}
    for key in ("mel", "attack", "body", "tail", "envelope_db", "t90_seconds"):
        a, b = [np.array([row[key] for row in rows]) for rows in (before, after)]
        # Resample pitches, keeping both pickup observations of each pitch together.
        grouped = [np.mean((b - a)[np.array([row["pitch"] == pitch for row in before])])
                   for pitch in sorted({row["pitch"] for row in before})]
        rng = np.random.default_rng(SEED)
        ci = np.quantile(np.mean(rng.choice(grouped, (2000, len(grouped))), axis=1), [0.025, 0.975])
        report[key] = {"before": float(a.mean()), "after": float(b.mean()),
                       "reduction_percent": float(100 * (1 - b.mean() / a.mean())),
                       "paired_delta_ci95": ci.tolist()}
    return report


def fit_di(root, manifest, renderer, iterations, prior=None):
    notes, audio = load(root, manifest, "Clean", "train")
    target = pack_features(audio)
    target_envelope = envelope(audio)
    target_t90 = t90(audio)
    defaults = renderer.defaults["electric_guitar"]
    keys = list(DI_BOUNDS)
    initial = defaults | {"neck_position": 0.18, "bridge_position": 0.04}
    if prior is not None:
        initial |= json.loads(prior.read_text(encoding="utf-8"))["parameters"]
    calls = 0

    def objective(vector):
        nonlocal calls
        calls += 1
        parameters = dict(zip(keys, map(float, vector), strict=True))
        candidate = di_audio(renderer, notes, parameters)
        value = float(errors(target, pack_features(candidate))["mel"].mean())
        value += 0.006 * float(abs(target_envelope - envelope(candidate)).mean())
        value += 0.15 * float(abs(target_t90 - t90(candidate)).mean())
        if calls % 25 == 0:
            print(f"DI evaluation {calls}: {value:.6f}", flush=True)
        return value

    result = differential_evolution(objective, list(DI_BOUNDS.values()),
                                    x0=[initial[key] for key in keys], seed=SEED,
                                    maxiter=iterations, popsize=4, polish=False, tol=1e-4)
    return {"parameters": dict(zip(keys, map(float, result.x), strict=True)),
            "objective": float(result.fun), "evaluations": calls, "seed": SEED,
            "bounds": DI_BOUNDS, "training_ids": [note["id"] for note in notes],
            "initial_parameters": {key: initial[key] for key in keys},
            "prior_sha256": hashlib.sha256(prior.read_bytes()).hexdigest() if prior else None,
            "objective_definition": "phase mel L1 + 0.006 * 20-ms envelope L1 in dB + 0.15 * absolute T90 error in seconds"}


def coefficients(kind, hz, q, gain=0, rate=24000):
    omega = 2 * np.pi * hz / rate
    c, alpha = np.cos(omega), np.sin(omega) / (2 * q)
    a = 10**(gain / 40)
    if kind == "peak":
        b = [1 + alpha * a, -2 * c, 1 - alpha * a]
        denominator = [1 + alpha / a, -2 * c, 1 - alpha / a]
    elif kind == "hp":
        b = [(1 + c) / 2, -(1 + c), (1 + c) / 2]
        denominator = [1 + alpha, -2 * c, 1 - alpha]
    else:
        s = 2 * np.sqrt(a) * alpha
        if kind == "low":
            b = [a * (a + 1 - (a - 1) * c + s), 2 * a * (a - 1 - (a + 1) * c), a * (a + 1 - (a - 1) * c - s)]
            denominator = [a + 1 + (a - 1) * c + s, -2 * (a - 1 + (a + 1) * c), a + 1 + (a - 1) * c - s]
        elif kind == "high":
            b = [a * (a + 1 + (a - 1) * c + s), -2 * a * (a - 1 + (a + 1) * c), a * (a + 1 + (a - 1) * c - s)]
            denominator = [a + 1 - (a - 1) * c + s, 2 * (a - 1 - (a + 1) * c), a + 1 - (a - 1) * c - s]
        else:
            raise ValueError("unknown filter")
    return (np.array(b + denominator) / denominator[0]).astype(np.float32).astype(float)


def amp_audio(audio, params, rate=24000):
    """Causal surrogate of the Rust amp with cabinet bypass; checked on actual 48-kHz output."""
    tone = np.array([coefficients("low", 150, 0.707, params["bass_db"], rate),
                     coefficients("peak", 750, 0.8, params["mid_db"], rate),
                     coefficients("high", 3000, 0.707, params["treble_db"], rate)])
    filtered = sosfilt(tone, audio, axis=-1)
    x = np.arange(257) - 128
    taps = (np.sinc(2 * 0.45 / 8 * x) * (2 * 0.45 / 8) * np.blackman(257)).astype(np.float32)
    taps /= taps.sum()
    up = upfirdn(taps * 8, filtered, up=8, axis=-1)[..., :audio.shape[-1] * 8]
    shaped = np.tanh(up * 10**(params["drive_db"] / 20))
    shaped = sosfilt(coefficients("hp", 25, 0.707, rate=rate * 8)[None, :], shaped, axis=-1)
    return upfirdn(taps, shaped, down=8, axis=-1)[..., :audio.shape[-1]]


def fit_amp(root, manifest, iterations):
    notes, clean = load(root, manifest, "Clean", "train")
    wet_notes, wet = load(root, manifest, "BluesDriver", "train")
    if [(note["pickup"], note["pitch"]) for note in notes] != [(note["pickup"], note["pitch"]) for note in wet_notes]:
        raise ValueError("dry/wet pairing differs")
    clean = normalize(clean)
    target = pack_features(wet)
    bounds = {"drive_db": (0, 48), "bass_db": (-12, 12), "mid_db": (-12, 12), "treble_db": (-12, 12)}
    keys = list(bounds)
    calls = 0

    def objective(vector):
        nonlocal calls
        calls += 1
        params = dict(zip(keys, map(float, vector), strict=True))
        value = float(errors(target, pack_features(amp_audio(clean, params)))["mel"].mean())
        if calls % 20 == 0:
            print(f"amp evaluation {calls}: {value:.6f}", flush=True)
        return value

    result = differential_evolution(objective, list(bounds.values()), seed=SEED,
                                    x0=[12, 0, 0, 0], maxiter=iterations, popsize=4,
                                    polish=False, tol=1e-4)
    return {"parameters": dict(zip(keys, map(float, result.x), strict=True)),
            "objective": float(result.fun), "evaluations": calls, "bounds": bounds,
            "training_ids": [note["id"] for note in wet_notes], "seed": SEED}


def fit_simple(root, manifest):
    notes, clean = load(root, manifest, "Clean", "train")
    _, wet = load(root, manifest, "BluesDriver", "train")
    clean = normalize(clean)
    target = pack_features(wet)
    def objective(drive):
        return float(errors(target, pack_features(np.tanh(clean * 10**(drive / 20))))["mel"].mean())
    result = minimize_scalar(objective, bounds=(0, 48), method="bounded", options={"xatol": 0.01})
    return {"parameters": {"drive_db": float(result.x)}, "objective": float(result.fun),
            "training_ids": [note["id"] for note in notes], "evaluations": result.nfev,
            "bounds": {"drive_db": [0, 48]}}


def input_files(root, notes, destination):
    destination.mkdir(parents=True, exist_ok=True)
    paths, arrays = [], []
    for note in notes:
        source = root / note["original"]
        if hashlib.sha256(source.read_bytes()).hexdigest() != note["original_sha256"]:
            raise ValueError("original recording hash changed")
        audio, rate = sf.read(source)
        if rate != 48000:
            raise ValueError("unexpected DI rate")
        offset = max(0, note["onset_sample_48000"] - 48)
        audio = audio[offset:offset + 144000]
        audio = np.pad(audio, (0, max(0, 144000 - len(audio))))
        audio = normalize(audio)[0]
        path = destination / (note["id"] + ".f32")
        audio.astype("<f4").tofile(path)
        paths.append(path.resolve())
        arrays.append(audio)
    return paths, np.array(arrays)


def evaluate(root, manifest, renderer, baseline, di, amp, output):
    report = {"source": manifest["source"], "source_license": manifest["license"],
              "reference_sha256": hashlib.sha256((root / "notes.json").read_bytes()).hexdigest(),
              "renderer_sha256": hashlib.sha256(Path(renderer.process.args[0]).read_bytes()).hexdigest(),
              "baseline_renderer_sha256": hashlib.sha256(Path(baseline.process.args[0]).read_bytes()).hexdigest(),
              "fit_parameters": {"di": di["parameters"], "amp": amp["parameters"]},
              "factory_di_parameters": renderer.defaults["electric_guitar"],
              "versions": {"python": platform.python_version(),
                           **{name: version(name) for name in ("numpy", "scipy", "soundfile")}},
              "evaluation_sources_sha256": {
                  name: hashlib.sha256(Path(__file__).with_name(name).read_bytes()).hexdigest()
                  for name in ("electric_copy.py", "physical_pack.py", "physical_mel.py",
                               "prepare_physical_reference.py", "fit_physical_mel.py")},
              "metrics": metric_configuration(), "di": {}, "pedal": {}}
    simple = json.loads((output.parent / "simple-fit.json").read_text())
    report["fit_parameters"]["simple_distortion"] = simple["parameters"]
    for split in ("train", "validation"):
        notes, clean = load(root, manifest, "Clean", split)
        old = di_audio(baseline, notes, {"pickup": 1}, rate=48000, model="guitar")
        acoustic = di_audio(baseline, notes, {}, rate=48000, model="guitar")
        current = di_audio(renderer, notes, di["parameters"], rate=48000)
        old24, acoustic24, current24 = [resample_poly(audio, 1, 2, axis=-1) for audio in (old, acoustic, current)]
        before, after = measurements(notes, clean, old24), measurements(notes, clean, current24)
        report["di"][split] = {"count": len(notes), "old_pickup": before, "electric": after,
                                "paired": paired(before, after),
                                "acoustic": measurements(notes, clean, acoustic24)}
        paths, input48 = input_files(root, notes, output.parent / "inputs")
        _, wet = load(root, manifest, "BluesDriver", split)
        baseline_fx = [{"id": "distortion", "params": simple["parameters"] | {"mode": 0, "mix": 1, "output_db": 0}}]
        amp_fx = [{"id": "amp", "params": amp["parameters"] | {"cabinet": 0, "output_db": 0}}]
        direct = renderer.render("electric_guitar", notes, {}, 3, 3, rate=48000, effects=baseline_fx, inputs=paths)
        processed = renderer.render("electric_guitar", notes, {}, 3, 3, rate=48000, effects=amp_fx, inputs=paths)
        surrogate = amp_audio(input48, amp["parameters"], rate=48000)
        a, b = [resample_poly(audio, 1, 2, axis=-1) for audio in (direct, processed)]
        before, after = measurements(notes, wet, a), measurements(notes, wet, b)
        report["pedal"][split] = {"count": len(notes), "simple_distortion": before, "amp": after,
                                   "paired": paired(before, after),
                                   "surrogate_max_error": float(abs(processed - surrogate).max()),
                                   "surrogate_relative_rms_error": float(np.sqrt(np.mean((processed - surrogate)**2) / np.mean(processed**2)))}
        print(split, json.dumps({"di": report["di"][split]["paired"]["mel"],
                                 "pedal": report["pedal"][split]["paired"]["mel"]}), flush=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("--renderer", type=Path, default=Path("target/release/examples/physical_fit_render.exe"))
    parser.add_argument("--baseline", type=Path, default=Path("target/electric-guitar/before-renderer.exe"))
    parser.add_argument("--stage", choices=("di", "amp", "simple", "evaluate"), required=True)
    parser.add_argument("--iterations", type=int, default=8)
    parser.add_argument("--prior", type=Path, help="explicit training-only DI fit used to initialize the search")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    manifest = json.loads((args.root / "notes.json").read_text(encoding="utf-8"))
    if args.stage == "amp":
        result = fit_amp(args.root, manifest, args.iterations)
    elif args.stage == "simple":
        result = fit_simple(args.root, manifest)
    else:
        renderer = Renderer(args.renderer)
        try:
            if args.stage == "di":
                result = fit_di(args.root, manifest, renderer, args.iterations, args.prior)
            else:
                baseline = Renderer(args.baseline)
                try:
                    evaluate(args.root, manifest, renderer, baseline,
                             json.loads((args.out.parent / "di-fit.json").read_text()),
                             json.loads((args.out.parent / "amp-fit.json").read_text()), args.out)
                finally:
                    baseline.close()
                return
        finally:
            renderer.close()
    args.out.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result["parameters"], indent=2), flush=True)


if __name__ == "__main__":
    main()
