"""Numerical contracts for the temporal copy-synthesis objective and corpus checks."""

import json
from pathlib import Path

import numpy as np
import pytest
from fit_physical_mel import body_profiles, color
from physical_mel import RATE, distances, features, mel_bank
from prepare_physical_reference import (
    manifest_digest,
    onset,
    pitch_regions,
    tuning_cents,
    write_note,
)


def test_identity_gain_and_polarity_do_not_change_mel_loss():
    time = np.arange(RATE) / RATE
    signal = np.sin(2 * np.pi * 220 * time) * np.exp(-time * 2)
    original = features(signal)
    np.testing.assert_allclose(distances(original, features(-signal * 0.003)), 0, atol=1e-13)


def test_decay_and_attack_errors_remain_after_whole_note_normalization():
    time = np.arange(RATE) / RATE
    carrier = np.sin(2 * np.pi * 220 * time)
    original = features(carrier * np.exp(-time * 2))
    decay_error = distances(original, features(carrier * np.exp(-time * 6)))[0]
    delayed = np.r_[np.zeros(1200), (carrier * np.exp(-time * 2))[:-1200]]
    onset_error = distances(original, features(delayed))[0]
    assert decay_error > 0.01
    assert onset_error > 0.01


def test_wrong_pitch_and_harmonics_increase_mel_error():
    time = np.arange(RATE) / RATE
    original = features(np.sin(2 * np.pi * 440 * time))
    assert distances(original, features(np.sin(2 * np.pi * 493.88 * time)))[0] > 0.05
    bright = np.sin(2 * np.pi * 440 * time) + np.sin(2 * np.pi * 1320 * time)
    assert distances(original, features(bright))[0] > 0.05


@pytest.mark.parametrize("signal", [np.zeros(RATE), np.full(RATE, np.nan), np.ones(100)])
def test_invalid_or_silent_candidates_are_rejected(signal):
    with pytest.raises(ValueError):
        features(signal)


def test_mel_band_area_is_normalized():
    for window in (512, 2048):
        bank = mel_bank(window)
        assert bank.shape == (64, window // 2 + 1)
        np.testing.assert_allclose(bank.sum(axis=1), 1, atol=1e-14)


def test_radiation_identity_and_causal_tail():
    impulse = np.zeros(RATE)
    impulse[0] = 1
    np.testing.assert_allclose(color(impulse, np.zeros(12)), impulse, atol=1e-14)
    profiles = body_profiles()
    assert set(profiles) == {"piano", "guitar", "violin"}
    for gains in profiles.values():
        result = color(impulse, gains)
        assert np.isfinite(result).all()
        assert np.max(abs(result[-1000:])) < 1e-6


def test_reference_extractor_finds_order_and_tuning_and_fails_on_missing_pitch():
    time = np.arange(2 * RATE) / RATE
    notes = [np.sin(2 * np.pi * 440 * 2 ** ((pitch - 69 + 0.2) / 12) * time)
             for pitch in (60, 61)]
    scale = np.r_[np.zeros(2400), notes[0], np.zeros(12000), notes[1], np.zeros(2400)]
    spans = pitch_regions(scale, [60, 61])
    assert spans[0][0] < 0.2
    assert 2.4 < spans[1][0] < 2.8
    assert abs(onset(scale, *spans[0]) / RATE - 0.1) < 0.02
    assert abs(tuning_cents(notes[0][:RATE], 60) - 20) < 1
    with pytest.raises(ValueError, match="no stable region"):
        pitch_regions(scale, [60, 61, 62])


def test_frozen_cohort_keeps_pitches_and_weak_dynamics_out_of_training():
    root = Path(__file__).parent / "references"
    manifest = json.loads((root / "iowa-notes.json").read_text(encoding="utf-8"))
    keys = [(note["model"], note["pitch"], note["velocity"]) for note in manifest["notes"]]
    assert len(keys) == len(set(keys)) == 165
    for model in ("piano", "guitar", "violin"):
        train = [note for note in manifest["notes"] if note["model"] == model and note["split"] == "train"]
        validation = [note for note in manifest["notes"] if note["model"] == model and note["split"] == "validation"]
        assert train and validation
        assert all(note["velocity"] != 0.35 for note in train)
        trained_pitches = {note["pitch"] for note in train}
        held_pitches = {note["pitch"] for note in validation if note["velocity"] != 0.35}
        assert trained_pitches.isdisjoint(held_pitches)


def test_reference_wav_is_byte_deterministic_and_preserves_float_pcm(tmp_path):
    import soundfile as sf

    signal = np.sin(np.arange(24000) * 2 * np.pi * 440 / RATE).astype(np.float32)
    path = tmp_path / "note.wav"
    write_note(path, signal)
    original = path.read_bytes()
    write_note(path, signal)
    assert path.read_bytes() == original
    assert b"PEAK" not in original[:100]
    actual, rate = sf.read(path)
    assert rate == RATE
    np.testing.assert_array_equal(actual, signal)


def test_manifest_identity_is_independent_of_line_endings_and_key_order():
    value = {"rate": 24000, "notes": [{"pitch": 60, "velocity": 0.65}]}
    windows = json.dumps(value, indent=2).replace("\n", "\r\n")
    compact = '{"notes":[{"velocity":0.65,"pitch":60}],"rate":24000}'
    assert manifest_digest(json.loads(windows)) == manifest_digest(json.loads(compact))
