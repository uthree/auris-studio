"""Integrity guards for the grand-piano calibration's fixed reference cuts."""

import json

import grand_piano as piano
import numpy as np
import pytest
import soundfile as sf


@pytest.fixture
def cohort(tmp_path, monkeypatch):
    locks = tmp_path / "locks"
    locks.mkdir()
    for name in ("iowa-captures.json", "iowa-notes.json"):
        (locks / name).write_text("{}", encoding="utf-8")
    monkeypatch.setattr(piano, "LOCKS", locks)
    note = {"model": "piano", "pitch": 60, "velocity": 0.65, "split": "validation"}
    (locks / "iowa-notes.json").write_text(
        json.dumps({"notes": [note]}), encoding="utf-8"
    )
    (tmp_path / "notes").mkdir()
    path = piano.note_path(tmp_path, note, 1)
    sf.write(path, np.ones(piano.RATE) * 0.1, piano.RATE, subtype="FLOAT")
    manifest = {
        "source_lock_sha256": piano.digest(locks / "iowa-captures.json"),
        "note_lock_sha256": piano.digest(locks / "iowa-notes.json"),
        "notes": [note],
        "files": {path.relative_to(tmp_path).as_posix(): piano.digest(path)},
    }
    (tmp_path / "grand-notes.json").write_text(json.dumps(manifest), encoding="utf-8")
    return tmp_path, path


def test_reference_loading_preserves_locked_split_and_samples(cohort):
    root, _ = cohort
    notes, audio = piano.load_cohort(root, 1, "validation")
    assert notes == [
        {"model": "piano", "pitch": 60, "velocity": 0.65, "split": "validation"}
    ]
    np.testing.assert_allclose(audio, 0.1)


def test_changed_audio_is_rejected_before_scoring(cohort):
    root, path = cohort
    path.write_bytes(b"modified audio")
    with pytest.raises(ValueError, match="prepared reference changed"):
        piano.load_cohort(root, 1)


def test_changed_source_lock_is_rejected(cohort):
    root, _ = cohort
    (piano.LOCKS / "iowa-notes.json").write_text('{"changed": true}', encoding="utf-8")
    with pytest.raises(ValueError, match="reference locks changed"):
        piano.load_cohort(root, 1)


@pytest.mark.parametrize(
    "samples,rate",
    [
        (np.ones(100), piano.RATE),
        (np.ones((piano.RATE, 2)), piano.RATE),
        (np.ones(piano.RATE), 48000),
        (np.full(piano.RATE, np.nan), piano.RATE),
    ],
)
def test_invalid_reference_format_is_rejected_even_with_matching_hash(
    cohort, samples, rate
):
    root, path = cohort
    sf.write(path, samples, rate, subtype="FLOAT")
    manifest_path = root / "grand-notes.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    manifest["files"][path.relative_to(root).as_posix()] = piano.digest(path)
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    with pytest.raises(ValueError, match="invalid reference format"):
        piano.load_cohort(root, 1)


def test_empty_training_split_cannot_enter_optimizer(cohort):
    with pytest.raises(ValueError, match="empty piano cohort"):
        piano.load_cohort(cohort[0], 1, "train")


def test_validation_note_cannot_be_relabelled_as_training(cohort):
    root, _ = cohort
    path = root / "grand-notes.json"
    manifest = json.loads(path.read_text(encoding="utf-8"))
    manifest["notes"][0]["split"] = "train"
    path.write_text(json.dumps(manifest), encoding="utf-8")
    with pytest.raises(ValueError, match="prepared note labels or splits changed"):
        piano.load_cohort(root, 1, "train")
