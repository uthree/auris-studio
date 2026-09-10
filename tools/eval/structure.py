# /// script
# requires-python = ">=3.10"
# dependencies = []
# ///
"""Descriptive symbolic measurements of saved Auris projects, without a quality score.

    uv run tools/eval/structure.py before/ --json before-structure.json
    uv run tools/eval/structure.py after/ --baseline before-structure.json --json after.json

Measures written notes, not rendered audio or performance transforms. Muted clips/tracks are
excluded from musical measurements; validation and role counts include all stored notes.
Loops, routing, solo, audio clips, articulation and microtiming are not simulated. Role names
come from clip recipes, singer/drum track kinds, then other clips on that track or known names.

Rhythm compares onset sets in complete, nonempty bars within each foreground clip. The unique
fraction is summed per clip, so copying a section does not masquerade as a new melodic idea.
Leaps are at least five semitones, measured only between adjacent monophonic onset groups
whose notes touch or overlap; rests and clip boundaries are never bridged. Foreground is the
union of active lead and actual vocal notes. Support means chords/stab/arp roles, excluding pads.
An attack collision is an exact shared tick; duration overlap merges polyphonic intervals
within each support track before weighting by sounding time. Neither overlap nor repetition is
intrinsically undesirable. Voicing motion matches ordered pitch sets at successive distinct
attacks within a chords/stab clip; entering/leaving voices are counted separately. Arpeggios
participate in support overlap, but their sequential notes are not treated as voicings. These are minimum
possible motions, not inferred player identities. Undefined ratios are null, never zero.

Inputs pair by unique filename stem; '<preset>-s<number>' stems also group into preset means.
Means weight each project equally and omit undefined ratios. Use the paired deltas and their
observation counts to select passages for listening, not to maximize or minimize all columns.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import statistics
from collections import Counter, defaultdict
from dataclasses import dataclass
from itertools import pairwise
from pathlib import Path

SCHEMA_VERSION = 1
TICKS_PER_QUARTER = 960
PARAMETERS = {"large_leap_semitones": 5, "melody_max_gap_ticks": 0}
FOREGROUND = {"lead", "vocal"}
VOICED_COMP = {"chords", "stab"}
SUPPORT = VOICED_COMP | {"arp"}
ROLES = (
    FOREGROUND
    | SUPPORT
    | {"pad", "arp", "bass", "drums", "kick", "snare", "hat", "crash", "riser"}
)
ALIASES = {
    "melody": "lead",
    "voice": "vocal",
    "keys": "chords",
    "comp": "chords",
    "strings": "pad",
}


@dataclass(frozen=True)
class Note:
    """One validated note at its absolute project position."""

    start: int
    end: int
    pitch: int


@dataclass
class Clip:
    """An active note clip, retaining its part and phrase boundaries."""

    track: int
    role: str
    start: int
    end: int
    notes: list[Note]


def finite(value: object) -> bool:
    return (
        isinstance(value, (float, int))
        and not isinstance(value, bool)
        and math.isfinite(value)
    )


def integer(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def nonfinite_count(value: object) -> int:
    if isinstance(value, dict):
        return sum(nonfinite_count(item) for item in value.values())
    if isinstance(value, list):
        return sum(nonfinite_count(item) for item in value)
    return int(isinstance(value, float) and not math.isfinite(value))


def role_name(value: str) -> str:
    name = value.strip().lower()
    return ALIASES.get(name, name) if name in ROLES or name in ALIASES else "unknown"


def read_clips(project: dict) -> tuple[list[Clip], dict[str, int], dict[str, int]]:
    """Validate stored notes once, then extract active clips without expanding playback."""
    clips = []
    counts: Counter[str] = Counter()
    violations = {
        "invalid_clip_count": 0,
        "invalid_note_count": 0,
        "nonfinite_values": nonfinite_count(project),
    }
    for index, track in enumerate(project.get("tracks", [])):
        kind = track.get("kind", {})
        if kind.get("type") not in {"instrument", "drum", "singer"}:
            continue
        stored = kind.get("clips", [])
        named = [
            role_name((clip.get("recipe") or {}).get("preset", "")) for clip in stored
        ]
        known = Counter(name for name in named if name != "unknown")
        fallback = (
            known.most_common(1)[0][0] if known else role_name(track.get("name", ""))
        )
        for clip, named_role in zip(stored, named):
            role = named_role if named_role != "unknown" else fallback
            if kind.get("type") == "singer":
                role = "vocal"
            elif kind.get("type") == "drum":
                role = "drums"
            start, length = clip.get("start"), clip.get("length")
            valid_clip = (
                integer(start) and integer(length) and start >= 0 and length > 0
            )
            violations["invalid_clip_count"] += int(not valid_clip)
            active = not clip.get("muted", False) and not track.get("mixer", {}).get(
                "mute", False
            )
            notes = []
            for note in clip.get("notes", []):
                voice = (
                    role_name(note.get("drum_voice", "")) if role == "drums" else role
                )
                counts[voice if voice != "unknown" else role] += 1
                at, duration = note.get("start"), note.get("length")
                pitch, velocity = note.get("pitch"), note.get("velocity")
                valid = (
                    valid_clip
                    and integer(at)
                    and integer(duration)
                    and at >= 0
                    and duration > 0
                    and at + duration <= length
                    and integer(pitch)
                    and 0 <= pitch <= 127
                    and finite(velocity)
                    and 0 <= velocity <= 1
                )
                violations["invalid_note_count"] += int(not valid)
                if valid and active:
                    notes.append(Note(start + at, start + at + duration, pitch))
            if valid_clip and active:
                clips.append(
                    Clip(
                        index,
                        role,
                        start,
                        start + length,
                        sorted(notes, key=lambda n: (n.start, n.pitch)),
                    )
                )
    return clips, dict(sorted(counts.items())), violations


def merge_intervals(intervals: list[tuple[int, int]]) -> list[tuple[int, int]]:
    """Union sounding intervals so a chord's simultaneous voices count only once."""
    result: list[tuple[int, int]] = []
    for start, end in sorted(intervals):
        if result and start <= result[-1][1]:
            result[-1] = (result[-1][0], max(end, result[-1][1]))
        elif end > start:
            result.append((start, end))
    return result


def intersection_length(a: list[tuple[int, int]], b: list[tuple[int, int]]) -> int:
    total = i = j = 0
    while i < len(a) and j < len(b):
        total += max(0, min(a[i][1], b[j][1]) - max(a[i][0], b[j][0]))
        if a[i][1] <= b[j][1]:
            i += 1
        else:
            j += 1
    return total


def voice_motion(before: list[int], after: list[int]) -> tuple[int, int]:
    """Minimum ordered matching cost and matched-voice count, with unmatched voices omitted."""
    shorter, longer = sorted((sorted(set(before)), sorted(set(after))), key=len)
    if not shorter:
        return 0, 0
    costs = [0.0] * (len(longer) + 1)
    for pitch in shorter:
        row = [math.inf] * (len(longer) + 1)
        for index, other in enumerate(longer):
            row[index + 1] = min(row[index], costs[index] + abs(pitch - other))
        costs = row
    return int(costs[-1]), len(shorter)


def full_bars(project: dict, clip: Clip) -> list[tuple[int, int]]:
    """Complete bars on the project's signature map, excluding partial clip edges."""
    points = project.get("signatures", {}).get("points", [])
    signatures = []
    for point in points:
        tick, signature = point.get("tick"), point.get("signature", {})
        num, den = signature.get("numerator"), signature.get("denominator")
        if (
            integer(tick)
            and tick >= 0
            and integer(num)
            and integer(den)
            and num > 0
            and den > 0
        ):
            size = TICKS_PER_QUARTER * 4 * num // den
            if size > 0:
                signatures.append((tick, size))
    signatures.sort()
    if not signatures or signatures[0][0] > 0:
        signatures.insert(0, (0, TICKS_PER_QUARTER * 4))
    bars = []
    for index, (origin, size) in enumerate(signatures):
        end = (
            min(clip.end, signatures[index + 1][0])
            if index + 1 < len(signatures)
            else clip.end
        )
        first = origin + max(0, (clip.start - origin + size - 1) // size) * size
        bars.extend(
            (start, start + size) for start in range(first, end - size + 1, size)
        )
    return bars


def ratio(numerator: int, denominator: int) -> float | None:
    return numerator / denominator if denominator else None


def measure(project: dict) -> tuple[dict, dict[str, int]]:
    """Return interpretable measurements plus stored note counts by recognized role."""
    clips, note_counts, violations = read_clips(project)
    foreground = [clip for clip in clips if clip.role in FOREGROUND]
    fg_notes = [note for clip in foreground for note in clip.notes]
    fg_onsets = {note.start for note in fg_notes}
    fg_intervals = merge_intervals([(note.start, note.end) for note in fg_notes])
    repeated = adjacent = unique = bar_count = leaps = melodic_pairs = 0
    for clip in foreground:
        patterns = []
        for start, end in full_bars(project, clip):
            pattern = tuple(
                sorted(
                    {
                        note.start - start
                        for note in clip.notes
                        if start <= note.start < end
                    }
                )
            )
            if pattern:
                patterns.append((start, end, pattern))
        unique += len({(end - start, pattern) for start, end, pattern in patterns})
        bar_count += len(patterns)
        for before, after in pairwise(patterns):
            if before[1] == after[0] and before[1] - before[0] == after[1] - after[0]:
                adjacent += 1
                repeated += int(before[2] == after[2])
        groups: dict[int, list[Note]] = defaultdict(list)
        for note in clip.notes:
            groups[note.start].append(note)
        ordered = [groups[at] for at in sorted(groups)]
        for before, after in pairwise(ordered):
            if len(before) == len(after) == 1 and after[0].start <= before[0].end:
                melodic_pairs += 1
                leaps += int(
                    abs(after[0].pitch - before[0].pitch)
                    >= PARAMETERS["large_leap_semitones"]
                )

    support_tracks: dict[int, list[Note]] = defaultdict(list)
    motion = matched = changes = entering_leaving = 0
    for clip in clips:
        if clip.role not in SUPPORT:
            continue
        support_tracks[clip.track].extend(clip.notes)
        if clip.role not in VOICED_COMP:
            continue
        groups: dict[int, set[int]] = defaultdict(set)
        for note in clip.notes:
            groups[note.start].add(note.pitch)
        ordered = [groups[at] for at in sorted(groups)]
        for before, after in pairwise(ordered):
            if before != after:
                cost, count = voice_motion(list(before), list(after))
                motion += cost
                matched += count
                changes += 1
                entering_leaving += abs(len(before) - len(after))
    attacks = collisions = sounding = overlap = 0
    for notes in support_tracks.values():
        onsets = {note.start for note in notes}
        attacks += len(onsets)
        collisions += len(onsets & fg_onsets)
        intervals = merge_intervals([(note.start, note.end) for note in notes])
        sounding += sum(end - start for start, end in intervals)
        overlap += intersection_length(intervals, fg_intervals)
    metrics = {
        "foreground_adjacent_bar_rhythm_repeat_fraction": ratio(repeated, adjacent),
        "foreground_bar_rhythm_unique_fraction": ratio(unique, bar_count),
        "foreground_nonempty_bars": bar_count,
        "foreground_adjacent_bar_pairs": adjacent,
        "melody_large_leap_fraction": ratio(leaps, melodic_pairs),
        "melody_continuous_note_pairs": melodic_pairs,
        "support_foreground_onset_collision_fraction": ratio(collisions, attacks)
        if fg_notes
        else None,
        "support_foreground_duration_overlap_fraction": ratio(overlap, sounding)
        if fg_notes
        else None,
        "support_attack_count": attacks,
        "support_sounding_ticks": sounding,
        "comp_voice_motion_semitones": ratio(motion, matched),
        "comp_voicing_changes": changes,
        "comp_matched_voice_count": matched,
        "comp_entering_leaving_voice_count": entering_leaving,
        "active_note_count": sum(len(clip.notes) for clip in clips),
        **violations,
    }
    return metrics, note_counts


def preset_name(label: str) -> str:
    return re.sub(r"-s\d+$", "", label)


def preset_means(projects: dict) -> dict:
    groups: dict[str, list[dict]] = defaultdict(list)
    for label, project in projects.items():
        groups[preset_name(label)].append(project)
    result = {}
    for preset, rows in sorted(groups.items()):
        keys = sorted({key for row in rows for key in row["metrics"]})
        metrics = {}
        for key in keys:
            values = [
                row["metrics"][key]
                for row in rows
                if row["metrics"].get(key) is not None
            ]
            metrics[key] = statistics.fmean(values) if values else None
        roles = sorted({role for row in rows for role in row["note_counts_by_role"]})
        result[preset] = {
            "projects": len(rows),
            "metrics": metrics,
            "note_counts_by_role": {
                role: statistics.fmean(
                    row["note_counts_by_role"].get(role, 0) for row in rows
                )
                for role in roles
            },
        }
    return result


def build_report(paths: list[Path]) -> dict:
    projects = {}
    for path in paths:
        label = path.stem
        if label in projects:
            raise ValueError(
                f"duplicate project label {label!r}; use unique project filenames"
            )
        raw = path.read_bytes()
        project = json.loads(raw)
        metrics, counts = measure(project)
        projects[label] = {
            "source": str(path.resolve()),
            "sha256": hashlib.sha256(raw).hexdigest(),
            "project_format_version": project.get("format_version"),
            "metrics": metrics,
            "note_counts_by_role": counts,
        }
    return {
        "schema_version": SCHEMA_VERSION,
        "parameters": PARAMETERS,
        "projects": projects,
        "preset_means": preset_means(projects),
    }


def comparison(current: dict, baseline: dict) -> dict:
    """Pair only matching labels and report signed current-minus-baseline changes."""
    if (
        baseline.get("schema_version") != SCHEMA_VERSION
        or baseline.get("parameters") != current["parameters"]
    ):
        raise ValueError("baseline structure schema or measurement parameters differ")
    before, after = baseline["projects"], current["projects"]
    paired = {}
    for label in sorted(before.keys() & after.keys()):
        old, new = before[label], after[label]
        metrics = {
            key: value - old["metrics"][key]
            if value is not None and old["metrics"].get(key) is not None
            else None
            for key, value in new["metrics"].items()
        }
        roles = old["note_counts_by_role"].keys() | new["note_counts_by_role"].keys()
        counts = {
            role: new["note_counts_by_role"].get(role, 0)
            - old["note_counts_by_role"].get(role, 0)
            for role in sorted(roles)
        }
        paired[label] = {
            "before_sha256": old["sha256"],
            "after_sha256": new["sha256"],
            "metrics": metrics,
            "note_counts_by_role": counts,
        }
    return {
        "paired_projects": paired,
        "preset_mean_deltas": preset_means(paired),
        "missing_from_current": sorted(before.keys() - after.keys()),
        "missing_from_baseline": sorted(after.keys() - before.keys()),
    }


def main() -> None:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "paths",
        nargs="+",
        type=Path,
        help=".auris files or folders to search recursively",
    )
    parser.add_argument(
        "--json", type=Path, help="write the complete report here (stdout otherwise)"
    )
    parser.add_argument(
        "--baseline", type=Path, help="compare matching labels in an earlier report"
    )
    args = parser.parse_args()
    paths = []
    for path in args.paths:
        if path.is_dir():
            paths.extend(sorted(path.rglob("*.auris")))
        elif path.is_file() and path.suffix.lower() == ".auris":
            paths.append(path)
        else:
            parser.error(f"not an .auris file or folder: {path}")
    paths = list(dict.fromkeys(path.resolve() for path in paths))
    if not paths:
        parser.error("no .auris projects found")
    try:
        report = build_report(paths)
        if args.baseline:
            raw = args.baseline.read_bytes()
            report["comparison"] = {
                "baseline_source": str(args.baseline.resolve()),
                "baseline_sha256": hashlib.sha256(raw).hexdigest(),
                **comparison(report, json.loads(raw)),
            }
    except (OSError, ValueError, KeyError, TypeError) as error:
        parser.error(str(error))
    text = json.dumps(report, indent=2, allow_nan=False) + "\n"
    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(text, encoding="utf-8")
    else:
        print(text, end="")


if __name__ == "__main__":
    main()
