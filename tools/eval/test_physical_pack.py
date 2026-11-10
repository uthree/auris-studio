"""Numerical checks for percussive copy synthesis and capture isolation."""

import json
from pathlib import Path

import numpy as np
import pytest
from fit_physical_pack import coordinate, radiation
from physical_pack import (
    FAMILIES,
    envelope,
    errors,
    pack_features,
    paired_summary,
    snare_fitness,
    t90,
    tom_role_margin,
)
from prepare_physical_pack import drum_onset, drum_selection
from prepare_physical_reference import RATE, manifest_digest

REFERENCES = Path(__file__).parent / "references"


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


def test_search_keeps_the_unchanged_model_when_candidates_are_worse():
    vector, loss = coordinate(lambda x: float(np.sum(x**2)), [0, 0], [(-1, 1)] * 2, [0.1])
    assert vector == [0, 0]
    assert loss == 0


def test_paired_report_clusters_hits_by_capture_and_rejects_unpaired_notes():
    before = [{"id": str(i), "source": "same-capture", "mel": 0.2, "envelope_db": 1} for i in range(3)]
    after = [row | {"mel": 0.1} for row in before]
    report = paired_summary(before, after)
    assert report["source_clusters"] == 1
    assert report["relative_reduction"] == pytest.approx(0.5)
    assert report["mean_delta_ci95"] == pytest.approx([-0.1, -0.1])
    with pytest.raises(ValueError, match="unpaired"):
        paired_summary(before, after[::-1])


def test_radiation_is_an_identity_at_zero_gain_and_shapes_only_its_own_band():
    from scipy.signal import freqz

    impulse = np.zeros(RATE)
    impulse[0] = 1
    np.testing.assert_allclose(radiation(impulse, np.zeros(12)), impulse, atol=1e-14)
    gains = np.zeros(12)
    gains[5] = 6
    centers = np.geomspace(90, 10000, 12)
    for rate in (RATE, 48000):
        shaped = radiation(np.r_[1.0, np.zeros(rate - 1)], gains, rate=rate)
        _, response = freqz(shaped, worN=2 * np.pi * centers[[0, 5, 11]] / rate)
        np.testing.assert_allclose(20 * np.log10(abs(response)), [0, 6, 0], atol=0.2)


def test_frozen_pack_has_disjoint_sources_and_all_requested_families():
    notes = json.loads((REFERENCES / "physical-pack-notes.json").read_text(encoding="utf-8"))["notes"]
    assert len({note["id"] for note in notes}) == len(notes) == 151
    assert {note["family"] for note in notes} == set(FAMILIES)
    for family in FAMILIES:
        subset = [note for note in notes if note["family"] == family]
        train = [note for note in subset if note["split"] == "train"]
        validation = [note for note in subset if note["split"] == "validation"]
        assert train and validation
        assert {note["source"] for note in train}.isdisjoint(note["source"] for note in validation)


def test_frozen_fit_names_the_frozen_cohort_and_has_a_causal_radiation_for_every_family():
    cohort = json.loads((REFERENCES / "physical-pack-notes.json").read_text(encoding="utf-8"))
    fit = json.loads((REFERENCES / "physical-pack-fit.json").read_text(encoding="utf-8"))
    assert fit["reference_sha256"] == manifest_digest(cohort)
    assert tuple(fit["families"]) == FAMILIES
    impulse = np.zeros(RATE)
    impulse[0] = 1
    for family in FAMILIES:
        result = radiation(impulse, fit["families"][family]["gains_db"])
        assert np.isfinite(result).all()
        assert np.max(abs(result[-1000:])) < 1e-6
