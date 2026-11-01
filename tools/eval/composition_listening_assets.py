# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Prepare reproducible listening assets from a composition-evaluation snapshot.

The composition runner writes JSON snapshots rather than native project folders.  This helper
copies one final snapshot into an isolated ``song.auris`` project, optionally synthesizes its
singer with the configured VOICEVOX connection, and renders a mix plus foreground/backing
auditions.  It never edits the source snapshot.  Foreground and backing copies change only
track mute/solo flags; the score and lyrics projection is checked before and after synthesis.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import math
import os
import re
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[2]

_SNAPSHOT_RE = re.compile(r"^project-(\d+)\.json$", re.IGNORECASE)
_SYNTHESIS_KEYS = {
    "voice",
    "speaker",
    "take",
    "takes",
    "frames",
    "backend_pitch",
    "rendered_audio",
    "singer_audio",
    "frame_hop",
}
_TRACK_NAME_HINTS = ("lead", "melody", "メロディ", "主旋律", "theme", "hook")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def write_json(path: Path, value: Any) -> None:
    path.write_text(
        json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )


def select_snapshot(source: Path, explicit: Path | None = None) -> Path:
    """Select the highest numbered composition snapshot, unless explicitly named."""
    if explicit is not None:
        path = explicit if explicit.is_absolute() else source / explicit
        if not path.is_file():
            raise FileNotFoundError(f"snapshot does not exist: {path}")
        return path
    numbered = []
    for path in source.glob("project-*.json"):
        match = _SNAPSHOT_RE.match(path.name)
        if match:
            numbered.append((int(match.group(1)), path))
    if numbered:
        return max(numbered, key=lambda item: item[0])[1]
    for name in ("project.json", "final-project.json", "final.json"):
        path = source / name
        if path.is_file():
            return path
    raise FileNotFoundError(f"no project-<n>.json or project.json in {source}")


def _without_synthesis(value: Any, key: str | None = None) -> Any:
    if isinstance(value, dict):
        return {
            name: _without_synthesis(child, name)
            for name, child in value.items()
            if name not in _SYNTHESIS_KEYS and name not in {"mixer"}
        }
    if isinstance(value, list):
        return [_without_synthesis(item) for item in value]
    return value


def score_lyrics_projection(project: dict[str, Any]) -> dict[str, Any]:
    """Return the score/lyrics-only view used to prove singing did not rewrite the score."""
    return _without_synthesis(
        {
            key: project.get(key)
            for key in ("tempo_map", "signatures", "harmony", "sections", "tracks")
            if key in project
        }
    )


def _track_type(track: dict[str, Any]) -> str:
    kind = track.get("kind", {})
    return str(kind.get("type", "")) if isinstance(kind, dict) else ""


def singer_tracks(project: dict[str, Any]) -> list[dict[str, Any]]:
    return [
        track for track in project.get("tracks", []) if _track_type(track) == "singer"
    ]


def focus_track(
    project: dict[str, Any], requested: str | None = None
) -> dict[str, Any] | None:
    tracks = project.get("tracks", [])
    if requested:
        for track in tracks:
            if str(track.get("name", "")) == requested:
                return track
        raise ValueError(f"focus track not found: {requested}")
    singers = singer_tracks(project)
    if singers:
        return singers[0]
    candidates = [
        track
        for track in tracks
        if _track_type(track) == "instrument"
        and any(
            hint in str(track.get("name", "")).lower() for hint in _TRACK_NAME_HINTS
        )
    ]
    if candidates:
        return candidates[0]
    return next((track for track in tracks if _track_type(track) == "instrument"), None)


def _set_audition_mix(project: dict[str, Any], focus_id: Any, mode: str) -> None:
    """Change only mixer mute/solo controls in a temporary audition project."""
    for track in project.get("tracks", []):
        mixer = track.get("mixer")
        if not isinstance(mixer, dict):
            continue
        mixer["solo"] = mode == "foreground" and track.get("id") == focus_id
        if mode == "foreground" and track.get("id") == focus_id:
            mixer["mute"] = False
        if mode == "backing" and track.get("id") == focus_id:
            mixer["mute"] = True


def _prepare_vocal_mix(project: dict[str, Any]) -> list[dict[str, Any]]:
    """Make the post-synthesis full mix audible without changing score data."""
    original: list[dict[str, Any]] = []
    for track in project.get("tracks", []):
        mixer = track.get("mixer")
        if not isinstance(mixer, dict):
            continue
        original.append(
            {
                "id": track.get("id"),
                "name": track.get("name"),
                "mute": mixer.get("mute"),
                "solo": mixer.get("solo"),
            }
        )
        mixer["solo"] = False
        if _track_type(track) == "singer":
            mixer["mute"] = False
    return original


def _number(value: Any) -> float | None:
    if isinstance(value, (int, float)):
        return float(value)
    if isinstance(value, dict):
        for key in ("raw", "value", "ticks"):
            if key in value:
                return _number(value[key])
    return None


def singer_audit(
    project: dict[str, Any], breath_threshold_ticks: float = 240
) -> dict[str, Any]:
    """Collect score-level vocal coverage and rest/breath proxies, not a perceptual judgment."""
    result: list[dict[str, Any]] = []
    for track in singer_tracks(project):
        notes: list[dict[str, Any]] = []
        for clip in track.get("kind", {}).get("clips", []):
            for note in clip.get("notes", []):
                if not clip.get("muted", False):
                    absolute = dict(note)
                    absolute["_absolute_start"] = (_number(clip.get("start")) or 0) + (
                        _number(note.get("start")) or 0
                    )
                    notes.append(absolute)
        notes.sort(key=lambda note: note.get("_absolute_start", 0))
        gaps: list[float] = []
        previous_end: float | None = None
        for note in notes:
            start = note.get("_absolute_start")
            length = _number(note.get("length")) or 0
            if start is None:
                continue
            if previous_end is not None and start > previous_end:
                gap = start - previous_end
                if gap >= breath_threshold_ticks:
                    gaps.append(gap)
            previous_end = max(previous_end or start, start + length)
        unassigned = sum(not (note.get("lyric") or "").strip() for note in notes)
        result.append(
            {
                "track": track.get("name"),
                "track_id": track.get("id"),
                "take_state": "present_unverified"
                if track.get("kind", {}).get("take")
                else "missing",
                "notes": len(notes),
                "lyrics_assigned": len(notes) - unassigned,
                "lyrics_unassigned": unassigned,
                "breath_gap_threshold_ticks": breath_threshold_ticks,
                "breath_gaps": len(gaps),
                "breath_gap_ticks": gaps,
            }
        )
    return {
        "singers": result,
        "score_level_only": True,
        "measurement_limit": "Explicit notes in unmuted clips only; loop expansion and generated performance are not inferred.",
    }


def find_auris(explicit: str | None) -> Path:
    if explicit:
        path = Path(explicit)
        if not path.is_file():
            raise FileNotFoundError(path)
        return path
    local_build = (
        REPO / "target" / "debug" / ("auris.exe" if os.name == "nt" else "auris")
    )
    if local_build.is_file():
        return local_build
    found = shutil.which("auris") or shutil.which("auris.exe")
    if found:
        return Path(found)
    local_appdata = os.environ.get("LOCALAPPDATA")
    if local_appdata:
        path = Path(local_appdata) / "Programs" / "Auris Studio" / "auris.exe"
        if path.is_file():
            return path
    raise FileNotFoundError("auris executable not found on PATH or LOCALAPPDATA")


def find_ffmpeg(explicit: str | None) -> Path:
    found = Path(explicit) if explicit else Path(shutil.which("ffmpeg") or "")
    if found.is_file():
        return found
    raise FileNotFoundError("ffmpeg executable not found")


def run_cli(auris: Path, args: list[str], cwd: Path, timeout: int) -> dict[str, Any]:
    started = time.monotonic()
    command = [str(auris), *args]
    try:
        process = subprocess.run(
            command,
            check=False,
            cwd=cwd,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
        )
        return {
            "command": command,
            "returncode": process.returncode,
            "stdout": process.stdout[-4000:],
            "stderr": process.stderr[-4000:],
            "elapsed_seconds": time.monotonic() - started,
        }
    except (
        OSError,
        subprocess.TimeoutExpired,
    ) as error:  # retain failures in the audit
        return {
            "command": command,
            "returncode": None,
            "stdout": "",
            "stderr": str(error),
            "elapsed_seconds": time.monotonic() - started,
        }


def _ffmpeg_true_peak(
    ffmpeg: Path, source: Path, timeout: int
) -> tuple[float | None, dict[str, Any]]:
    command = [
        str(ffmpeg),
        "-hide_banner",
        "-i",
        str(source),
        "-af",
        "loudnorm=print_format=json",
        "-f",
        "null",
        os.devnull,
    ]
    try:
        process = subprocess.run(
            command,
            check=False,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
        )
        match = re.search(r'"input_tp"\s*:\s*"?([^",\n]+)', process.stderr)
        value = (
            float(match.group(1))
            if match and match.group(1).lower() not in {"-inf", "inf", "nan"}
            else None
        )
        if process.returncode != 0 or value is None or not math.isfinite(value):
            value = None
        return value, {
            "command": command,
            "returncode": process.returncode,
            "stderr": process.stderr[-4000:],
        }
    except (OSError, subprocess.TimeoutExpired) as error:
        return None, {"command": command, "returncode": None, "stderr": str(error)}


def _make_listening_wav(
    ffmpeg: Path, source: Path, destination: Path, timeout: int
) -> dict[str, Any]:
    peak, measurement = _ffmpeg_true_peak(ffmpeg, source, timeout)
    result: dict[str, Any] = {
        "float": _asset(source),
        "measurement": measurement,
        "input_true_peak_dbtp": peak,
    }
    if peak is None:
        result["failure"] = "true peak unavailable or non-finite"
        return result
    gain = min(0.0, -1.0 - peak)
    result["gain_db"] = gain
    command = [
        str(ffmpeg),
        "-y",
        "-i",
        str(source),
        "-af",
        f"volume={gain}dB",
        "-c:a",
        "pcm_s24le",
        str(destination),
    ]
    try:
        process = subprocess.run(
            command,
            check=False,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
        )
        result["command"] = command
        result["returncode"] = process.returncode
        result["stderr"] = process.stderr[-4000:]
        if process.returncode == 0 and destination.is_file():
            output_peak, output_measurement = _ffmpeg_true_peak(
                ffmpeg, destination, timeout
            )
            result["output_true_peak_dbtp"] = output_peak
            result["output_measurement"] = output_measurement
            result["asset"] = _asset(destination)
            if output_peak is None:
                result["failure"] = "output true peak unavailable or non-finite"
            elif output_peak > -0.9:
                result["failure"] = "output true peak exceeds -0.9 dBTP safety margin"
        else:
            result["failure"] = "24-bit listening render failed"
    except (OSError, subprocess.TimeoutExpired) as error:
        result["returncode"] = None
        result["failure"] = str(error)
    return result


def _asset(path: Path) -> dict[str, Any]:
    return {
        "path": str(path.resolve()),
        "bytes": path.stat().st_size,
        "sha256": sha256(path),
    }


def _voice_metadata(path: Path | None) -> dict[str, Any]:
    if path is None or not path.is_file():
        return {}
    metadata: dict[str, Any] = {"config": _asset(path)}
    try:
        config = json.loads(path.read_text(encoding="utf-8"))
        endpoint = config.get("url")
        metadata["endpoint"] = endpoint
        if endpoint:
            with urllib.request.urlopen(
                endpoint.rstrip("/") + "/version", timeout=15
            ) as response:
                metadata["engine_version"] = json.loads(response.read().decode("utf-8"))
    except (
        OSError,
        urllib.error.URLError,
        json.JSONDecodeError,
        UnicodeError,
        TimeoutError,
    ) as error:
        metadata["engine_version_error"] = str(error)
    return metadata


def _take_assets(project: dict[str, Any], project_path: Path) -> list[dict[str, Any]]:
    assets: list[dict[str, Any]] = []
    sources = project.get("audio_sources", {})
    for track in singer_tracks(project):
        take = track.get("kind", {}).get("take")
        if not isinstance(take, dict):
            continue
        source = sources.get(str(take.get("source")), sources.get(take.get("source")))
        if not isinstance(source, dict):
            continue
        asset_path = source.get("path", {})
        if isinstance(asset_path, dict) and "inside" in asset_path:
            candidate = project_path.parent / asset_path["inside"]
        elif isinstance(asset_path, dict) and "external" in asset_path:
            candidate = Path(asset_path["external"])
        elif isinstance(asset_path, str):
            candidate = Path(asset_path)
        else:
            continue
        if candidate.is_file():
            assets.append(
                {
                    "track": track.get("name"),
                    "source": take.get("source"),
                    **_asset(candidate),
                }
            )
    return assets


def _frame_hops(project: dict[str, Any]) -> list[dict[str, Any]]:
    return [
        {
            "track": t.get("name"),
            "track_id": t.get("id"),
            "frame_hop": t.get("kind", {}).get("frame_hop"),
        }
        for t in singer_tracks(project)
    ]


def prepare(args: argparse.Namespace) -> int:
    source = args.source_dir.resolve()
    snapshot = select_snapshot(source, args.snapshot)
    original = json.loads(snapshot.read_text(encoding="utf-8"))
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    project_path = output / "song.auris"
    project_path.write_text(
        json.dumps(original, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    if (source / "Audio").is_dir():
        shutil.copytree(source / "Audio", output / "Audio")

    auris = find_auris(args.auris)
    ffmpeg = find_ffmpeg(args.ffmpeg)
    audit: dict[str, Any] = {
        "schema_version": 1,
        "source_snapshot": _asset(snapshot),
        "source_dir": str(source),
        "auris": str(auris),
        "voice": str(args.voice) if args.voice else None,
        "speaker": args.speaker,
        "seed": args.seed,
        "singer": singer_audit(original, args.breath_threshold_ticks),
        "singer_frame_hop_before": _frame_hops(original),
        "score_lyrics_sha256_before": hashlib.sha256(
            json.dumps(
                score_lyrics_projection(original), sort_keys=True, ensure_ascii=False
            ).encode()
        ).hexdigest(),
        "commands": [],
        "assets": {},
        "failures": [],
        "master_processing_note": "Foreground/backing are separate full-master renders with temporary mute/solo; they are not additive stems.",
        "render_range": "whole_project",
        "render_tail": "no_tail (CLI has no from/to range flags; avoids appending effect-tail silence)",
        "cli_binary_sha256": sha256(auris),
        "ffmpeg": str(ffmpeg),
        "ffmpeg_sha256": sha256(ffmpeg),
    }
    audit["cli_help_probe"] = run_cli(auris, ["help"], output, min(args.timeout, 30))
    audit["voice_metadata"] = _voice_metadata(args.voice)
    singers = singer_tracks(original)
    if singers:
        if not args.voice:
            audit["failures"].append("singer track exists but --voice was not supplied")
        else:
            command = [
                "sing",
                str(project_path),
                "--voice",
                str(args.voice),
                "--seed",
                str(args.seed),
            ]
            if len(singers) == 1:
                command += ["--track", str(singers[0].get("name", ""))]
            if args.speaker:
                command += ["--speaker", args.speaker]
            result = run_cli(auris, command, output, args.timeout)
            audit["commands"].append(result)
            if result["returncode"] != 0:
                audit["failures"].append("sing failed")
            else:
                audit["sing_success"] = True

    try:
        after = json.loads(project_path.read_text(encoding="utf-8"))
        audit["score_lyrics_sha256_after_sing"] = hashlib.sha256(
            json.dumps(
                score_lyrics_projection(after), sort_keys=True, ensure_ascii=False
            ).encode()
        ).hexdigest()
        audit["score_lyrics_unchanged"] = (
            audit["score_lyrics_sha256_before"]
            == audit["score_lyrics_sha256_after_sing"]
        )
        if not audit["score_lyrics_unchanged"]:
            audit["failures"].append("score or lyrics changed during singing")
        audit["singer_after"] = singer_audit(after, args.breath_threshold_ticks)
        audit["singer_frame_hop_after"] = _frame_hops(after)
        for row, track in zip(
            audit["singer_after"]["singers"], singer_tracks(after), strict=True
        ):
            take = track.get("kind", {}).get("take")
            row["take_seed"] = take.get("seed") if isinstance(take, dict) else None
            row["take_validation"] = (
                "cli_sing_succeeded_and_seed_matches"
                if audit.get("sing_success") and row["take_seed"] == args.seed
                else "not_verified"
            )
            if row["take_validation"] != "cli_sing_succeeded_and_seed_matches":
                audit["failures"].append(
                    f"singer take was not verified current: {row['track']}"
                )
    except (OSError, json.JSONDecodeError, TypeError, ValueError) as error:
        audit["failures"].append(f"cannot inspect project after sing: {error}")
        after = original

    audit["prepared_mix_original_mixer"] = _prepare_vocal_mix(after)
    write_json(project_path, after)
    audit["take_assets"] = _take_assets(after, project_path)
    if singers and len(audit["take_assets"]) < len(singers):
        audit["failures"].append("one or more singer take audio paths are missing")

    focus = focus_track(after, args.focus)
    if focus is None:
        audit["failures"].append("no focus track found")
    else:
        audit["focus_track"] = {
            "id": focus.get("id"),
            "name": focus.get("name"),
            "type": _track_type(focus),
        }
        for mode in ("foreground", "backing"):
            temporary = copy.deepcopy(after)
            _set_audition_mix(temporary, focus.get("id"), mode)
            audit.setdefault("audition_mix_changes", {})[mode] = {
                "focus_track": focus.get("name"),
                "focus_id": focus.get("id"),
                "rule": "foreground=solo focus and unmute focus; backing=mute focus; other controls copied",
                "source_focus_mute": next(
                    (
                        track.get("mixer", {}).get("mute")
                        for track in after.get("tracks", [])
                        if track.get("id") == focus.get("id")
                    ),
                    None,
                ),
                "render_focus_mute": next(
                    (
                        track.get("mixer", {}).get("mute")
                        for track in temporary.get("tracks", [])
                        if track.get("id") == focus.get("id")
                    ),
                    None,
                ),
            }
            path = output / f"{mode}.auris"
            write_json(path, temporary)
            float_wav = output / f"{mode}.float.wav"
            wav = output / f"{mode}.wav"
            result = run_cli(
                auris,
                [
                    "render",
                    str(path),
                    "-o",
                    str(float_wav),
                    "--bit-depth",
                    "32",
                    "--no-tail",
                    "--force",
                ],
                output,
                args.timeout,
            )
            audit["commands"].append(result)
            if result["returncode"] != 0 or not float_wav.is_file():
                audit["failures"].append(f"{mode} render failed")
            else:
                loudness = _make_listening_wav(ffmpeg, float_wav, wav, args.timeout)
                audit.setdefault("loudness", {})[mode] = loudness
                if "failure" in loudness:
                    audit["failures"].append(f"{mode} 24-bit conversion failed")
                else:
                    audit["assets"][mode] = loudness["asset"]

    mix_float = output / "mix.float.wav"
    mix = output / "mix.wav"
    result = run_cli(
        auris,
        [
            "render",
            str(project_path),
            "-o",
            str(mix_float),
            "--bit-depth",
            "32",
            "--no-tail",
            "--force",
        ],
        output,
        args.timeout,
    )
    audit["commands"].append(result)
    if result["returncode"] != 0 or not mix_float.is_file():
        audit["failures"].append("mix render failed")
    else:
        loudness = _make_listening_wav(ffmpeg, mix_float, mix, args.timeout)
        audit.setdefault("loudness", {})["mix"] = loudness
        if "failure" in loudness:
            audit["failures"].append("mix 24-bit conversion failed")
        else:
            audit["assets"]["mix"] = loudness["asset"]
    audit["assets"]["project"] = _asset(project_path)
    audit["ready"] = not audit["failures"] and (
        not singers or audit.get("sing_success") is True
    )
    write_json(output / "audit.json", audit)
    return 1 if audit["failures"] else 0


def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--source-dir", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--snapshot", type=Path)
    p.add_argument("--auris")
    p.add_argument("--ffmpeg")
    p.add_argument("--voice", type=Path)
    p.add_argument("--speaker", default="ずんだもん / ノーマル")
    p.add_argument("--seed", type=int, default=0)
    p.add_argument("--focus")
    p.add_argument("--timeout", type=int, default=900)
    p.add_argument("--breath-threshold-ticks", type=float, default=240)
    return p


def main() -> int:
    args = parser().parse_args()
    return prepare(args)


if __name__ == "__main__":
    sys.exit(main())
