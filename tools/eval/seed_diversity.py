# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy<2", "soundfile>=0.12"]
# ///
"""Render a frozen composer over several seeds and prepare controlled listening excerpts.

The complete arrangements vary with the seed. Full WAVs remain unchanged for model scoring;
first-chorus excerpts use linear gain to a common integrated LUFS target within each preset.
FFmpeg supplies loudness measurements, not generated audio. No subjective ratings are invented.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import subprocess
from pathlib import Path

import numpy as np
import soundfile as sf


def file_hash(path: Path) -> str:
    """Hash a local artifact without loading a complete sound bank into memory."""
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def save(path: Path, value: dict) -> None:
    path.write_text(json.dumps(value, indent=2, allow_nan=False) + "\n", encoding="utf8")


def run(command: list[str], log: Path, env: dict | None = None) -> None:
    with log.open("w", encoding="utf8") as stream:
        subprocess.run(
            command, stdout=stream, stderr=subprocess.STDOUT, check=True,
            env=env, timeout=240,
        )


def loudness(ffmpeg: Path, path: Path, log: Path) -> dict:
    """Read input loudness from FFmpeg's meter; discard its processed output."""
    command = [
        str(ffmpeg), "-hide_banner", "-nostdin", "-i", str(path), "-af",
        "loudnorm=I=-23:TP=-1:LRA=50:print_format=json", "-f", "null", "-",
    ]
    done = subprocess.run(command, capture_output=True, text=True, check=True, timeout=90)
    log.write_text(done.stderr, encoding="utf8")
    matches = re.findall(r'\{\s*"input_i"[^}]+\}', done.stderr)
    if len(matches) != 1:
        raise ValueError(f"Expected one loudness report for {path}")
    raw = json.loads(matches[0])
    result = {"lufs": float(raw["input_i"]), "true_peak_dbfs": float(raw["input_tp"])}
    if not all(math.isfinite(value) for value in result.values()):
        raise ValueError(f"Nonfinite loudness for {path}")
    return result


def chorus_bounds(project: dict) -> tuple[int, int, float]:
    tempi = project["tempo_map"]["points"]
    if len(tempi) != 1 or tempi[0]["tick"] != 0 or tempi[0]["bpm"] <= 0:
        raise ValueError("This controlled corpus requires one constant tempo")
    sections = project["sections"]["points"]
    matches = [i for i, point in enumerate(sections[:-1]) if point["label"] == "chorus"]
    if len(matches) != 1:
        raise ValueError("Expected one first chorus")
    index = matches[0]
    start, end = sections[index]["tick"], sections[index + 1]["tick"]
    if end - start != 8 * 4 * 960:
        raise ValueError("This controlled corpus requires an eight-bar 4/4 chorus")
    return start, end, tempi[0]["bpm"]


def faded_excerpt(audio: np.ndarray, rate: int, start: int, end: int, bpm: float):
    first, last = (round(tick / 960 * 60 / bpm * rate) for tick in (start, end))
    if not 0 <= first < last <= len(audio):
        raise ValueError("Chorus bounds exceed rendered audio")
    excerpt = audio[first:last].copy()
    fade = min(round(rate * 0.005), len(excerpt) // 2)
    ramp = np.linspace(0, 1, fade, dtype=np.float32)[:, None]
    excerpt[:fade] *= ramp
    excerpt[-fade:] *= ramp[::-1]
    return excerpt, first, last


def corpus(args) -> None:
    root = Path(__file__).resolve().parents[2]
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    for folder in ("projects", "audio", "raw-excerpts", "excerpts", "logs"):
        (out / folder).mkdir()
    cli, ffmpeg, assets = args.cli.resolve(), args.ffmpeg.resolve(), args.assets.resolve()
    font = assets / "SoundFonts/MuseScore_General.sf2"
    env = os.environ.copy()
    env.update({
        "AURIS_SOUNDFONTS": str(assets / "SoundFonts"),
        "AURIS_DICTIONARY": str(assets / "Dictionary"),
        "AURIS_JAPANESE_DICTIONARY": str(assets / "Dictionary/naist-jdic"),
        "AURIS_FETCH_SOUNDFONTS": "0",
    })
    manifest = {
        "schema_version": 1, "presets": args.presets, "seeds": args.seeds,
        "composer_revision": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=root, text=True,
        ).strip(),
        "cli": {"path": str(cli), "sha256": file_hash(cli)},
        "ffmpeg": {"path": str(ffmpeg), "sha256": file_hash(ffmpeg)},
        "soundfont": {"path": str(font), "sha256": file_hash(font)},
        "writer_sha256": {str(path.relative_to(root)).replace("\\", "/"): file_hash(path)
                          for path in (root / "crates/auris-compose/src/parts/melody.rs",
                                       root / "crates/auris-compose/src/parts/melody_rhythm.rs")},
        "design": "Presets and composer fixed; whole arrangements vary with seed. Models use full unmodified renders. Listening uses first-chorus excerpts with linear loudness matching per preset.",
        "files": {},
    }
    for preset in args.presets:
        reference_context = None
        for seed in args.seeds:
            label = f"{preset}-s{seed}"
            print(f"Composing and rendering {label}", flush=True)
            requested = out / "projects" / f"{label}.auris"
            compose_command = [str(cli), "compose", "--preset", preset, "--seed", str(seed), "-o", str(requested)]
            run(compose_command, out / "logs" / f"{label}-compose.log", env)
            project_path = requested.parent / label / requested.name
            project = json.loads(project_path.read_text(encoding="utf8"))
            start, end, bpm = chorus_bounds(project)
            context = {key: project[key] for key in ("tempo_map", "signatures", "harmony", "sections")}
            if reference_context is None:
                reference_context = context
            elif reference_context != context:
                raise ValueError(f"Seed changed controlled musical context: {label}")
            wav = out / "audio" / f"{label}.wav"
            render_command = [str(cli), "render", str(project_path), "--bit-depth", "32", "--no-tail", "-o", str(wav)]
            run(render_command, out / "logs" / f"{label}-render.log", env)
            audio, rate = sf.read(wav, dtype="float32", always_2d=True)
            if rate != 48000 or audio.shape[1] != 2 or not np.isfinite(audio).all():
                raise ValueError(f"Invalid final audio: {label}")
            peak, rms = float(np.max(np.abs(audio))), float(np.sqrt(np.mean(audio.astype(np.float64) ** 2)))
            if peak > 1 or rms <= 1e-7:
                raise ValueError(f"Clipping or silence: {label}")
            excerpt, first, last = faded_excerpt(audio, rate, start, end, bpm)
            raw_excerpt = out / "raw-excerpts" / f"{label}.wav"
            sf.write(raw_excerpt, excerpt, rate, subtype="FLOAT")
            measured = loudness(ffmpeg, raw_excerpt, out / "logs" / f"{label}-loudness-before.log")
            manifest["files"][label] = {
                "preset": preset, "seed": seed,
                "project": {"path": str(project_path), "sha256": file_hash(project_path)},
                "wav": {"path": str(wav), "sha256": file_hash(wav), "frames": len(audio), "sample_rate": rate, "channels": 2, "peak": peak, "rms": rms},
                "raw_excerpt": {"path": str(raw_excerpt), "sha256": file_hash(raw_excerpt), **measured},
                "excerpt": {"path": str(out / "excerpts" / f"{label}.wav"), "duration_seconds": len(excerpt) / rate, "start_tick": start, "end_tick": end, "start_frame": first, "end_frame": last, "bpm": bpm},
                "commands": {"compose": compose_command, "render": render_command},
            }
            save(out / "manifest.json", manifest)
        rows = [row for row in manifest["files"].values() if row["preset"] == preset]
        # Choose one target for the whole group, lowering it if a peak needs more headroom.
        target = min([-23.0, *(row["raw_excerpt"]["lufs"] - 1.2 - row["raw_excerpt"]["true_peak_dbfs"] for row in rows)])
        for row in rows:
            raw = row["raw_excerpt"]
            gain = target - raw["lufs"]
            audio, rate = sf.read(raw["path"], dtype="float32", always_2d=True)
            audio *= 10 ** (gain / 20)
            path = Path(row["excerpt"]["path"])
            sf.write(path, audio, rate, subtype="FLOAT")
            checked = loudness(ffmpeg, path, out / "logs" / f"{path.stem}-loudness-after.log")
            if abs(checked["lufs"] - target) > 0.15 or checked["true_peak_dbfs"] > -1:
                raise ValueError(f"Loudness matching failed for {path}")
            row["excerpt"]["sha256"] = file_hash(path)
            row["excerpt"]["normalization"] = {
                "method": "Five-millisecond edge fades, then linear gain to common integrated LUFS target per genre; no limiter",
                "input_lufs": raw["lufs"], "input_true_peak_dbfs": raw["true_peak_dbfs"],
                "target_lufs": target, "gain_db": gain,
                "measured_output_lufs": checked["lufs"], "output_true_peak_dbfs": checked["true_peak_dbfs"],
            }
        save(out / "manifest.json", manifest)
    if file_hash(cli) != manifest["cli"]["sha256"]:
        raise ValueError("Renderer changed during experiment")
    print(f"Wrote {out / 'manifest.json'}", flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--cli", required=True, type=Path)
    parser.add_argument("--ffmpeg", required=True, type=Path)
    parser.add_argument("--assets", required=True, type=Path)
    parser.add_argument("--presets", nargs="+", default=["rock", "city-pop", "pop-band"])
    parser.add_argument("--seeds", nargs="+", type=int, default=list(range(101, 109)))
    args = parser.parse_args()
    if len(args.seeds) != len(set(args.seeds)) or len(args.presets) != len(set(args.presets)):
        parser.error("Presets and seeds must be unique")
    if any(seed < 0 or seed > 2**64 - 1 for seed in args.seeds):
        parser.error("Seeds must be unsigned 64-bit values")
    if any(not re.fullmatch(r"[a-z][a-z0-9-]*", preset) for preset in args.presets):
        parser.error("Presets must be plain preset identifiers")
    corpus(args)


if __name__ == "__main__":
    main()
