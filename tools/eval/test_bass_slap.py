"""Regression tests for the slapped-bass measures and the archive reader behind them."""

import io
import zipfile

import numpy as np
import pytest
from bass_slap import (
    RATE,
    Remote,
    difference,
    features,
    gather,
    members,
    note_name,
    summarise,
)


def note(hz, click=0.0, seconds=0.7):
    """A decaying tone at `hz` after a moment of silence, with a burst of noise at its start."""
    time = np.arange(int(seconds * RATE)) / RATE
    tone = np.sin(2 * np.pi * hz * time) * np.exp(-2 * time)
    burst = np.random.default_rng(1).standard_normal(len(time)) * np.exp(-time / 0.01)
    return np.concatenate([np.zeros(500), tone + click * burst])


def test_a_note_is_read_from_its_onset_and_its_fundamental_is_found():
    read = features(note(110.0), 110.0)
    # Nearly all of a pure tone is its fundamental, in the band from 90 to 180 Hz.
    assert read["fundamental_db"] > -0.5
    assert np.argmax(read["held_bands_db"]) == 1
    # Two nepers a second is 17.4 dB a second, over four tenths of one.
    assert read["fall_db"] == pytest.approx(6.95, abs=0.3)


def test_a_blow_is_heard_as_brightness_and_level_at_the_start_only():
    plain, struck = features(note(110.0), 110.0), features(note(110.0, click=3.0), 110.0)
    changed = difference(struck, plain)
    assert changed["attack_over_held_db"] > 3
    assert changed["crest_db"] > 3
    # The top of the attack comes up by tens of decibels; what is held is the same note.
    assert changed["attack_bands_db"][5] > 30
    assert abs(changed["held_bands_db"][1]) < 0.5
    assert abs(changed["fundamental_db"]) < 0.5


def test_a_note_too_short_to_measure_is_refused():
    with pytest.raises(ValueError, match="shorter"):
        features(note(110.0, seconds=0.3), 110.0)


def test_each_slapped_technique_is_summarised_against_the_same_notes_plucked():
    plain, struck = features(note(110.0), 110.0), features(note(110.0, click=3.0), 110.0)
    summary = summarise({"pluck": [plain, plain], "thumb": [struck, struck], "pop": [plain, plain]})
    assert summary["thumb_less_pluck"]["crest_db"]["mean"] > 3
    assert summary["pop_less_pluck"]["crest_db"] == {"mean": 0.0, "sd": 0.0}
    assert len(summary["thumb"]["attack_bands_db"]["mean"]) == 8
    assert gather([{"x": 1.0}, {"x": 3.0}]) == {"x": {"mean": 2.0, "sd": 1.0}}


def test_the_recorded_notes_are_named_as_the_dataset_names_them():
    assert note_name(2, "ST", 1, 12) == "BS_2_EQ_1_ST_NO_1_12.wav"


class Held(Remote):
    """An archive held in memory, counting what is asked of it."""

    def __init__(self, data):
        self.data, self.requests, self.fetched = data, 0, 0
        super().__init__("memory")

    def locate(self, url):
        return url, len(self.data)

    def fetch_range(self, start, end):
        self.requests += 1
        self.fetched += end + 1 - start
        return self.data[start:end + 1]


def test_members_are_read_out_of_an_archive_without_reading_the_archive():
    contents = {f"PS/ST/{index}.wav": bytes([index]) * 200_000 for index in range(8)}
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", zipfile.ZIP_STORED) as archive:
        for name, data in contents.items():
            archive.writestr(name, data)
    remote = Held(buffer.getvalue())
    wanted = ["PS/ST/6.wav", "PS/ST/1.wav"]
    assert list(members(remote, wanted)) == [contents[name] for name in wanted]
    # The directory and two members of eight: a little over a quarter of the archive, in a
    # handful of requests.
    assert remote.fetched < 0.4 * len(buffer.getvalue())
    assert remote.requests <= 6
