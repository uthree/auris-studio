"""Regression checks for the controlled render and listening preparation."""

import json
from types import SimpleNamespace

import numpy as np
import pytest
from seed_diversity import chorus_bounds, faded_excerpt, loudness


def project():
    return {
        "tempo_map": {"points": [{"tick": 0, "bpm": 120}]},
        "sections": {"points": [
            {"tick": 0, "label": "verse"},
            {"tick": 30720, "label": "chorus"},
            {"tick": 61440, "label": "outro"},
        ]},
    }


def test_chorus_uses_actual_section_bounds_and_rejects_changed_context():
    value = project()
    assert chorus_bounds(value) == (30720, 61440, 120)
    value["tempo_map"]["points"].append({"tick": 100, "bpm": 90})
    with pytest.raises(ValueError, match="constant tempo"):
        chorus_bounds(value)
    value = project()
    value["sections"]["points"][-1]["tick"] -= 960
    with pytest.raises(ValueError, match="eight-bar"):
        chorus_bounds(value)


def test_fade_only_changes_the_requested_edges_without_mutating_source():
    original = np.full((48000 * 2, 2), 0.25, dtype=np.float32)
    excerpt, first, last = faded_excerpt(original, 48000, 960, 2880, 120)
    assert (first, last, len(excerpt)) == (24000, 72000, 48000)
    assert np.all(original == 0.25)
    assert np.all(excerpt[240:-240] == 0.25)
    assert np.all(excerpt[[0, -1]] == 0)
    with pytest.raises(ValueError, match="bounds"):
        faded_excerpt(original, 48000, 960, 100000, 120)


def test_meter_reads_input_not_processed_output_and_rejects_silence(tmp_path, monkeypatch):
    report = {"input_i": "-18.25", "input_tp": "-2.7", "output_i": "-23.00"}

    def invoke(*args, **kwargs):
        return SimpleNamespace(stderr="meter output\n" + json.dumps(report))

    monkeypatch.setattr("seed_diversity.subprocess.run", invoke)
    result = loudness(tmp_path / "ffmpeg", tmp_path / "audio.wav", tmp_path / "meter.log")
    assert result == {"lufs": -18.25, "true_peak_dbfs": -2.7}
    report["input_i"] = "-inf"
    with pytest.raises(ValueError, match="Nonfinite"):
        loudness(tmp_path / "ffmpeg", tmp_path / "audio.wav", tmp_path / "meter.log")
