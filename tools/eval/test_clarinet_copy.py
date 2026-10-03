"""Regression tests for the bounded clarinet copy-synthesis protocol."""

import hashlib
import json

import numpy as np
import pytest
from clarinet_copy import load
from scipy.io import wavfile


def test_loader_preserves_pitch_split_and_hash(tmp_path):
    path = tmp_path / "notes" / "mf-60.wav"
    path.parent.mkdir()
    wavfile.write(path, 24_000, np.zeros(72_000, dtype="<f4"))
    manifest = {"source": "test", "license": "test", "rate": 24_000,
                "seconds": 3.0, "hold": 2.9,
                "notes": [{"id": "mf-60", "model": "clarinet", "pitch": 60,
                            "dynamic": "mf", "split": "train", "path": "notes/mf-60.wav",
                            "hold_seconds": 1.0,
                            "tuning_cents": 0.0,
                            "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}]}
    (tmp_path / "notes.json").write_text(json.dumps(manifest), encoding="utf-8")
    loaded, rows = load(tmp_path)
    assert loaded["rate"] == 24_000
    assert rows[0][0]["split"] == "train"
    assert rows[0][0]["velocity"] == .65


def test_loader_rejects_pitch_leakage_between_splits(tmp_path):
    path = tmp_path / "note.wav"
    wavfile.write(path, 24_000, np.zeros(72_000, dtype="<f4"))
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    note = {"model": "clarinet", "pitch": 60, "dynamic": "mf", "path": "note.wav",
            "hold_seconds": 1.0, "tuning_cents": 0.0, "sha256": digest}
    manifest = {"rate": 24_000, "seconds": 3.0, "notes": [
        {**note, "id": "train", "split": "train"},
        {**note, "id": "validation", "split": "validation"}]}
    (tmp_path / "notes.json").write_text(json.dumps(manifest), encoding="utf-8")
    with pytest.raises(ValueError, match="pitch appears"):
        load(tmp_path)
