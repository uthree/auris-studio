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


def test_a_sung_midi_part_is_cut_into_lines_with_its_own_metre(tmp_path: Path) -> None:
    from melody_corpus import is_voice, midi_part, song_lines

    def track(events: bytes) -> bytes:
        return b"MTrk" + len(events).to_bytes(4, "big") + events

    # 3/4, a track named Voice: two quarters, a rest of a beat, then a quarter on beat 1.
    events = bytes([0, 0xFF, 0x58, 4, 3, 2, 24, 8, 0, 0xFF, 0x03, 5]) + b"Voice"
    for wait, pitch in [(0, 60), (0, 62), (96, 64)]:
        events += bytes([wait, 0x90, pitch, 90, 96, 0x80, pitch, 0])
    header = (
        b"MThd"
        + (6).to_bytes(4, "big")
        + (1).to_bytes(2, "big")
        + (1).to_bytes(2, "big")
    )
    path = tmp_path / "song.mid"
    path.write_bytes(header + (96).to_bytes(2, "big") + track(events))
    name, notes, points = midi_part(path)
    assert is_voice(name) and not is_voice("Piano")
    assert [(n["at"], n["length"], n["pitch"]) for n in notes] == [
        (0, 960, 60),
        (960, 960, 62),
        (2880, 960, 64),
    ]
    assert points == [{"tick": 0, "signature": {"numerator": 3, "denominator": 4}}]
    lines = song_lines(notes, points)
    # The second line starts the second bar of 3/4: both lines enter on beat 1.
    assert lines["lines"] == 2
    assert lines["most_common_entry_share"] == 1.0
    assert lines["second_half_entries"] == 0


def test_corpus_statistics_count_songs_with_enough_lines() -> None:
    from melody_corpus import line_statistics

    def one(lines: int, share: float, pickups: int) -> dict:
        return {
            "lines": lines,
            "notes_per_line": [8] * lines,
            "bars_per_line": [2.0] * lines,
            "most_common_entry_share": share,
            "second_half_entries": pickups,
            "spacings": 7 * lines,
            "most_common_spacing_share": share,
        }

    statistics = line_statistics(
        [one(10, 1.0, 0), one(10, 0.5, 4), one(20, 0.4, 10), one(4, 1.0, 0)]
    )
    # The four-line song is too short for the entry warnings and is not counted.
    assert statistics["songs"] == 3
    assert statistics["songs_at_or_over_uniform_entry_share"] == 0.333
    assert statistics["songs_without_a_second_half_entry"] == 0.333
    assert statistics["second_half_entry_share"]["50"] == 0.4
    # Spacing shares are read from songs with fifty spacings or more: all three here.
    assert statistics["songs_at_or_over_uniform_spacing_share"] == 0.333
