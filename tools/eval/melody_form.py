# /// script
# requires-python = ">=3.10"
# dependencies = []
# ///
"""Line-level form, rhythm, cadence and predictability of a saved song, without a quality score.

    uv run tools/eval/melody_form.py MySong/MySong.auris --json form.json
    uv run tools/eval/melody_form.py runs/ --track 歌

Reads written notes from saved `.auris` projects. The foreground is a singer track (or the one
named by --track), cut into lines at the lines of the song-wide lyric annotation, at clip
boundaries and at rests of a beat or more; a line still longer than four bars is cut again at its
longest rest. For every line it reports where it
enters in the bar, its onset positions, syncopated and anticipated notes, the spacing of its
onsets, the scale degree and chord it ends on, its contour type and how predictable it is from
the song so far. Lines with the same onsets and pitches share a letter; the same onsets under
other pitches add a prime. Section summaries add each section's chord progression and main drum
groove, and strummed tracks report the spread their strum stage actually gives a chord.

Predictability is an IDyOM-style short-term model: a PPM model (escape method C) over
(interval, inter-onset interval) symbols that learns only from the song itself, predicting each
note before seeing it. Its information content is -log2 p in bits per note. A section that
repeats an earlier line exactly falls towards zero; a new idea is high. It is not a corpus
model and says nothing about how conventional a line is within a genre.

Flags mark extremes only, the kind no song would choose by accident: every line of a section
the same, nine lines in ten entering at one position, no line entering in the second half of a
bar, no syncopation at all, no line ending on the
tonic. None of them is a target to optimise, and an absent flag is not a judgement. Loops,
performance timing other than strum spread, and audio are not simulated.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from itertools import pairwise
from pathlib import Path

SCHEMA_VERSION = 1
TICKS_PER_QUARTER = 960
PARAMETERS = {
    "line_gap_ticks": TICKS_PER_QUARTER,
    "long_line_bars": 4,
    "split_rest_ticks": TICKS_PER_QUARTER // 2,
    "contour_tolerance_semitones": 0.5,
    "ppm_order": 4,
    "identical_section_min_lines": 3,
    "uniform_min_lines": 8,
    "uniform_entry_share": 0.9,
    "uniform_ioi_share": 0.82,
    "uniform_ioi_min_count": 50,
    "plateau_bits": 1.0,
    "plateau_min_lines": 4,
    "shared_groove_min_sections": 3,
    "tight_strum_ms": 10.0,
    "strummed_min_chords": 8,
}
PITCH_CLASSES = {"C": 0, "D": 2, "E": 4, "F": 5, "G": 7, "A": 9, "B": 11}
DEGREES = ["1", "b2", "2", "b3", "3", "4", "#4", "5", "b6", "6", "b7", "7"]
CHORUS_WORDS = ("サビ", "chorus", "hook", "refrain")
VERSE_WORDS = ("aメロ", "verse")
GUITAR_WORDS = ("guitar", "gtr", "ギター")


@dataclass
class Line:
    """One sung or played phrase of the foreground."""

    start: int
    notes: list[dict]
    section: str | None = None
    letter: str = ""
    features: dict = field(default_factory=dict)


def file_digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def bar_length(signature: dict) -> int:
    return round(
        signature["numerator"] * 4 * TICKS_PER_QUARTER / signature["denominator"]
    )


def beat_length(signature: dict) -> int:
    """The counted beat: a dotted quarter in compound eighths, otherwise the denominator."""
    numerator, denominator = signature["numerator"], signature["denominator"]
    if denominator == 8 and numerator % 3 == 0 and numerator > 3:
        return 3 * TICKS_PER_QUARTER // 2
    return 4 * TICKS_PER_QUARTER // denominator


class Meter:
    """Bar and beat positions from the project's signature map."""

    def __init__(self, project: dict) -> None:
        points = project.get("signatures", {}).get("points") or [
            {"tick": 0, "signature": {"numerator": 4, "denominator": 4}}
        ]
        self.points = sorted(points, key=lambda point: point["tick"])

    def at(self, tick: int) -> dict:
        current = self.points[0]
        for point in self.points:
            if point["tick"] <= tick:
                current = point
        return current

    def bar_start(self, tick: int) -> int:
        point = self.at(tick)
        length = bar_length(point["signature"])
        return point["tick"] + (tick - point["tick"]) // length * length

    def bar_length(self, tick: int) -> int:
        return bar_length(self.at(tick)["signature"])

    def beat_length(self, tick: int) -> int:
        return beat_length(self.at(tick)["signature"])


def spans(
    points: list[dict], value_key: str, end: int
) -> list[tuple[int, int, object]]:
    """Consecutive (start, end, value) spans of a point map; a None value ends a span."""
    ordered = sorted(points, key=lambda point: point["tick"])
    result = []
    for index, point in enumerate(ordered):
        stop = ordered[index + 1]["tick"] if index + 1 < len(ordered) else end
        if point.get(value_key) is not None and stop > point["tick"]:
            result.append((point["tick"], stop, point[value_key]))
    return result


def value_at(points: list[dict], value_key: str, tick: int):
    current = None
    for point in sorted(points, key=lambda point: point["tick"]):
        if point["tick"] <= tick:
            current = point.get(value_key)
    return current


def parse_key(name: str | None) -> tuple[int, str] | None:
    """The tonic pitch class and mode of a key name such as 'F# minor'."""
    if not name:
        return None
    match = re.match(r"^\s*([A-Ga-g])([#b♯♭]?)\s+(major|minor)\s*$", name)
    if not match:
        return None
    pitch = PITCH_CLASSES[match.group(1).upper()]
    pitch += {"#": 1, "♯": 1, "b": -1, "♭": -1}.get(match.group(2), 0)
    return pitch % 12, match.group(3)


def section_kind(label: str | None) -> str:
    """A section label without its number or a final marker: サビ2 and 最終サビ are サビ."""
    kind = (label or "").lower()
    for word in ("final", "last", "最終", "ラスト", "ラス"):
        kind = kind.replace(word, "")
    return re.sub(r"[\s0-9０-９]", "", kind)


def is_chorus(label: str | None) -> bool:
    return bool(label) and any(word in label.lower() for word in CHORUS_WORDS)


def is_verse(label: str | None) -> bool:
    return bool(label) and any(word in label.lower() for word in VERSE_WORDS)


def active_clips(track: dict):
    if track.get("mixer", {}).get("mute", False):
        return
    for clip in track.get("kind", {}).get("clips", []) or []:
        if not clip.get("muted", False):
            yield clip


def sounding_length(clip: dict) -> int:
    """How long a clip sounds: its content, or its `loop_end` when that reaches further."""
    return max(clip.get("length", 0), clip.get("loop_end") or 0)


def clip_notes(clip: dict) -> list[dict]:
    """Notes of a clip on the song timeline, once for every pass of a looped clip.

    A note past the end of the content does not play, and the last pass is cut at `loop_end`,
    as `auris_core::loop_passes` cuts it.
    """
    length = clip.get("length", 0)
    total = sounding_length(clip)
    notes = []
    for offset in range(0, total, length) if length > 0 else ():
        visible = min(length, total - offset)
        for note in clip.get("notes", []) or []:
            if note["start"] < visible:
                notes.append(dict(note, at=clip["start"] + offset + note["start"]))
    return sorted(notes, key=lambda note: (note["at"], note["pitch"]))


def pick_foreground(project: dict, wanted: str | None) -> dict | None:
    tracks = project.get("tracks", [])
    if wanted is not None:
        for track in tracks:
            if track.get("name") == wanted or str(track.get("id")) == wanted:
                return track
        raise SystemExit(f"No track named or numbered {wanted!r}")
    singers = [
        track
        for track in tracks
        if track.get("kind", {}).get("type") == "singer"
        and any(clip.get("notes") for clip in active_clips(track))
    ]
    return singers[0] if singers else None


def top_line(notes: list[dict]) -> list[dict]:
    """The highest note at each onset: a melody, not its doubled chord tones."""
    by_onset: dict[int, dict] = {}
    for note in notes:
        current = by_onset.get(note["at"])
        if current is None or note["pitch"] > current["pitch"]:
            by_onset[note["at"]] = note
    return [by_onset[at] for at in sorted(by_onset)]


def split_long(notes: list[dict], meter: Meter) -> list[list[dict]]:
    """`notes` as one line, or cut at its longest rest while it spans more than four bars.

    The rest must be half a beat or more; among equals the one nearest the middle is taken.
    """
    start = notes[0]["at"]
    span = notes[-1]["at"] + notes[-1]["length"] - start
    middle = start + span // 2
    rest, _, index = max(
        (
            (b["at"] - (a["at"] + a["length"]), -abs(b["at"] - middle), index)
            for index, (a, b) in enumerate(pairwise(notes))
        ),
        default=(0, 0, 0),
    )
    if (
        span <= PARAMETERS["long_line_bars"] * meter.bar_length(start)
        or rest < PARAMETERS["split_rest_ticks"]
    ):
        return [notes]
    return [
        *split_long(notes[: index + 1], meter),
        *split_long(notes[index + 1 :], meter),
    ]


def cut_lines(
    notes: list[dict], annotated: list[int], meter: Meter
) -> list[list[dict]]:
    """One part's notes cut at annotation line starts and rests of a beat, long lines again."""
    lines: list[list[dict]] = []
    current: list[dict] = []
    for note in notes:
        if current:
            previous = current[-1]
            gap = note["at"] - (previous["at"] + previous["length"])
            next_line = any(previous["at"] < start <= note["at"] for start in annotated)
            if gap >= PARAMETERS["line_gap_ticks"] or next_line:
                lines.extend(split_long(current, meter))
                current = []
        current.append(note)
    if current:
        lines.extend(split_long(current, meter))
    return lines


def entry_spread(starts: list[int], meter: Meter) -> dict:
    """Where lines starting at `starts` enter the bar: the commonest beat, its share, pickups."""
    beats = Counter(
        round(1 + (at - meter.bar_start(at)) / meter.beat_length(at), 3)
        for at in starts
    )
    beat, count = beats.most_common(1)[0] if beats else (None, 0)
    return {
        "most_common_entry_beat": beat,
        "most_common_entry_share": round(count / len(starts), 3) if starts else None,
        "second_half_entries": sum(
            (at - meter.bar_start(at)) * 2 >= meter.bar_length(at) for at in starts
        ),
    }


def lines_of(project: dict, track: dict) -> list[Line]:
    """Cut the foreground into lines: at lyric annotation lines, clip boundaries and rests.

    An annotation laid over a whole clip names a section, not a line, so the rests inside it
    still cut, and a line still longer than four bars is cut again at its longest rest.
    """
    meter = Meter(project)
    clips = list(active_clips(track))
    end = max((clip["start"] + sounding_length(clip) for clip in clips), default=0)
    annotated: list[int] = []
    if track.get("kind", {}).get("type") == "singer":
        lyric_points = project.get("lyrics", {}).get("points") or []
        annotated = [start for start, _, _ in spans(lyric_points, "text", end)]
    lines = []
    for clip in sorted(clips, key=lambda clip: clip["start"]):
        for notes in cut_lines(top_line(clip_notes(clip)), annotated, meter):
            lines.append(Line(start=notes[0]["at"], notes=notes))
    return lines


def entry_kind(line: Line, meter: Meter) -> tuple[float, str]:
    """The first onset's beat within its bar (1-based) and whether it is a pickup."""
    first = line.notes[0]["at"]
    bar = meter.bar_start(first)
    offset = first - bar
    beat = meter.beat_length(first)
    position = 1 + offset / beat
    if offset == 0:
        return position, "downbeat"
    next_bar = bar + meter.bar_length(first)
    later = sum(1 for note in line.notes if note["at"] >= next_bar)
    in_first_bar = len(line.notes) - later
    if offset >= meter.bar_length(first) / 2 and later > in_first_bar:
        return position, "pickup"
    return position, "late"


def onset_profile(line: Line, meter: Meter) -> dict:
    positions = Counter()
    syncopated = anticipations = 0
    for note in line.notes:
        at = note["at"]
        beat = meter.beat_length(at)
        bar = meter.bar_start(at)
        offset = at - bar
        if offset % beat == 0:
            positions["beat"] += 1
        elif offset % (beat // 2) == 0:
            positions["half"] += 1
        elif offset % (beat // 4) == 0:
            positions["quarter"] += 1
        else:
            positions["other"] += 1
        next_beat = bar + (offset // beat + 1) * beat
        if offset % beat != 0 and at + note["length"] > next_beat:
            syncopated += 1
            half_bar = meter.bar_length(at) // 2
            strong = (next_beat - bar) % half_bar == 0
            if strong and next_beat - at <= beat // 2:
                anticipations += 1
    return {
        "positions": dict(positions),
        "syncopated": syncopated,
        "anticipations": anticipations,
    }


def contour_type(pitches: list[int]) -> str:
    """Huron's nine phrase contours from the first, mean middle and last pitch."""
    if len(pitches) < 3:
        return "too_short"
    first, last = pitches[0], pitches[-1]
    middle = sum(pitches[1:-1]) / (len(pitches) - 2)
    tolerance = PARAMETERS["contour_tolerance_semitones"]

    def compare(a: float, b: float) -> str:
        if abs(a - b) <= tolerance:
            return "="
        return "<" if a < b else ">"

    names = {
        ("<", "<"): "ascending",
        ("<", ">"): "convex",
        ("<", "="): "ascending-horizontal",
        ("=", "<"): "horizontal-ascending",
        ("=", "="): "horizontal",
        ("=", ">"): "horizontal-descending",
        (">", "<"): "concave",
        (">", ">"): "descending",
        (">", "="): "descending-horizontal",
    }
    return names[(compare(first, middle), compare(middle, last))]


class PPM:
    """A PPM model with escape method C and exclusion, learning as it predicts."""

    def __init__(self, order: int, alphabet: int) -> None:
        self.order = order
        self.alphabet = alphabet
        self.counts: dict[tuple, Counter] = defaultdict(Counter)

    def information(self, history: list, symbol) -> float:
        escape, excluded = 1.0, set()
        for length in range(min(self.order, len(history)), -1, -1):
            context = tuple(history[len(history) - length :])
            table = self.counts.get(context)
            if not table:
                continue
            seen = {s: c for s, c in table.items() if s not in excluded}
            total = sum(seen.values())
            if not total:
                continue
            if symbol in seen:
                return -math.log2(escape * seen[symbol] / (total + len(seen)))
            escape *= len(seen) / (total + len(seen))
            excluded |= set(seen)
        return -math.log2(escape / max(1, self.alphabet - len(excluded)))

    def learn(self, history: list, symbol) -> None:
        for length in range(min(self.order, len(history)) + 1):
            self.counts[tuple(history[len(history) - length :])][symbol] += 1


def predictability(lines: list[Line]) -> list[float | None]:
    """Mean short-term information content of each line, in bits per note."""
    model = PPM(PARAMETERS["ppm_order"], alphabet=25 * 16)
    history: list = []
    previous = None
    result: list[float | None] = []
    for line in lines:
        values = []
        for note in line.notes:
            if previous is not None:
                interval = max(-12, min(12, note["pitch"] - previous["pitch"]))
                ioi = round((note["at"] - previous["at"]) / (TICKS_PER_QUARTER / 4))
                symbol = (interval, max(1, min(16, ioi)))
                values.append(model.information(history, symbol))
                model.learn(history, symbol)
                history.append(symbol)
            previous = note
        result.append(round(sum(values) / len(values), 3) if values else None)
    return result


def letter_lines(lines: list[Line]) -> None:
    """Same onsets and pitches share a letter; same onsets, other pitches add a prime."""
    rhythms: dict[tuple, str] = {}
    variants: dict[tuple, str] = {}
    primes: Counter = Counter()
    letters = (chr(code) for code in range(ord("A"), ord("Z") + 1))
    for line in lines:
        first = line.notes[0]["at"]
        rhythm = tuple(note["at"] - first for note in line.notes)
        full = (rhythm, tuple(note["pitch"] for note in line.notes))
        if full not in variants:
            if rhythm in rhythms:
                base = rhythms[rhythm]
                primes[base] += 1
                variants[full] = base + "'" * primes[base]
            else:
                base = next(letters, "?")
                rhythms[rhythm] = base
                variants[full] = base
        line.letter = variants[full]


def section_of(sections: list[dict], line: Line) -> str | None:
    """The section most of a line's notes start in, the later one when two hold as many.

    A line entering with a pickup before its section's barline is that section's line.
    """
    counts: list[list] = []
    for note in line.notes:
        label = value_at(sections, "label", note["at"])
        if counts and counts[-1][0] == label:
            counts[-1][1] += 1
        else:
            counts.append([label, 1])
    best = max(count for _, count in counts)
    return [label for label, count in counts if count == best][-1]


def describe_lines(project: dict, lines: list[Line], meter: Meter) -> None:
    keys = project.get("harmony", {}).get("keys", {}).get("points") or []
    chords = project.get("harmony", {}).get("chords", {}).get("points") or []
    sections = project.get("sections", {}).get("points") or []
    for line in lines:
        line.section = section_of(sections, line)
        position, kind = entry_kind(line, meter)
        last = line.notes[-1]
        key = parse_key(value_at(keys, "key", last["at"]))
        pitches = [note["pitch"] for note in line.notes]
        iois = [b["at"] - a["at"] for a, b in zip(line.notes, line.notes[1:])]
        line.features = {
            "entry_beat": round(position, 3),
            "entry": kind,
            **onset_profile(line, meter),
            "notes": len(line.notes),
            "iois_beats": [round(ioi / TICKS_PER_QUARTER, 3) for ioi in iois],
            "final_degree": DEGREES[(last["pitch"] - key[0]) % 12] if key else None,
            "final_chord": value_at(chords, "chord", last["at"]),
            "contour": contour_type(pitches),
            "ends_higher": pitches[-1] > pitches[0],
        }


def chord_bars(project: dict, start: int, end: int, meter: Meter) -> list:
    chords = project.get("harmony", {}).get("chords", {}).get("points") or []
    bars = []
    at = meter.bar_start(start)
    while at < end:
        bars.append(value_at(chords, "chord", at))
        at += meter.bar_length(at)
    return bars


def period(sequence: list) -> list:
    """The shortest repeating unit of a sequence, so a looped progression compares as its loop."""
    for size in range(1, len(sequence) + 1):
        if all(sequence[i] == sequence[i % size] for i in range(len(sequence))):
            return sequence[:size]
    return sequence


def same_cycle(a: list, b: list) -> bool:
    """Whether two loops are the same progression, possibly started on another chord."""
    if not a or len(a) != len(b):
        return False
    doubled = a + a
    return any(doubled[i : i + len(b)] == b for i in range(len(a)))


def drum_bars(project: dict, meter: Meter) -> dict[int, tuple]:
    bars: dict[int, set] = defaultdict(set)
    for track in project.get("tracks", []):
        if track.get("kind", {}).get("type") != "drum":
            continue
        for clip in active_clips(track):
            for note in clip_notes(clip):
                bar = meter.bar_start(note["at"])
                sixteenth = round((note["at"] - bar) / (TICKS_PER_QUARTER / 4))
                bars[bar].add((sixteenth, note["pitch"]))
    return {bar: tuple(sorted(hits)) for bar, hits in bars.items()}


def is_guitar(track: dict) -> bool:
    """A guitar by name, or by a General MIDI guitar program (25-32, stored as 24-31)."""
    name = track.get("name", "").lower()
    state = track.get("kind", {}).get("instrument_state") or {}
    patch = (state.get("extra") or {}).get("preset", {}).get("patch", -1)
    return any(word in name for word in GUITAR_WORDS) or 24 <= patch <= 31


def chord_spread(stage: dict, size: int) -> float:
    """The milliseconds from a chord's first string to its last under a strum stage."""
    per_string = float(stage.get("string_ms", 0.0) or 0.0)
    if per_string > 0:
        return min(per_string * (size - 1), 100.0)
    return min(float(stage.get("spread_ms", 0.0)), 100.0)


def strum_report(project: dict) -> list[dict]:
    """The spread each strum stage gives the chords it plays, per track."""
    report = []
    for track in project.get("tracks", []):
        if track.get("kind", {}).get("type") != "instrument":
            continue
        chords = settings = 0
        spreads: list[float] = []
        unstrummed = 0
        for clip in active_clips(track):
            by_onset = Counter(note["at"] for note in clip_notes(clip))
            sizes = [count for count in by_onset.values() if count >= 3]
            if not sizes:
                continue
            chords += len(sizes)
            stage = next(
                (
                    transform.get("settings", {})
                    for transform in clip.get("transforms", []) or []
                    if transform.get("kind") == "strum"
                ),
                None,
            )
            if stage is None:
                unstrummed += len(sizes)
                continue
            settings += 1
            spreads.extend(chord_spread(stage, size) for size in sizes)
        if not chords:
            continue
        spreads.sort()
        report.append(
            {
                "track": track.get("name", ""),
                "guitar": is_guitar(track),
                "chords": chords,
                "unstrummed_chords": unstrummed,
                "strummed_clips": settings,
                "min_spread_ms": round(spreads[0], 2) if spreads else None,
                "median_spread_ms": round(spreads[len(spreads) // 2], 2)
                if spreads
                else None,
            }
        )
    return report


def flags_for(
    lines: list[Line],
    sections: list[dict],
    strums: list[dict],
    ics: list,
    entries: dict,
) -> list[dict]:
    flags = []
    for section in sections:
        letters = section["form"]
        if (
            len(letters) >= PARAMETERS["identical_section_min_lines"]
            and len(set(letters)) == 1
        ):
            flags.append(
                {
                    "code": "identical_lines",
                    "section": section["section"],
                    "message": f"all {len(letters)} lines of {section['section']} sing the same onsets and pitches",
                }
            )
    if len(lines) >= PARAMETERS["uniform_min_lines"]:
        share = entries["most_common_entry_share"]
        if share >= PARAMETERS["uniform_entry_share"]:
            flags.append(
                {
                    "code": "same_entry",
                    "message": f"{share:.0%} of {len(lines)} lines enter at beat {entries['most_common_entry_beat']}",
                }
            )
        if entries["second_half_entries"] == 0:
            flags.append(
                {
                    "code": "no_second_half_entry",
                    "message": f"none of {len(lines)} lines enters in the second half of a bar",
                }
            )
        if sum(line.features["syncopated"] for line in lines) == 0:
            flags.append(
                {
                    "code": "no_syncopation",
                    "message": f"no note in {len(lines)} lines starts off the beat and holds across it",
                }
            )
        counts = {line.features["notes"] for line in lines}
        if len(counts) == 1:
            flags.append(
                {
                    "code": "same_note_count",
                    "message": f"every line has {counts.pop()} notes",
                }
            )
    iois = Counter(ioi for line in lines for ioi in line.features["iois_beats"])
    total = sum(iois.values())
    if total >= PARAMETERS["uniform_ioi_min_count"]:
        value, count = iois.most_common(1)[0]
        if count / total >= PARAMETERS["uniform_ioi_share"]:
            flags.append(
                {
                    "code": "uniform_note_value",
                    "message": f"{count / total:.0%} of onsets are {value} beats after the previous one",
                }
            )
    degrees = [
        line.features["final_degree"] for line in lines if line.features["final_degree"]
    ]
    if len(degrees) >= 4 and "1" not in degrees:
        flags.append(
            {
                "code": "no_tonic_ending",
                "message": f"none of {len(degrees)} lines ends on the tonic",
            }
        )
    run = best = 0
    for value in ics:
        run = run + 1 if value is not None and value < PARAMETERS["plateau_bits"] else 0
        best = max(best, run)
    if best >= PARAMETERS["plateau_min_lines"]:
        flags.append(
            {
                "code": "predictability_plateau",
                "message": f"{best} consecutive lines are predictable from the song so far (under {PARAMETERS['plateau_bits']} bit a note)",
            }
        )
    grooves = defaultdict(list)
    for section in sections:
        if section.get("main_groove"):
            grooves[section["main_groove"]].append(section["section"])
    for names in grooves.values():
        kinds = {section_kind(name) for name in names}
        if len(kinds) >= PARAMETERS["shared_groove_min_sections"]:
            flags.append(
                {
                    "code": "shared_main_groove",
                    "sections": names,
                    "message": f"{', '.join(names)} share one main drum bar",
                }
            )
    verses = [
        section
        for section in sections
        if is_verse(section["section"]) and section["chord_loop"]
    ]
    for chorus in (
        section
        for section in sections
        if is_chorus(section["section"]) and section["chord_loop"]
    ):
        for verse in verses:
            if same_cycle(verse["chord_loop"], chorus["chord_loop"]):
                flags.append(
                    {
                        "code": "chorus_reuses_verse_harmony",
                        "section": chorus["section"],
                        "message": f"{chorus['section']} loops {verse['section']}'s progression {' '.join(map(str, verse['chord_loop']))}",
                    }
                )
                break
    for strum in strums:
        if strum["chords"] < PARAMETERS["strummed_min_chords"] or not strum["guitar"]:
            continue
        if strum["unstrummed_chords"] == strum["chords"]:
            flags.append(
                {
                    "code": "unstrummed_chords",
                    "track": strum["track"],
                    "message": f"{strum['track']} strikes {strum['chords']} chords with every string at once",
                }
            )
        elif (
            strum["min_spread_ms"] is not None
            and strum["min_spread_ms"] < PARAMETERS["tight_strum_ms"]
        ):
            flags.append(
                {
                    "code": "tight_strum",
                    "track": strum["track"],
                    "message": f"{strum['track']} strums a chord over only {strum['min_spread_ms']} ms",
                }
            )
    return flags


def measure(project: dict, wanted: str | None = None) -> dict:
    meter = Meter(project)
    track = pick_foreground(project, wanted)
    lines = lines_of(project, track) if track else []
    letter_lines(lines)
    describe_lines(project, lines, meter)
    ics = predictability(lines)
    for line, value in zip(lines, ics):
        line.features["information_bits"] = value
    end = max(
        (line.notes[-1]["at"] + line.notes[-1]["length"] for line in lines), default=0
    )
    section_points = project.get("sections", {}).get("points") or []
    song_end = max(
        [end]
        + [
            clip["start"] + clip["length"]
            for t in project.get("tracks", [])
            for clip in active_clips(t)
        ]
    )
    drums = drum_bars(project, meter)
    sections = []
    for start, stop, label in spans(section_points, "label", song_end):
        inside = [line for line in lines if start <= line.start < stop]
        bars = [drums[bar] for bar in drums if start <= bar < stop]
        main = Counter(bars).most_common(1)[0][0] if bars else None
        sections.append(
            {
                "section": label,
                "start_tick": start,
                "form": [line.letter for line in inside],
                "chord_loop": period(chord_bars(project, start, stop, meter)),
                "main_groove": main,
            }
        )
    strums = strum_report(project)
    entries = entry_spread([line.start for line in lines], meter)
    flags = flags_for(lines, sections, strums, ics, entries)
    for section in sections:
        section["main_groove"] = (
            None if section["main_groove"] is None else len(section["main_groove"])
        )
    letters = {line.letter.rstrip("'") for line in lines}
    return {
        "foreground": None if track is None else track.get("name"),
        "lines": [
            {
                "start_tick": line.start,
                "section": line.section,
                "letter": line.letter,
                **line.features,
            }
            for line in lines
        ],
        "sections": sections,
        "strums": strums,
        "summary": {
            "lines": len(lines),
            "distinct_lines": len({line.letter for line in lines}),
            "distinct_rhythms": len(letters),
            "downbeat_entries": sum(
                line.features["entry"] == "downbeat" for line in lines
            ),
            **entries,
            "syncopated_notes": sum(line.features["syncopated"] for line in lines),
            "tonic_endings": sum(
                line.features["final_degree"] == "1" for line in lines
            ),
            "lines_ending_higher": sum(line.features["ends_higher"] for line in lines),
        },
        "flags": flags,
    }


def project_paths(inputs: list[Path]) -> list[Path]:
    paths = []
    for item in inputs:
        if item.is_dir():
            paths.extend(sorted(item.rglob("*.auris")))
        else:
            paths.append(item)
    return paths


def build_report(paths: list[Path], wanted: str | None) -> dict:
    songs = []
    for path in paths:
        project = json.loads(path.read_text(encoding="utf-8"))
        songs.append(
            {"path": str(path), "sha256": file_digest(path), **measure(project, wanted)}
        )
    return {"schema_version": SCHEMA_VERSION, "parameters": PARAMETERS, "songs": songs}


def print_song(song: dict) -> None:
    print(f"== {song['path']} (foreground: {song['foreground']})")
    for section in song["sections"]:
        if section["form"]:
            print(
                f"   {section['section']}: {' '.join(section['form'])}   chords {' '.join(map(str, section['chord_loop']))}"
            )
    summary = song["summary"]
    print(
        f"   lines {summary['lines']}, distinct {summary['distinct_lines']}, rhythms {summary['distinct_rhythms']},"
        f" downbeat entries {summary['downbeat_entries']}, syncopated notes {summary['syncopated_notes']},"
        f" tonic endings {summary['tonic_endings']}"
    )
    bits = [line["information_bits"] for line in song["lines"]]
    print(
        "   bits/note per line:",
        " ".join("-" if b is None else f"{b:.2f}" for b in bits),
    )
    for strum in song["strums"]:
        print(
            f"   strum {strum['track']}: {strum['chords']} chords, spread {strum['min_spread_ms']}..{strum['median_spread_ms']} ms"
        )
    for flag in song["flags"]:
        print(f"   [{flag['code']}] {flag['message']}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "inputs", nargs="+", type=Path, help=".auris files or folders holding them"
    )
    parser.add_argument(
        "--track", help="foreground track name or numeric ID (default: first singer)"
    )
    parser.add_argument("--json", type=Path, help="write the full report here")
    args = parser.parse_args()
    paths = project_paths(args.inputs)
    if not paths:
        raise SystemExit("No .auris projects found")
    report = build_report(paths, args.track)
    for song in report["songs"]:
        print_song(song)
    if args.json:
        args.json.write_text(
            json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8"
        )


if __name__ == "__main__":
    main()
