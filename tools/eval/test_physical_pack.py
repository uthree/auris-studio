"""Numerical checks for percussive copy synthesis and capture isolation."""

import json
from pathlib import Path

import numpy as np
import pytest
from physical_pack import (
    FAMILIES,
    envelope,
    errors,
    pack_features,
    snare_fitness,
    t90,
    tom_role_margin,
)
from prepare_physical_pack import drum_onset, drum_selection
from prepare_physical_reference import RATE, pitch_regions


def signal(decay=0.2):
    time = np.arange(3 * RATE) / RATE
    return np.sin(2 * np.pi * 440 * time) * np.exp(-time / decay)


def test_percussive_measurement_keeps_timing_but_ignores_gain_and_polarity():
    audio = signal()
    target = pack_features(audio)
    np.testing.assert_allclose(errors(target, pack_features(-audio * 0.003))["mel"], 0, atol=1e-12)
    delayed = np.r_[np.zeros(1200), audio[:-1200]]
    measured = errors(target, pack_features(delayed))
    assert measured["attack"][0] > 0.05
    assert measured["mel"][0] > 0.02
    assert errors(target, pack_features(signal(0.6)))["tail"][0] > 0.01
    np.testing.assert_allclose(envelope(audio), envelope(audio * 10), atol=1e-10)


def test_energy_decay_time_recovers_an_exponential_and_preserves_units():
    for decay in (0.18, 0.55):
        assert t90(signal(decay)[None, :])[0] == pytest.approx(decay * np.log(10) / 2, abs=0.004)


def test_short_stable_notes_are_located_without_assigning_a_missing_pitch():
    time = np.arange(RATE // 2) / RATE
    audio = np.concatenate([np.r_[np.zeros(RATE // 2), np.sin(2 * np.pi * 440 * 2**((pitch-69)/12) * time)]
                            for pitch in (60, 61, 62)])
    spans = pitch_regions(audio, [60, 61, 62], minimum_seconds=0.2)
    assert len(spans) == 3
    assert all(start < end for start, end in spans)
    with pytest.raises(ValueError, match="no stable region"):
        pitch_regions(audio, [60, 61, 62, 63], minimum_seconds=0.2)


def test_original_drum_hits_are_split_without_aliases_or_alternative_articulations():
    assert drum_selection("OH/snare_OH_F_2.wav") == ("snare", 38, 0.8, 2)
    assert drum_selection("OH/loTom_OH_PP_8.wav") == ("tom", 41, 0.2, 8)
    for name in ("snare2_OH_F_2.wav", "snare_OH_Ghost_2.wav", "hihatClosed_OH_P_9.wav"):
        assert drum_selection(name) is None
    audio = np.r_[np.zeros(2 * RATE), signal()]
    assert abs(drum_onset(audio) / RATE - 2) < 0.002


def test_snare_guard_rejects_a_tonal_resonance_and_accepts_diffuse_midrange():
    from scipy.signal import butter, sosfilt

    time = np.arange(3 * 48000) / 48000
    tone = np.sin(2 * np.pi * 185 * time) * np.exp(-time / 0.04)
    noise = np.random.default_rng(7).normal(size=len(time)) * np.exp(-time / 0.04)
    noise = sosfilt(butter(2, [600, 5000], fs=48000, btype="bandpass", output="sos"), noise)
    assert snare_fitness(tone) < 0.1
    assert snare_fitness(noise) > 0.8


def test_tom_role_guard_distinguishes_a_deep_kick_from_a_higher_stable_head():
    time = np.arange(3 * 48000) / 48000
    def tone(hz):
        return np.sin(2 * np.pi * hz * time) * np.exp(-time / 0.08)
    assert tom_role_margin(tone(65)) < -0.5
    assert tom_role_margin(tone(185)) > 0.5


def test_frozen_pack_has_disjoint_sources_and_all_requested_families():
    path = Path(__file__).parent / "references" / "physical-pack-notes.json"
    notes = json.loads(path.read_text(encoding="utf-8"))["notes"]
    assert len({note["id"] for note in notes}) == len(notes) == 391
    for family in FAMILIES:
        subset = [note for note in notes if note.get("family", note["model"]) == family]
        train = [note for note in subset if note["split"] == "train"]
        validation = [note for note in subset if note["split"] == "validation"]
        assert train and validation
        if train[0]["model"] == "drums":
            assert {note["source"] for note in train}.isdisjoint(note["source"] for note in validation)
        else:
            assert all(note["velocity"] != 0.35 for note in train)
            assert {note["pitch"] for note in train}.isdisjoint(note["pitch"] for note in validation if note["velocity"] != 0.35)
