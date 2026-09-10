# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy<2", "soundfile>=0.12"]
# ///
"""Prespecified melody-continuity A/B over frozen, previously auditioned backing.

The baseline reuses hash-verified diagnostic/reference scores and full WAVs from the
seed-listening corpus; held-out seeds are composed with that corpus's frozen CLI.
The candidate transplants only new instrumental melody notes into each baseline
project. Both sides use the frozen baseline renderer. No subjective rating file is
read. All prespecified cases survive into the manifest, including regressions.

The primary learned comparison uses the exact listening excerpts: first eight-bar
4/4 chorus, five-millisecond edge fades, linear gain to -23 integrated LUFS, no limiter.
Use aesthetics.py on before/excerpts and after/excerpts. Use clap.py on those same
folders with unchanged prompts and --segments 1 (the center ten-second window).
These measurements describe excerpts, not complete-song quality or hook recall.

    python tools/eval/melody_continuity_ab.py baseline --out target/experiment/before \
        --cli frozen/auris.exe --reference target/seed-diversity/manifest.json \
        --assets target/composition-eval/assets --ffmpeg path/to/ffmpeg.exe
    python tools/eval/melody_continuity_ab.py candidate --before target/experiment/before/manifest.json \
        --out target/experiment/after --cli candidate/auris.exe
"""

from __future__ import annotations

import argparse
import json
import math
import os
import shutil
from pathlib import Path

import numpy as np
import soundfile as sf
from melody_ab import reject_relative_assets, write_comparison
from seed_diversity import chorus_bounds, faded_excerpt, file_hash, loudness, run, save

CASES = (
    ("pop-band", 102, "diagnostic"),
    ("pop-band", 105, "reference"),
    ("rock", 105, "diagnostic"),
    ("rock", 107, "reference"),
    ("city-pop", 102, "diagnostic"),
    *(
        (preset, seed, "held-out")
        for preset in ("rock", "city-pop", "pop-band")
        for seed in (201, 202)
    ),
)
TARGET_LUFS = -23.0
MODEL_CONDITIONS = {
    "input": "The exact normalized first-chorus listening excerpts",
    "audiobox": "Existing evaluator over the complete excerpt",
    "clap": "Existing frozen prompt manifest; --segments 1; centered ten-second window",
    "limitations": "Neither model directly measures groove, memorability, or whole-song quality",
}


def read(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def artifact(path: Path) -> dict:
    return {"path": str(path.resolve()), "sha256": file_hash(path)}


def verified(value: dict) -> Path:
    path = Path(value["path"]).resolve()
    if file_hash(path) != value["sha256"]:
        raise ValueError(f"Artifact changed: {path}")
    return path


def environment(assets: Path) -> dict:
    result = os.environ.copy()
    result.update(
        {
            "AURIS_SOUNDFONTS": str(assets / "SoundFonts"),
            "AURIS_DICTIONARY": str(assets / "Dictionary"),
            "AURIS_JAPANESE_DICTIONARY": str(assets / "Dictionary/naist-jdic"),
            "AURIS_FETCH_SOUNDFONTS": "0",
        }
    )
    return result


def create_output(out: Path) -> Path:
    out = out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    for name in ("projects", "audio", "raw-excerpts", "excerpts", "logs", "candidates"):
        (out / name).mkdir()
    return out


def compose(
    cli: Path, out: Path, preset: str, seed: int, env: dict, folder: str = "projects"
) -> Path:
    label = f"{preset}-s{seed}"
    requested = out / folder / f"{label}.auris"
    command = [
        str(cli),
        "compose",
        "--preset",
        preset,
        "--seed",
        str(seed),
        "-o",
        str(requested),
    ]
    run(command, out / "logs" / f"{label}-compose.log", env)
    return requested.parent / label / requested.name


def render(cli: Path, project: Path, wav: Path, log: Path, env: dict) -> None:
    run(
        [
            str(cli),
            "render",
            str(project),
            "--bit-depth",
            "32",
            "--no-tail",
            "-o",
            str(wav),
        ],
        log,
        env,
    )


def normalize(audio: np.ndarray, measured: dict) -> tuple[np.ndarray, float]:
    """Use linear gain only; reject a required gain that cannot retain true-peak headroom."""
    lufs, peak = measured["lufs"], measured["true_peak_dbfs"]
    if not all(math.isfinite(value) for value in (lufs, peak)):
        raise ValueError("Nonfinite input loudness")
    gain = TARGET_LUFS - lufs
    if peak + gain > -1.0:
        raise ValueError(
            "Exact -23 LUFS requires excess true peak; do not silently limit or retarget"
        )
    result = audio * np.float32(10 ** (gain / 20))
    if not np.isfinite(result).all() or np.max(np.abs(result)) >= 1:
        raise ValueError("Invalid normalized excerpt")
    return result, gain


def measure_audio(
    out: Path, label: str, project_path: Path, wav: Path, ffmpeg: Path
) -> dict:
    project = read(project_path)
    start, end, bpm = chorus_bounds(project)
    audio, rate = sf.read(wav, dtype="float32", always_2d=True)
    if (
        rate != 48000
        or audio.shape[1] != 2
        or not len(audio)
        or not np.isfinite(audio).all()
    ):
        raise ValueError(f"Invalid final WAV: {label}")
    peak = float(np.max(np.abs(audio)))
    rms = float(np.sqrt(np.mean(audio.astype(np.float64) ** 2)))
    if peak >= 1 or rms <= 1e-7:
        raise ValueError(f"Clipping or silence: {label}")
    excerpt, first, last = faded_excerpt(audio, rate, start, end, bpm)
    raw = out / "raw-excerpts" / f"{label}.wav"
    sf.write(raw, excerpt, rate, subtype="FLOAT")
    measured = loudness(ffmpeg, raw, out / "logs" / f"{label}-loudness-before.log")
    normalized, gain = normalize(excerpt, measured)
    path = out / "excerpts" / f"{label}.wav"
    sf.write(path, normalized, rate, subtype="FLOAT")
    checked = loudness(ffmpeg, path, out / "logs" / f"{label}-loudness-after.log")
    if abs(checked["lufs"] - TARGET_LUFS) > 0.15 or checked["true_peak_dbfs"] > -1:
        raise ValueError(f"Excerpt loudness check failed: {label}")
    return {
        "project": artifact(project_path),
        "wav": {
            **artifact(wav),
            "frames": len(audio),
            "sample_rate": rate,
            "channels": 2,
            "peak": peak,
            "rms": rms,
        },
        "raw_excerpt": {**artifact(raw), **measured},
        "excerpt": {
            **artifact(path),
            "duration_seconds": len(excerpt) / rate,
            "start_tick": start,
            "end_tick": end,
            "start_frame": first,
            "end_frame": last,
            "bpm": bpm,
            "normalization": {
                "method": "Five-millisecond edge fades, then linear gain; no limiter",
                "input_lufs": measured["lufs"],
                "input_true_peak_dbfs": measured["true_peak_dbfs"],
                "target_lufs": TARGET_LUFS,
                "gain_db": gain,
                "measured_output_lufs": checked["lufs"],
                "output_true_peak_dbfs": checked["true_peak_dbfs"],
            },
        },
    }


def prepare_baseline(
    out: Path, cli: Path, reference_path: Path, assets: Path, ffmpeg: Path, cases=CASES
) -> Path:
    """Freeze all declared cases, preserving the exact previously auditioned reference files."""
    cli, reference_path, assets, ffmpeg = (
        path.resolve() for path in (cli, reference_path, assets, ffmpeg)
    )
    reference = read(reference_path)
    if file_hash(cli) != reference["cli"]["sha256"]:
        raise ValueError(
            "Baseline CLI does not match the auditioned reference renderer"
        )
    if (
        file_hash(assets / "SoundFonts/MuseScore_General.sf2")
        != reference["soundfont"]["sha256"]
    ):
        raise ValueError("SoundFont differs from the auditioned reference")
    out = create_output(out)
    env = environment(assets)
    manifest = {
        "schema_version": 1,
        "stage": "before",
        "complete": False,
        "presets": list(dict.fromkeys(preset for preset, _, _ in cases)),
        "seeds": list(dict.fromkeys(seed for _, seed, _ in cases)),
        "cases": [
            {"preset": preset, "seed": seed, "cohort": cohort}
            for preset, seed, cohort in cases
        ],
        "composer_revision": reference["composer_revision"],
        "cli": artifact(cli),
        "reference_manifest": artifact(reference_path),
        "assets": str(assets),
        "soundfont": artifact(assets / "SoundFonts/MuseScore_General.sf2"),
        "ffmpeg": artifact(ffmpeg),
        "model_conditions": MODEL_CONDITIONS,
        "design": "Prespecified diagnostic/reference cases plus held-out seeds; all retained. First chorus at -23 LUFS is the primary listening/model input.",
        "files": {},
    }
    save(out / "manifest.json", manifest)
    for preset, seed, cohort in cases:
        label = f"{preset}-s{seed}"
        print(f"Baseline {label} ({cohort})", flush=True)
        wav = out / "audio" / f"{label}.wav"
        if cohort != "held-out":
            original = reference["files"][label]
            source_project, source_wav = (
                verified(original["project"]),
                verified(original["wav"]),
            )
            reject_relative_assets(read(source_project))
            project = out / "projects" / label / f"{label}.auris"
            project.parent.mkdir()
            shutil.copyfile(source_project, project)
            shutil.copyfile(source_wav, wav)
            origin = {
                "method": "Exact copy of hash-verified auditioned project and WAV",
                "project": original["project"],
                "wav": original["wav"],
            }
        else:
            project = compose(cli, out, preset, seed, env)
            render(cli, project, wav, out / "logs" / f"{label}-render.log", env)
            origin = {"method": "Composed and rendered with the frozen baseline CLI"}
        manifest["files"][label] = {
            "preset": preset,
            "seed": seed,
            "cohort": cohort,
            "origin": origin,
            **measure_audio(out, label, project, wav, ffmpeg),
        }
        save(out / "manifest.json", manifest)
    verified(manifest["cli"])
    verified(manifest["soundfont"])
    manifest["complete"] = True
    save(out / "manifest.json", manifest)
    return out / "manifest.json"


def validate_baseline(before: dict, expected_cases=CASES) -> None:
    """Reject partial cohorts and changed reference assets before creating any candidate."""
    if (
        before.get("schema_version") != 1
        or before.get("stage") != "before"
        or not before.get("complete")
    ):
        raise ValueError("Candidate requires a complete baseline manifest")
    cases = before["cases"]
    observed = [(case["preset"], case["seed"], case["cohort"]) for case in cases]
    if observed != list(expected_cases):
        raise ValueError("Baseline cases differ from the prespecified cohort")
    labels = [f"{preset}-s{seed}" for preset, seed, _ in observed]
    if (
        not labels
        or len(set(labels)) != len(labels)
        or set(labels) != set(before["files"])
    ):
        raise ValueError(
            "Baseline cases and files must match exactly, without duplicates"
        )
    for case, label in zip(cases, labels):
        row = before["files"][label]
        if any(row.get(key) != value for key, value in case.items()):
            raise ValueError(f"Baseline case metadata differs: {label}")
        for name in ("project", "wav", "raw_excerpt", "excerpt"):
            verified(row[name])
        full = sf.info(row["wav"]["path"])
        if (full.frames, full.samplerate, full.channels) != (
            row["wav"]["frames"],
            row["wav"]["sample_rate"],
            row["wav"]["channels"],
        ):
            raise ValueError(f"Baseline WAV metadata differs: {label}")
        excerpt = sf.info(row["excerpt"]["path"])
        if (
            excerpt.samplerate != full.samplerate
            or excerpt.channels != full.channels
            or abs(
                excerpt.frames / excerpt.samplerate - row["excerpt"]["duration_seconds"]
            )
            > 1 / excerpt.samplerate
        ):
            raise ValueError(f"Baseline excerpt metadata differs: {label}")


def prepare_candidate(
    out: Path, cli: Path, before_path: Path, expected_cases=CASES
) -> Path:
    """Compose new melodies, but retain every baseline backing and the baseline renderer."""
    cli, before_path = cli.resolve(), before_path.resolve()
    before = read(before_path)
    validate_baseline(before, expected_cases)
    renderer, ffmpeg = verified(before["cli"]), verified(before["ffmpeg"])
    verified(before["soundfont"])
    assets = Path(before["assets"])
    env = environment(assets)
    out = create_output(out)
    manifest = {
        key: before[key]
        for key in (
            "schema_version",
            "presets",
            "seeds",
            "cases",
            "assets",
            "soundfont",
            "ffmpeg",
            "model_conditions",
        )
    }
    manifest.update(
        {
            "stage": "after",
            "complete": False,
            "cli": artifact(renderer),
            "candidate_cli": artifact(cli),
            "baseline_manifest": artifact(before_path),
            "design": "Only new instrumental melody notes/digest transplanted into each baseline project; same frozen baseline renderer; all cases retained.",
            "files": {},
        }
    )
    save(out / "manifest.json", manifest)
    for case in before["cases"]:
        preset, seed = case["preset"], case["seed"]
        label = f"{preset}-s{seed}"
        print(f"Candidate {label} ({case['cohort']})", flush=True)
        old = before["files"][label]
        source = verified(old["project"])
        candidate = compose(cli, out, preset, seed, env, "candidates")
        project = out / "projects" / label / f"{label}.auris"
        transplant = write_comparison(source, candidate, project)
        proof = read(transplant)
        if proof["preserved_source_sha256"] != proof["preserved_output_sha256"]:
            raise ValueError(f"Non-melody content changed: {label}")
        wav = out / "audio" / f"{label}.wav"
        render(renderer, project, wav, out / "logs" / f"{label}-render.log", env)
        measured = measure_audio(out, label, project, wav, ffmpeg)
        for field in ("frames", "sample_rate", "channels"):
            if old["wav"][field] != measured["wav"][field]:
                raise ValueError(f"Paired audio {field} changed: {label}")
        manifest["files"][label] = {
            **case,
            **measured,
            "candidate_project": artifact(candidate),
            "transplant_manifest": artifact(transplant),
            "preserved_content_sha256": proof["preserved_source_sha256"],
            "backing_notes_sha256": proof["backing_notes_sha256"],
            "source_melody_sha256": proof["source_melody_sha256"],
            "output_melody_sha256": proof["output_melody_sha256"],
        }
        save(out / "manifest.json", manifest)
    verified(manifest["cli"])
    verified(manifest["candidate_cli"])
    verified(manifest["soundfont"])
    verified(manifest["baseline_manifest"])
    manifest["complete"] = True
    save(out / "manifest.json", manifest)
    return out / "manifest.json"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="stage", required=True)
    baseline = subparsers.add_parser("baseline")
    candidate = subparsers.add_parser("candidate")
    for command in (baseline, candidate):
        command.add_argument("--out", required=True, type=Path)
        command.add_argument("--cli", required=True, type=Path)
    baseline.add_argument("--reference", required=True, type=Path)
    baseline.add_argument("--assets", required=True, type=Path)
    baseline.add_argument("--ffmpeg", required=True, type=Path)
    candidate.add_argument("--before", required=True, type=Path)
    args = parser.parse_args()
    if args.stage == "baseline":
        print(
            prepare_baseline(
                args.out, args.cli, args.reference, args.assets, args.ffmpeg
            )
        )
    else:
        print(prepare_candidate(args.out, args.cli, args.before))


if __name__ == "__main__":
    main()
