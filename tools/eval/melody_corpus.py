# /// script
# requires-python = ">=3.10"
# dependencies = []
# ///
"""How conventional a song's melody is against a corpus: an IDyOM-style long-term model.

    uv run tools/eval/melody_corpus.py train corpus/ --out pjs-model.json
    uv run tools/eval/melody_corpus.py score MySong/MySong.auris --model pjs-model.json

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

from melody_form import PARAMETERS, PPM, TICKS_PER_QUARTER, lines_of, pick_foreground

SCHEMA_VERSION = 1
ALPHABET = 25 * 16
FOLDS = 5
STEPS = {"C": 0, "D": 2, "E": 4, "F": 5, "G": 7, "A": 9, "B": 11}


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


def midi_melody(path: Path) -> list[tuple[float, int]]:
    """The first non-drum track holding notes, as (onset in quarters, top pitch at each onset).

    A score exported from notation software puts its parts in score order, so the first part is
    the melody of a lead sheet and the right hand's top line of a piano arrangement.
    """
    data = path.read_bytes()
    if data[:4] != b"MThd":
        return []
    division = int.from_bytes(data[12:14], "big")
    if division & 0x8000 or not division:
        return []
    at = 8 + int.from_bytes(data[4:8], "big")
    while at + 8 <= len(data):
        kind, length = data[at : at + 4], int.from_bytes(data[at + 4 : at + 8], "big")
        chunk, at = data[at + 8 : at + 8 + length], at + 8 + length
        if kind != b"MTrk":
            continue
        onsets: dict[int, int] = {}
        tick, position, status = 0, 0, 0
        while position < len(chunk):
            delta, position = _varlen(chunk, position)
            tick += delta
            byte = chunk[position]
            if byte == 0xFF:
                _, position = chunk[position + 1], position + 2
                size, position = _varlen(chunk, position)
                position += size
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
            if kind_of == 0x90 and channel != 9 and len(values) == 2 and values[1] > 0:
                onsets[tick] = max(onsets.get(tick, 0), values[0])
        if len(onsets) >= 2:
            return [(t / division, p) for t, p in sorted(onsets.items())]
    return []


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
