# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy==2.5.3", "scipy==1.18.1", "soundfile==0.14.0"]
# ///
"""Freeze onset-aligned isolated drum hits with disjoint capture splits."""

import argparse
import hashlib
import json
import re
import tarfile
from math import gcd
from pathlib import Path

import numpy as np
import soundfile as sf
from prepare_physical_reference import RATE, manifest_digest, write_note
from scipy.signal import resample_poly

SECONDS = 3.0
DRUMS = {"kick": ("kick", 36), "snare": ("snare", 38),
         "hihatClosed": ("closed_hat", 42), "hihatOpen": ("open_hat", 46),
         "crash1": ("crash", 49), "ride1": ("ride", 51),
         "hiTom": ("tom", 50), "loTom": ("tom", 41)}
LAYERS = {"PP": 0.2, "P": 0.35, "MP": 0.5, "MF": 0.65, "F": 0.8, "FF": 0.95}
DRUM_PATTERN = re.compile(r"^(kick|snare|hihatClosed|hihatOpen|crash1|ride1|hiTom|loTom)_OH_(PP|P|MP|MF|F|FF)_(\d+)\.wav$")


def drum_selection(name):
    """Freeze the first eight hits per original family/layer, without listening."""
    match = DRUM_PATTERN.fullmatch(Path(name).name)
    if match is None or not 1 <= int(match[3]) <= 8:
        return None
    family, pitch = DRUMS[match[1]]
    return family, pitch, LAYERS[match[2]], int(match[3])


def drum_onset(audio):
    """First 2% local RMS rise at 1 ms resolution; preserve leading attack."""
    hop = RATE // 1000
    frames = audio[:len(audio) // hop * hop].reshape(-1, hop)
    energy = np.sqrt(np.mean(frames**2, axis=1))
    if energy.max() < 1e-6:
        raise ValueError("silent drum capture")
    return max(0, int(np.flatnonzero(energy > energy.max() * 0.02)[0]) * hop - hop)


def read_audio(path):
    audio, rate = sf.read(path, always_2d=True)
    audio = audio.mean(axis=1)
    divisor = gcd(rate, RATE)
    return resample_poly(audio, RATE // divisor, rate // divisor)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--lock", type=Path)
    args = parser.parse_args()
    root = args.directory
    source = json.loads((root / "captures.json").read_text(encoding="utf-8"))
    output = root / "notes"
    output.mkdir(exist_ok=True)
    notes = []
    archive_path = root / source["salamander"]["path"]
    if hashlib.sha256(archive_path.read_bytes()).hexdigest() != source["salamander"]["sha256"]:
        raise ValueError("original drum archive hash mismatch")
    originals = root / "drums"
    originals.mkdir(exist_ok=True)
    with tarfile.open(archive_path, "r:bz2") as archive:
        for member in archive:
            selected = drum_selection(member.name)
            if not member.isfile() or selected is None:
                continue
            if member.size > 40_000_000:
                raise ValueError("oversized drum capture")
            path = originals / Path(member.name).name
            with archive.extractfile(member) as stream:
                path.write_bytes(stream.read())
            family, pitch, velocity, repetition = selected
            audio = read_audio(path)
            offset = drum_onset(audio)
            clip = audio[offset:offset + int(SECONDS * RATE)]
            available = len(clip) / RATE
            clip = np.pad(clip, (0, int(SECONDS * RATE) - len(clip)))
            identifier = path.stem
            target = output / f"{identifier}.wav"
            write_note(target, clip)
            notes.append({"id": identifier, "model": "drums", "family": family,
                          "pitch": pitch, "velocity": velocity, "split": "train" if repetition % 2 else "validation",
                          "source": path.name, "path": f"notes/{identifier}.wav", "seconds": SECONDS,
                          "hold": SECONDS, "offset_seconds": offset / RATE, "available_seconds": available,
                          "source_sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                          "sha256": hashlib.sha256(target.read_bytes()).hexdigest()})
    print("drums", len(notes), flush=True)
    manifest = {"rate": RATE, "seconds": SECONDS, "metric_policy": "onset aligned, whole-note RMS, no time warp",
                "split_policy": "even repetitions held out",
                "captures_sha256": manifest_digest(source),
                "notes": sorted(notes, key=lambda note: note["id"])}
    if args.lock and manifest != json.loads(args.lock.read_text(encoding="utf-8")):
        raise ValueError("prepared reference differs from frozen cohort")
    (root / "notes.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print("manifest", manifest_digest(manifest), len(notes), flush=True)


if __name__ == "__main__":
    main()
