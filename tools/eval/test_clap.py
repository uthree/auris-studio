"""Measurement-contract tests; no model downloads or neural inference required."""

import copy
import json
from pathlib import Path

import numpy as np
import pytest
import soundfile
from clap import (
    MANIFEST,
    RATE,
    WINDOW,
    compare,
    cosine_scores,
    load_manifest,
    mono_resample,
    preset_for,
    repeat_pad,
    score_file,
    segment_starts,
)


def test_excerpts_cover_full_song_deterministically():
    assert segment_starts(70 * RATE) == [0, 30 * RATE, 60 * RATE]
    assert segment_starts(70 * RATE, 1) == [30 * RATE]
    assert segment_starts(WINDOW) == [0]
    assert segment_starts(RATE) == [0]
    assert segment_starts(WINDOW + 1, 5) == [0, 1]
    with pytest.raises(ValueError):
        segment_starts(0)


def test_short_clip_uses_native_repeat_then_zero_padding():
    excerpt = np.ones(3 * RATE, dtype=np.float32)
    padded = repeat_pad(excerpt)
    assert padded.shape == (WINDOW,)
    np.testing.assert_array_equal(padded[: 9 * RATE], 1)
    np.testing.assert_array_equal(padded[9 * RATE :], 0)


def test_stereo_is_averaged_and_resampling_preserves_frequency():
    rate = 44_100
    source = np.sin(2 * np.pi * 1000 * np.arange(rate) / rate)
    stereo = np.column_stack((source, source * 0.5))
    mono = mono_resample(stereo, rate)
    assert len(mono) == RATE
    spectrum = np.abs(np.fft.rfft(mono))
    assert np.argmax(spectrum) == 1000
    assert np.sqrt(np.mean(mono[100:-100] ** 2)) == pytest.approx(
        0.75 / np.sqrt(2), abs=0.002
    )
    np.testing.assert_array_equal(
        mono_resample(np.column_stack((source, -source)), rate), 0
    )


def test_nonfinite_audio_is_rejected_before_resampling():
    for value in (np.nan, np.inf, -np.inf):
        with pytest.raises(ValueError, match="nonfinite"):
            mono_resample(np.array([[value]]), RATE)


def test_cosines_are_normalized_and_margin_is_not_a_probability():
    result = cosine_scores(np.array([3.0, 0.0]), np.array([[2.0, 0.0], [-2.0, 0.0]]), 1)
    assert result["positive_cosine"] == 1
    assert result["contrast_cosine"] == -1
    assert result["contrast_margin"] == 2
    with pytest.raises(ValueError, match="zero"):
        cosine_scores(np.zeros(2), np.eye(2), 1)
    with pytest.raises(ValueError, match="nonfinite"):
        cosine_scores(np.array([np.nan, 0]), np.eye(2), 1)


class FakeBackend:
    """Count calls to ensure silent/invalid windows do not reach the model."""

    def __init__(self):
        self.audio_calls = 0

    def text_embeddings(self, profile):
        return np.array([[1.0, 0.0], [-1.0, 0.0]])

    def audio_embedding(self, excerpt):
        assert excerpt.shape == (WINDOW,)
        self.audio_calls += 1
        return np.array([1.0, 0.0])


def test_silent_windows_are_reported_without_fake_zero_scores(tmp_path: Path):
    path = tmp_path / "jazz-trio.wav"
    data = np.zeros(30 * RATE, dtype=np.float32)
    data[10 * RATE : 20 * RATE] = 0.1
    soundfile.write(path, data, RATE, subtype="FLOAT")
    backend = FakeBackend()
    result = score_file(path, "jazz-trio", {"positive": ["placeholder"]}, backend, 3)
    assert result["status"] == "partial"
    assert backend.audio_calls == result["valid_segments"] == 1
    assert result["aggregate"]["positive_cosine"] == 1
    assert [item["status"] for item in result["segments"]] == ["silent", "ok", "silent"]
    assert "positive_cosine" not in result["segments"][0]
    json.dumps(result, allow_nan=False)


def test_nonfinite_wav_produces_invalid_report(tmp_path: Path):
    path = tmp_path / "invalid.wav"
    soundfile.write(path, np.array([np.nan, 0.1]), RATE, subtype="FLOAT")
    backend = FakeBackend()
    result = score_file(path, "jazz-trio", {}, backend, 3)
    assert result["status"] == "invalid"
    assert result["aggregate"] is None
    assert backend.audio_calls == 0
    json.dumps(result, allow_nan=False)


def test_audio_below_native_int16_resolution_is_silence(tmp_path: Path):
    path = tmp_path / "quiet.wav"
    soundfile.write(path, np.full(RATE, 1e-6), RATE, subtype="FLOAT")
    backend = FakeBackend()
    result = score_file(path, "rock", {}, backend, 3)
    assert result["status"] == "silent"
    assert result["segments"][0]["quantized_to_silence"]
    assert backend.audio_calls == 0


def test_invalid_embedding_is_not_reported_as_silence(tmp_path: Path):
    class BrokenBackend(FakeBackend):
        def audio_embedding(self, excerpt):
            return np.array([np.nan, 1.0])

    path = tmp_path / "broken.wav"
    soundfile.write(path, np.full(RATE, 0.1), RATE, subtype="FLOAT")
    result = score_file(path, "rock", {"positive": ["placeholder"]}, BrokenBackend(), 3)
    assert result["status"] == "invalid"
    assert result["segments"][0]["status"] == "invalid_embedding"
    assert result["aggregate"] is None
    json.dumps(result, allow_nan=False)


def test_manifest_covers_presets_and_seed_names_without_guessing():
    profiles = load_manifest(MANIFEST)["presets"]
    assert len(profiles) == 9
    assert preset_for(Path("jazz-trio-s101.wav"), profiles) == "jazz-trio"
    assert preset_for(Path("mix.wav"), profiles, "rock") == "rock"
    with pytest.raises(ValueError, match="no prompt"):
        preset_for(Path("mix.wav"), profiles)


def test_comparison_refuses_changed_measurement_conditions():
    report = {
        "schema_version": 1,
        "prompts_sha256": "fixed",
        "preprocessing": {"segments": 3},
        "model": {
            "checkpoint_sha256": "fixed",
            "packages": {},
            "tokenizer_artifacts": {},
            "device": "cpu",
        },
        "files": {},
    }
    assert compare(report, copy.deepcopy(report)) == {}
    changed = copy.deepcopy(report)
    changed["prompts_sha256"] = "tuned after listening"
    with pytest.raises(ValueError, match="prompts_sha256"):
        compare(report, changed)
    changed = copy.deepcopy(report)
    changed["model"]["checkpoint_sha256"] = "other checkpoint"
    with pytest.raises(ValueError, match="checkpoint_sha256"):
        compare(report, changed)
