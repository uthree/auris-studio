"""Tests for the VocalSet preparation recipe."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import numpy as np
import pytest
import soundfile as sf

_SPEC = importlib.util.spec_from_file_location(
    "prepare_vocalset", Path(__file__).resolve().parents[1] / "scripts" / "prepare_vocalset.py"
)
prepare_vocalset = importlib.util.module_from_spec(_SPEC)
sys.modules["prepare_vocalset"] = prepare_vocalset
_SPEC.loader.exec_module(prepare_vocalset)

SR = 48_000


def tone(seconds: float, freq: float = 200.0, amplitude: float = 0.5) -> np.ndarray:
    t = np.arange(int(seconds * SR), dtype=np.float32) / SR
    return (amplitude * np.sin(2 * np.pi * freq * t)).astype(np.float32)


def silence(seconds: float) -> np.ndarray:
    return np.zeros(int(seconds * SR), dtype=np.float32)


def test_find_recordings_filters_and_reads_the_vowel(tmp_path):
    for rel in [
        "male1/scales/straight/m1_scales_straight_a.wav",
        "male1/scales/vocal_fry/m1_scales_vocal_fry_e.wav",  # excluded technique
        "male1/excerpts/straight/m1_caro_straight.wav",  # no trailing vowel
        "male9/scales/straight/m9_scales_straight_o.wav",  # excluded speaker
    ]:
        path = tmp_path / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        sf.write(path, tone(0.1), SR)

    found = prepare_vocalset.find_recordings(tmp_path, {"male1"}, {"straight"})
    assert [(speaker, vowel) for speaker, vowel, _ in found] == [("male1", "a")]


def test_regions_split_at_internal_silence_only_when_long_enough():
    wav = np.concatenate([tone(0.5), silence(0.6), tone(0.5)])
    split = prepare_vocalset.find_sound_regions(wav, SR, 480, -40.0, 0.4)
    assert len(split) == 2

    joined = prepare_vocalset.find_sound_regions(wav, SR, 480, -40.0, 1.0)
    assert len(joined) == 1, "a silence shorter than the threshold must not split"


def test_regions_are_relative_to_the_files_own_peak():
    """VocalSet's pp takes sit far below its forte takes."""
    quiet = np.concatenate([silence(0.3), tone(0.5, amplitude=0.01), silence(0.3)])
    regions = prepare_vocalset.find_sound_regions(quiet, SR, 480, -40.0, 0.4)
    assert len(regions) == 1
    start, end = regions[0]
    assert start > 0 and end < len(quiet)


def test_split_long_bounds_every_piece():
    pieces = prepare_vocalset.split_long((0, 20 * SR), SR, 7.5)
    assert len(pieces) == 3
    assert all(end - start <= int(7.5 * SR) for start, end in pieces)
    assert pieces[0][0] == 0 and pieces[-1][1] == 20 * SR
    assert all(a[1] == b[0] for a, b in zip(pieces, pieces[1:])), "pieces must tile the region"


def test_split_long_leaves_a_short_region_alone():
    assert prepare_vocalset.split_long((0, SR), SR, 7.5) == [(0, SR)]


def test_transcript_marks_silence_only_where_it_exists():
    assert prepare_vocalset.transcript("a", True, True) == "<sil> a <sil>"
    assert prepare_vocalset.transcript("a", False, True) == "a <sil>"
    assert prepare_vocalset.transcript("a", False, False) == "a"


def test_gain_matches_the_target_level():
    clips = [tone(1.0, amplitude=0.05)]
    gain = prepare_vocalset.gain_to_match(clips, target_dbfs=-20.0, ceiling=0.95)
    rms = np.sqrt(((clips[0] * gain) ** 2).mean())
    assert 20 * np.log10(rms) == pytest.approx(-20.0, abs=0.1)


def test_gain_is_capped_so_the_peak_stays_under_the_ceiling():
    clips = [tone(1.0, amplitude=0.5)]
    gain = prepare_vocalset.gain_to_match(clips, target_dbfs=0.0, ceiling=0.95)
    assert np.abs(clips[0] * gain).max() == pytest.approx(0.95, rel=1e-6)


def test_gain_is_shared_across_clips_so_dynamics_survive():
    """Per-clip normalization would erase VocalSet's pp/forte contrast."""
    clips = [tone(1.0, amplitude=0.5), tone(1.0, amplitude=0.05)]
    gain = prepare_vocalset.gain_to_match(clips, target_dbfs=-20.0, ceiling=0.95)
    loud, quiet = (np.abs(c * gain).max() for c in clips)
    assert loud / quiet == pytest.approx(10.0, rel=1e-3)
