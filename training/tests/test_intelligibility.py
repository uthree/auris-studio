"""The consonant instruments: manner classes, class-resolved distance, the sibilant tilt."""

from __future__ import annotations

import math

import numpy as np
import pytest
import torch

from auris_singer.intelligibility import (
    CLASS_METRICS,
    SIBILANT_SPLIT_HZ,
    align,
    class_spectral_metrics,
    confusion_rows,
    confusion_table,
    edit_distance,
    frame_classes,
    tally_confusions,
)
from auris_singer.text.ipa import (
    IPA_SYMBOLS,
    PHONEME_CLASSES,
    SIBILANTS,
    SPECIAL_SYMBOLS,
    VOICELESS,
    phoneme_class,
)
from auris_singer.utils.audio import mel_spectrogram, spectrogram

SR, N_FFT, HOP, WIN = 48_000, 2048, 480, 2048


def test_every_symbol_has_one_class_and_no_class_has_a_stranger():
    for symbol in IPA_SYMBOLS:
        assert phoneme_class(symbol) != "unknown", symbol
    for symbol in SPECIAL_SYMBOLS:
        assert phoneme_class(symbol) == "special"
    members = [s for group in PHONEME_CLASSES.values() for s in group]
    assert len(members) == len(set(members)), "a symbol in two classes"
    assert set(members) - set(IPA_SYMBOLS) - set(SPECIAL_SYMBOLS) == set()
    assert phoneme_class("ʈʂ") == "unknown"


def test_the_classes_agree_with_what_the_table_already_knew():
    # Every devoiced vowel is a vowel; every voiceless symbol that is not a vowel or a
    # special is an obstruent — the two tables were written apart and must not disagree.
    for symbol in VOICELESS - set(SPECIAL_SYMBOLS):
        assert phoneme_class(symbol) in {"vowel", "plosive", "affricate", "fricative"}, symbol
    for symbol in SIBILANTS:
        assert phoneme_class(symbol) in {"fricative", "affricate"}, symbol


def test_frames_are_sorted_by_the_phoneme_on_them():
    vowel, consonant, sibilant = frame_classes(["sil", "s", "a", "<sil>", "k", "ɕ", "ɴ"])
    assert vowel.tolist() == [False, False, True, False, False, False, False]
    assert consonant.tolist() == [False, True, False, False, True, True, True]
    assert sibilant.tolist() == [False, True, False, False, False, True, False]


def _noise(n_frames: int, seed: int, highpass: bool) -> torch.Tensor:
    """White noise, or the same with everything below the split removed — a hiss."""
    g = torch.Generator().manual_seed(seed)
    wav = torch.randn(n_frames * HOP, generator=g) * 0.1
    if highpass:
        spec = torch.fft.rfft(wav)
        cutoff = int(SIBILANT_SPLIT_HZ / (SR / 2) * (spec.numel() - 1))
        spec[:cutoff] = 0
        wav = torch.fft.irfft(spec, n=wav.numel())
    return wav


def _tone(n_frames: int) -> torch.Tensor:
    t = torch.arange(n_frames * HOP) / SR
    return 0.3 * torch.sin(2 * math.pi * 220.0 * t)


def _measure(pred: torch.Tensor, real: torch.Tensor, tokens: list[str]) -> dict[str, float]:
    mel = lambda w: mel_spectrogram(w, SR, N_FFT, HOP, WIN, 32)  # noqa: E731
    power = lambda w: spectrogram(w, N_FFT, HOP, WIN, power=2.0)  # noqa: E731
    return class_spectral_metrics(mel(pred), mel(real), power(pred), power(real), tokens, SR, N_FFT)


def test_a_render_that_is_the_recording_is_at_distance_zero_on_every_class():
    n = 40
    tokens = ["a"] * 20 + ["s"] * 10 + ["k"] * 10
    real = torch.cat([_tone(20), _noise(10, 1, True), _noise(10, 2, False)])
    out = _measure(real, real, tokens)
    assert set(out) == set(CLASS_METRICS)
    for name in ("mel_l1_vowel", "mel_l1_consonant", "mel_l1_sibilant"):
        assert out[name] == pytest.approx(0.0, abs=1e-6)
    assert out["sibilant_tilt_db"] == pytest.approx(0.0, abs=1e-6)
    assert len(tokens) == n


def test_the_split_by_class_lands_a_change_where_it_was_made():
    tokens = ["a"] * 20 + ["s"] * 10 + ["k"] * 10
    real = torch.cat([_tone(20), _noise(10, 1, True), _noise(10, 2, False)])
    # The vowel halved, the consonants untouched.
    pred = real.clone()
    pred[: 20 * HOP] *= 0.5
    out = _measure(pred, real, tokens)
    assert out["mel_l1_vowel"] > 0.2
    # Not exactly zero: the analysis window is four frames wide and straddles the boundary,
    # so the last vowel frames leak a little of the change into the first consonant ones.
    assert out["mel_l1_consonant"] < 0.1
    assert out["mel_l1_sibilant"] < 0.1


def test_a_sibilant_the_model_never_formed_tilts_the_other_way():
    tokens = ["a"] * 20 + ["s"] * 10
    real = torch.cat([_tone(20), _noise(10, 1, True)])
    # Where the singer hissed, the render hums the vowel on: a low-band /s/.
    pred = torch.cat([_tone(20), _tone(10)])
    out = _measure(pred, real, tokens)
    assert out["sibilant_tilt_db"] < -20, out
    assert out["mel_l1_sibilant"] > out["mel_l1_vowel"]
    # And a render hissing *more* than the singer tilts positive.
    louder = torch.cat([_tone(20), _noise(10, 1, True) * 4])
    assert _measure(louder, real, tokens)["sibilant_tilt_db"] > 0


def test_a_class_the_utterance_lacks_is_nan_not_zero():
    tokens = ["a"] * 10
    out = _measure(_tone(10), _tone(10), tokens)
    assert out["mel_l1_vowel"] == pytest.approx(0.0)
    assert math.isnan(out["mel_l1_consonant"])
    assert math.isnan(out["mel_l1_sibilant"])
    assert math.isnan(out["sibilant_tilt_db"])


def test_frames_and_tokens_must_agree():
    with pytest.raises(ValueError, match="tokens"):
        _measure(_tone(10), _tone(10), ["a"] * 9)
    with pytest.raises(ValueError, match="frame grid"):
        mel = mel_spectrogram(_tone(10), SR, N_FFT, HOP, WIN, 32)
        class_spectral_metrics(mel, mel[:, :5], mel, mel, ["a"] * 10, SR, N_FFT)
    assert np.asarray(frame_classes([])[0]).size == 0


def test_the_alignment_is_a_shortest_path_and_reads_the_way_a_person_would():
    asked = ["s", "a", "k", "ɯ", "ɾ", "a"]
    assert align(asked, asked) == list(zip(asked, asked)), "heard as asked: every pair a match"
    # A vowel heard wrong is a substitution, not a deletion beside an insertion.
    assert align(["k", "a", "s", "a"], ["k", "a", "s", "o"])[-1] == ("a", "o")
    # A dropped /s/ pairs with nothing; an extra あ is an insertion, and the ら stays a ら.
    assert align(asked, asked[1:])[0] == ("s", "")
    assert ("", "a") in align(asked, asked + ["a"])[-2:]
    tail = align(asked, ["s", "a", "k", "ɯ", "r", "a", "a"])[4:]
    assert sorted(tail) == [("", "a"), ("a", "a"), ("ɾ", "r")]
    for heard in (asked[1:], asked + ["a"], ["k", "o"], []):
        pairs = align(asked, heard)
        assert sum(a != h for a, h in pairs) == edit_distance(asked, heard), "the path costs the distance"
        assert [a for a, _ in pairs if a] == asked and [h for _, h in pairs if h] == heard


def test_the_tally_adds_up_over_utterances_and_the_table_names_the_worst_first():
    tally: dict[tuple[str, str], int] = {}
    tally_confusions(["s", "a", "ɕ", "i"], ["a", "ɕ", "i"], tally)
    tally_confusions(["s", "a", "ɕ", "i"], ["s", "a", "s", "i"], tally)
    tally_confusions(["s", "a", "ɕ", "i"], ["s", "a", "s", "i", "i"], tally)
    assert tally[("s", "")] == 1 and tally[("ɕ", "s")] == 2 and tally[("", "i")] == 1
    rows = confusion_rows(tally)
    assert rows[0][2] == 3 and ["ɕ", "s", 2] in rows, "most frequent pair first, as plain data"
    table = confusion_table(tally)
    lines = table.split("\n")
    assert lines[1].startswith("ɕ") and "s×2" in lines[1], "ɕ lost twice, both times as s"
    assert lines[2].startswith("s") and lines[2].split()[3] == "1", "s dropped once"
    assert lines[-1] == "inserted: i×1"
    assert confusion_table({}) == ""
