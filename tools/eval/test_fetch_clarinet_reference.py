"""Numeric tests for clarinet source segmentation and pitch checks."""

import numpy as np
import pytest
from fetch_clarinet_reference import locate, prepare_note, tuning_cents

RATE = 44_100


def tone(pitch, seconds=1.1, level=.25):
    time = np.arange(round(seconds * RATE)) / RATE
    return level * np.sin(2 * np.pi * 440 * 2 ** ((pitch - 69) / 12) * time)


def test_locate_is_chronological_and_keeps_each_note_isolated():
    gap = np.zeros(round(.65 * RATE))
    source = np.concatenate([gap, tone(60), gap, tone(62), gap, tone(64), gap])
    spans = locate(source, [60, 62, 64])
    assert len(spans) == 3
    assert spans[0][0] > .6
    assert spans[0][1] < spans[1][0]
    assert spans[1][1] < spans[2][0]
    prepared = prepare_note(source, *spans[0])
    assert len(prepared) == 72_000
    assert np.sqrt(np.mean(prepared[-round(.25 * 24_000):] ** 2)) < .01


def test_tuning_rejects_octave_mislabel_without_fundamental():
    overtone_only = tone(72, seconds=1.2)
    with pytest.raises(ValueError, match="fundamental|unexpected"):
        tuning_cents(overtone_only, 60)


def test_tuning_accepts_concert_pitch_with_small_offset():
    clip = tone(60, seconds=1.2)
    assert abs(tuning_cents(clip, 60)) < 5
