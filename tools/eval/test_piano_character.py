"""Guards for the piano's character measures: they must read known signals back."""

import numpy as np
import piano_character as character

RATE = character.RATE


def string(f0, b=1e-4, decays=None, beat=0.0, extra=None, seconds=character.SECONDS):
    """A stiff string's note: partials on n f0 sqrt(1 + b n^2), each falling at its own rate in
    dB/s, the upper ones beating at `beat` Hz, with an optional off-series tone."""
    t = np.arange(int(seconds * RATE)) / RATE
    x = np.zeros_like(t)
    for n in range(1, 25):
        p = n * f0 * np.sqrt(1 + b * n * n)
        if p > 7000:
            break
        rate = decays(n) if decays else 6.0
        swing = 1.0 + (0.5 * np.cos(2 * np.pi * beat * t) if beat and n > 1 else 0.0)
        x += 1 / n * 10 ** (-rate * t / 20) * swing * np.sin(2 * np.pi * p * t)
    if extra:
        hz, level = extra
        x += level * np.exp(-3 * t) * np.sin(2 * np.pi * hz * t)
    return x


def test_the_series_is_found_with_its_stretch():
    x = string(65.4, b=1.3e-4)
    f0, b = character.fit_series(*character.spectrum(x, 0.1, 1.6), 65.4)
    assert abs(f0 - 65.4) < 0.3
    assert abs(b - 1.3e-4) < 0.3e-4


def test_partials_falling_alike_have_no_spread_and_unlike_do():
    even = character.measure(string(110.0), 45)
    uneven = character.measure(string(110.0, decays=lambda n: 3.0 + 9.0 * (n % 2)), 45)
    assert even["spread"][0] < 1.0
    assert uneven["spread"][0] > 5.0


def test_a_beating_partial_wobbles():
    still = character.measure(string(110.0), 45)
    beating = character.measure(string(110.0, beat=0.8), 45)
    assert still["wobble"][0] < 0.5
    assert beating["wobble"][0] > 2.0


def test_a_tone_off_the_series_is_metal():
    plain = character.measure(string(65.4), 36)
    clang = character.measure(string(65.4, extra=(1_010.0, 0.05)), 36)
    assert plain["metal"][0] < -40.0
    assert clang["metal"][0] > plain["metal"][0] + 10.0


def test_a_note_scores_nothing_against_itself():
    captures = [
        {"path": "a", "pitch": 36, "dynamic": "mf"},
        {"path": "b", "pitch": 60, "dynamic": "ff"},
    ]
    stats = {
        "a": character.measure(string(65.4), 36),
        "b": character.measure(string(261.6, b=3e-4), 60),
    }
    scores, _ = character.score_stats(stats, stats, captures)
    assert all(scores[m] == 0.0 for m in character.METRICS if np.isfinite(scores[m]))
