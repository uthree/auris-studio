"""Write the PJS corpus's sung lines as the input of the `accent_corpus` example.

PJS (Koguchi et al., 2020) is 100 short Japanese songs with a MusicXML score each, lyrics in
kana one note at a time. A line ends at every rest. Run with the folder holding pjs001..pjs100:

    uv run tools/eval/pjs_accent_lines.py <PJS_corpus_ver1.1> > pjs.jsonl
    cargo run --release -p auris-session --example accent_corpus -- pjs.jsonl
"""

import json
import sys
from pathlib import Path
from xml.etree import ElementTree

STEPS = {"C": 0, "D": 2, "E": 4, "F": 5, "G": 7, "A": 9, "B": 11}


def lines_of(score: Path) -> list[list[dict]]:
    lines: list[list[dict]] = []
    line: list[dict] = []
    for note in ElementTree.parse(score).getroot().iter("note"):
        if note.find("rest") is not None:
            if line:
                lines.append(line)
            line = []
            continue
        pitch = note.find("pitch")
        if pitch is None:
            continue
        midi = (
            12 * (int(pitch.findtext("octave")) + 1)
            + STEPS[pitch.findtext("step")]
            + int(float(pitch.findtext("alter") or 0))
        )
        tied = any(tie.get("type") == "stop" for tie in note.findall("tie"))
        kana = "" if tied else (note.findtext("lyric/text") or "").strip()
        if tied and line and line[-1]["pitch"] == midi:
            continue
        line.append({"pitch": midi, "kana": kana})
    if line:
        lines.append(line)
    return lines


def main() -> None:
    sys.stdout.reconfigure(encoding="utf-8")
    root = Path(sys.argv[1])
    for folder in sorted(root.glob("pjs[0-9][0-9][0-9]")):
        score = folder / f"{folder.name}.musicxml"
        if score.is_file():
            print(
                json.dumps(
                    {"song": folder.name, "lines": lines_of(score)}, ensure_ascii=False
                )
            )


if __name__ == "__main__":
    main()
