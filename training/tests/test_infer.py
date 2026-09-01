"""Tests for the inference-time voicing derivation."""

from __future__ import annotations

import numpy as np

from auris_singer.infer import frame_voicing
from auris_singer.text import SIL, is_voiceless


def test_voiceless_classification_covers_the_japanese_core():
    for symbol in ["k", "t", "p", "s", "ɕ", "tɕ", "ts", "ç", "ɸ", "h", "ʔ", "ḁ", SIL, "<pau>"]:
        assert is_voiceless(symbol), symbol
    for symbol in ["a", "i", "ɯ", "ɴ", "m", "n", "b", "d", "g", "z", "dʑ", "ɾ", "j", "w", "v"]:
        assert not is_voiceless(symbol), symbol


def test_unknown_symbols_default_to_voiced():
    # A wrongly-voiced frame keeps the pitch contour; a wrongly-unvoiced one
    # silences it — so the unknown case must err toward voiced.
    assert not is_voiceless("ʘ")


def test_frame_voicing_follows_the_phoneme_not_the_contour():
    # /k a/ with the note's pitch written across the consonant, the way a
    # score front-end does: the k frames must still come out unvoiced.
    voiced = frame_voicing([SIL, "k", "a"], [2, 3, 4], [0.0, 0.0] + [220.0] * 7)
    assert voiced.tolist() == [0, 0, 0, 0, 0, 1, 1, 1, 1]


def test_frame_voicing_clears_frames_with_no_pitch():
    # A vowel over f0=0 (a rest the front-end left inside the note) cannot be
    # voiced — there is no pitch to sing it at.
    voiced = frame_voicing(["a"], [4], [220.0, 220.0, 0.0, 220.0])
    assert voiced.tolist() == [1, 1, 0, 1]
    assert voiced.dtype == np.float32
