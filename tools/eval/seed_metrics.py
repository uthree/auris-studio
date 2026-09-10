# /// script
# requires-python = ">=3.10"
# dependencies = []
# ///
"""Symbolic seed diversity in an eight-bar instrumental lead chorus.

    uv run tools/eval/seed_metrics.py --manifest target/seed-diversity/manifest.json \\
        --json target/seed-diversity/symbolic.json

Onsets retain their phase in the bar. Nearest-sixteenth quantization removes small swing
and timing deviations; it does not shift a phrase to its first note. Pitch intervals and
relative pitches retain octaves and remove only a uniform transposition. Pairwise Jaccard
scores below compare the whole chorus, including absolute position within its eight bars.
These measurements describe differences, not catchiness or musical quality.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import statistics
from collections import defaultdict
from itertools import combinations, pairwise
from pathlib import Path

SCHEMA_VERSION = 1
GRID_TICKS = 240
BAR_TICKS = 3840
CHORUS_BARS = 8


class InvalidProjectError(ValueError):
    """A saved score or corpus cannot support the specified comparison."""


def _integer(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def _finite(value: object) -> bool:
    return (
        isinstance(value, (float, int))
        and not isinstance(value, bool)
        and math.isfinite(value)
    )


def _hash(value: object) -> str:
    try:
        text = json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False)
    except (TypeError, ValueError) as error:
        raise InvalidProjectError("project must contain finite JSON values") from error
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def _points(project: dict, field: str) -> list[dict]:
    value = project.get(field)
    if not isinstance(value, dict) or not isinstance(value.get("points"), list):
        raise InvalidProjectError(f"missing or malformed {field} map")
    points = value["points"]
    if not points or any(
        not isinstance(point, dict)
        or not _integer(point.get("tick"))
        or point["tick"] < 0
        for point in points
    ):
        raise InvalidProjectError(f"malformed {field} points")
    if points[0]["tick"] != 0 or any(
        a["tick"] >= b["tick"] for a, b in pairwise(points)
    ):
        raise InvalidProjectError(f"{field} must start at zero and be strictly ordered")
    return points


def _window(project: dict) -> tuple[dict, int, int, float]:
    signatures = _points(project, "signatures")
    if len(signatures) != 1 or signatures[0].get("signature") != {
        "numerator": 4,
        "denominator": 4,
    }:
        raise InvalidProjectError("seed comparison requires one fixed 4/4 signature")
    tempos = _points(project, "tempo_map")
    if len(tempos) != 1 or not _finite(tempos[0].get("bpm")) or tempos[0]["bpm"] <= 0:
        raise InvalidProjectError("seed comparison requires one fixed positive tempo")
    sections = _points(project, "sections")
    for index, section in enumerate(sections):
        label = section.get("label")
        if isinstance(label, str) and re.fullmatch(
            r"chorus(?:\s*\d+)?", label.strip(), re.IGNORECASE
        ):
            if index + 1 == len(sections):
                raise InvalidProjectError(
                    "first chorus has no explicit ending boundary"
                )
            start, end = section["tick"], sections[index + 1]["tick"]
            if start % BAR_TICKS or end - start != CHORUS_BARS * BAR_TICKS:
                raise InvalidProjectError(
                    "first chorus must cover exactly eight complete bars"
                )
            return section, start, end, float(tempos[0]["bpm"])
    raise InvalidProjectError("project has no first chorus section")


def _lead_clip(project: dict, start: int, end: int) -> tuple[dict, dict]:
    tracks = project.get("tracks")
    if not isinstance(tracks, list):
        raise InvalidProjectError("missing or malformed tracks")
    candidates = []
    for track in tracks:
        if not isinstance(track, dict) or not isinstance(track.get("kind"), dict):
            raise InvalidProjectError("malformed track")
        kind = track["kind"]
        if kind.get("type") != "instrument":
            continue
        if not isinstance(kind.get("clips"), list):
            raise InvalidProjectError("instrument track has no clip list")
        for clip in kind["clips"]:
            if not isinstance(clip, dict):
                raise InvalidProjectError("malformed instrument clip")
            recipe = clip.get("recipe")
            if not isinstance(recipe, dict) or recipe.get("preset") != "lead":
                continue
            at, length = clip.get("start"), clip.get("length")
            if not _integer(at) or not _integer(length) or at < 0 or length <= 0:
                raise InvalidProjectError("lead clip has invalid bounds")
            if at < end and at + length > start:
                candidates.append((track, clip))
    if len(candidates) != 1:
        raise InvalidProjectError(
            "first chorus requires exactly one overlapping recipe=lead clip"
        )
    track, clip = candidates[0]
    mixer = track.get("mixer", {})
    if (
        not isinstance(mixer, dict)
        or mixer.get("mute", False)
        or clip.get("muted", False)
    ):
        raise InvalidProjectError(
            "first chorus lead is muted or has malformed mixer state"
        )
    if clip["start"] > start or clip["start"] + clip["length"] < end:
        raise InvalidProjectError("lead clip does not span the complete first chorus")
    return track, clip


def _notes(clip: dict, start: int, end: int) -> list[list]:
    stored = clip.get("notes")
    if not isinstance(stored, list):
        raise InvalidProjectError("lead clip has no note list")
    selected = []
    for note in stored:
        if not isinstance(note, dict):
            raise InvalidProjectError("malformed lead note")
        at, duration = note.get("start"), note.get("length")
        pitch, velocity = note.get("pitch"), note.get("velocity")
        if not (
            _integer(at)
            and _integer(duration)
            and at >= 0
            and duration > 0
            and at + duration <= clip["length"]
            and _integer(pitch)
            and 0 <= pitch <= 127
            and _finite(velocity)
            and 0 <= velocity <= 1
        ):
            raise InvalidProjectError(
                "lead note has invalid bounds, pitch, or velocity"
            )
        absolute = clip["start"] + at
        if absolute < start < absolute + duration:
            raise InvalidProjectError(
                "lead note crosses the first chorus opening boundary"
            )
        if start <= absolute < end:
            if absolute + duration > end:
                raise InvalidProjectError(
                    "lead note crosses the first chorus ending boundary"
                )
            selected.append([absolute - start, duration, pitch, velocity])
    selected.sort(key=lambda note: (note[0], note[2]))
    if not selected or len({note[0] for note in selected}) != len(selected):
        raise InvalidProjectError("lead chorus must contain notes with distinct onsets")
    return selected


def _quantize(tick: int) -> int:
    return ((tick + GRID_TICKS // 2) // GRID_TICKS) * GRID_TICKS


def _signature(notes: list[list[int]]) -> dict:
    pitches = [note[2] for note in notes]
    return {
        "onset_duration": [[note[0], note[1]] for note in notes],
        "intervals": [b - a for a, b in pairwise(pitches)],
        "relative_pitches": [pitch - pitches[0] for pitch in pitches],
    }


def analyze_project(project: dict) -> dict:
    """Measure one validated, fixed-meter, eight-bar first chorus; reject unsupported inputs."""
    if not isinstance(project, dict):
        raise InvalidProjectError("project must be a dictionary")
    project_hash = _hash(project)
    section, start, end, bpm = _window(project)
    track, clip = _lead_clip(project, start, end)
    raw = _notes(clip, start, end)
    quantized = [[_quantize(note[0]), _quantize(note[1]), note[2]] for note in raw]
    if any(
        at + duration > end - start or duration <= 0 for at, duration, _ in quantized
    ) or len({note[0] for note in quantized}) != len(quantized):
        raise InvalidProjectError(
            "quantization collapses notes or moves them outside the chorus"
        )
    first = [note for note in quantized if note[0] < BAR_TICKS]
    if not first:
        raise InvalidProjectError("first chorus bar contains no lead notes")
    bars = [
        {
            "onset_duration": [
                [at - bar * BAR_TICKS, duration]
                for at, duration, _ in quantized
                if bar * BAR_TICKS <= at < (bar + 1) * BAR_TICKS
            ],
            "note_count": sum(
                bar * BAR_TICKS <= note[0] < (bar + 1) * BAR_TICKS for note in quantized
            ),
        }
        for bar in range(CHORUS_BARS)
    ]
    rhythms = [tuple(map(tuple, bar["onset_duration"])) for bar in bars]
    errors = [[abs(a[0] - b[0]), abs(a[1] - b[1])] for a, b in zip(raw, quantized)]
    return {
        "schema_version": SCHEMA_VERSION,
        "region": {
            "section_label": section["label"],
            "start_tick": start,
            "end_tick": end,
            "bars": CHORUS_BARS,
            "bpm": bpm,
            "track_id": track.get("id"),
            "clip_id": clip.get("id"),
        },
        "quantization": {
            "step_ticks": GRID_TICKS,
            "rounding": "nearest_half_up",
            "max_onset_error_ticks": max(error[0] for error in errors),
            "max_duration_error_ticks": max(error[1] for error in errors),
            "mean_abs_onset_error_ticks": statistics.mean(error[0] for error in errors),
            "mean_abs_duration_error_ticks": statistics.mean(
                error[1] for error in errors
            ),
            "changed_onset_count": sum(error[0] != 0 for error in errors),
            "changed_duration_count": sum(error[1] != 0 for error in errors),
        },
        "hashes": {
            "project_sha256": project_hash,
            "raw_notes_sha256": _hash(raw),
            "quantized_notes_sha256": _hash(quantized),
        },
        "note_count": len(quantized),
        "first_bar_note_count": len(first),
        "first_bar": _signature(first),
        "chorus": _signature(quantized),
        "bars": bars,
        "within_chorus": {
            "unique_rhythm_count": len(set(rhythms)),
            "empty_bar_count": sum(not rhythm for rhythm in rhythms),
            "duplicate_pair_count": sum(a == b for a, b in combinations(rhythms, 2)),
            "pair_count": math.comb(CHORUS_BARS, 2),
            "adjacent_repeat_count": sum(a == b for a, b in pairwise(rhythms)),
            "adjacent_pair_count": CHORUS_BARS - 1,
        },
    }


def _frozen(signature: object) -> str:
    return json.dumps(signature, separators=(",", ":"), allow_nan=False)


def _jaccard(a: set, b: set) -> float:
    return len(a & b) / len(a | b) if a or b else 1.0


def _comparison(a: dict, b: dict) -> dict:
    a, b = a["symbolic"], b["symbolic"]
    onsets_a = {note[0] for note in a["chorus"]["onset_duration"]}
    onsets_b = {note[0] for note in b["chorus"]["onset_duration"]}
    rhythm_a = set(map(tuple, a["chorus"]["onset_duration"]))
    rhythm_b = set(map(tuple, b["chorus"]["onset_duration"]))
    return {
        "first_bar_rhythm_equal": a["first_bar"]["onset_duration"]
        == b["first_bar"]["onset_duration"],
        "chorus_rhythm_equal": a["chorus"]["onset_duration"]
        == b["chorus"]["onset_duration"],
        "first_bar_intervals_equal": a["first_bar"]["intervals"]
        == b["first_bar"]["intervals"],
        "chorus_intervals_equal": a["chorus"]["intervals"] == b["chorus"]["intervals"],
        "onset_jaccard": _jaccard(onsets_a, onsets_b),
        "onset_duration_jaccard": _jaccard(rhythm_a, rhythm_b),
    }


def _distribution(values: list[float]) -> dict:
    return {
        "min": min(values) if values else None,
        "max": max(values) if values else None,
        "mean": statistics.mean(values) if values else None,
        "median": statistics.median(values) if values else None,
    }


def corpus_summary(files: dict) -> dict:
    """Compare seeds within each genre and matched seeds across genres, retaining every pair."""
    if not isinstance(files, dict) or not files:
        raise InvalidProjectError("corpus must contain named project rows")
    grouped = defaultdict(list)
    seen = set()
    for name, row in files.items():
        if (
            not isinstance(row, dict)
            or not isinstance(row.get("preset"), str)
            or not row["preset"]
            or not _integer(row.get("seed"))
            or row["seed"] < 0
            or not isinstance(row.get("symbolic"), dict)
            or row["symbolic"].get("schema_version") != SCHEMA_VERSION
        ):
            raise InvalidProjectError(f"invalid corpus row {name!r}")
        identity = row["preset"], row["seed"]
        if identity in seen:
            raise InvalidProjectError(f"duplicate preset/seed row: {identity}")
        seen.add(identity)
        grouped[row["preset"]].append(row)
    per_genre = {}
    for preset, rows in sorted(grouped.items()):
        rows.sort(key=lambda row: row["seed"])
        pairs = [
            {"seed_a": a["seed"], "seed_b": b["seed"], **_comparison(a, b)}
            for a, b in combinations(rows, 2)
        ]
        result = {
            "seed_count": len(rows),
            "seeds": [row["seed"] for row in rows],
            "pair_count": len(pairs),
        }
        for region in ("first_bar", "chorus"):
            for label, field in (
                ("rhythm", "onset_duration"),
                ("interval", "intervals"),
            ):
                signatures = [_frozen(row["symbolic"][region][field]) for row in rows]
                result[f"unique_{region}_{label}_count"] = len(set(signatures))
                result[f"duplicate_{region}_{label}_pair_count"] = sum(
                    a == b for a, b in combinations(signatures, 2)
                )
        result["pairs"] = pairs
        for field in ("onset_jaccard", "onset_duration_jaccard"):
            result[field] = _distribution([pair[field] for pair in pairs])
        per_genre[preset] = result
    cross_pairs = []
    for preset_a, preset_b in combinations(sorted(grouped), 2):
        rows_a = {row["seed"]: row for row in grouped[preset_a]}
        rows_b = {row["seed"]: row for row in grouped[preset_b]}
        for seed in sorted(rows_a.keys() & rows_b.keys()):
            cross_pairs.append(
                {
                    "preset_a": preset_a,
                    "preset_b": preset_b,
                    "seed": seed,
                    **_comparison(rows_a[seed], rows_b[seed]),
                }
            )
    cross = {"pair_count": len(cross_pairs), "pairs": cross_pairs}
    for region in ("first_bar", "chorus"):
        for label, field in (("rhythm", "rhythm"), ("interval", "intervals")):
            cross[f"{region}_{label}_collision_pair_count"] = sum(
                pair[f"{region}_{field}_equal"] for pair in cross_pairs
            )
    return {
        "schema_version": SCHEMA_VERSION,
        "project_count": len(files),
        "per_genre": per_genre,
        "cross_genre_same_seed": cross,
    }


def analyze_manifest(manifest_path: Path, output_path: Path) -> dict:
    """Verify every source hash and enrich a complete generation manifest in a new file.

    Relative project paths resolve against the manifest's directory. Existing output files
    are refused, so neither the generation manifest nor any source score can be overwritten.
    """
    manifest_path, output_path = manifest_path.resolve(), output_path.resolve()
    if output_path == manifest_path or output_path.exists():
        raise InvalidProjectError(
            "output must be a new file, separate from the input manifest"
        )
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if (
        not isinstance(manifest, dict)
        or manifest.get("schema_version") != SCHEMA_VERSION
    ):
        raise InvalidProjectError("expected generation manifest schema_version=1")
    files, presets, seeds = (
        manifest.get("files"),
        manifest.get("presets"),
        manifest.get("seeds"),
    )
    if (
        not isinstance(files, dict)
        or not files
        or not isinstance(presets, list)
        or not presets
        or any(not isinstance(preset, str) or not preset for preset in presets)
        or not isinstance(seeds, list)
        or not seeds
        or any(not _integer(seed) or seed < 0 for seed in seeds)
    ):
        raise InvalidProjectError(
            "manifest requires nonempty files, presets, and seeds"
        )
    if len(set(presets)) != len(presets) or len(set(seeds)) != len(seeds):
        raise InvalidProjectError("manifest presets and seeds must be unique")
    observed = set()
    for name, row in files.items():
        if not isinstance(row, dict) or not isinstance(row.get("project"), dict):
            raise InvalidProjectError(f"manifest row {name!r} has no project artifact")
        artifact = row["project"]
        stored_path, expected = artifact.get("path"), artifact.get("sha256")
        if (
            not isinstance(stored_path, str)
            or not stored_path
            or not isinstance(expected, str)
            or not re.fullmatch(r"[0-9a-fA-F]{64}", expected)
            or not isinstance(row.get("preset"), str)
            or not _integer(row.get("seed"))
        ):
            raise InvalidProjectError(
                f"manifest row {name!r} has malformed project metadata"
            )
        path = Path(stored_path)
        if not path.is_absolute():
            path = manifest_path.parent / path
        data = path.read_bytes()
        if hashlib.sha256(data).hexdigest() != expected.lower():
            raise InvalidProjectError(f"project SHA256 mismatch for {name!r}: {path}")
        row["symbolic"] = analyze_project(json.loads(data))
        observed.add((row["preset"], row["seed"]))
    if observed != {(preset, seed) for preset in presets for seed in seeds}:
        raise InvalidProjectError(
            "manifest files do not cover exactly the declared preset/seed corpus"
        )
    manifest["diversity"] = corpus_summary(files)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    with output_path.open("x", encoding="utf-8") as stream:
        stream.write(json.dumps(manifest, indent=2, allow_nan=False) + "\n")
    return manifest


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--json", type=Path, required=True)
    args = parser.parse_args()
    try:
        result = analyze_manifest(args.manifest, args.json)
    except (OSError, ValueError) as error:
        parser.error(str(error))
    print(f"Measured {len(result['files'])} projects; wrote {args.json}")


if __name__ == "__main__":
    main()
