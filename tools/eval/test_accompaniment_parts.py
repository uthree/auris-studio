from pathlib import Path

import mido
from accompaniment_parts import (
    BARS,
    STEPS,
    bass_measures,
    bass_row,
    drum_measures,
    interval,
    read_midi,
    together_row,
    windows,
)


def eighths_on(pitch: int, bars: int = BARS) -> list[tuple[int, int, int, int]]:
    """A root pumped in eighths: (step, length in steps, pitch, velocity)."""
    return [
        (bar * STEPS + step, 2, pitch, 90)
        for bar in range(bars)
        for step in range(0, STEPS, 2)
    ]


def test_a_window_is_sixteen_bars_with_a_note_in_at_least_fourteen() -> None:
    assert len(list(windows(eighths_on(36)))) == 1
    assert len(list(windows(eighths_on(36, bars=32)))) == 2
    sparse = [note for note in eighths_on(36) if note[0] // STEPS < 10]
    assert list(windows(sparse)) == []
    # A window is counted from its own first bar, whatever bar of the file that was.
    late = [
        (step + 4 * STEPS, length, pitch, velocity)
        for step, length, pitch, velocity in eighths_on(36)
    ]
    assert min(step for step, *_ in next(iter(windows(late)))) == 0


def test_a_pumped_root_is_all_eighths_all_unisons_and_one_rhythm() -> None:
    measured = bass_measures(eighths_on(36))
    assert measured["notes_per_bar"] == 8
    assert measured["ioi_2"] == 1.0
    assert measured["interval_unison"] == 1.0
    assert measured["downbeat_struck"] == 1.0
    assert measured["sounding"] == 1.0
    assert measured["distinct_bar_rhythms"] == 1
    assert measured["bar_rhythm_same_as_previous"] == 1.0
    assert measured["notes_tied_over_bar_line"] == 0


def test_a_pushed_bass_note_is_a_tie_and_a_bar_whose_downbeat_is_not_struck() -> None:
    line = [note for note in eighths_on(36) if note[0] not in (STEPS - 2, STEPS)]
    # The last eighth of bar one is held over the line, through the first eighth of bar two.
    line.append((STEPS - 2, 4, 41, 90))
    measured = bass_measures(line)
    assert measured["notes_tied_over_bar_line"] == 1
    assert measured["downbeat_struck"] == 15 / 16
    assert measured["sounding"] == 1.0
    # A rest is counted as one: the same line with every note cut to a sixteenth sounds half.
    clipped = [
        (step, 1, pitch, velocity) for step, _, pitch, velocity in eighths_on(36)
    ]
    assert bass_measures(clipped)["sounding"] == 0.5
    assert bass_measures(clipped)["rest_after_note"] == 1.0


def rock_kit() -> list[tuple[int, int, int, int]]:
    notes = []
    for bar in range(BARS):
        at = bar * STEPS
        notes += [(at + step, 1, 36, 100) for step in (0, 6, 10)]
        notes += [(at + step, 1, 38, 100) for step in (4, 12)]
        notes += [(at + step, 1, 42, 80) for step in range(0, STEPS, 2)]
    return notes


def test_a_plain_groove_is_one_bar_with_no_fill_and_three_drums() -> None:
    measured = drum_measures(rock_kit())
    assert measured["whole_bar_distinct"] == 1
    assert measured["kick_hits_per_bar"] == 3
    assert measured["fill_bars"] == 0
    assert measured["uses_crash"] == measured["uses_toms"] == 0.0
    assert measured["voices_used"] == 3
    assert measured["kick_on_odd_sixteenth"] == 0.0


def test_a_fill_is_a_bar_three_strokes_busier_and_a_crash_is_counted_on_its_downbeat() -> (
    None
):
    kit = rock_kit()
    # A run through the last beat of bar eight, half of it on a tom, and a crash where it lands.
    kit += [(7 * STEPS + step, 1, 38, 100) for step in (13,)]
    kit += [(7 * STEPS + step, 1, 47, 100) for step in (14, 15)]
    kit.append((8 * STEPS, 1, 49, 110))
    measured = drum_measures(kit)
    assert measured["fill_bars"] == 1
    assert measured["fill_on_eighth_bar"] == 1.0
    assert measured["has_fill_before_bar_16"] == 1.0
    assert measured["uses_toms"] == measured["uses_crash"] == 1.0
    assert measured["bars_with_downbeat_crash"] == 1 / 16
    assert measured["whole_bar_distinct"] == 3
    # A ghost in every second bar does not hide it: a fill is counted against the commonest
    # bar, and the median of this kit is half a stroke above that.
    ghosted = kit + [(bar * STEPS + 7, 1, 38, 40) for bar in range(1, BARS, 2)]
    assert drum_measures(ghosted)["fill_bars"] == 1
    # Two extra strokes are a pickup, not a fill.
    pickup = rock_kit() + [(7 * STEPS + step, 1, 38, 100) for step in (14, 15)]
    assert drum_measures(pickup)["fill_bars"] == 0


def write_midi(path: Path, numerator: int, program: int, channel: int) -> None:
    midi = mido.MidiFile(ticks_per_beat=480)
    track = mido.MidiTrack()
    midi.tracks.append(track)
    track.append(
        mido.MetaMessage("time_signature", numerator=numerator, denominator=4, time=0)
    )
    track.append(
        mido.Message("program_change", channel=channel, program=program, time=0)
    )
    for _ in range(4):
        track.append(
            mido.Message("note_on", channel=channel, note=40, velocity=90, time=0)
        )
        track.append(
            mido.Message("note_off", channel=channel, note=40, velocity=0, time=240)
        )
        track.append(
            mido.Message("note_on", channel=channel, note=43, velocity=0, time=240)
        )
    midi.save(path)


def test_a_midi_file_is_read_in_sixteenths_and_only_in_four_four(
    tmp_path: Path,
) -> None:
    write_midi(tmp_path / "four.mid", 4, 33, 0)
    write_midi(tmp_path / "three.mid", 3, 33, 0)
    write_midi(tmp_path / "kit.mid", 4, 0, 9)
    ((is_drum, program, notes),) = read_midi(tmp_path / "four.mid")
    assert (is_drum, program) == (False, 33)
    # An eighth at 480 ticks to the quarter is two sixteenths; the next begins a quarter on.
    assert [(step, length, pitch) for step, length, pitch, _ in notes] == [
        (0, 2, 40),
        (4, 2, 40),
        (8, 2, 40),
        (12, 2, 40),
    ]
    assert read_midi(tmp_path / "three.mid") is None
    assert read_midi(tmp_path / "kit.mid")[0][0] is True


def test_a_bass_note_is_named_by_its_place_in_the_chord_under_it() -> None:
    # One bar of C and one of G: (start, length, root, bass, classes), in ticks and semitones.
    chords = [[0, 3840, 0, 0, [0, 4, 7]], [3840, 3840, 7, 7, [7, 11, 2]]]
    take = {
        "notes": [
            [0, 960, 36, 0.8, ""],
            [960, 960, 43, 0.8, ""],
            [1920, 960, 40, 0.8, ""],
            [2880, 960, 41, 0.8, ""],
            [3840, 3840, 43, 0.8, ""],
        ]
    }
    row = bass_row(take, chords)
    assert row["role root"] == 2 / 5
    assert row["role fifth"] == 1 / 5
    assert row["role third"] == 1 / 5
    assert row["role non-chord"] == 1 / 5


def test_a_corpus_interval_resamples_files_not_windows() -> None:
    # Two files that disagree, one of them cut into nine windows: the mean of the windows is
    # 0.9, and the interval still reaches from one file to the other.
    rows = [(0, {"sounding": 1.0})] * 9 + [(1, {"sounding": 0.0})]
    low, high = interval(rows)["sounding"]
    assert low == 0.0
    assert high == 1.0
    # One file is no evidence of a spread at all.
    assert interval([(0, {"sounding": 1.0})] * 9) == {}


def test_parts_are_read_against_each_other_where_they_are_struck() -> None:
    # A tune on E4 for a bar. The chords strike C-E-G under it, then C-F-A: F is a semitone
    # from the tune. The bass holds C2, and the pad strikes a B1 under it.
    lead = [[0, 3840, 64, 0.8, ""]]
    chords = [[0, 1920, pitch, 0.7, ""] for pitch in (48, 52, 55)]
    chords += [[1920, 1920, pitch, 0.7, ""] for pitch in (48, 53, 57)]
    bass = [[0, 3840, 36, 0.8, ""]]
    pad = [[0, 3840, 35, 0.6, ""]]
    arp = [[0, 480, 72, 0.6, ""], [480, 480, 52, 0.6, ""]]
    row = together_row(lead, chords, bass, arp, pad)
    assert row["chords: strikes a semitone from the tune"] == 0.5
    assert row["chords: strikes with a note above the tune"] == 0.0
    assert row["pad: strikes with a note under the bass"] == 1.0
    assert row["chords: strikes with a note under the bass"] == 0.0
    assert row["arp: strikes with a note above the tune"] == 0.5
    assert row["arp: notes inside the chords' voicing"] == 0.5
