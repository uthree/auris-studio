# /// script
# requires-python = ">=3.10"
# dependencies = []
# ///
"""Copy candidate melody into a saved backing for an editable, controlled A/B.

    uv run tools/eval/melody_ab.py --source after/song/song.auris \
        --candidate candidate/song/song.auris --output ab/song/song.auris \
        --cli target/debug/auris.exe --wav ab/song.wav

Only instrument Lead/Melody note arrays and their generated-text digest can change.
Every other source field, including backing notes, mixer, harmony, instrument state,
IDs and performance transforms, is preserved. Recipe knobs must match, and the
candidate's digest is copied with its complete notes; no regeneration occurs here.
Sources must use external asset references so moving a document cannot break paths.
Existing outputs and ambiguous/misaligned layouts are rejected. The manifest records
content hashes, not quality scores. Optional rendering uses the existing CLI only.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import math
import subprocess
from pathlib import Path

MELODY_ROLES = {"lead", "melody"}
CONTEXT_FIELDS = (
    "format_version",
    "sample_rate",
    "tempo_map",
    "signatures",
    "harmony",
    "sections",
    "loop_region",
    "loop_enabled",
)


def content_hash(value: object) -> str:
    """Hash parsed content, independent of JSON whitespace and object-key order."""
    return hashlib.sha256(
        json.dumps(
            value,
            sort_keys=True,
            ensure_ascii=False,
            allow_nan=False,
            separators=(",", ":"),
        ).encode("utf-8")
    ).hexdigest()


def file_hash(path: Path) -> str:
    with path.open("rb") as stream:
        digest = hashlib.sha256()
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def index_unique(items: list[dict], key, label: str) -> dict:
    result = {}
    for item in items:
        identity = key(item)
        if identity in result:
            raise ValueError(f"Ambiguous {label}: {identity!r}")
        result[identity] = item
    return result


def clip_key(clip: dict) -> tuple:
    return clip["name"], clip["start"], clip["length"]


def track_index(project: dict) -> dict:
    return index_unique(
        project["tracks"], lambda t: (t["name"], t["kind"]["type"]), "track name/type"
    )


def clip_index(track: dict) -> dict:
    return index_unique(
        track["kind"].get("clips", []), clip_key, "clip name/start/length"
    )


def melody_keys(track: dict) -> set[tuple]:
    """Recognize generated leads, and endings on an otherwise pure lead track."""
    if track["kind"]["type"] != "instrument":
        return set()
    clips = clip_index(track)
    roles = {(c.get("recipe") or {}).get("preset") for c in clips.values()}
    roles.discard(None)
    pure_lead = bool(roles) and roles <= MELODY_ROLES
    named_lead = not roles and track["name"].lower() in MELODY_ROLES
    return {
        key
        for key, clip in clips.items()
        if (clip.get("recipe") or {}).get("preset") in MELODY_ROLES
        or (not clip.get("recipe") and (pure_lead or named_lead))
    }


def recipe_knobs(clip: dict) -> dict | None:
    recipe = copy.deepcopy(clip.get("recipe"))
    if recipe is not None:
        recipe.pop("text_digest", None)
    return recipe


def validate_notes(clip: dict) -> None:
    def integer(n: object) -> bool:
        return isinstance(n, int) and not isinstance(n, bool)

    length, start = clip["length"], clip["start"]
    if not (integer(length) and length > 0 and integer(start) and start >= 0):
        raise ValueError(f"Invalid clip bounds: {clip_key(clip)!r}")
    for note in clip["notes"]:
        at, duration, pitch, velocity = (
            note[k] for k in ("start", "length", "pitch", "velocity")
        )
        if not (
            integer(at)
            and integer(duration)
            and at >= 0
            and duration > 0
            and at + duration <= length
            and integer(pitch)
            and 0 <= pitch <= 127
            and isinstance(velocity, (int, float))
            and not isinstance(velocity, bool)
            and math.isfinite(velocity)
            and 0 <= velocity <= 1
        ):
            raise ValueError(f"Invalid note in {clip_key(clip)!r}: {note!r}")


def preserved_content(project: dict, selected: dict) -> dict:
    """Mask exactly the fields which this controlled experiment may replace."""
    result = copy.deepcopy(project)
    for track_key, track in track_index(result).items():
        for key, clip in clip_index(track).items():
            if key in selected.get(track_key, set()):
                clip["notes"] = "candidate melody"
                if clip.get("recipe") is not None:
                    clip["recipe"].pop("text_digest", None)
    return result


def score_content(project: dict, selected: dict, *, melody: bool) -> list:
    return [
        [track_key, key, clip["notes"]]
        for track_key, track in track_index(project).items()
        for key, clip in clip_index(track).items()
        if "notes" in clip and ((key in selected.get(track_key, set())) == melody)
    ]


def replace_melody(source: dict, candidate: dict) -> tuple[dict, dict]:
    """Return a new document and proof of the preserved backing; never mutate inputs."""
    for field in CONTEXT_FIELDS:
        if source.get(field) != candidate.get(field):
            raise ValueError(f"Mismatched {field}")
    old_tracks, new_tracks = track_index(source), track_index(candidate)
    if old_tracks.keys() != new_tracks.keys():
        raise ValueError("Mismatched track layout")
    result = copy.deepcopy(source)
    result_tracks = track_index(result)
    selected, changes = {}, []
    for identity, old_track in old_tracks.items():
        old_clips, new_clips = clip_index(old_track), clip_index(new_tracks[identity])
        if old_clips.keys() != new_clips.keys():
            raise ValueError(f"Mismatched clip layout: {identity!r}")
        keys = melody_keys(old_track)
        if keys != melody_keys(new_tracks[identity]):
            raise ValueError(f"Mismatched melody roles: {identity!r}")
        selected[identity] = keys
        output_clips = clip_index(result_tracks[identity])
        for key in sorted(keys):
            old_clip, new_clip = old_clips[key], new_clips[key]
            validate_notes(old_clip)
            validate_notes(new_clip)
            if recipe_knobs(old_clip) != recipe_knobs(new_clip):
                raise ValueError(f"Mismatched recipe knobs: {identity!r}, {key!r}")
            if old_clip["notes"] and not new_clip["notes"]:
                raise ValueError(f"Candidate unexpectedly erased all melody: {key!r}")
            output = output_clips[key]
            output["notes"] = copy.deepcopy(new_clip["notes"])
            if output.get("recipe") is not None:
                output["recipe"].pop("text_digest", None)
                if "text_digest" in new_clip["recipe"]:
                    output["recipe"]["text_digest"] = new_clip["recipe"]["text_digest"]
            changes.append(
                {
                    "track": identity[0],
                    "clip": list(key),
                    "source_note_count": len(old_clip["notes"]),
                    "candidate_note_count": len(new_clip["notes"]),
                    "source_notes_sha256": content_hash(old_clip["notes"]),
                    "output_notes_sha256": content_hash(output["notes"]),
                }
            )
    if not changes:
        raise ValueError("No instrument Lead/Melody clips found")
    before = content_hash(preserved_content(source, selected))
    after = content_hash(preserved_content(result, selected))
    if before != after:
        raise AssertionError("Non-melody content changed")
    manifest = {
        "schema_version": 1,
        "replacement": "Complete instrument melody notes and matching recipe text_digest only",
        "preserved_source_sha256": before,
        "preserved_output_sha256": after,
        "backing_notes_sha256": content_hash(
            score_content(result, selected, melody=False)
        ),
        "source_melody_sha256": content_hash(
            score_content(source, selected, melody=True)
        ),
        "output_melody_sha256": content_hash(
            score_content(result, selected, melody=True)
        ),
        "clips": changes,
    }
    return result, manifest


def reject_relative_assets(value: object) -> None:
    if isinstance(value, dict):
        if "inside" in value:
            raise ValueError(
                "Relative assets require collection/relocation; use external assets"
            )
        for child in value.values():
            reject_relative_assets(child)
    elif isinstance(value, list):
        for child in value:
            reject_relative_assets(child)


def write_comparison(
    source: Path,
    candidate: Path,
    output: Path,
    *,
    cli: Path | None = None,
    wav: Path | None = None,
) -> Path:
    """Write a fresh project and manifest, optionally rendering without an effect tail."""
    source, candidate, output = (p.resolve() for p in (source, candidate, output))
    if output.suffix.lower() != ".auris":
        raise ValueError("Output must be an .auris project")
    manifest_path = output.with_suffix(".melody-ab.json")
    if not cli and wav:
        raise ValueError("--wav requires --cli")
    wav = (wav or output.with_suffix(".wav")).resolve() if cli else None
    destinations = [output, manifest_path] + ([wav] if wav else [])
    if len(set(destinations)) != len(destinations):
        raise ValueError("Output paths must be distinct")
    for path in destinations:
        if path in {source, candidate} or path.exists():
            raise ValueError(f"Refusing to overwrite: {path}")
    if any(output.parent.glob("*.auris")):
        raise ValueError("Use a separate output project folder")
    old = json.loads(source.read_text(encoding="utf-8"))
    new = json.loads(candidate.read_text(encoding="utf-8"))
    renderer_hash = file_hash(cli) if cli else None
    reject_relative_assets(old)
    result, manifest = replace_melody(old, new)
    manifest["inputs"] = {
        "source": {"path": str(source), "sha256": file_hash(source)},
        "candidate": {"path": str(candidate), "sha256": file_hash(candidate)},
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("x", encoding="utf-8", newline="\n") as stream:
        json.dump(result, stream, ensure_ascii=False, indent=2, allow_nan=False)
        stream.write("\n")
    manifest["output"] = {"path": str(output), "sha256": file_hash(output)}
    if cli:
        wav.parent.mkdir(parents=True, exist_ok=True)
        command = [
            str(cli.resolve()),
            "render",
            str(output),
            "--bit-depth",
            "32",
            "--no-tail",
            "--output",
            str(wav),
        ]
        done = subprocess.run(
            command,
            capture_output=True,
            encoding="utf-8",
            errors="replace",
            check=False,
        )
        manifest["render"] = {
            "command": command,
            "cli_sha256": renderer_hash,
            "returncode": done.returncode,
            "stdout": done.stdout,
            "stderr": done.stderr,
        }
        if done.returncode == 0 and wav.is_file():
            manifest["render"]["wav_sha256"] = file_hash(wav)
    with manifest_path.open("x", encoding="utf-8", newline="\n") as stream:
        json.dump(manifest, stream, ensure_ascii=False, indent=2, allow_nan=False)
        stream.write("\n")
    if cli and (done.returncode != 0 or not wav.is_file()):
        raise RuntimeError(f"CLI render failed; see {manifest_path}")
    return manifest_path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("source", "candidate", "output"):
        parser.add_argument(f"--{name}", required=True, type=Path)
    parser.add_argument(
        "--cli", type=Path, help="Existing auris executable; also render WAV"
    )
    parser.add_argument(
        "--wav", type=Path, help="Fresh WAV path (default: beside output)"
    )
    args = parser.parse_args()
    if args.wav and not args.cli:
        parser.error("--wav requires --cli")
    try:
        print(
            write_comparison(
                args.source, args.candidate, args.output, cli=args.cli, wav=args.wav
            )
        )
    except (ValueError, OSError, KeyError, RuntimeError) as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
