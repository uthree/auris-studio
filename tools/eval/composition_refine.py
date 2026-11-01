# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy>=2", "soundfile>=0.13"]
# ///
"""Compose, evaluate, and optionally make one local revision without a score winner.

Use a pinned panel-test executable, renderer and SongEval installation. Every stage is
retained, including rejected edits. A revision changes one foreground clip;
lyrics, accompaniment, instrument state, gain and form remain protected.
"""

from __future__ import annotations

import argparse
import copy
import html
import json
import math
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

import composition_quality as quality
import numpy as np
import soundfile as sf

ROOT = Path(__file__).resolve().parents[2]
FIELDS = {"pitch", "start", "length"}


def write_json(path: Path, value) -> None:
    """Write the exact UTF-8 bytes which will subsequently be hashed."""
    payload = (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_bytes(payload)
    temporary.replace(path)


def load(path: Path):
    return json.loads(path.read_bytes())


def tree_hashes(root: Path) -> dict[str, str]:
    return {
        p.relative_to(root).as_posix(): quality.digest(p)
        for p in sorted(root.rglob("*"))
        if p.is_file()
    }


def recipe(path: Path) -> dict:
    """Read a musical brief and edit scope; the agent chooses any repair after evaluation."""
    value = load(path)
    if value.get("schema_version") != 2:
        raise ValueError("Use recipe schema_version 2 with revision_scope")
    for key in ("brief", "focus_track"):
        if not isinstance(value.get(key), str) or not value[key].strip():
            raise ValueError(f"Recipe needs {key}")
    if "interventions" in value:
        raise ValueError("Use revision_scope instead of predefined interventions")
    scope = {
        "fields": sorted(FIELDS),
        "max_changed_notes": 16,
        **value.get("revision_scope", {}),
    }
    fields = scope["fields"]
    if (
        not isinstance(fields, list)
        or not fields
        or not all(isinstance(f, str) for f in fields)
        or not set(fields) <= FIELDS
    ):
        raise ValueError("Revision fields must be pitch/start/length")
    if (
        type(scope["max_changed_notes"]) is not int
        or not 1 <= scope["max_changed_notes"] <= 128
    ):
        raise ValueError("Use 1..128 maximum changed notes")
    clips = scope.get("clip_ids")
    if clips is not None and (
        not isinstance(clips, list)
        or not clips
        or not all(type(c) is int and c > 0 for c in clips)
        or len(set(clips)) != len(clips)
    ):
        raise ValueError("clip_ids must contain distinct positive integers")
    value["revision_scope"] = scope
    return value


def configure(args):
    """Resolve machine-specific settings once, relative to the shared config file."""
    path = args.config.expanduser().resolve(strict=True)
    config = load(path)
    if config.get("schema_version") != 1:
        raise ValueError("Use runtime config schema_version 1")
    settings = {
        "model": "gpt-6.1-sol",
        "backend": "codex",
        "prompt_profile": "legacy",
        "timeout": 900,
        "speaker": None,
        **config,
    }
    for key in (
        "executable",
        "auris",
        "evaluator_python",
        "songeval_config",
        "source_suite",
        "voice",
    ):
        value = settings.get(key)
        if value is None and key in {"source_suite", "voice"}:
            settings[key] = None
            continue
        if not isinstance(value, str) or not value:
            raise ValueError(f"Runtime config needs {key}")
        target = Path(value).expanduser()
        settings[key] = (
            target if target.is_absolute() else path.parent / target
        ).resolve(strict=True)
    if type(settings["timeout"]) is not int or not 60 <= settings["timeout"] <= 3600:
        raise ValueError("Use a timeout between 60 and 3600 seconds")
    return argparse.Namespace(**{**settings, **vars(args), "config": path})


def focus(project: dict, name: str) -> dict:
    matches = [t for t in project["tracks"] if t.get("name") == name]
    if len(matches) != 1 or matches[0].get("kind", {}).get("type") not in {
        "singer",
        "instrument",
    }:
        raise ValueError("Focus must uniquely name a singer or instrument track")
    return matches[0]


def validate_notes(project: dict, name: str) -> dict:
    """Check a monophonic foreground without treating rests or leaps as quality scores."""
    track = focus(project, name)
    count, pitches, gaps = 0, [], []
    for clip in track["kind"]["clips"]:
        end = 0
        if (
            not isinstance(clip.get("length"), (int, float))
            or not math.isfinite(clip["length"])
            or clip["length"] <= 0
        ):
            raise ValueError("Invalid foreground clip length")
        for note in clip.get("notes", []):
            pitch, start, length = (
                note.get(key) for key in ("pitch", "start", "length")
            )
            if not all(
                isinstance(x, (int, float))
                and not isinstance(x, bool)
                and math.isfinite(x)
                for x in (pitch, start, length)
            ):
                raise ValueError("Nonfinite or missing foreground note")
            if (
                not 0 <= pitch <= 127
                or length <= 0
                or start < end
                or start + length > clip["length"]
            ):
                raise ValueError(
                    "Foreground notes overlap, leave bounds, or have invalid pitch"
                )
            gaps.append(start - end)
            end = start + length
            pitches.append(pitch)
            count += 1
    if not count:
        raise ValueError("Foreground contains no notes")
    return {
        "notes": count,
        "pitch_min": min(pitches),
        "pitch_max": max(pitches),
        "gaps_ticks": gaps,
    }


def protected_projection(
    project: dict, name: str, fields: list[str], clip_ids=None
) -> dict:
    """Exclude only permitted note fields and the invalidated foreground singer take."""
    value = copy.deepcopy(project)
    track = focus(value, name)
    track["kind"].pop("take", None)
    for clip in track["kind"]["clips"]:
        if clip_ids is None or clip["id"] in clip_ids:
            for note in clip.get("notes", []):
                for field in fields:
                    note.pop(field, None)
    return value


def audit_edit(before: dict, after: dict, name: str, step: dict) -> dict:
    """Reject off-scope edits even when predicted aesthetic scores would increase."""
    old_metrics = validate_notes(before, name)
    new_metrics = validate_notes(after, name)
    fields, clips = step["fields"], step.get("clip_ids")
    if protected_projection(before, name, fields, clips) != protected_projection(
        after, name, fields, clips
    ):
        raise ValueError(
            "Edit changed protected notes, lyrics, backing, mix, assets or form"
        )
    changes = []
    for old_clip, new_clip in zip(
        focus(before, name)["kind"]["clips"],
        focus(after, name)["kind"]["clips"],
        strict=True,
    ):
        for index, (old, new) in enumerate(
            zip(old_clip["notes"], new_clip["notes"], strict=True)
        ):
            changes_here = {
                key: {"before": old[key], "after": new[key]}
                for key in fields
                if old[key] != new[key]
            }
            if changes_here:
                changes.append(
                    {
                        "clip_id": old_clip["id"],
                        "note_index": index,
                        "fields": changes_here,
                    }
                )
    if len(changes) > step["max_changed_notes"]:
        raise ValueError("Edit exceeded the declared note-change budget")
    return {
        "valid": True,
        "changed_notes": len(changes),
        "changes": changes,
        "before": old_metrics,
        "after": new_metrics,
    }


def command(argv: list[str], log: Path, timeout: int) -> None:
    """Persist commands and output, preserving failures for inspection."""
    write_json(
        log.with_suffix(".command.json"), {"argv": argv, "timeout_seconds": timeout}
    )
    with log.open("w", encoding="utf-8") as stream:
        process = subprocess.Popen(
            argv,
            cwd=ROOT,
            env=dict(os.environ, PYTHONIOENCODING="utf-8"),
            stdout=stream,
            stderr=subprocess.STDOUT,
        )
        try:
            process.wait(timeout=timeout)
        except (subprocess.TimeoutExpired, KeyboardInterrupt):
            if os.name == "nt":
                subprocess.run(
                    ["taskkill", "/PID", str(process.pid), "/T", "/F"],
                    capture_output=True,
                    check=False,
                )
            else:
                process.kill()
            process.wait()
            raise
    if process.returncode:
        raise RuntimeError(f"Command exited {process.returncode}; see {log}")


def copy_project(snapshot: Path, audio_root: Path | None, destination: Path) -> Path:
    """Create a separate native project without changing the saved source snapshot."""
    destination.mkdir(parents=True, exist_ok=False)
    target = destination / f"{destination.name}.auris"
    shutil.copy2(snapshot, target)
    if audio_root is not None and (audio_root / "Audio").is_dir():
        shutil.copytree(audio_root / "Audio", destination / "Audio")
    return target


def freeze_runtime(source: Path, output: Path) -> Path:
    output.mkdir()
    target = output / source.name
    shutil.copy2(source, target)
    for folder in (source.parent.parent, source.parent):
        for dll in folder.glob("*.dll"):
            shutil.copy2(dll, output / dll.name)
    return target


def run_stage(
    args, root: Path, prompt: str, seed: Path | None, *, review=False
) -> Path:
    conditions = (
        "Work only in the open Auris project. Read notes before editing and read back edits. "
        "Do not invoke external search, shell tools, audio critics, sing, render, gain changes or exports. "
        "The host saves snapshots, synthesizes and renders separately with a fixed runtime. "
        "Do not claim you heard the result. Do not read other trials or reference songs."
    )
    suite = {
        "schema_version": 1,
        "axes": quality.AXES,
        "conditions": conditions,
        "delivery_prompt": "Read back the changed notes and check bounds, time order, lyrics and the stated edit limits. Fix structural errors within scope only. Summarize the musical reason and any unresolved tradeoff; no further creative rewrite. Audio synthesis and evaluation are pending.",
        "cases": [
            {
                "id": "song",
                "split": "development",
                "fixture": "blank",
                "prompts": [prompt],
            }
        ],
    }
    if review:
        suite["delivery_prompt"] = suite["delivery_prompt"].replace(
            "Audio synthesis and evaluation are pending.",
            "The baseline has already been synthesized and evaluated by the host. "
            "A no-change decision reuses that audio and evaluation; only an edited candidate awaits them.",
        )
        suite["delivery_prompt"] += (
            "\nExplain the one local repair, its concrete musical reason, expected benefit "
            "and tradeoff, or why keeping the baseline is preferable. Report actual edits, "
            "not intentions. A lower model score alone is not a repair reason."
        )
    write_json(root / "manifest.json", suite)
    argv = [
        sys.executable,
        str(ROOT / "tools/eval/composition_quality.py"),
        "run",
        "--executable",
        str(args.executable),
        "--output",
        str(root / "agent"),
        "--manifest",
        str(root / "manifest.json"),
        "--model",
        args.model,
        "--backend",
        args.backend,
        "--prompt-profile",
        args.prompt_profile,
        "--no-candidate-tools",
        "--split",
        "development",
        "--repetitions",
        "1",
        "--timeout",
        str(args.timeout),
    ]
    if seed:
        argv += ["--seed-project", str(seed)]
    if args.source_suite:
        argv += ["--source-suite", str(args.source_suite)]
    command(argv, root / "agent.log", args.timeout * 2 + 180)
    result = load(root / "agent/suite.json")
    if len(result["trials"]) != 1 or not result["trials"][0]["transport_pass"]:
        raise RuntimeError("Agent trial did not complete both turns")
    return root / "agent/song-01/project-02.json"


def audio_metrics(path: Path, *, allow_silence: bool = False) -> dict:
    pcm, rate = sf.read(path, dtype="float32", always_2d=True)
    if not len(pcm) or not np.isfinite(pcm).all():
        raise ValueError("Empty or nonfinite rendered audio")
    peak = float(np.max(np.abs(pcm)))
    rms = float(np.sqrt(np.mean(np.square(pcm, dtype=np.float64))))
    if peak > 1 or (rms < 1e-6 and not allow_silence):
        raise ValueError("Rendered audio clips or is effectively silent")
    return {
        "sha256": quality.digest(path),
        "seconds": len(pcm) / rate,
        "sample_rate": rate,
        "channels": pcm.shape[1],
        "sample_peak": peak,
        "rms": rms,
        "gain_normalization": "none",
    }


def render(
    args, snapshot: Path, source: Path | None, destination: Path, focus_name: str
) -> dict:
    project = copy_project(snapshot, source, destination / "Song")
    original = load(project)
    singers = [t for t in original["tracks"] if t["kind"]["type"] == "singer"]
    if singers:
        if not args.voice:
            raise ValueError("A singer project requires --voice")
        argv = [
            str(args.auris),
            "sing",
            str(project),
            "--voice",
            str(args.voice),
            "--seed",
            "0",
        ]
        if args.speaker:
            argv += ["--speaker", args.speaker]
        command(argv, destination / "sing.log", args.timeout)
    performed = load(project)
    # Singing may add takes, voice metadata and audio sources, but cannot rewrite score.
    from composition_listening_assets import score_lyrics_projection

    if score_lyrics_projection(original) != score_lyrics_projection(performed):
        raise ValueError("Synthesis rewrote notes or lyrics")
    selected = focus(performed, focus_name)["id"]
    assets = {}
    for mode in ("mix", "foreground", "backing") if args.auditions else ("mix",):
        audition = copy.deepcopy(performed)
        if mode != "mix":
            for track in audition["tracks"]:
                mixer = track.get("mixer", {})
                mixer["solo"] = mode == "foreground" and track["id"] == selected
                if mode == "foreground" and track["id"] == selected:
                    mixer["mute"] = False
                elif mode == "backing" and track["id"] == selected:
                    mixer["mute"] = True
        audition_path = project
        if mode != "mix":
            audition_path = copy_project(project, project.parent, destination / mode)
            write_json(audition_path, audition)
        wav = destination / f"{mode}.wav"
        command(
            [
                str(args.auris),
                "render",
                str(audition_path),
                "-o",
                str(wav),
                "--bit-depth",
                "32",
                "--no-tail",
                "--force",
            ],
            destination / f"render-{mode}.log",
            args.timeout,
        )
        assets[mode] = {
            "path": str(wav),
            **audio_metrics(wav, allow_silence=mode == "backing"),
        }
    return {
        "assets": assets,
        "project": str(project),
        "auditions": "Separate full-master renders; not additive stems",
    }


def revision_prompt(brief: dict, evaluation: dict) -> str:
    step = brief["revision_scope"]
    return (
        brief["brief"] + "\n\nThis is a limited revision of the retained baseline. "
        "Keep its recognisable identity. The task is not to maximize an evaluator. "
        "SongEval predicts whole-recording attributes and has no timestamped diagnosis or rhythm axis. "
        "Memorability is not a recall test; naturalness is a vocal phrasing/breathing prediction. "
        "Small differences are uncalibrated; scores can disagree with listeners. "
        "Use these as uncertain context only; inspect actual notes and the brief before acting. "
        "Do not add silence, change loudness, instrumentation, lyrics, form or backing to raise scores. "
        f"\nEvaluation (all axes, no total): {json.dumps(evaluation['scores'], ensure_ascii=False)}"
        "\nInspect the written melody and lyrics, then choose KEEP or one local repair. "
        "Identify a specific musical problem, benefit and tradeoff before editing. "
        "If there is no justified repair, leave the score unchanged. Otherwise choose ONE "
        "musical theme and edit ONE clip only. Do not make a whole-song rewrite."
        f"\nFocus track: {brief['focus_track']}; allowed existing note fields: {step['fields']}; "
        f"allowed clip IDs: {step.get('clip_ids', 'all in focus')}; at most {step['max_changed_notes']} changed notes. "
        "Preserve every other field, note count/order, lyric mapping and clip boundary. "
        "State a concrete musical reason, expected benefit and possible tradeoff. "
        "No change is valid if there is no justified edit. Do not claim an improvement before listening."
    )


def review_decision(transcript: Path, audit: dict) -> dict:
    """Derive the decision from actual edits, retaining the agent's musical explanation."""
    messages = [
        m["text"].strip()
        for m in load(transcript)
        if m.get("kind") == "assistant" and m.get("text", "").strip()
    ]
    if not messages:
        raise ValueError("Missing review decision")
    clips = {change["clip_id"] for change in audit["changes"]}
    if len(clips) > 1:
        raise ValueError("Revision changes more than one clip")
    return {
        "action": "revise" if clips else "keep",
        "clip_id": next(iter(clips), None),
        "reason": messages[-1],
    }


def listening_page(output: Path, stages: list[dict]) -> None:
    rows = []
    for stage in stages:
        audio = stage.get("render", {}).get("assets", {})
        if not audio:
            continue
        rows.append(
            f"<h2>{html.escape(stage['name'])}</h2><p>{html.escape(stage['status'])}</p>"
        )
        for mode in audio:
            relative = Path(audio[mode]["path"]).relative_to(output).as_posix()
            rows.append(
                f'<p>{mode}</p><audio controls preload="none" src="{html.escape(relative, quote=True)}"></audio>'
            )
    page = (
        "<!doctype html><meta charset='utf-8'><title>Composition revision auditions</title><style>body{font:16px system-ui;max-width:850px;margin:32px auto;padding:16px}audio{width:100%}</style><h1>Composition revision auditions</h1><p>Original levels; no independent normalization. The baseline remains the default. Optional foreground/backing auditions are separate master renders.</p>"
        + "\n".join(rows)
    )
    (output / "listening.html").write_bytes(page.encode("utf-8"))


def compose_take(
    args,
    output: Path,
    report: dict,
    brief: dict,
    prompt: str,
    seed: Path | None,
    *,
    review=False,
) -> dict:
    """Use the same composition and scope checks for the baseline and optional revision."""
    name = "01-revision" if review else "00-baseline"
    root = output / name
    root.mkdir()
    stage = {"name": name, "status": "running", "prompt": prompt}
    report["stages"].append(stage)
    write_json(output / "report.json", report)
    snapshot = run_stage(args, root, prompt, seed, review=review)
    stage["snapshot_sha256"] = quality.digest(snapshot)
    stage["symbolic"] = validate_notes(load(snapshot), brief["focus_track"])
    if seed:
        scope = (
            brief["revision_scope"]
            if review
            else {"fields": sorted(FIELDS), "max_changed_notes": 100000}
        )
        stage["edit_audit"] = audit_edit(
            load(seed), load(snapshot), brief["focus_track"], scope
        )
    stage["project"] = str(
        copy_project(
            snapshot, seed.parent if seed else root / "agent/song-01", root / "Score"
        )
    )
    if review:
        stage["decision"] = review_decision(
            root / "agent/song-01/transcript.json", stage["edit_audit"]
        )
    return stage


def assess_take(
    args, output: Path, stage: dict, focus_name: str, baseline: dict | None = None
) -> None:
    """Synthesize, render, validate and score a take through one shared path."""
    root = output / stage["name"]
    project = Path(stage["project"])
    stage["render"] = render(args, project, project.parent, root / "audio", focus_name)
    if baseline:
        before, after = baseline["render"]["assets"], stage["render"]["assets"]
        if (
            abs(before["mix"]["seconds"] - after["mix"]["seconds"])
            > 1 / before["mix"]["sample_rate"]
        ):
            raise ValueError("Rendered duration changed")
        if args.auditions and before["backing"]["sha256"] != after["backing"]["sha256"]:
            raise ValueError("Backing audition changed")
    stage["scores"] = evaluate(args, root, stage["render"]["assets"]["mix"])
    if baseline:
        stage["score_delta"] = compare_scores(baseline["scores"], stage["scores"])


def run(args) -> dict:
    brief = recipe(args.recipe)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    report = {
        "schema_version": 2,
        "status": "running",
        "policy": "compose, evaluate, optionally revise one clip once; retain baseline without a score winner",
        "recipe": brief,
        "recipe_sha256": quality.digest(args.recipe),
        "stages": [],
        "started_unix": time.time(),
        "selected_stage": None,
    }
    frozen_seed = None
    original_files = {}
    try:
        scripts = output / "scripts"
        scripts.mkdir()
        for name in (
            "composition_refine.py",
            "composition_quality.py",
            "composition_listening_assets.py",
            "songeval.py",
        ):
            shutil.copy2(Path(__file__).with_name(name), scripts / name)
        report["scripts"] = tree_hashes(scripts)
        shutil.copy2(args.config, output / "runtime-config.json")
        evaluator_config = output / "songeval-config.json"
        shutil.copy2(args.songeval_config, evaluator_config)
        args.songeval_config = evaluator_config
        # Freeze all renderer DLLs/configuration and the optional user project before execution.
        args.auris = freeze_runtime(
            args.auris.resolve(strict=True), output / "renderer"
        )
        report["renderer"] = tree_hashes(output / "renderer")
        if args.voice:
            target = output / args.voice.name
            shutil.copy2(args.voice, target)
            args.voice = target
            report["voice_config_sha256"] = quality.digest(target)
            from composition_listening_assets import _voice_metadata

            report["voice_runtime"] = _voice_metadata(target)
        if args.seed_project:
            provenance = quality.freeze_seed_project(
                args.seed_project, output / "input"
            )
            report["seed"] = provenance
            frozen_seed = (
                output / "input" / args.seed_project.stem / args.seed_project.name
            )
            original_files = {
                str(args.seed_project.parent.parent / name): sha
                for name, sha in provenance["files"].items()
            }
        write_json(output / "recipe.json", brief)
        write_json(output / "report.json", report)
        prompt = brief["brief"]
        if frozen_seed:
            prompt += f"\nRewrite only existing pitch/start/length fields in {brief['focus_track']}. Preserve all other fields and every note/lyric in order."
        else:
            prompt += f"\nCreate the monophonic foreground on a track named exactly {brief['focus_track']}."
        baseline = compose_take(args, output, report, brief, prompt, frozen_seed)
        assess_take(args, output, baseline, brief["focus_track"])
        baseline["status"] = "baseline_retained"
        report["selected_stage"] = "00-baseline"
        write_json(output / "report.json", report)
        listening_page(output, report["stages"])
        print(
            "Baseline ready; reviewing whether one local repair is justified.",
            flush=True,
        )

        candidate = compose_take(
            args,
            output,
            report,
            brief,
            revision_prompt(brief, baseline["scores"]),
            Path(baseline["project"]),
            review=True,
        )
        if candidate["decision"]["action"] == "keep":
            candidate["status"] = "no_change"
            report["status"] = "baseline_retained_no_change"
        else:
            assess_take(args, output, candidate, brief["focus_track"], baseline)
            candidate["status"] = "candidate_for_listening"
            report["status"] = "completed_review_required"
    except Exception as error:  # noqa: BLE001 - preserve failed takes; the CLI exits unsuccessfully.
        status = (
            "baseline_only_revision_failed" if report["selected_stage"] else "failed"
        )
        report.update(status=status, error=f"{type(error).__name__}: {error}")
        if report["stages"]:
            report["stages"][-1].update(
                status="failed_or_rejected", error=report["error"]
            )
    finally:
        if frozen_seed and tree_hashes(output / "input") != report["seed"]["files"]:
            report.update(status="failed", error="Frozen input changed")
        if any(
            not Path(p).is_file() or quality.digest(Path(p)) != sha
            for p, sha in original_files.items()
        ):
            report.update(status="failed", error="Original input changed")
        report["finished_unix"] = time.time()
        for stage in report["stages"]:
            write_json(output / stage["name"] / "stage.json", stage)
        write_json(output / "report.json", report)
        listening_page(output, report["stages"])
    print(
        report["status"] + (": " + report["error"] if "error" in report else ""),
        flush=True,
    )
    return report


def evaluate(args, stage_root: Path, asset: dict) -> dict:
    """Run SongEval out of process so the model environment stays optional."""
    result = stage_root / "songeval.json"
    command(
        [
            str(args.evaluator_python),
            str(ROOT / "tools/eval/songeval.py"),
            "--config",
            str(args.songeval_config),
            "--output",
            str(result),
            asset["path"],
        ],
        stage_root / "songeval.log",
        args.timeout,
    )
    report = load(result)
    return extract_scores(report, asset["sha256"])


def extract_scores(report: dict, expected_sha256: str) -> dict:
    """The adapter report contract is checked here before feedback is sent to an agent."""
    # Kept separate from orchestration for report identity/condition tests.
    from songeval import AXES

    rows = report["rows"]
    if len(rows) != 1 or rows[0]["sha256"] != expected_sha256:
        raise ValueError("Evaluator did not score the retained mix")
    row = rows[0]
    if row.get("status") != "scored" or not row.get("diagnostics", {}).get(
        "admissible"
    ):
        raise ValueError("Evaluator rejected the retained mix")
    scores = row["scores"]
    if set(scores) != set(AXES) or not all(
        isinstance(v, (int, float))
        and not isinstance(v, bool)
        and math.isfinite(v)
        and 1 <= v <= 5
        for v in scores.values()
    ):
        raise ValueError("Invalid SongEval axes")
    return {
        "scores": scores,
        "conditions": report["runtime"],
        "audio_sha256": expected_sha256,
    }


def compare_scores(baseline: dict, candidate: dict) -> dict:
    if (
        baseline["conditions"] != candidate["conditions"]
        or baseline["scores"].keys() != candidate["scores"].keys()
    ):
        raise ValueError("Evaluator conditions or axes changed")
    return {
        axis: round(candidate["scores"][axis] - value, 4)
        for axis, value in baseline["scores"].items()
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ("recipe", "output"):
        parser.add_argument("--" + key, type=Path, required=True)
    parser.add_argument(
        "--config",
        type=Path,
        default=Path.home() / ".config/auris-studio/composition-refinement.json",
    )
    parser.add_argument("--seed-project", type=Path)
    parser.add_argument(
        "--auditions",
        action="store_true",
        help="Also render foreground and backing auditions",
    )
    report = run(configure(parser.parse_args()))
    if report["status"] in {"failed", "baseline_only_revision_failed"}:
        raise SystemExit(2)


if __name__ == "__main__":
    main()
