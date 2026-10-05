# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12"]
# ///
"""Calibrate the native piano on three-second Steinway notes, check one/six seconds.

Use separate prepare, fit and evaluate invocations. The existing Iowa split and
onset/tuning labels are frozen before fitting; validation never selects controls.
"""

import argparse
import hashlib
import json
import os
import sys
from math import gcd
from pathlib import Path
from urllib.request import urlopen

os.environ.setdefault("OPENBLAS_NUM_THREADS", "1")
os.environ.setdefault("OMP_NUM_THREADS", "1")

import numpy as np
import scipy
import soundfile as sf
from fit_physical_mel import BOUNDS, REPO, Renderer, body_profiles, fit_model, summary
from physical_mel import RATE, configuration, distances, features, normalize
from scipy.signal import resample_poly

LOCKS = REPO / "tools/eval/references"
DURATIONS = (1, 3, 6)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def read_lock(name):
    return json.loads((LOCKS / name).read_text(encoding="utf-8"))


def note_path(root, note, seconds):
    return (
        root
        / "notes"
        / f"piano-{note['pitch']}-{round(note['velocity'] * 100)}-{seconds}s.wav"
    )


def prepare(root):
    """Download hash-locked originals and cut fixed, unwarped mono reference notes."""
    root.mkdir(parents=True, exist_ok=True)
    (root / "notes").mkdir(exist_ok=True)
    notes = [
        note
        for note in read_lock("iowa-notes.json")["notes"]
        if note["model"] == "piano"
    ]
    for capture in read_lock("iowa-captures.json")["captures"]:
        if capture["model"] != "piano":
            continue
        path = root / capture["path"]
        if not path.exists():
            with urlopen(capture["url"], timeout=90) as response:
                raw = response.read(40_000_001)
            if len(raw) > 40_000_000 or raw[:4] != b"FORM":
                raise ValueError(f"invalid AIFF: {path.name}")
            path.write_bytes(raw)
        if digest(path) != capture["sha256"]:
            raise ValueError(f"reference hash mismatch: {path}")
        audio, rate = sf.read(path, always_2d=True)
        divisor = gcd(rate, RATE)
        audio = resample_poly(audio.mean(axis=1), RATE // divisor, rate // divisor)
        for note in (note for note in notes if note["source"] == capture["path"]):
            offset = round(note["offset_seconds"] * RATE)
            for seconds in DURATIONS:
                clip = audio[offset : offset + seconds * RATE]
                if (
                    len(clip) != seconds * RATE
                    or note["stable_end_seconds"] < note["offset_seconds"] + seconds
                ):
                    raise ValueError(f"short stable reference: {path.name}")
                # SoundFile's automatic PEAK timestamp must not enter the note hash.
                with sf.SoundFile(
                    note_path(root, note, seconds),
                    mode="w",
                    samplerate=RATE,
                    channels=1,
                    subtype="FLOAT",
                ) as output:
                    output.write(clip)
    manifest = {
        "source_lock_sha256": digest(LOCKS / "iowa-captures.json"),
        "note_lock_sha256": digest(LOCKS / "iowa-notes.json"),
        "notes": notes,
        "files": {
            note_path(root, note, seconds).relative_to(root).as_posix(): digest(
                note_path(root, note, seconds)
            )
            for note in notes
            for seconds in DURATIONS
        },
    }
    (root / "grand-notes.json").write_text(
        json.dumps(manifest, indent=2) + "\n", encoding="utf-8"
    )


def load_cohort(root, seconds, split=None):
    """Reject changed prepared files before loading references for any stage."""
    manifest = json.loads((root / "grand-notes.json").read_text(encoding="utf-8"))
    if manifest["source_lock_sha256"] != digest(
        LOCKS / "iowa-captures.json"
    ) or manifest["note_lock_sha256"] != digest(LOCKS / "iowa-notes.json"):
        raise ValueError("reference locks changed")
    locked_notes = [
        note
        for note in read_lock("iowa-notes.json")["notes"]
        if note["model"] == "piano"
    ]
    if manifest["notes"] != locked_notes:
        raise ValueError("prepared note labels or splits changed")
    notes = [
        note for note in manifest["notes"] if split is None or note["split"] == split
    ]
    if not notes:
        raise ValueError("empty piano cohort")
    audio = []
    for note in notes:
        path = note_path(root, note, seconds)
        expected = manifest["files"][path.relative_to(root).as_posix()]
        if digest(path) != expected:
            raise ValueError(f"prepared reference changed: {path}")
        samples, rate = sf.read(path)
        if (
            rate != RATE
            or samples.ndim != 1
            or len(samples) != seconds * RATE
            or not np.isfinite(samples).all()
        ):
            raise ValueError(f"invalid reference format: {path}")
        audio.append(samples)
    return notes, np.array(audio)


def fit(root, executable, output, initial_profile=None):
    load_cohort(root, 3, "train")
    prepared = json.loads((root / "grand-notes.json").read_text(encoding="utf-8"))
    notes = prepared["notes"]
    manifest = {
        "seconds": 3.0,
        "hold": 3.0,
        "notes": [
            note
            | {
                "path": note_path(root, note, 3).relative_to(root).as_posix(),
                "sha256": prepared["files"][
                    note_path(root, note, 3).relative_to(root).as_posix()
                ],
            }
            for note in notes
        ],
    }
    renderer = Renderer(executable.resolve())
    try:
        gains = (
            np.array(
                json.loads(initial_profile.read_text(encoding="utf-8"))[
                    "initial_gains_db"
                ]
            )
            if initial_profile
            else body_profiles()["piano"]
        )
        result = fit_model(renderer, root, manifest, "piano", gains, 8, 20261005)
        report = {
            "metric": configuration(),
            "fit_seconds": 3,
            "seed": 20261005,
            "iterations": 8,
            "bounds": BOUNDS["piano"],
            "renderer_sha256": digest(executable),
            "reference_sha256": digest(root / "grand-notes.json"),
            "defaults": renderer.defaults["piano"],
            "initial_gains_db": gains.tolist(),
            "versions": versions(),
            "result": result,
        }
        output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    finally:
        renderer.close()


def versions():
    return {
        "python": sys.version,
        "numpy": np.__version__,
        "scipy": scipy.__version__,
        "soundfile": sf.__version__,
    }


def evaluate(root, before_path, after_path, output):
    before, after = Renderer(before_path.resolve()), Renderer(after_path.resolve())
    report = {
        "metric": configuration(),
        "versions": versions(),
        "before_sha256": digest(before_path),
        "after_sha256": digest(after_path),
        "reference_sha256": digest(root / "grand-notes.json"),
        "before_defaults": before.defaults["piano"],
        "after_defaults": after.defaults["piano"],
        "measurements": [],
    }
    try:
        for rate in (RATE, 48000):
            for seconds in DURATIONS:
                for split in ("train", "validation"):
                    notes, reference = load_cohort(root, seconds, split)
                    target = features(reference)
                    renders = [
                        renderer.render("piano", notes, {}, seconds, seconds, rate=rate)
                        for renderer in (before, after)
                    ]
                    if rate != RATE:
                        renders = [
                            resample_poly(audio, RATE, rate, axis=-1)
                            for audio in renders
                        ]
                    losses = [distances(target, features(audio)) for audio in renders]
                    report["measurements"].append(
                        {
                            "rate": rate,
                            "seconds": seconds,
                            "split": split,
                            "notes": notes,
                            "before": summary(losses[0]),
                            "after": summary(losses[1]),
                            "relative_reduction": float(
                                1 - losses[1].mean() / losses[0].mean()
                            ),
                            "improved_notes": int(np.sum(losses[1] < losses[0])),
                            "median_rms_ratio": float(
                                np.median(
                                    np.sqrt(
                                        np.mean(renders[1] ** 2, axis=1)
                                        / np.mean(renders[0] ** 2, axis=1)
                                    )
                                )
                            ),
                        }
                    )
                    print(
                        f"{rate} Hz / {seconds}s / {split}: {losses[0].mean():.6f} -> {losses[1].mean():.6f}",
                        flush=True,
                    )
                    if rate == 48000 and seconds == 3 and split == "validation":
                        index = next(
                            i
                            for i, note in enumerate(notes)
                            if note["pitch"] == 60 and note["velocity"] == 0.65
                        )
                        clips = [
                            normalize(audio[index])[0]
                            for audio in (reference, *renders)
                        ]
                        gain = min(0.5, 0.9 / max(np.max(abs(clip)) for clip in clips))
                        audition = []
                        for clip in clips:
                            clip *= gain
                            clip[:120] *= np.linspace(0, 1, 120)
                            clip[-120:] *= np.linspace(1, 0, 120)
                            audition.extend((clip, np.zeros(6000)))
                        sf.write(
                            output.with_suffix(".wav"),
                            np.concatenate(audition),
                            RATE,
                            subtype="PCM_24",
                        )
        output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    finally:
        before.close()
        after.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("prepare")
    fitter = commands.add_parser("fit")
    fitter.add_argument("renderer", type=Path)
    fitter.add_argument("output", type=Path)
    fitter.add_argument(
        "--initial-profile",
        type=Path,
        help="previous calibration report with frozen initial_gains_db",
    )
    evaluator = commands.add_parser("evaluate")
    evaluator.add_argument("before", type=Path)
    evaluator.add_argument("after", type=Path)
    evaluator.add_argument("output", type=Path)
    args = parser.parse_args()
    if args.command == "prepare":
        prepare(args.reference)
    elif args.command == "fit":
        fit(args.reference, args.renderer, args.output, args.initial_profile)
    else:
        evaluate(args.reference, args.before, args.after, args.output)


if __name__ == "__main__":
    main()
