"""Numerical regression checks for real-recording choir calibration tooling."""

import hashlib
import io
import json
import zipfile
from pathlib import Path

import numpy as np
import pytest
from choir_reference import RATE, SECONDS, extract, fundamental, load_notes
from fetch_choir_reference import cohort_lock, decode_entry
from physical_mel import distances, features
from scipy.io import wavfile


@pytest.mark.parametrize("frequency", (130.81, 261.63, 523.25, 698.46))
def test_yin_measures_frequency_with_stronger_upper_harmonics(frequency):
    time = np.arange(4096) / RATE
    frame = 0.5 * np.sin(2 * np.pi * frequency * time) + 0.65 * np.sin(4 * np.pi * frequency * time)
    measured, error = fundamental(frame)
    assert abs(measured / frequency - 1) < 0.003
    assert error < 0.02


@pytest.mark.parametrize("frequency", (130.81, 261.63, 523.25))
def test_yin_rejects_a_half_period_when_the_second_harmonic_dominates(frequency):
    time = np.arange(4096) / RATE
    frame = (0.2 * np.sin(2 * np.pi * frequency * time)
             + np.sin(4 * np.pi * frequency * time)
             + 0.1 * np.sin(6 * np.pi * frequency * time))
    measured, error = fundamental(frame)
    assert abs(measured / frequency - 1) < 0.003
    assert error < 0.02


def test_extraction_preserves_onsets_and_keeps_pitch_guides_within_each_note():
    def tone(pitch):
        time = np.arange(round(RATE * 2.0)) / RATE
        frequency = 440 * 2 ** ((pitch - 69) / 12)
        phase = 2 * np.pi * frequency * time
        return (np.sin(phase) + 0.5 * np.sin(2 * phase)) * np.minimum(1, time / 0.04) * 0.1

    audio = np.r_[np.zeros(round(RATE * 0.3)), tone(48),
                  np.zeros(round(RATE * 0.5)), tone(60), np.zeros(round(RATE * 0.3))]
    notes = extract(audio)
    assert [note["pitch"] for note in notes] == [48, 60]
    assert abs(notes[0]["offset_seconds"] - 0.3) < 0.02
    assert abs(notes[1]["offset_seconds"] - 2.8) < 0.02
    for note in notes:
        assert note["stable_end_seconds"] - note["offset_seconds"] >= SECONDS
        assert abs(note["measured_cents"]) < 0.5
        assert len(note["bends"]) == 60
        assert all(0 <= point["seconds"] < SECONDS and abs(point["value"]) < 0.01
                   for point in note["bends"])


def test_loader_reads_training_audio_without_opening_held_out_audio(tmp_path):
    path = tmp_path / "train.wav"
    time = np.arange(round(RATE * SECONDS)) / RATE
    wavfile.write(path, RATE, np.sin(2 * np.pi * 220 * time).astype(np.float32))
    manifest = {"seconds": SECONDS, "notes": [
        {"path": "train.wav", "split": "train", "vowel": 1,
         "sha256": hashlib.sha256(path.read_bytes()).hexdigest()},
        {"path": "validation-must-not-be-read.wav", "split": "validation", "vowel": 1,
         "sha256": "unavailable"}]}
    notes, audio = load_notes(tmp_path, manifest, "train")
    assert len(notes) == 1 and audio.shape == (1, round(RATE * SECONDS))
    with pytest.raises(FileNotFoundError):
        load_notes(tmp_path, manifest, "validation")
    path.write_bytes(b"changed source")
    with pytest.raises(ValueError, match="hash mismatch"):
        load_notes(tmp_path, manifest, "train")


def test_cohort_fingerprint_detects_split_and_pitch_guide_changes():
    manifest = {"notes": [{"split": "train", "bends": [{"seconds": 0.2, "value": 0.3}]}]}
    locked = cohort_lock(manifest)
    assert "bends" not in locked["notes"][0]
    manifest["notes"][0]["bends"][0]["value"] += 0.01
    assert cohort_lock(manifest) != locked
    manifest["notes"][0]["split"] = "validation"
    assert cohort_lock(manifest)["notes"][0]["split"] == "validation"
    assert json.loads(json.dumps(locked)) == locked


def test_partial_zip_download_checks_original_audio_hash_and_crc():
    raw = b"RIFF" + b"recorded vowel" * 100
    stream = io.BytesIO()
    with zipfile.ZipFile(stream, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("FULL/vowel.wav", raw)
    with zipfile.ZipFile(stream) as archive:
        entry = archive.infolist()[0]
        capture = {"zip_path": entry.filename, "size": entry.file_size,
                   "compressed": entry.compress_size, "crc": entry.CRC,
                   "sha256": hashlib.sha256(raw).hexdigest()}
    blob = stream.getvalue()
    assert decode_entry(blob, capture) == raw
    with pytest.raises(ValueError, match="checksum"):
        decode_entry(blob, capture | {"sha256": "changed"})
    with pytest.raises(ValueError, match="entry name"):
        decode_entry(blob, capture | {"zip_path": "wrong.wav"})
    with pytest.raises(ValueError, match="truncated"):
        decode_entry(blob[:30], capture)


def test_loss_is_gain_invariant_but_detects_wrong_vowel_harmonics():
    time = np.arange(round(RATE * SECONDS)) / RATE
    reference = np.sin(2 * np.pi * 220 * time) + 0.5 * np.sin(2 * np.pi * 880 * time)
    correct = features(reference)
    assert distances(correct, features(reference * 0.2))[0] < 1e-12
    wrong = np.sin(2 * np.pi * 220 * time) + 0.5 * np.sin(2 * np.pi * 1760 * time)
    assert distances(correct, features(wrong))[0] > 0.01


def test_frozen_cohort_contains_disjoint_singers_and_all_vowels():
    lock = json.loads((Path(__file__).parent / "references/choir-notes.json").read_text())
    train = {note["singer"] for note in lock["notes"] if note["split"] == "train"}
    validation = {note["singer"] for note in lock["notes"] if note["split"] == "validation"}
    assert len(train) == 4 and len(validation) == 16 and not train & validation
    for split in ("train", "validation"):
        assert {note["vowel"] for note in lock["notes"] if note["split"] == split} == {0, 1, 2}
