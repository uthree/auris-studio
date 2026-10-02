# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy==2.5.3", "scipy==1.18.1", "soundfile==0.14.0", "mido>=1.3"]
# ///
"""Copy-synthesize synchronized real piano/MIDI excerpts with pedal ablations."""

import argparse
import hashlib
import json
import os
import subprocess
from pathlib import Path

os.environ.setdefault("OPENBLAS_NUM_THREADS", "1")
os.environ.setdefault("OMP_NUM_THREADS", "1")

import mido
import numpy as np
import soundfile as sf
from physical_mel import RATE, configuration, features
from physical_trajectory import envelope_error
from scipy.signal import resample_poly

SECONDS = 12


def score(reference, candidate):
    return float(np.mean([abs(real - synth).mean() for real, synth in
                          zip(features(reference), features(candidate), strict=True)]))


def midi_events(path):
    time = 0.0
    events = []
    for message in mido.MidiFile(path):
        time += message.time
        if time >= SECONDS:
            break
        if message.type in ("note_on", "note_off"):
            on = message.type == "note_on" and message.velocity > 0
            events.append({"seconds": time, "kind": "on" if on else "off",
                           "pitch": message.note, "value": message.velocity / 127})
        elif message.type == "control_change" and message.control == 64:
            events.append({"seconds": time, "kind": "pedal", "value": message.value / 127})
    if not any(event["kind"] == "pedal" for event in events):
        raise ValueError("the prespecified excerpt has no pedal annotation")
    return events


def render(worker, events, gain, half):
    request = {"events": events, "seconds": SECONDS, "resonance": gain, "half_pedal": half}
    data = subprocess.check_output([str(worker)], input=(json.dumps(request) + "\n").encode())
    count = int.from_bytes(data[:4], "little")
    if count != SECONDS * 48000 or len(data) != count * 4 + 4:
        raise ValueError("invalid pedal renderer output")
    audio = np.frombuffer(data[4:], dtype="<f4").astype(float)
    if not np.isfinite(audio).all():
        raise ValueError("nonfinite pedal candidate")
    return resample_poly(audio, 1, 2)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    parser.add_argument("worker", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    if args.output.exists():
        parser.error("preserve previous measurements")
    manifest = json.loads((args.reference / "captures.json").read_text(encoding="utf-8"))
    cases = []
    for record in manifest["files"]:
        path = args.reference / record["path"]
        if hashlib.sha256(path.read_bytes()).hexdigest() != record["sha256"]:
            raise ValueError("SMD capture hash changed")
        if path.suffix != ".wav":
            continue
        audio, rate = sf.read(path)
        audio = resample_poly(audio[:SECONDS * rate], RATE, rate)
        cases.append({"id": path.stem, "split": record["split"], "audio": audio,
                      "events": midi_events(path.with_suffix(".mid"))})
    # Select a single bank gain on the prespecified training work, never validation.
    train = next(case for case in cases if case["split"] == "train")
    trials = []
    for gain in (0, 1, 2, 4, 8, 16):
        value = score(train["audio"], render(args.worker.resolve(), train["events"], gain, False))
        trials.append({"gain": gain, "mel": value})
        print("training sympathetic", gain, value, flush=True)
    gain = min(trials, key=lambda row: row["mel"])["gain"]
    report = {"source": manifest["source"], "captures_sha256": hashlib.sha256(
                (args.reference / "captures.json").read_bytes()).hexdigest(),
              "renderer_sha256": hashlib.sha256(args.worker.read_bytes()).hexdigest(),
              "seconds": SECONDS, "production_rate": 48000, "metric": configuration(),
              "loss": "unweighted whole-excerpt mean L1 across both log-mel resolutions",
              "training_trials": trials, "selected_gain": gain, "cases": []}
    for case in cases:
        values = {}
        for name, amount, half in (("before", 0, False), ("sympathetic", gain, False),
                                  ("half_pedal", 0, True), ("combined", gain, True)):
            candidate = render(args.worker.resolve(), case["events"], amount, half)
            values[name] = {"mel": score(case["audio"], candidate),
                            "envelope_db": envelope_error(case["audio"], candidate)}
            print(case["id"], name, values[name], flush=True)
        report["cases"].append({"id": case["id"], "split": case["split"], "events": len(case["events"]),
                    "pedal_events": sum(e["kind"] == "pedal" for e in case["events"]), "variants": values})
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
