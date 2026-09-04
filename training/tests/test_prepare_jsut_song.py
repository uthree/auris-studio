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
        "start 13950000 c@xx^pau-k+e=N_xx\n"
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


def test_durations_line_up_with_the_transcript_and_the_trimmed_edges():
    # A long leading pause is clipped to the pad; a symbol the mapping drops leaves its
    # time with the token before it; the trailing pause is kept whole, being shorter than
    # the pad. Whatever happens, the seconds sum to the clip.
    phrase = make([("pau", 1.0), ("k", 0.08), ("a", 0.4), ("xx", 0.05), ("t", 0.06), ("o", 0.3), ("pau", 0.1)])
    start, end, symbols = prepare.trim_edges(phrase, 0.15)
    tokens = prepare.to_ipa_line(symbols).split()
    durations = prepare.to_durations(phrase, start, end)
    assert len(durations) == len(tokens) == 6
    assert durations == pytest.approx([0.15, 0.08, 0.45, 0.06, 0.3, 0.1])
    assert sum(durations) == pytest.approx(end - start)
    # Dropped at the very start, the time goes forward instead.
    leading = make([("xx", 0.05), ("k", 0.08), ("a", 0.4)])
    assert prepare.to_durations(leading, 0.0, 0.53) == pytest.approx([0.13, 0.4])


def test_the_script_writes_a_duration_beside_every_transcript(tmp_path, monkeypatch):
    import subprocess
    import sys

    import numpy as np
    import soundfile as sf

    wav_dir, label_dir, out = tmp_path / "wav", tmp_path / "lab", tmp_path / "out"
    wav_dir.mkdir(), label_dir.mkdir()
    sf.write(wav_dir / "001.wav", np.zeros(48_000 * 4, dtype=np.float32), 48_000)
    lines = [(0, 5, "sil"), (5, 10, "k"), (10, 30, "a"), (30, 35, "sil")]
    label_dir.joinpath("001.lab").write_text(
        "\n".join(f"{a * 1_000_000} {b * 1_000_000} x-{s}+y" for a, b, s in lines), encoding="utf-8"
    )
    done = subprocess.run(
        [sys.executable, str(SCRIPT), "--wav-dir", str(wav_dir), "--label-dir", str(label_dir),
         "--output", str(out), "--min-seconds", "0.5", "--min-clip-seconds", "0.5"],
        capture_output=True, text=True,
    )
    assert done.returncode == 0, done.stderr
    texts = sorted((out / "text").glob("*.txt"))
    assert texts, "nothing was prepared"
    for text in texts:
        tokens = text.read_text(encoding="utf-8").split()
        seconds = [float(x) for x in (out / "dur" / text.name).read_text(encoding="utf-8").split()]
        assert len(seconds) == len(tokens) and all(s >= 0 for s in seconds)
