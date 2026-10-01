# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12"]
# ///
"""Prepare long Iowa notes and hand-labeled TU-Note glissandi for local analysis."""

import argparse
import hashlib
import json
from math import gcd
from pathlib import Path

import numpy as np
import soundfile as sf
from physical_trajectory import curve, expression_curve
from prepare_physical_reference import RATE, manifest_digest, write_note
from scipy.signal import resample_poly


def read_capture(path, digest):
    if hashlib.sha256(path.read_bytes()).hexdigest() != digest:
        raise ValueError(f"capture hash mismatch: {path}")
    audio, rate = sf.read(path, always_2d=True)
    divisor = gcd(rate, RATE)
    return resample_poly(audio.mean(axis=1), RATE // divisor, rate // divisor)


def save(root, name, audio, note):
    destination = root / f"{name}.wav"
    write_note(destination, audio)
    return note | {"id": name, "path": destination.name,
                   "sha256": hashlib.sha256(destination.read_bytes()).hexdigest()}


def midi(name):
    import re

    match = re.fullmatch(r"([A-G])(#?)(\d)", name)
    if match is None:
        raise ValueError(f"invalid pitch name: {name}")
    return 12 * (int(match[3]) + 1) + {"C": 0, "D": 2, "E": 4, "F": 5,
                                      "G": 7, "A": 9, "B": 11}[match[1]] + bool(match[2])


def prepare(iowa, transitions, root, lock=None):
    root.mkdir(parents=True, exist_ok=True)
    original = json.loads((iowa / "notes.json").read_text(encoding="utf-8"))
    captures = json.loads((iowa / "captures.json").read_text(encoding="utf-8"))
    hashes = {capture["path"]: capture["sha256"] for capture in captures["captures"]}
    audio_cache = {}
    notes = []
    excluded = []
    for source in original["notes"]:
        if source["source"] not in audio_cache:
            audio_cache[source["source"]] = read_capture(iowa / source["source"], hashes[source["source"]])
        audio = audio_cache[source["source"]]
        for seconds in (3.0, 6.0):
            offset = round(source["offset_seconds"] * RATE)
            if source["stable_end_seconds"] - source["offset_seconds"] < seconds + 0.1:
                excluded.append({"source": source["path"], "seconds": seconds, "reason": "less than duration + 100 ms stable audio"})
                continue
            clip = audio[offset:offset + round(seconds * RATE)]
            note = {key: source[key] for key in ("model", "pitch", "velocity", "split", "tuning_cents", "source", "offset_seconds")}
            note |= {"dataset": "iowa", "kind": "sustain", "seconds": seconds, "hold": seconds}
            if source["model"] == "violin":
                note["bends"], note["pitch_bounds_hz"] = curve(clip, source["pitch"], source["pitch"], source["pitch"], seconds)
                note["expression"] = expression_curve(clip, seconds)
            name = f"iowa-{source['model']}-{source['pitch']}-{round(source['velocity'] * 100)}-{int(seconds)}s"
            notes.append(save(root, name, clip, note))
    tu = json.loads((transitions / "captures.json").read_text(encoding="utf-8"))
    records = {record["path"]: record for record in tu["files"]}
    rows = [line.split() for line in (transitions / "list_TwoNote.txt").read_text().splitlines()[1:]]
    for row in rows:
        identifier = int(row[0])
        filename = f"TwoNote_DPA_{identifier:02}.wav"
        if filename not in records:
            continue
        labels_path = transitions / f"TwoNote_DPA_{identifier:02}.txt"
        if hashlib.sha256(labels_path.read_bytes()).hexdigest() != records[labels_path.name]["sha256"]:
            raise ValueError("TU-Note annotations changed")
        labels = np.loadtxt(labels_path)
        if (list(labels[:7, 1]) != [0, 2, 1, 2, 1, 2, 0]
                or np.any(labels[7:, 1] != 0) or np.any(np.diff(labels[:, 0]) <= 0)):
            raise ValueError(f"unexpected transition annotations: {filename}")
        audio = read_capture(transitions / filename, records[filename]["sha256"])
        offset = round(max(0, labels[1, 0] - 0.005) * RATE)
        end = min(len(audio), round((labels[6, 0] + 0.1) * RATE))
        clip = audio[offset:end]
        seconds = len(clip) / RATE
        hold = round(float(labels[5, 0] - offset / RATE), 5)
        pitch, second = midi(row[1]), midi(row[4])
        note = {"model": "violin", "pitch": pitch, "velocity": 0.5 if row[9] == "mp" else 0.95,
                "split": "train" if row[3] in ("G", "D") else "validation",
                "dataset": "tu-note", "kind": "glissando", "seconds": seconds, "hold": hold,
                "source": filename, "offset_seconds": offset / RATE,
                "transition": [round(float(t - offset / RATE), 5) for t in labels[3:5, 0]],
                "direction": row[8], "vibrato": row[-1] == "true", "tuning_cents": 0.0}
        note["bends"], note["pitch_bounds_hz"] = curve(clip, pitch, min(pitch, second), max(pitch, second), hold)
        note["expression"] = expression_curve(clip, hold)
        notes.append(save(root, f"tu-glissando-{identifier:02}", clip, note))
    manifest = {"rate": RATE, "sources": [captures["source"], tu["source"]],
                "iowa_notes_sha256": manifest_digest(original), "tu_captures_sha256": manifest_digest(tu),
                "policy": "Iowa original split; TU G/D strings train, A/E strings held out; no warped or pitch-shifted reference audio",
                "controls": "pitch: 4096-sample Hann, 10 ms hop; expression: 20 ms RMS, 60 ms Gaussian, square-root amplitude, 50 ms controls; these are supplied performance guides",
                "notes": notes, "excluded": excluded}
    if lock:
        from fit_physical_trajectories import compact

        expected = json.loads(lock.read_text(encoding="utf-8"))
        if manifest_digest(compact(manifest)) != manifest_digest(expected):
            raise ValueError("trajectory reference cohort differs from its lock")
    (root / "trajectories.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    for model in ("piano", "guitar", "violin"):
        for dataset in ("iowa", "tu-note"):
            selected = [note for note in notes if note["model"] == model and note["dataset"] == dataset]
            if selected:
                print(model, dataset, len(selected), "train", sum(note["split"] == "train" for note in selected), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("iowa", type=Path)
    parser.add_argument("transitions", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--lock", type=Path)
    args = parser.parse_args()
    prepare(args.iowa, args.transitions, args.output, args.lock)
