from pathlib import Path

from melody_corpus import (
    corpus_files,
    cross_validate,
    mean_information,
    musicxml_melody,
    symbols,
    train,
)

SCORE = """<?xml version="1.0"?>
<score-partwise><part id="P1">
<measure number="1"><attributes><divisions>2</divisions></attributes>
<note><pitch><step>C</step><octave>4</octave></pitch><duration>2</duration><voice>1</voice></note>
<note><pitch><step>E</step><octave>4</octave></pitch><duration>2</duration><voice>1</voice><tie type="start"/></note>
<note><pitch><step>E</step><octave>4</octave></pitch><duration>2</duration><voice>1</voice><tie type="stop"/></note>
<note><rest/><duration>1</duration><voice>1</voice></note>
<note><pitch><step>G</step><alter>1</alter><octave>4</octave></pitch><duration>1</duration><voice>1</voice></note>
<note><chord/><pitch><step>B</step><octave>4</octave></pitch><duration>1</duration><voice>1</voice></note>
</measure></part></score-partwise>
"""


def test_a_tie_is_one_note_a_rest_moves_time_and_a_chord_keeps_its_top(
    tmp_path: Path,
) -> None:
    path = tmp_path / "song.musicxml"
    path.write_text(SCORE, encoding="utf-8")
    assert musicxml_melody(path) == [(0.0, 60), (1.0, 64), (3.5, 71)]
    assert symbols(musicxml_melody(path)) == [(4, 4), (7, 10)]


def test_a_song_shipped_twice_is_read_once(tmp_path: Path) -> None:
    for name in ("a.musicxml", "a.xml", "b.xml"):
        (tmp_path / name).write_text(SCORE, encoding="utf-8")
    assert [p.name for p in corpus_files([tmp_path])] == ["a.musicxml", "b.xml"]


def test_a_commonplace_is_cheaper_than_a_novelty_on_a_model_that_never_saw_either() -> (
    None
):
    scale = [(i, 60 + step) for i, step in enumerate([0, 2, 4, 5, 7, 5, 4, 2, 0])]
    corpus = [symbols(scale)] * 10
    model = train(corpus, 4)
    leap = [(i, 60 + step) for i, step in enumerate([0, 11, -1, 10, 3, 9, 1])]
    assert mean_information(model, symbols(scale)) < 1.0
    assert mean_information(model, symbols(leap)) > 5.0
    # Each fold scores songs it never learnt; identical songs still find their twins.
    assert all(v < 1.0 for v in cross_validate(corpus, 4))


def test_a_midi_file_gives_its_first_melodic_track_and_skips_drums(
    tmp_path: Path,
) -> None:
    from melody_corpus import midi_melody

    def track(events: bytes) -> bytes:
        return b"MTrk" + len(events).to_bytes(4, "big") + events

    drums = bytes([0, 0x99, 36, 100, 96, 0x89, 36, 0])
    # C then E a quarter later (96 ticks), the E with a G under it; running status on the G.
    lead = bytes(
        [0, 0x90, 60, 90, 96, 0x80, 60, 0, 0, 0x90, 64, 90, 0, 55, 90, 96, 0x80, 64, 0]
    )
    header = (
        b"MThd"
        + (6).to_bytes(4, "big")
        + (1).to_bytes(2, "big")
        + (2).to_bytes(2, "big")
    )
    path = tmp_path / "song.mid"
    path.write_bytes(header + (96).to_bytes(2, "big") + track(drums) + track(lead))
    assert midi_melody(path) == [(0.0, 60), (1.0, 64)]
