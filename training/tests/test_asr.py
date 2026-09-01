"""The listener: a recogniser per language, the front-end behind it, and the rate between.

Everything but the last test runs with a stand-in recogniser, so no model is downloaded and
no dictionary fetched; the last one drives the real ReazonSpeech and is marked ``slow``,
skipping wherever the ``asr`` extra is not installed.
"""

from __future__ import annotations

import math

import numpy as np
import pytest

from auris_singer.asr import RECOGNISERS, Listener, ReazonSpeech, recogniser_for
from auris_singer.intelligibility import (
    content_phonemes,
    edit_distance,
    hearable,
    phoneme_error_rate,
)


class Parrot:
    """A recogniser that says what it was told to."""

    language = "ipa"

    def __init__(self, text: str):
        self.text = text
        self.calls: list[tuple[int, int]] = []

    def transcribe(self, wav: np.ndarray, sample_rate: int) -> str:
        self.calls.append((int(wav.shape[0]), sample_rate))
        return self.text


def test_edit_distance_counts_every_kind_of_edit():
    assert edit_distance([], []) == 0
    assert edit_distance(["a", "b"], ["a", "b"]) == 0
    assert edit_distance(["a", "b"], ["a", "c"]) == 1, "substitution"
    assert edit_distance(["a", "b"], ["a"]) == 1, "deletion"
    assert edit_distance(["a", "b"], ["a", "x", "b"]) == 1, "insertion"
    assert edit_distance(["k", "a"], []) == 2
    assert edit_distance(list("kitten"), list("sitting")) == 3


def test_the_rate_is_edits_per_reference_phoneme():
    assert phoneme_error_rate(["a", "i", "u"], ["a", "i", "u"]) == 0.0
    assert phoneme_error_rate(["a", "i", "u"], ["a", "e", "u"]) == pytest.approx(1 / 3)
    assert phoneme_error_rate(["a"], ["a", "b", "c"]) == pytest.approx(2.0), "insertions can exceed one"
    assert math.isnan(phoneme_error_rate([], ["a"])), "nothing asked for, no rate"


def test_what_a_listener_can_hear_is_the_words_without_rests_or_devoicing():
    assert content_phonemes(["<sil>", "k", "a", "<pau>", "sil", "<unk>", "i"]) == ["k", "a", "i"]
    # A recogniser hears a vowel, not whether the singer voiced it; jpreprocess decides
    # devoicing by rule and the corpus by label, so both sides are held to the voiced spelling.
    assert hearable(["<sil>", "k", "i̥", "t", "a", "ɯ̥", "<sil>"]) == ["k", "i", "t", "a", "ɯ"]
    assert hearable(["ḁ"]) == ["a"]
    assert hearable([]) == []


def test_the_listener_turns_text_into_comparable_phonemes():
    parrot = Parrot("k o ɴ i̥ tɕ i w a")
    listener = Listener(parrot)
    heard = listener.hear(np.zeros(4800, dtype=np.float32), 48_000)
    assert heard.text == "k o ɴ i̥ tɕ i w a"
    assert heard.phonemes == ["k", "o", "ɴ", "i", "tɕ", "i", "w", "a"]
    assert parrot.calls == [(4800, 48_000)]
    assert phoneme_error_rate(hearable(["<sil>", "k", "o", "ɴ", "i", "tɕ", "i", "w", "a"]), heard.phonemes) == 0.0


def test_silence_heard_is_no_phonemes_at_all():
    listener = Listener(Parrot("   "))
    assert listener.hear(np.zeros(10, dtype=np.float32), 16_000).phonemes == []


def test_a_language_without_a_recogniser_is_refused_by_name():
    with pytest.raises(ValueError, match="no recogniser for 'xx'"):
        recogniser_for("xx")
    assert "ja" in RECOGNISERS
    assert isinstance(recogniser_for("Japanese"), ReazonSpeech)
    assert recogniser_for("ja", precision="int8").precision == "int8"


def test_the_japanese_listener_needs_no_boundary_silence_from_its_front_end():
    listener = Listener.for_language("ja")
    assert listener.recogniser.language == "ja"
    assert listener.frontend.add_boundary_silence is False


def test_a_new_language_is_one_registration(monkeypatch):
    class Klingon:
        language = "ipa"

        def transcribe(self, wav, sample_rate):
            return "q a p l a"

    monkeypatch.setitem(RECOGNISERS, "tlh", Klingon)
    listener = Listener(recogniser_for("tlh"))
    assert listener.hear(np.zeros(1), 16_000).phonemes == ["q", "a", "p", "l", "a"]


@pytest.mark.slow
def test_reazonspeech_hears_a_recording_it_was_never_trained_on():
    pytest.importorskip("reazonspeech.k2.asr")
    pytest.importorskip("jpreprocess")
    listener = Listener.for_language("ja")
    # Two seconds of nothing is heard as nothing. Played to the model it would be うん — a
    # recogniser trained on speech expects some — so silence is answered before the model.
    assert listener.hear(np.zeros(32_000, dtype=np.float32), 16_000).phonemes == []
    # And a tone is not a word either: whatever the model makes of it, the run survives.
    t = np.arange(32_000) / 16_000
    heard = listener.hear((0.3 * np.sin(2 * np.pi * 440.0 * t)).astype(np.float32), 16_000)
    assert isinstance(heard.text, str) and all(isinstance(p, str) for p in heard.phonemes)
