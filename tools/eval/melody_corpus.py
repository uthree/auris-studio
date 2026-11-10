# /// script
# requires-python = ">=3.10"
# dependencies = []
# ///
"""How conventional a song's melody is against a corpus: an IDyOM-style long-term model.

    uv run tools/eval/melody_corpus.py train corpus/ --out pjs-model.json
    uv run tools/eval/melody_corpus.py score MySong/MySong.auris --model pjs-model.json
    uv run tools/eval/melody_corpus.py lines corpus/

`train` reads melodies from the MusicXML and MIDI files in the folders given (MusicXML: one
part, its first voice, ties merged, chords reduced to the top note; MIDI: the first non-drum
track with notes, its top note at each onset) and counts a PPM model over the same symbols as
`melody_form.py`'s short-term model: the interval from the note before, clipped to an octave,
and the onset spacing in sixteenths, clipped to a bar. It also cross-validates the corpus in five
folds, so a song's score can be read against how the corpus's own songs score on a model that
never saw them.

`score` reads the foreground of saved `.auris` projects exactly as `melody_form.py` cuts it into
lines, and gives each line its information content under the frozen long-term model, in bits per
note: low is a line the corpus has heard many times, high is one it has not. The song's mean is
reported beside the corpus's cross-validated mean and spread. It is not a quality score and is
never to be maximised (Cheung et al., 2019, found pleasure where surprise meets low uncertainty,
not where surprise is highest); it says whether a melody is built from the corpus's commonplaces.

`lines` cuts each MIDI file's sung part into lines by `melody_form.py`'s rule and reports how the
corpus's songs spread their line entries over the bar: the share of a song's lines entering at
its commonest point, and the share entering in the second half of a bar, as a pickup does. The
thresholds of the two entry warnings are read off these distributions. It reports the share of
a song's onset spacings taken by the commonest one as well, beside the share the note-value
warning fires at. Only files whose first
melodic track is named as a voice are counted unless --any-part is given, since the rests of a
piano's right hand are not a singer's breaths.

The model and its scores stay on the machine that made them. A corpus's licence travels with
what is made from it, so a model trained on CC BY-SA scores is CC BY-SA too, and a model trained
on scores whose rights are unclear is not published at all.
"""

from __future__ import annotations

import argparse
import json
import statistics
import xml.etree.ElementTree as ET
from collections import Counter
from itertools import pairwise
from pathlib import Path

from melody_form import (
    PARAMETERS,
    PPM,
    TICKS_PER_QUARTER,
    Meter,
    cut_lines,
    entry_spread,
    lines_of,
    pick_foreground,
    top_line,
)

SCHEMA_VERSION = 1
ALPHABET = 25 * 16
FOLDS = 5
STEPS = {"C": 0, "D": 2, "E": 4, "F": 5, "G": 7, "A": 9, "B": 11}
VOICE_WORDS = (
    "voice",
    "vocal",
    "sopran",
    "alto",
    "tenor",
    "bariton",
    "lead",
    "solo",
    "sing",
    "melod",
    "voix",
    "chant",
    "stimme",
    "gesang",
    "voz",
    "voce",
    "canto",
)
PERCENTILES = (5, 10, 25, 50, 75, 90, 95)


def symbol(previous: tuple[float, int], note: tuple[float, int]) -> tuple[int, int]:
    """The (interval, spacing in sixteenths) symbol arriving at `note`; onsets are in quarters."""
    interval = max(-12, min(12, note[1] - previous[1]))
    spacing = round((note[0] - previous[0]) * 4)
    return (interval, max(1, min(16, spacing)))


def symbols(notes: list[tuple[float, int]]) -> list[tuple[int, int]]:
    """The symbol sequence of a melody given as (onset in quarters, MIDI pitch) pairs."""
    return [symbol(a, b) for a, b in pairwise(notes)]


def musicxml_melody(path: Path) -> list[tuple[float, int]]:
    """The first part's first voice as (onset in quarters, pitch), ties merged, top note of chords."""
    root = ET.parse(path).getroot()
    part = root.find("part")
    if part is None:
        return []
    divisions = 1
    at = 0.0
    voice = None
    notes: list[tuple[float, int]] = []
    tied = False
    for measure in part.findall("measure"):
        for element in measure:
            if element.tag == "attributes":
                found = element.find("divisions")
                if found is not None and found.text:
                    divisions = int(found.text)
            elif element.tag == "backup":
                at -= int(element.findtext("duration", "0")) / divisions
            elif element.tag == "forward":
                at += int(element.findtext("duration", "0")) / divisions
            elif element.tag == "note":
                this_voice = element.findtext("voice", "1")
                voice = voice or this_voice
                duration = int(element.findtext("duration", "0") or 0) / divisions
                chord = element.find("chord") is not None
                if element.find("grace") is not None:
                    continue
                if this_voice != voice:
                    if not chord:
                        at += duration
                    continue
                pitch = element.find("pitch")
                onset = at - duration if chord else at
                if pitch is not None:
                    midi = (
                        12 * (int(pitch.findtext("octave", "4")) + 1)
                        + STEPS[pitch.findtext("step", "C")]
                        + round(float(pitch.findtext("alter", "0") or 0))
                    )
                    ties = {t.get("type") for t in element.findall("tie")}
                    if chord and notes and notes[-1][0] == onset:
                        notes[-1] = (onset, max(notes[-1][1], midi))
                    elif not (tied and "stop" in ties):
                        notes.append((onset, midi))
                    if not chord:
                        tied = "start" in ties
                elif not chord:
                    tied = False
                if not chord:
                    at += duration
    return notes


def _varlen(data: bytes, at: int) -> tuple[int, int]:
    value = 0
    while True:
        byte = data[at]
        at += 1
        value = (value << 7) | (byte & 0x7F)
        if byte < 0x80:
            return value, at


def midi_tracks(
    path: Path,
) -> tuple[int, list[dict], list[tuple[int, int, int]]] | None:
    """A MIDI file's division, tracks and time signatures as (tick, numerator, denominator).

    Each track is its name, the top pitch struck at each tick, and its notes as (on, off, pitch);
    channel 10 is left out. A note never released has an onset and no note.
    """
    data = path.read_bytes()
    if data[:4] != b"MThd":
        return None
    division = int.from_bytes(data[12:14], "big")
    if division & 0x8000 or not division:
        return None
    at = 8 + int.from_bytes(data[4:8], "big")
    tracks = []
    signatures = []
    while at + 8 <= len(data):
        kind, length = data[at : at + 4], int.from_bytes(data[at + 4 : at + 8], "big")
        chunk, at = data[at + 8 : at + 8 + length], at + 8 + length
        if kind != b"MTrk":
            continue
        name = ""
        onsets: dict[int, int] = {}
        sounding: dict[int, int] = {}
        notes: list[tuple[int, int, int]] = []
        tick, position, status = 0, 0, 0
        while position < len(chunk):
            delta, position = _varlen(chunk, position)
            tick += delta
            byte = chunk[position]
            if byte == 0xFF:
                meta, position = chunk[position + 1], position + 2
                size, position = _varlen(chunk, position)
                body, position = chunk[position : position + size], position + size
                if meta == 0x58 and size >= 2:
                    signatures.append((tick, body[0], 2 ** body[1]))
                elif meta == 0x03:
                    name = body.decode("utf-8", "replace").strip("\x00 ")
                continue
            if byte in (0xF0, 0xF7):
                size, position = _varlen(chunk, position + 1)
                position += size
                continue
            if byte & 0x80:
                status, position = byte, position + 1
            kind_of, channel = status & 0xF0, status & 0x0F
            width = 1 if kind_of in (0xC0, 0xD0) else 2
            values, position = chunk[position : position + width], position + width
            if channel == 9 or len(values) != 2:
                continue
            if kind_of == 0x90 and values[1] > 0:
                onsets[tick] = max(onsets.get(tick, 0), values[0])
                sounding.setdefault(values[0], tick)
            elif kind_of in (0x80, 0x90) and values[0] in sounding:
                notes.append((sounding.pop(values[0]), tick, values[0]))
        tracks.append({"name": name, "onsets": onsets, "notes": notes})
    return division, tracks, sorted(set(signatures))


def midi_melody(path: Path) -> list[tuple[float, int]]:
    """The first non-drum track holding notes, as (onset in quarters, top pitch at each onset).

    A score exported from notation software puts its parts in score order, so the first part is
    the melody of a lead sheet and the right hand's top line of a piano arrangement.
    """
    read = midi_tracks(path)
    if read is None:
        return []
    division, tracks, _ = read
    for track in tracks:
        if len(track["onsets"]) >= 2:
            return [(t / division, p) for t, p in sorted(track["onsets"].items())]
    return []


def midi_part(path: Path) -> tuple[str, list[dict], list[dict]] | None:
    """The track `midi_melody` reads, with lengths: its name, notes and signature points.

    Notes and signature points are in a project's own shape and ticks, so `melody_form.py` cuts
    them into lines exactly as it cuts a saved song.
    """
    read = midi_tracks(path)
    if read is None:
        return None
    division, tracks, signatures = read

    def ticks(value: int) -> int:
        return round(value * TICKS_PER_QUARTER / division)

    for track in tracks:
        if len(track["onsets"]) < 2:
            continue
        notes = [
            {"at": ticks(on), "length": ticks(off) - ticks(on), "pitch": pitch}
            for on, off, pitch in track["notes"]
        ]
        points = [
            {"tick": ticks(at), "signature": {"numerator": n, "denominator": d}}
            for at, n, d in signatures
        ]
        if not points or points[0]["tick"] > 0:
            points.insert(
                0, {"tick": 0, "signature": {"numerator": 4, "denominator": 4}}
            )
        return track["name"], notes, points
    return None


def is_voice(name: str) -> bool:
    """Whether a track's name says it is sung, in the languages the corpus's scores use."""
    return any(word in name.lower() for word in VOICE_WORDS)


def song_lines(notes: list[dict], points: list[dict]) -> dict | None:
    """One corpus song's lines: how many, how long, and where they enter the bar."""
    meter = Meter({"signatures": {"points": points}})
    lines = cut_lines(top_line(notes), [], meter)
    if not lines:
        return None
    spacings = Counter(b["at"] - a["at"] for line in lines for a, b in pairwise(line))
    total = sum(spacings.values())
    return {
        "lines": len(lines),
        "spacings": total,
        "most_common_spacing_share": max(spacings.values()) / total if total else None,
        "notes_per_line": [len(line) for line in lines],
        "bars_per_line": [
            (line[-1]["at"] + line[-1]["length"] - line[0]["at"])
            / meter.bar_length(line[0]["at"])
            for line in lines
        ],
        **entry_spread([line[0]["at"] for line in lines], meter),
    }


def percentiles(values: list[float]) -> dict[str, float]:
    """The values at `PERCENTILES`, nearest rank."""
    ordered = sorted(values)
    return {
        str(p): round(ordered[min(len(ordered) - 1, len(ordered) * p // 100)], 3)
        for p in PERCENTILES
    }


def line_statistics(songs: list[dict]) -> dict:
    """How a corpus's songs spread their line entries, from each song's `song_lines`.

    Songs with fewer lines than the entry warnings need are left out, as the warnings leave
    them out.
    """
    counted = [s for s in songs if s["lines"] >= PARAMETERS["uniform_min_lines"]]
    if not counted:
        return {"songs": 0}
    shares = [song["most_common_entry_share"] for song in counted]
    pickups = [song["second_half_entries"] / song["lines"] for song in counted]
    threshold = PARAMETERS["uniform_entry_share"]
    spacings = [
        song["most_common_spacing_share"]
        for song in counted
        if song["spacings"] >= PARAMETERS["uniform_ioi_min_count"]
    ]
    return {
        "songs": len(counted),
        "lines_per_song": percentiles([song["lines"] for song in counted]),
        "bars_per_line": percentiles([b for s in counted for b in s["bars_per_line"]]),
        "notes_per_line": percentiles(
            [n for s in counted for n in s["notes_per_line"]]
        ),
        "most_common_entry_share": percentiles(shares),
        "songs_at_or_over_uniform_entry_share": round(
            sum(share >= threshold for share in shares) / len(counted), 3
        ),
        "second_half_entry_share": percentiles(pickups),
        "songs_without_a_second_half_entry": round(
            sum(share == 0 for share in pickups) / len(counted), 3
        ),
        "most_common_spacing_share": percentiles(spacings) if spacings else None,
        "songs_at_or_over_uniform_spacing_share": round(
            sum(share >= PARAMETERS["uniform_ioi_share"] for share in spacings)
            / len(spacings),
            3,
        )
        if spacings
        else None,
    }


def melody(path: Path) -> list[tuple[float, int]]:
    """A corpus file's melody, read by its extension."""
    if path.suffix.lower() in (".mid", ".midi"):
        return midi_melody(path)
    return musicxml_melody(path)


def corpus_files(folders: list[Path]) -> list[Path]:
    """Every MusicXML file under `folders`, in a stable order, one per song.

    A song shipped in several formats (PJS has `.musicxml`, `.mid` and once `.xml`) is counted
    once, from the first found: MusicXML before MIDI, since it keeps ties and voices apart.
    """
    files: dict[Path, Path] = {}
    for folder in folders:
        found = (
            [folder]
            if folder.is_file()
            else [
                *sorted(folder.rglob("*.musicxml")),
                *sorted(folder.rglob("*.xml")),
                *sorted(folder.rglob("*.mid")),
            ]
        )
        for path in found:
            files.setdefault(path.with_suffix(""), path)
    return sorted(files.values())


def train(sequences: list[list[tuple[int, int]]], order: int) -> PPM:
    """A PPM model that has learnt every sequence, each from its own start."""
    model = PPM(order, ALPHABET)
    for sequence in sequences:
        for index, item in enumerate(sequence):
            model.learn(sequence[max(0, index - order) : index], item)
    return model


def mean_information(model: PPM, sequence: list[tuple[int, int]]) -> float | None:
    """Mean bits per note of `sequence` under a frozen `model`."""
    values = [
        model.information(sequence[max(0, i - model.order) : i], item)
        for i, item in enumerate(sequence)
    ]
    return sum(values) / len(values) if values else None


def cross_validate(sequences: list[list[tuple[int, int]]], order: int) -> list[float]:
    """Each song's mean bits per note on a model trained on the other folds."""
    scores = []
    for fold in range(FOLDS):
        held = [s for i, s in enumerate(sequences) if i % FOLDS == fold]
        model = train([s for i, s in enumerate(sequences) if i % FOLDS != fold], order)
        scores.extend(v for s in held if (v := mean_information(model, s)) is not None)
    return scores


def save(model: PPM, path: Path, corpus: dict) -> None:
    counts = [
        [list(map(list, context)), [[list(s), c] for s, c in table.items()]]
        for context, table in model.counts.items()
    ]
    path.write_text(
        json.dumps(
            {
                "schema_version": SCHEMA_VERSION,
                "order": model.order,
                "corpus": corpus,
                "counts": counts,
            },
            ensure_ascii=False,
        ),
        encoding="utf-8",
    )


def load(path: Path) -> tuple[PPM, dict]:
    data = json.loads(path.read_text(encoding="utf-8"))
    model = PPM(data["order"], ALPHABET)
    for context, table in data["counts"]:
        model.counts[tuple(tuple(c) for c in context)] = Counter(
            {tuple(s): c for s, c in table}
        )
    return model, data["corpus"]


def song_symbols(project: dict, wanted: str | None) -> list[tuple[str | None, list]]:
    """Each line of a project's foreground as (section, symbols arriving at its notes)."""
    track = pick_foreground(project, wanted)
    if track is None:
        return []
    result = []
    previous = None
    for line in lines_of(project, track):
        items = []
        for note in line.notes:
            here = (note["at"] / TICKS_PER_QUARTER, note["pitch"])
            if previous is not None:
                items.append(symbol(previous, here))
            previous = here
        result.append((line.section, items))
    return result


def score(model: PPM, corpus: dict, project: dict, wanted: str | None) -> dict:
    """Per-line long-term information content of a project, with the corpus's spread beside it."""
    history: list = []
    lines = []
    every = []
    for section, items in song_symbols(project, wanted):
        values = []
        for item in items:
            values.append(model.information(history[-model.order :], item))
            history.append(item)
        every.extend(values)
        lines.append(
            {
                "section": section,
                "bits_per_note": round(sum(values) / len(values), 3)
                if values
                else None,
            }
        )
    mean = sum(every) / len(every) if every else None
    spread = corpus["cross_validated_sd"] or 1.0
    return {
        "lines": lines,
        "mean_bits_per_note": round(mean, 3) if mean is not None else None,
        "z_against_corpus": round((mean - corpus["cross_validated_mean"]) / spread, 2)
        if mean is not None
        else None,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    trainer = commands.add_parser(
        "train", help="count a long-term model from MusicXML folders"
    )
    trainer.add_argument("folders", nargs="+", type=Path)
    trainer.add_argument("--out", type=Path, required=True)
    trainer.add_argument("--name", default=None, help="the corpus's name in the model")
    scorer = commands.add_parser("score", help="score saved .auris projects")
    scorer.add_argument("songs", nargs="+", type=Path)
    scorer.add_argument("--model", type=Path, required=True)
    scorer.add_argument("--track", default=None)
    liner = commands.add_parser(
        "lines", help="measure where a corpus's sung lines enter the bar"
    )
    liner.add_argument("folders", nargs="+", type=Path)
    liner.add_argument(
        "--any-part",
        action="store_true",
        help="count files whose first melodic track is not named as a voice too",
    )
    arguments = parser.parse_args()
    if arguments.command == "train":
        files = corpus_files(arguments.folders)
        sequences = [s for f in files if len(s := symbols(melody(f))) >= 2]
        order = PARAMETERS["ppm_order"]
        scores = cross_validate(sequences, order)
        corpus = {
            "name": arguments.name or ", ".join(str(f) for f in arguments.folders),
            "songs": len(sequences),
            "notes": sum(len(s) for s in sequences),
            "cross_validated_mean": round(statistics.mean(scores), 3),
            "cross_validated_sd": round(statistics.pstdev(scores), 3),
        }
        save(train(sequences, order), arguments.out, corpus)
        print(json.dumps(corpus, ensure_ascii=False))
    elif arguments.command == "lines":
        songs = []
        for path in corpus_files(arguments.folders):
            if path.suffix.lower() not in (".mid", ".midi"):
                continue
            part = midi_part(path)
            if part is None or not (arguments.any_part or is_voice(part[0])):
                continue
            if (song := song_lines(part[1], part[2])) is not None:
                songs.append(song)
        parameters = {
            key: PARAMETERS[key]
            for key in (
                "line_gap_ticks",
                "long_line_bars",
                "split_rest_ticks",
                "uniform_min_lines",
                "uniform_entry_share",
                "uniform_ioi_share",
                "uniform_ioi_min_count",
            )
        }
        print(
            json.dumps(
                {"parameters": parameters, **line_statistics(songs)},
                ensure_ascii=False,
                indent=1,
            )
        )
    else:
        model, corpus = load(arguments.model)
        for path in arguments.songs:
            project = json.loads(path.read_text(encoding="utf-8"))
            project = project.get("project", project)
            report = score(model, corpus, project, arguments.track)
            print(
                json.dumps(
                    {"path": str(path), "corpus": corpus["name"], **report},
                    ensure_ascii=False,
                )
            )


if __name__ == "__main__":
    main()
