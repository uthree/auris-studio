"""Tests for the JSUT-song phrase segmentation logic."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "prepare_jsut_song.py"
_spec = importlib.util.spec_from_file_location("prepare_jsut_song", SCRIPT)
prepare = importlib.util.module_from_spec(_spec)
sys.modules["prepare_jsut_song"] = prepare
_spec.loader.exec_module(prepare)

Phoneme = prepare.Phoneme


def make(sequence: list[tuple[str, float]], start: float = 0.0) -> list[Phoneme]:
    """Build timed phonemes from ``(symbol, duration)`` pairs."""
    phonemes = []
    for symbol, duration in sequence:
        phonemes.append(Phoneme(start, start + duration, symbol))
        start += duration
    return phonemes


def test_read_label_parses_times_and_phonemes(tmp_path):
    path = tmp_path / "x.lab"
    path.write_text(
        "0 13500000 p@xx^xx-pau+d=e_xx%xx\n"
        "13500000 13950000 c@xx^pau-d+e=N_xx\n"
        "not a label line\n",
        encoding="utf-8",
    )
    phonemes = prepare.read_label(path)
    assert [p.symbol for p in phonemes] == ["pau", "d"]
    assert phonemes[0].start == pytest.approx(0.0)
    assert phonemes[0].end == pytest.approx(1.35)
    assert phonemes[1].duration == pytest.approx(0.045)
    assert phonemes[0].is_pause and not phonemes[1].is_pause


def test_phrases_are_cut_at_long_pauses():
    phonemes = make(
        [("pau", 0.5)]
        + [("k", 0.2), ("a", 2.0)]
        + [("pau", 0.6)]
        + [("t", 0.2), ("o", 2.0)]
        + [("pau", 0.5)]
    )
    phrases = prepare.split_into_phrases(
        phonemes, min_pause=0.25, min_seconds=2.0, max_seconds=8.0
    )
    assert len(phrases) == 2
    assert [p.symbol for p in phrases[0]] == ["pau", "k", "a", "pau"]
    # The boundary pause is kept by both neighbours, so each phrase has silence
    # at both ends.
    assert [p.symbol for p in phrases[1]] == ["pau", "t", "o", "pau"]


def test_short_pauses_do_not_cut():
    phonemes = make([("k", 0.2), ("a", 1.0), ("pau", 0.1), ("t", 0.2), ("o", 1.0)])
    phrases = prepare.split_into_phrases(
        phonemes, min_pause=0.25, min_seconds=2.0, max_seconds=8.0
    )
    assert len(phrases) == 1


def test_silence_only_phrases_are_dropped():
    phonemes = make([("pau", 3.0), ("pau", 3.0), ("k", 0.2), ("a", 3.0), ("pau", 0.5)])
    phrases = prepare.split_into_phrases(
        phonemes, min_pause=0.25, min_seconds=2.0, max_seconds=8.0
    )
    assert all(any(not p.is_pause for p in phrase) for phrase in phrases)


def test_long_legato_phrase_is_force_split():
    """Some songs sustain a line for a minute with no usable pause."""
    phrase = make([("k", 0.3), ("a", 2.7)] * 10)  # 30 s, no pauses at all
    parts = prepare.enforce_max_length(phrase, max_seconds=8.0)
    assert len(parts) > 1
    for part in parts:
        assert part[-1].end - part[0].start <= 8.0
    # No phoneme is lost or duplicated.
    assert [p.symbol for part in parts for p in part] == [p.symbol for p in phrase]


def test_force_split_prefers_a_consonant_onset():
    phrase = make([("k", 0.5), ("a", 3.0), ("t", 0.5), ("o", 3.0), ("s", 0.5), ("a", 3.0)])
    parts = prepare.enforce_max_length(phrase, max_seconds=8.0)
    # Japanese is CV, so a cut before a consonant is a syllable boundary.
    assert all(part[0].symbol not in prepare.VOWEL_PHONEMES for part in parts[1:])


def test_short_phrase_is_left_alone():
    phrase = make([("k", 0.2), ("a", 1.0)])
    assert prepare.enforce_max_length(phrase, max_seconds=8.0) == [phrase]


def test_trim_edges_clips_long_boundary_silence():
    phrase = make([("pau", 2.0), ("k", 0.2), ("a", 1.0), ("pau", 2.0)])
    start, end, symbols = prepare.trim_edges(phrase, pad=0.15)
    assert start == pytest.approx(1.85)
    assert end == pytest.approx(3.35)
    assert symbols == ["pau", "k", "a", "pau"]


def test_trim_edges_keeps_short_silence_intact():
    phrase = make([("pau", 0.1), ("k", 0.2), ("a", 1.0)])
    start, end, _ = prepare.trim_edges(phrase, pad=0.15)
    assert start == pytest.approx(0.0)
    assert end == pytest.approx(1.3)


def test_transcript_marks_real_silence_only():
    with_silence = prepare.to_ipa_line(["pau", "k", "a", "pau"])
    assert with_silence.split()[0] == "<sil>"
    assert with_silence.split()[-1] == "<sil>"

    # A hard cut leaves no silence at that edge, and none is invented.
    hard_cut = prepare.to_ipa_line(["k", "a", "t", "o"])
    assert "<sil>" not in hard_cut
    assert hard_cut == "k a t o"


def test_transcript_keeps_internal_pauses_distinct_from_boundaries():
    line = prepare.to_ipa_line(["pau", "k", "a", "pau", "t", "o", "pau"]).split()
    assert line[0] == "<sil>" and line[-1] == "<sil>"
    assert "<pau>" in line, "an internal pause must stay a <pau>"
