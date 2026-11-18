"""Contract tests for performance_stats.py, on hand-built performances."""

import sys
from pathlib import Path

import mido
import pytest

sys.path.insert(0, str(Path(__file__).parent))
import performance_stats as ps


def write_midi(path: Path, events: list[tuple[float, mido.Message]]) -> None:
    """Writes messages at absolute seconds, at 120 BPM and 480 ticks a beat."""
    midi = mido.MidiFile(ticks_per_beat=480)
    track = mido.MidiTrack()
    midi.tracks.append(track)
    last = 0
    for seconds, message in sorted(events, key=lambda event: event[0]):
        tick = round(seconds * 960)
        track.append(message.copy(time=tick - last))
        last = tick
    midi.save(path)


def test_a_voiced_chord_reads_as_its_soften_and_lead(tmp_path):
    events = []
    for bar in range(60):
        at = bar * 1.0
        events += [
            (at, mido.Message("note_on", note=72, velocity=100)),
            (at + 0.010, mido.Message("note_on", note=64, velocity=75)),
            (at + 0.010, mido.Message("note_on", note=48, velocity=90)),
            (at + 0.5, mido.Message("note_off", note=72)),
            (at + 0.5, mido.Message("note_off", note=64)),
            (at + 0.5, mido.Message("note_off", note=48)),
        ]
    path = tmp_path / "chords.midi"
    write_midi(path, events)
    stats = ps.piano_stats(ps.read_piano(path))
    assert stats["inner_ratio"][0] == pytest.approx(0.75)
    assert stats["bass_ratio"][0] == pytest.approx(0.90)
    assert stats["lead_ms"][0] == pytest.approx(10.0, abs=1.5)
    # Inner 25% softer and bass 10% softer is the voicing stage at full strength.
    per_piece = {key: ps.summary(values) for key, values in stats.items() if values}
    implied = ps.amounts(per_piece, ps.summary([20.0]))
    assert implied["voicing_from_inner"] == pytest.approx(1.0)
    assert implied["voicing_from_bass"] == pytest.approx(1.0)


def test_a_note_released_under_the_pedal_sounds_until_the_lift():
    notes = [[0.0, 0.5, 0.5, 60, 80], [1.0, 1.5, 1.5, 62, 80]]
    ps.sustain_under_pedal(notes, [(0.2, True), (0.9, False)])
    assert notes[0][2] == 0.9
    assert notes[1][2] == 1.5


def test_a_rest_longer_than_a_quarter_ends_a_phrase():
    line = [[0.0, 0.4, 0.4, 60, 80], [0.4, 0.8, 0.8, 62, 80], [1.4, 1.8, 1.8, 64, 80]]
    assert [len(phrase) for phrase in ps.phrases(line)] == [2, 1]


def test_a_strum_spans_its_first_and_last_string():
    jams = {
        "annotations": [
            {
                "namespace": "note_midi",
                "annotation_metadata": {"data_source": str(string)},
                "data": [
                    {"time": 1.0 + 0.004 * string},
                    {"time": 3.0 + 0.006 * string},
                ],
            }
            for string in range(6)
        ]
        + [
            {
                "namespace": "pitch_contour",
                "annotation_metadata": {"data_source": "0"},
                "data": [],
            }
        ]
    }
    assert ps.strum_spreads(jams) == pytest.approx([20.0, 30.0])
