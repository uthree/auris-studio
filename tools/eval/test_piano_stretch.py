"""Guards for the piano tuning measurements: they must read known strings back."""

import json

import numpy as np
import piano_register as register
import piano_stretch as stretch
import pytest
import soundfile as sf

RATE = stretch.RATE
LEVELS = (1.0, 0.5, 0.25, 0.125)


def sharp(stiffness, n):
    """Cents a stiff string's `n`th partial lies above `n` times its first."""
    return 600 * np.log2((1 + stiffness * n * n) / (1 + stiffness))


def string(pitch, cents=0.0, stiffness=0.0, levels=LEVELS, seconds=2.5, falling=None, rate=RATE):
    """A stiff string whose first partial lies `cents` from equal temperament.

    Partial `n` is `levels[n - 1]` strong and falls `falling[n - 1]` decibels a second.
    """
    t = np.arange(round(seconds * rate)) / rate
    first = stretch.hz(pitch) * 2 ** (cents / 1200)
    falling = falling or (0.0,) * len(levels)
    audio = np.zeros(len(t))
    for n, (level, fall) in enumerate(zip(levels, falling, strict=True), start=1):
        hz = first * n * 2 ** (sharp(stiffness, n) / 1200)
        audio += level * 10 ** (-fall * t / 20) * np.sin(2 * np.pi * hz * t)
    return audio


def unison(pitch, strings, seconds=2.0):
    """The first partials of one key's strings, each `(cents, level)`, and the middle of their power."""
    audio = sum(string(pitch, cents=cents, levels=(level,), seconds=seconds) for cents, level in strings)
    power = np.array([level ** 2 for _, level in strings])
    hz = np.array([2 ** (cents / 1200) for cents, _ in strings])
    return audio, 1200 * np.log2((power * hz).sum() / power.sum())


def test_partials_are_read_where_they_lie_and_as_strong_as_they_are():
    stiffness = 3.8e-4
    spectrum = stretch.spectrum(string(60, cents=-2.3, stiffness=stiffness), RATE)
    reading = stretch.partial(*spectrum, stretch.hz(60))
    assert reading.cents == pytest.approx(-2.3, abs=0.02)
    assert reading.level == pytest.approx(0.0, abs=0.1)
    assert reading.clear == np.inf
    reading = stretch.partial(*spectrum, stretch.hz(60) * 4)
    assert reading.cents == pytest.approx(-2.3 + sharp(stiffness, 4), abs=0.02)
    assert reading.level == pytest.approx(20 * np.log10(LEVELS[3]), abs=0.1)


def test_a_weak_fundamental_is_not_pulled_by_its_neighbours():
    # A0 as the piano sounds it: the fundamental far under the partials above it. Through a
    # bare rectangle this one reads nearly five cents flat.
    levels = (0.015, 0.12, 0.3, 0.5, 0.45, 0.4, 0.3, 0.25, 0.2, 0.15)
    spectrum = stretch.spectrum(string(21, cents=-21.0, stiffness=3e-4, levels=levels, seconds=2.0), RATE)
    assert stretch.partial(*spectrum, stretch.hz(21)).cents == pytest.approx(-21.0, abs=0.1)


def test_nothing_is_read_from_silence_or_beyond_the_spectrum():
    assert stretch.partial(*stretch.spectrum(np.zeros(RATE), RATE), 440.0) is None
    assert stretch.partial(*stretch.spectrum(string(69), RATE), 30000.0) is None


def test_a_unison_reads_at_the_middle_of_its_power_whichever_string_is_strongest():
    # The three strings of a recorded C7 lie twelve cents apart.
    for strings in (((14.0, 1.0), (22.0, 0.54), (26.0, 0.52)), ((14.0, 0.52), (22.0, 0.54), (26.0, 1.0))):
        audio, middle = unison(96, strings)
        reading = stretch.partial(*stretch.spectrum(audio, RATE), stretch.hz(96))
        assert reading.cents == pytest.approx(middle, abs=0.1)
        strongest = max(strings, key=lambda each: each[1])[0]
        assert abs(reading.cents - strongest) > 2.0


def test_two_microphones_in_opposite_phase_still_hold_both_strings():
    one, _ = unison(72, ((-3.0, 1.0),))
    other, _ = unison(72, ((5.0, 0.8),))
    _, middle = unison(72, ((-3.0, 1.0), (5.0, 0.8)))
    microphones = np.stack([one + other, one - other], axis=1)
    assert stretch.partial(*stretch.spectrum(microphones, RATE), stretch.hz(72)).cents == pytest.approx(middle, abs=0.1)
    # Added as samples they are the first string alone.
    mixed = stretch.partial(*stretch.spectrum(microphones.mean(axis=1), RATE), stretch.hz(72))
    assert mixed.cents == pytest.approx(-3.0, abs=0.1)


@pytest.mark.parametrize("pitch", [21, 60, 96, 108])
def test_a_harmonic_note_is_heard_at_its_fundamental(pitch):
    # A bare sum of products reads A0 a quarter of a cent sharp, and whole lags read C7
    # three quarters of a cent flat.
    audio = string(pitch, cents=5.0, seconds=0.5)
    assert stretch.heard_cents(audio, RATE, pitch) == pytest.approx(5.0, abs=0.05)
    # Two microphones that cancel as samples are heard all the same.
    assert stretch.heard_cents(np.stack([audio, -audio], axis=1), RATE, pitch) == pytest.approx(5.0, abs=0.05)


def test_a_stiff_string_is_heard_above_its_fundamental():
    stiffness, levels = 1.4e-4, (0.2, 0.5, 1.0, 1.0, 0.8, 0.6, 0.4, 0.3)
    heard = stretch.heard_cents(string(36, stiffness=stiffness, levels=levels, seconds=0.5), RATE, 36)
    assert 1.0 < heard < sharp(stiffness, len(levels))


@pytest.mark.parametrize(("pitch", "stiffness"), [(21, 2e-4), (96, 8.6e-3)])
def test_a_note_that_holds_its_period_reads_the_same_under_any_weights(pitch, stiffness):
    # A window overlaps itself less as the lag grows. Left in the reading, that alone puts a
    # bass note of four partials a cent and a half sharp.
    plain, weighted = stretch.heard_readings(string(pitch, stiffness=stiffness, seconds=0.5), RATE, pitch)
    assert abs(plain - weighted) < 0.1


def test_a_period_that_moves_parts_the_two_readings():
    # The sharp overtone is gone before the half second is, and takes the pitch down with it.
    dying = string(96, cents=20.0, stiffness=8.6e-3, levels=(1.0, 0.6), seconds=0.5, falling=(30.0, 200.0))
    plain, weighted = stretch.heard_readings(dying, RATE, 96)
    assert plain - weighted > 2 * stretch.HEARD_TOLERANCE


def test_a_note_too_short_for_its_stretches_is_refused():
    struck = string(60, cents=-2.3, stiffness=3.8e-4)
    note = stretch.Note(struck, RATE)
    assert note.partial(stretch.hz(60)).cents == pytest.approx(-2.3, abs=0.02)
    assert len(note.early) == round(0.5 * RATE)
    with pytest.raises(ValueError, match="shorter than the stretch"):
        stretch.Note(struck[:RATE], RATE)


def test_an_octave_is_the_lower_partial_minus_the_upper():
    low, high = 1.6e-4, 3.8e-4
    lower = stretch.Note(string(48, cents=-3.0, stiffness=low), RATE)
    upper = stretch.Note(string(60, cents=-2.0, stiffness=high), RATE)
    two_one = stretch.octave(lower, upper, stretch.hz(48), 2)
    four_two = stretch.octave(lower, upper, stretch.hz(48), 4)
    assert two_one == pytest.approx(-3.0 + sharp(low, 2) + 2.0, abs=0.03)
    assert four_two == pytest.approx(-3.0 + sharp(low, 4) + 2.0 - sharp(high, 2), abs=0.03)


def test_a_bass_octave_is_read_on_the_six_to_three_and_the_eight_to_four():
    low, high = 2.6e-4, 1.4e-4
    lower = stretch.Note(string(24, cents=-16.0, stiffness=low, levels=(0.2, 0.9, 1.0, 0.2, 0.5, 0.8, 0.3, 0.3)), RATE)
    upper = stretch.Note(string(36, cents=-7.0, stiffness=high), RATE)
    assert stretch.octave(lower, upper, stretch.hz(24), 6) == pytest.approx(-9.0 + sharp(low, 6) - sharp(high, 3), abs=0.03)
    assert stretch.octave(lower, upper, stretch.hz(24), 8) == pytest.approx(-9.0 + sharp(low, 8) - sharp(high, 4), abs=0.03)


def test_an_octave_is_not_judged_by_a_partial_too_weak_to_hear():
    lower = stretch.Note(string(84, levels=(1.0, 0.5, 0.25, 1e-4)), RATE)
    upper = stretch.Note(string(96), RATE)
    assert stretch.octave(lower, upper, stretch.hz(84), 2) is not None
    assert stretch.octave(lower, upper, stretch.hz(84), 4) is None


def test_a_line_the_recording_holds_without_the_note_is_not_a_partial():
    # A steady line where the fourth partial would be, as loud after the note as during it.
    rng = np.random.default_rng(20261110)
    line = 2e-3 * np.sin(2 * np.pi * stretch.hz(84) * 4 * np.arange(round(5.0 * RATE)) / RATE)
    audio = line + rng.normal(0.0, 1e-5, len(line))
    audio[:round(2.5 * RATE)] += string(84, levels=(1.0, 0.5))
    taken_at_its_word = stretch.Note(audio, RATE)
    assert taken_at_its_word.partial(stretch.hz(84) * 4) is not None
    heard_out = stretch.Note(audio, RATE, noise=audio)
    assert heard_out.partial(stretch.hz(84) * 4) is None
    assert heard_out.partial(stretch.hz(84) * 2).clear > 60.0
    assert stretch.octave(heard_out, heard_out, stretch.hz(84), 4) is None


class Strings:
    """Stands in for the worker: stiff strings a known distance from equal temperament.

    The whole keyboard lies five cents above 440 Hz, as a piano tuned to another pitch does.
    """

    STIFFNESS = 2e-4

    def __init__(self):
        self.calls = []

    @staticmethod
    def tuned(pitch):
        return 5.0 + 0.25 * (pitch - 69)

    def render(self, model, notes, params, seconds, hold, rate):
        self.calls.append((model, notes, params, seconds, hold, rate))
        return [string(note["pitch"], cents=self.tuned(note["pitch"]), stiffness=self.STIFFNESS, seconds=seconds)
                for note in notes]


@pytest.fixture(scope="module")
def keyboard():
    strings = Strings()
    return strings, stretch.played(strings)


def test_every_key_is_measured_from_one_render_of_the_factory_piano(keyboard):
    strings, notes = keyboard
    rows = stretch.measure(notes)
    assert [(row["key"], row["pitch"]) for row in rows] == list(stretch.KEYS)
    ((model, played, params, seconds, hold, rate),) = strings.calls
    assert (model, params, seconds, hold, rate) == ("piano", {}, 2.5, 2.5, 48000)
    assert [note["pitch"] for note in played] == [21, 23, 24, 33, 35, 36, 45, 48, 57, 60, 69, 72, 81, 84, 96, 108]
    assert all(note["velocity"] == 0.7 for note in played)
    for row in rows:
        assert row["fundamental"] == pytest.approx(Strings.tuned(row["pitch"]), abs=0.02)
        assert row["fundamental"] < row["heard"] < row["fundamental"] + sharp(Strings.STIFFNESS, 4)
        assert row["heard_readings"][0] == row["heard"]
    # An octave of these strings is three cents wide, less what stiffness adds to the lower one.
    for row in rows[:-1]:
        assert row["octave_2_1"] == pytest.approx(sharp(Strings.STIFFNESS, 2) - 3.0, abs=0.03)
        assert row["octave_4_2"] == pytest.approx(sharp(Strings.STIFFNESS, 4) - sharp(Strings.STIFFNESS, 2) - 3.0, abs=0.03)
    # These strings have four partials, so nothing is read on the sixth and the eighth.
    assert all(row["octave_6_3"] is None and row["octave_8_4"] is None for row in rows[:-1])
    assert not any(label in rows[-1] for label, _ in stretch.OCTAVES)


def test_a_note_that_dies_within_its_stretch_gets_no_heard_pitch(keyboard):
    notes = dict(keyboard[1])
    for pitch, stiffness in ((96, 8.6e-3), (108, 1.3e-2)):
        dying = string(pitch, cents=Strings.tuned(pitch), stiffness=stiffness, falling=(30.0, 200.0, 200.0, 200.0))
        notes[pitch] = stretch.Note(dying, RATE)
    rows = stretch.measure(notes)
    assert [row["key"] for row in rows if row["heard"] is None] == ["C7", "C8"]
    assert all(len(row["heard_readings"]) == 2 for row in rows)


def test_a_key_that_is_not_there_leaves_its_row_and_its_octaves_empty(keyboard):
    notes = {pitch: note for pitch, note in keyboard[1].items() if pitch not in (21, 72)}
    rows = {row["key"]: row for row in stretch.measure(notes)}
    assert rows["A0"] == {"key": "A0", "pitch": 21, "fundamental": None, "heard": None, "heard_readings": None,
                          "octave_2_1": None, "octave_4_2": None, "octave_6_3": None, "octave_8_4": None}
    # C4 is there, but the C5 its octave is judged against is not.
    assert rows["C4"]["fundamental"] is not None and rows["C4"]["octave_2_1"] is None
    assert rows["C5"]["fundamental"] is None and rows["C6"]["octave_2_1"] is not None


def test_a_piano_tuned_to_another_pitch_is_measured_from_its_own_a4(keyboard):
    rows = stretch.measure(keyboard[1])
    octaves = [row.get("octave_2_1") for row in rows]
    heard = rows[0]["heard"]
    a4 = stretch.onto_its_own_a4(rows)
    assert a4 == pytest.approx(5.0, abs=0.02)
    for row in rows:
        assert row["fundamental"] == pytest.approx(0.25 * (row["pitch"] - 69), abs=0.03)
        assert row["heard_readings"][0] == row["heard"]
    assert rows[0]["heard"] == pytest.approx(heard - a4)
    assert [row.get("octave_2_1") for row in rows] == octaves
    next(row for row in rows if row["key"] == "A4")["fundamental"] = None
    with pytest.raises(ValueError, match="no A4"):
        stretch.onto_its_own_a4(rows)


def test_recordings_are_read_from_their_lock_with_their_own_noise(tmp_path, capsys):
    rate, rng, captures = 44100, np.random.default_rng(20261110), []
    for name, pitch in (("C4", 60), ("A4", 69), ("C5", 72), ("A5", 81)):
        note = string(pitch, cents=Strings.tuned(pitch), stiffness=Strings.STIFFNESS, rate=rate)
        audio = np.r_[np.zeros(round(0.2 * rate)), note, np.zeros(round(2.2 * rate))]
        audio = np.stack([audio, 0.5 * audio], axis=1) * 0.25 + rng.normal(0.0, 1e-4, (len(audio), 2))
        path = tmp_path / f"Piano.mf.{name}.aiff"
        sf.write(path, audio, rate, format="AIFF", subtype="PCM_16")
        captures.append({"path": path.name, "url": "", "pitch": pitch, "sha256": register.digest(path)})
    lock = tmp_path / "lock.json"
    lock.write_text(json.dumps({"captures": captures}), encoding="utf-8")
    # Nothing is fetched for a file that is already there and matches its hash.
    register.prepare(tmp_path, lock)
    assert capsys.readouterr().out.split() == [capture["path"] for capture in captures]
    notes = stretch.recorded(tmp_path, lock)
    assert sorted(notes) == [60, 69, 72, 81] and notes[69].rate == rate
    rows = {row["key"]: row for row in stretch.measure(notes)}
    assert [key for key, row in rows.items() if row["fundamental"] is not None] == ["C4", "A4", "C5"]
    assert rows["A4"]["fundamental"] == pytest.approx(5.0, abs=0.05)
    assert rows["A4"]["octave_2_1"] == pytest.approx(sharp(Strings.STIFFNESS, 2) - 3.0, abs=0.05)
    assert rows["C4"]["octave_4_2"] == pytest.approx(sharp(Strings.STIFFNESS, 4) - sharp(Strings.STIFFNESS, 2) - 3.0, abs=0.05)
    (tmp_path / "Piano.mf.C5.aiff").write_bytes(b"FORM")
    with pytest.raises(ValueError, match="hash mismatch"):
        stretch.recorded(tmp_path, lock)


def test_a_figure_is_shown_to_a_tenth_with_no_sign_on_a_zero():
    assert [stretch.shown(value) for value in (2.34, -15.56, -0.01, 0.04, None)] == ["+2.3", "-15.6", "0.0", "0.0", "-"]
