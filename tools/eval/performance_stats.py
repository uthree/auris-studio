# /// script
# requires-python = ">=3.10"
# dependencies = ["mido==1.3.3", "numpy"]
# ///
"""How human players shape what Auris's performance stages shape, read as dial amounts.

    uv run tools/eval/performance_stats.py MAESTRO_DIR GUITARSET_DIR --out stats.json

Each score-reading stage in `auris-core/src/project/interpretation.rs` and the expression and
stroke stages scale their effect linearly with their amount, so a statistic measured on human
playing, divided by the stage's effect at full strength, is the amount that reproduces it. That is
what the performance panel's starting values (`crate::ui::techniques::Technique::start` in the
desktop) are read from.

Piano statistics come from MAESTRO v3 MIDI (Disklavier performances: velocities, key releases
and the sustain pedal). The top note at each onset stands in for the melody, and a rest longer
than half a second (a quarter note at 120 BPM, the phrase rule the stages use) ends a phrase.
Guitar strum spreads come from GuitarSet's per-string note annotations of its comping takes.

Neither corpus is redistributable here (MAESTRO is CC BY-NC-SA 4.0): download them into
`target/`, never commit them, and commit only the aggregate numbers.

Two limits matter when reading the result. A performance is not its score, so nothing here can
tell a written legato from a detached step; the step-overlap figure mixes both and does not
measure the articulation stage. Repeated keys are the same: their median lift grows with the
interval (about two thirds of it), so it is the touch of detached repeats, and the lower quartile
of each player's lifts stands in for a written-legato repeat. And a classical Disklavier corpus is one repertoire: these are
starting values for a dial, not the right amount for every song.
"""

from __future__ import annotations

import argparse
import itertools
import json
import random
from pathlib import Path

import mido
import numpy as np

CHORD_WINDOW_S = 0.050
PHRASE_REST_S = 0.500
STRUM_WINDOW_S = 0.080

# Full-strength effects of the Auris stages (auris-core/src/project/interpretation.rs and
# expression.rs; the stroke dial runs 0..1 over 0..100 ms).
HIGH_LOUD_PER_OCTAVE = 0.20
DURATION_CONTRAST = 0.03
FINAL_SOFTEN = 0.07
SWELL_PEAK = 0.6
INNER_SOFTEN = 0.25
BASS_SOFTEN = 0.30
MELODY_LEAD_MS = 15.0
STEP_OVERLAP_MS = 25.0
LEAP_GAP_MS = 30.0
REPEAT_GAP_MS = 140.0
STROKE_FULL_MS = 100.0

PIANO_KEYS = (
    "inner_ratio",
    "bass_ratio",
    "bass_ratio_three",
    "lead_ms",
    "chord_spread_ms",
    "contour_slope",
    "final_ratio",
    "joint_pitch",
    "joint_duration",
    "joint_final",
    "joint_swell",
    "step_overlap_ms",
    "leap_gap_ms",
    "repeat_gap_ms",
    "repeat_gap_q1_ms",
    "pedalled",
    "pedal_extension_s",
)


def read_piano(path: Path) -> list[list[float]]:
    """Notes as [onset, key release, sound end, pitch, velocity], in seconds."""
    held: dict[int, list[tuple[float, int]]] = {}
    notes: list[list[float]] = []
    pedal_changes: list[tuple[float, bool]] = []
    pedal = False
    now = 0.0
    for msg in mido.MidiFile(path):
        now += msg.time
        if msg.type == "control_change" and msg.control == 64:
            down = msg.value >= 64
            if down != pedal:
                pedal = down
                pedal_changes.append((now, down))
        elif msg.type == "note_on" and msg.velocity > 0:
            held.setdefault(msg.note, []).append((now, msg.velocity))
        elif msg.type in ("note_off", "note_on") and held.get(msg.note):
            onset, velocity = held[msg.note].pop(0)
            notes.append([onset, now, now, msg.note, velocity])
    notes.sort()
    return sustain_under_pedal(notes, pedal_changes)


def sustain_under_pedal(
    notes: list[list[float]], pedal_changes: list[tuple[float, bool]]
) -> list[list[float]]:
    """Carries a note released with the pedal down on to the pedal's next lift."""
    lifts = [time for time, down in pedal_changes if not down]
    for note in notes:
        down = False
        for time, state in pedal_changes:
            if time > note[1]:
                break
            down = state
        if down:
            note[2] = next((lift for lift in lifts if lift > note[1]), note[1])
    return notes


def chords(notes: list[list[float]]) -> list[list[list[float]]]:
    """Notes grouped by onsets within the chord window of the group's first."""
    groups: list[list[list[float]]] = []
    current: list[list[float]] = []
    for note in notes:
        if current and note[0] - current[0][0] > CHORD_WINDOW_S:
            groups.append(current)
            current = []
        current.append(note)
    if current:
        groups.append(current)
    return groups


def phrases(line: list[list[float]]) -> list[list[list[float]]]:
    """A melody cut where a rest is longer than the phrase rest."""
    if not line:
        return []
    cut: list[list[list[float]]] = [[line[0]]]
    for before, after in itertools.pairwise(line):
        if after[0] - before[1] > PHRASE_REST_S:
            cut.append([])
        cut[-1].append(after)
    return cut


def piano_stats(notes: list[list[float]]) -> dict[str, list[float]]:
    """Every per-chord, per-phrase and per-note statistic of one performance."""
    out: dict[str, list[float]] = {key: [] for key in PIANO_KEYS}
    line = []
    for group in chords(notes):
        top = max(group, key=lambda note: note[3])
        line.append(top)
        pitches = len({note[3] for note in group})
        if pitches < 2:
            continue
        # The stage softens the lowest note of every chord of two pitches or more.
        bottom = min(group, key=lambda note: note[3])
        out["bass_ratio"].append(bottom[4] / top[4])
        if pitches < 3:
            continue
        inner = [note[4] for note in group if note[3] not in (top[3], bottom[3])]
        out["inner_ratio"].append(float(np.mean(inner)) / top[4])
        out["bass_ratio_three"].append(bottom[4] / top[4])
        others = [note[0] for note in group if note is not top]
        out["lead_ms"].append((float(np.mean(others)) - top[0]) * 1000)
        onsets = [note[0] for note in group]
        out["chord_spread_ms"].append((max(onsets) - min(onsets)) * 1000)
    for phrase in phrases(line):
        if len(phrase) >= 6:
            phrase_stats(phrase, out)
    for note in notes:
        pedalled = note[2] > note[1] + 1e-6
        out["pedalled"].append(1.0 if pedalled else 0.0)
        if pedalled:
            out["pedal_extension_s"].append(note[2] - note[1])
    return out


def phrase_stats(phrase: list[list[float]], out: dict[str, list[float]]) -> None:
    """Dynamics against pitch and position, and the joins between neighbouring notes.

    The joint fit reads the contour stage's three terms together — pitch against the phrase
    mean, each inter-onset interval against the phrase median, and the last attack — with the
    phrase arch beside them, so a final note that is soft because it is low is not read twice.
    """
    velocity = np.array([note[4] for note in phrase], float)
    pitch = np.array([note[3] for note in phrase], float)
    onset = np.array([note[0] for note in phrase], float)
    octaves = np.clip((pitch - pitch.mean()) / 12.0, -1.5, 1.5)
    out["final_ratio"].append(float(velocity[-1] / velocity[:-1].mean()))
    if np.ptp(octaves) > 0.25:
        relative = velocity / velocity.mean()
        out["contour_slope"].append(float(np.polyfit(octaves, relative - 1.0, 1)[0]))
        if onset[-1] > onset[0]:
            arch = np.sin(np.pi * (onset - onset[0]) / (onset[-1] - onset[0]))
            intervals = np.maximum(np.diff(onset), 1e-3)
            duration = np.zeros_like(octaves)
            duration[:-1] = np.clip(
                np.log2(intervals / max(float(np.median(intervals)), 1e-3)), -1.0, 1.0
            )
            final = np.zeros_like(octaves)
            final[-1] = 1.0
            design = np.column_stack(
                [np.ones_like(octaves), octaves, duration, final, arch]
            )
            (level, per_octave, per_doubling, last, swell), *_ = np.linalg.lstsq(
                design, relative, rcond=None
            )
            if level > 0:
                out["joint_pitch"].append(float(per_octave / level))
                out["joint_duration"].append(float(per_doubling / level))
                out["joint_final"].append(float(last / level))
                out["joint_swell"].append(float(swell / level))
    for before, after in itertools.pairwise(phrase):
        if after[0] - before[0] > 1.0:
            continue
        interval = abs(after[3] - before[3])
        if interval == 0:
            out["repeat_gap_ms"].append((after[0] - before[1]) * 1000)
        elif interval <= 2:
            out["step_overlap_ms"].append((before[1] - after[0]) * 1000)
        elif interval >= 5:
            out["leap_gap_ms"].append((after[0] - before[1]) * 1000)


def strum_spreads(jams: dict) -> list[float]:
    """First-to-last string onset of every stroke striking at least three strings, in ms."""
    onsets = sorted(
        (observation["time"], annotation["annotation_metadata"]["data_source"])
        for annotation in jams["annotations"]
        if annotation["namespace"] == "note_midi"
        for observation in annotation["data"]
    )
    spreads: list[float] = []
    stroke: list[tuple[float, str]] = []
    for onset, string in [*onsets, (float("inf"), "")]:
        if stroke and onset - stroke[0][0] > STRUM_WINDOW_S:
            if len({s for _, s in stroke}) >= 3:
                spreads.append((stroke[-1][0] - stroke[0][0]) * 1000)
            stroke = []
        stroke.append((onset, string))
    return spreads


def summary(values: list[float]) -> dict[str, float]:
    array = np.array(values, float)
    return {
        "n": int(array.size),
        "median": float(np.median(array)),
        "q1": float(np.percentile(array, 25)),
        "q3": float(np.percentile(array, 75)),
    }


def amounts(
    per_piece: dict[str, dict[str, float]], strums: dict[str, float]
) -> dict[str, float]:
    """The dial amount each statistic implies, from the median over pieces.

    A statistic the performances gave no instance of reads as NaN rather than as a guess.
    """
    m = {key: per_piece.get(key, {}).get("median", float("nan")) for key in PIANO_KEYS}
    return {
        "voicing_from_inner": (1 - m["inner_ratio"]) / INNER_SOFTEN,
        "voicing_from_bass": (1 - m["bass_ratio"]) / BASS_SOFTEN,
        "voicing_from_lead": m["lead_ms"] / MELODY_LEAD_MS,
        "contour_from_pitch": m["joint_pitch"] / HIGH_LOUD_PER_OCTAVE,
        "contour_from_duration": m["joint_duration"] / DURATION_CONTRAST,
        "contour_from_final": -m["joint_final"] / FINAL_SOFTEN,
        "swell_from_arch": m["joint_swell"] / SWELL_PEAK,
        "articulate_from_leap": m["leap_gap_ms"] / LEAP_GAP_MS,
        "articulate_from_repeat": m["repeat_gap_ms"] / REPEAT_GAP_MS,
        "articulate_from_repeat_q1": m["repeat_gap_q1_ms"] / REPEAT_GAP_MS,
        "pedal_from_share": m["pedalled"],
        "piano_chord_spread": m["chord_spread_ms"] / STROKE_FULL_MS,
        "guitar_stroke": strums["median"] / STROKE_FULL_MS,
        "sustain_beats_at_120": m["pedal_extension_s"] * 2.0,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("maestro", type=Path)
    parser.add_argument("guitarset", type=Path)
    parser.add_argument("--pieces", type=int, default=200)
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()

    files = sorted(args.maestro.rglob("*.midi"))
    random.Random(args.seed).shuffle(files)
    pooled: dict[str, list[float]] = {key: [] for key in PIANO_KEYS}
    per_piece: dict[str, list[float]] = {key: [] for key in PIANO_KEYS}
    used = 0
    for path in files[: args.pieces]:
        notes = read_piano(path)
        if len(notes) < 50:
            continue
        used += 1
        stats = piano_stats(notes)
        if stats["repeat_gap_ms"]:
            # The most legato quarter of a player's repeats: the lift a written-legato repeat
            # needs, since a performance cannot say which repeats were written detached.
            stats["repeat_gap_q1_ms"] = [
                float(np.percentile(stats["repeat_gap_ms"], 25))
            ]
        for key, values in stats.items():
            pooled[key].extend(values)
            if values:
                per_piece[key].append(
                    float(np.median(values) if key != "pedalled" else np.mean(values))
                )
    strums = [
        spread
        for path in sorted(args.guitarset.glob("*_comp.jams"))
        for spread in strum_spreads(json.loads(path.read_text()))
    ]
    pieces = {key: summary(values) for key, values in per_piece.items() if values}
    strum_summary = summary(strums)
    result = {
        "maestro_pieces": used,
        "seed": args.seed,
        "piano_pooled": {
            key: summary(values) for key, values in pooled.items() if values
        },
        "piano_per_piece": pieces,
        "guitar_strum_spread_ms": strum_summary,
        "amounts": amounts(pieces, strum_summary),
    }
    text = json.dumps(result, indent=2)
    if args.out:
        args.out.write_text(text)
    print(json.dumps(result["amounts"], indent=2))


if __name__ == "__main__":
    main()
