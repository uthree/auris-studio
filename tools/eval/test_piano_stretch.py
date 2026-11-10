"""Guards for the piano tuning measurements: they must read known strings back."""

import numpy as np
import piano_stretch as stretch
import pytest

RATE = stretch.RATE
LEVELS = (1.0, 0.5, 0.25, 0.125)


def sharp(stiffness, n):
    """Cents a stiff string's `n`th partial lies above `n` times its first."""
    return 600 * np.log2((1 + stiffness * n * n) / (1 + stiffness))


def string(pitch, cents=0.0, stiffness=0.0, levels=LEVELS, seconds=2.0, falling=None):
    """A stiff string whose first partial lies `cents` from equal temperament.

    Partial `n` is `levels[n - 1]` strong and falls `falling[n - 1]` decibels a second.
    """
    t = np.arange(round(seconds * RATE)) / RATE
    first = stretch.hz(pitch) * 2 ** (cents / 1200)
    falling = falling or (0.0,) * len(levels)
    audio = np.zeros(len(t))
    for n, (level, fall) in enumerate(zip(levels, falling, strict=True), start=1):
        hz = first * n * 2 ** (sharp(stiffness, n) / 1200)
        audio += level * 10 ** (-fall * t / 20) * np.sin(2 * np.pi * hz * t)
    return audio


def test_partials_are_read_where_they_lie_and_as_strong_as_they_are():
    stiffness = 3.8e-4
    spectrum = stretch.spectrum(string(60, cents=-2.3, stiffness=stiffness), RATE)
    cents, level = stretch.partial(*spectrum, stretch.hz(60))
    assert cents == pytest.approx(-2.3, abs=0.02)
    assert level == pytest.approx(0.0, abs=0.1)
    cents, level = stretch.partial(*spectrum, stretch.hz(60) * 4)
    assert cents == pytest.approx(-2.3 + sharp(stiffness, 4), abs=0.02)
    assert level == pytest.approx(20 * np.log10(LEVELS[3]), abs=0.1)


def test_a_weak_fundamental_is_not_pulled_by_its_neighbours():
    # A0 as the piano sounds it: the fundamental far under the partials above it. Through a
    # bare rectangle this one reads nearly five cents flat.
    levels = (0.015, 0.12, 0.3, 0.5, 0.45, 0.4, 0.3, 0.25, 0.2, 0.15)
    spectrum = stretch.spectrum(string(21, cents=-21.0, stiffness=3e-4, levels=levels), RATE)
    assert stretch.partial(*spectrum, stretch.hz(21))[0] == pytest.approx(-21.0, abs=0.1)


def test_nothing_is_read_from_silence_or_beyond_the_spectrum():
    assert stretch.partial(*stretch.spectrum(np.zeros(RATE), RATE), 440.0) is None
    assert stretch.partial(*stretch.spectrum(string(69), RATE), 30000.0) is None


@pytest.mark.parametrize("pitch", [21, 60, 96, 108])
def test_a_harmonic_note_is_heard_at_its_fundamental(pitch):
    # A bare sum of products reads A0 a quarter of a cent sharp, and whole lags read C7
    # three quarters of a cent flat.
    audio = string(pitch, cents=5.0, seconds=0.5)
    assert stretch.heard_cents(audio, RATE, pitch) == pytest.approx(5.0, abs=0.05)


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


def test_an_octave_is_the_lower_partial_minus_the_upper():
    low, high = 1.6e-4, 3.8e-4
    lower = stretch.spectrum(string(48, cents=-3.0, stiffness=low), RATE)
    upper = stretch.spectrum(string(60, cents=-2.0, stiffness=high), RATE)
    two_one = stretch.octave(lower, upper, stretch.hz(48), 2)
    four_two = stretch.octave(lower, upper, stretch.hz(48), 4)
    assert two_one == pytest.approx(-3.0 + sharp(low, 2) + 2.0, abs=0.03)
    assert four_two == pytest.approx(-3.0 + sharp(low, 4) + 2.0 - sharp(high, 2), abs=0.03)


def test_an_octave_is_not_judged_by_a_partial_too_weak_to_hear():
    lower = stretch.spectrum(string(84, levels=(1.0, 0.5, 0.25, 1e-4)), RATE)
    upper = stretch.spectrum(string(96), RATE)
    assert stretch.octave(lower, upper, stretch.hz(84), 2) is not None
    assert stretch.octave(lower, upper, stretch.hz(84), 4) is None


class Strings:
    """Stands in for the worker: stiff strings a known distance from equal temperament.

    `dying` gives a key a stiffness of its own and overtones that are gone within the note.
    """

    STIFFNESS = 2e-4

    def __init__(self, dying=None):
        self.dying = dying or {}
        self.calls = []

    @staticmethod
    def tuned(pitch):
        return 0.25 * (pitch - 69)

    def render(self, model, notes, params, seconds, hold, rate):
        self.calls.append((model, notes, params, seconds, hold, rate))
        return [string(note["pitch"], cents=self.tuned(note["pitch"]), seconds=seconds,
                       stiffness=self.dying.get(note["pitch"], self.STIFFNESS),
                       falling=(30.0, 200.0, 200.0, 200.0) if note["pitch"] in self.dying else None)
                for note in notes]


def test_every_key_is_measured_from_one_render_of_the_factory_piano():
    strings = Strings()
    rows = stretch.measure(strings)
    assert [(row["key"], row["pitch"]) for row in rows] == list(stretch.KEYS)
    ((model, notes, params, seconds, hold, rate),) = strings.calls
    assert (model, params, seconds, hold, rate) == ("piano", {}, 2.5, 2.5, 48000)
    assert [note["pitch"] for note in notes] == [21, 24, 33, 36, 45, 48, 57, 60, 69, 72, 81, 84, 96, 108]
    assert all(note["velocity"] == 0.7 for note in notes)
    for row in rows:
        assert row["fundamental"] == pytest.approx(Strings.tuned(row["pitch"]), abs=0.02)
        assert row["fundamental"] < row["heard"] < row["fundamental"] + sharp(Strings.STIFFNESS, 4)
        assert row["heard_readings"][0] == row["heard"]
    # An octave of these strings is three cents wide, less what stiffness adds to the lower one.
    for row in rows[:-1]:
        assert row["octave_2_1"] == pytest.approx(sharp(Strings.STIFFNESS, 2) - 3.0, abs=0.03)
        assert row["octave_4_2"] == pytest.approx(sharp(Strings.STIFFNESS, 4) - sharp(Strings.STIFFNESS, 2) - 3.0, abs=0.03)
    assert "octave_2_1" not in rows[-1] and "octave_4_2" not in rows[-1]


def test_a_figure_is_shown_to_a_tenth_with_no_sign_on_a_zero():
    assert [stretch.shown(value) for value in (2.34, -15.56, -0.01, 0.04, None)] == ["+2.3", "-15.6", "0.0", "0.0", "-"]


def test_a_note_that_dies_within_its_stretch_gets_no_heard_pitch():
    rows = stretch.measure(Strings(dying={96: 8.6e-3, 108: 1.3e-2}))
    assert [row["key"] for row in rows if row["heard"] is None] == ["C7", "C8"]
    assert all(len(row["heard_readings"]) == 2 for row in rows)
