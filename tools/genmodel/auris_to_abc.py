"""Write one track of an Auris project as the ABC score YuE2 performs.

Development experiment only. YuE2 takes a two-voice ABC score (`Vocal` and
`Ins`, sixteenth-note units, chord symbols in quotes) in place of the score it
would otherwise plan itself. This puts a melody track of a saved `.auris`
project into one of those voices, with the document's chord progression over
it, so YuE2 performs Auris's melody and harmony instead of inventing its own.

    python tools/genmodel/auris_to_abc.py Song/Song.auris --track Lead --voice Ins > song.abc

Only 4/4, one tempo and one key are handled: that is what the test songs use.
Notes are quantised to sixteenths; overlapping notes keep the higher one.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

TICKS_PER_QUARTER = 960
UNIT = TICKS_PER_QUARTER // 4  # one sixteenth, ABC `L:1/16`
BAR_UNITS = 16

LETTERS = "CDEFGAB"
NATURAL_PC = {"C": 0, "D": 2, "E": 4, "F": 5, "G": 7, "A": 9, "B": 11}
MAJOR = [0, 2, 4, 5, 7, 9, 11]
MINOR = [0, 2, 3, 5, 7, 8, 10]
NUMERALS = ["i", "ii", "iii", "iv", "v", "vi", "vii"]
SHARP_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]


def parse_key(text: str) -> tuple[str, int, list[int]]:
    """`"E minor"` -> (letter-with-accidental, tonic pitch class, scale degrees)."""
    name, mode = text.split()
    tonic = NATURAL_PC[name[0]] + name[1:].count("#") - name[1:].count("b")
    return name, tonic % 12, MINOR if mode == "minor" else MAJOR


def scale_spelling(
    name: str, tonic: int, scale: list[int]
) -> dict[int, tuple[str, int]]:
    """Pitch class -> (letter, alteration) for the seven notes of the key."""
    start = LETTERS.index(name[0])
    spelled = {}
    for i, step in enumerate(scale):
        letter = LETTERS[(start + i) % 7]
        pc = (tonic + step) % 12
        alt = (pc - NATURAL_PC[letter] + 6) % 12 - 6
        spelled[pc] = (letter, alt)
    return spelled


def chord_name(numeral: str, tonic: int, scale: list[int]) -> str | None:
    """Roman numeral in the key -> a chord symbol YuE2's scores use."""
    seventh = numeral.endswith("7")
    core = numeral.rstrip("7")
    if core.lower() not in NUMERALS:
        return None
    degree = NUMERALS.index(core.lower())
    root = SHARP_NAMES[(tonic + scale[degree]) % 12]
    if core.isupper():
        return root + ("7" if seventh else "")
    if degree == 6 and scale is MAJOR:
        return root + ("m7b5" if seventh else "dim")
    return root + ("m7" if seventh else "m")


def abc_note(pitch: int, spelled, signature, bar_state) -> str:
    pc = pitch % 12
    if pc in spelled:
        letter, alt = spelled[pc]
    else:  # outside the key: a raised note of the letter below
        letter, alt = spelled[(pc - 1) % 12][0], spelled[(pc - 1) % 12][1] + 1
    octave = (pitch - NATURAL_PC[letter] - alt) // 12 - 1  # MIDI 60 is C4
    mark = ""
    if bar_state.get(letter, signature.get(letter, 0)) != alt:
        mark = {-1: "_", 0: "=", 1: "^", 2: "^^", -2: "__"}[alt]
        bar_state[letter] = alt
    if octave >= 5:
        text = letter.lower() + "'" * (octave - 5)
    else:
        text = letter + "," * (4 - octave)
    return mark + text


def bars_of(notes, total_units: int) -> list[list[tuple[int | None, int, bool]]]:
    """Per bar, a list of (pitch or None, units, tied-to-next) events."""
    grid: list[int | None] = [None] * total_units
    starts = set()
    for note in sorted(notes, key=lambda n: (n["start"], -n["pitch"])):
        a = round(note["start"] / UNIT)
        b = max(a + 1, round((note["start"] + note["length"]) / UNIT))
        if a >= total_units:
            continue
        if grid[a] is not None and grid[a] >= note["pitch"]:
            continue
        for u in range(a, min(b, total_units)):
            grid[u] = note["pitch"]
        starts.add(a)
    bars = []
    for bar in range(total_units // BAR_UNITS):
        events = []
        u = bar * BAR_UNITS
        end = u + BAR_UNITS
        while u < end:
            pitch = grid[u]
            v = u + 1
            while v < end and grid[v] == pitch and v not in starts:
                v += 1
            tied = (
                pitch is not None
                and v == end
                and v < total_units
                and grid[v] == pitch
                and v not in starts
            )
            events.append((pitch, v - u, tied))
            u = v
        bars.append(events)
    return bars


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("project", type=Path)
    parser.add_argument("--track", default="Lead")
    parser.add_argument("--voice", choices=["Vocal", "Ins"], default="Ins")
    parser.add_argument(
        "--section-bars", type=int, default=8, help="bars per section comment"
    )
    parser.add_argument("--no-chords", action="store_true")
    args = parser.parse_args()

    doc = json.loads(args.project.read_text(encoding="utf-8"))
    bpm = doc["tempo_map"]["points"][0]["bpm"]
    name, tonic, scale = parse_key(doc["harmony"]["keys"]["points"][0]["key"])
    spelled = scale_spelling(name, tonic, scale)
    signature = {letter: alt for letter, alt in spelled.values()}
    track = next(t for t in doc["tracks"] if t["name"] == args.track)
    notes = [
        {**n, "start": n["start"] + clip["start"]}
        for clip in track["kind"]["clips"]
        for n in clip["notes"]
    ]
    end_tick = max(c["start"] + c["length"] for c in track["kind"]["clips"])
    total = -(-round(end_tick / UNIT) // BAR_UNITS) * BAR_UNITS
    bars = bars_of(notes, total)

    chords: dict[int, str] = {}
    if not args.no_chords:
        for point in doc["harmony"]["chords"]["points"]:
            symbol = chord_name(point["chord"], tonic, scale)
            if symbol:
                chords[round(point["tick"] / UNIT)] = symbol

    def bar_text(index: int) -> str:
        state: dict[str, int] = {}
        out = []
        u = index * BAR_UNITS
        current = None
        for c_at in sorted(k for k in chords if k <= u):
            current = chords[c_at]
        for pitch, units, tied in bars[index]:
            label = chords.get(u)
            if u == index * BAR_UNITS and label is None and current:
                label = current
            piece = f'"{label}"' if label else ""
            if pitch is None:
                piece += f"z{units}"
            else:
                piece += abc_note(pitch, spelled, signature, state) + (
                    str(units) if units != 1 else ""
                )
                if tied:
                    piece += "-"
            out.append(piece)
            u += units
        return "".join(out) + "|"

    def rest_bar(index: int) -> str:
        u = index * BAR_UNITS
        label = None
        for c_at in sorted(k for k in chords if k <= u):
            label = chords[c_at]
        return (f'"{label}"' if label else "") + "z16|"

    key_field = name + ("m" if scale is MINOR else "")
    lines = [
        "X:1",
        "T:",
        "M:4/4",
        "L:1/16",
        f"Q:1/4={round(bpm)}",
        'V: Vocal clef=treble name="Vocal Melody" snm="Vocal"',
        'V: Ins clef=treble name="Ins Melody" snm="Inst."',
        f"K:{key_field}",
    ]
    for start in range(0, len(bars), 4):
        if start % args.section_bars == 0:
            lines.append(
                "% verse" if (start // args.section_bars) % 2 == 0 else "% chorus"
            )
        idx = range(start, min(start + 4, len(bars)))
        melody = "".join(bar_text(i) for i in idx)
        rests = "".join(rest_bar(i) for i in idx)
        for voice in ("Vocal", "Ins"):
            lines.append(f"V: {voice}")
            lines.append(melody if voice == args.voice else rests)
    sys.stdout.write("\n".join(lines) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
