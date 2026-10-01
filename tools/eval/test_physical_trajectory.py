"""Numerical contracts for long-note losses, pitch tracks and reference controls."""

import numpy as np
import pytest
from fit_physical_mel import color
from fit_physical_trajectories import compact, performance_color
from physical_mel import RATE, features
from physical_trajectory import (
    curve,
    expression_curve,
    metrics,
    phase_masks,
    pitch_track,
    temporal_error,
)
from prepare_physical_reference import manifest_digest


def test_late_sustain_error_cannot_hide_behind_a_matching_attack():
    time = np.arange(RATE * 6) / RATE
    reference = np.sin(2 * np.pi * 220 * time)
    candidate = reference.copy()
    candidate[2 * RATE:] *= 0.05
    note = {"seconds": 6.0, "hold": 6.0}
    loss, phases = temporal_error(features(reference), features(candidate), note)
    assert loss > 0.01
    assert phases["late"] > 0.02
    assert metrics(reference, candidate, note)["envelope_db"] > 8


def test_glissando_and_release_are_separate_nonempty_weighted_phases():
    note = {"seconds": 8.0, "hold": 7.0, "transition": [3.0, 4.0]}
    masks, weights = phase_masks(note, 801)
    assert set(masks) == {"attack", "early", "late", "transition", "release"}
    np.testing.assert_array_equal(sum(mask.astype(int) for mask in masks.values()), np.r_[np.ones(800), 0])
    assert sum(weights.values()) == pytest.approx(1)
    assert masks["transition"].sum() == 100
    assert masks["release"].sum() == 100


def test_excerpt_end_does_not_invent_a_note_release_or_late_short_note_phase():
    masks, _ = phase_masks({"seconds": 6.0, "hold": 6.0}, 601)
    assert "release" not in masks
    assert not any(mask[-1] for mask in masks.values())
    short, _ = phase_masks({"seconds": 1.0, "hold": 1.0}, 101)
    assert set(short) == {"attack", "early"}


def test_pitch_tracker_follows_played_vibrato_and_glide_with_stronger_second_harmonic():
    time = np.arange(RATE * 4) / RATE
    semitones = np.clip(time - 1, 0, 1) * 5 + 0.2 * np.sin(2 * np.pi * 5 * time)
    frequency = 220 * 2 ** (semitones / 12)
    phase = np.cumsum(2 * np.pi * frequency / RATE)
    audio = np.sin(phase) + 2 * np.sin(phase * 2)
    times, tracked, voiced = pitch_track(audio, 205, 310)
    mask = voiced & (times > 0.15) & (times < 3.8)
    expected = np.interp(times[mask], time, frequency)
    error = abs(1200 * np.log2(tracked[mask] / expected))
    assert mask.sum() > 350
    assert np.median(error) < 4
    assert np.percentile(error, 95) < 10
    points, _ = curve(audio, 57, 57, 62, 4)
    assert points[0]["seconds"] == 0
    assert 4.7 < points[-1]["value"] < 5.3


def test_missing_rendered_pitch_is_penalized_instead_of_ignored():
    time = np.arange(RATE * 3) / RATE
    reference = np.sin(2 * np.pi * 220 * time)
    note = {"seconds": 3.0, "hold": 3.0, "pitch_bounds_hz": [205, 235]}
    correct = metrics(reference, reference, note)
    wrong = metrics(reference, np.sin(2 * np.pi * 500 * time), note)
    assert correct["pitch_cents"] == pytest.approx(0)
    assert correct["voiced_coverage"] == pytest.approx(1)
    assert wrong["voiced_coverage"] < 0.05
    assert wrong["pitch_penalty_cents"] > 95


def test_tracker_rejects_polyphonic_octave_search_and_nonfinite_audio():
    with pytest.raises(ValueError, match="sub-octave"):
        pitch_track(np.ones(RATE), 100, 250)
    with pytest.raises(ValueError, match="invalid trajectory"):
        pitch_track(np.full(RATE, np.nan), 205, 235)


def test_control_points_remain_strictly_before_fractional_note_off():
    time = np.arange(RATE * 8) / RATE
    points, _ = curve(np.sin(2 * np.pi * 220 * time), 57, 57, 57, 7.07)
    assert max(np.float32(point["seconds"]) for point in points) < np.float32(7.07)


def test_radiation_surrogate_preserves_filter_before_moving_expression():
    carrier = np.sin(np.arange(RATE) * 2 * np.pi * 440 / RATE)[None, :]
    envelope = np.ones_like(carrier)
    envelope[:, 6000:] = 0.1
    gains = np.linspace(-8, 8, 12)
    note = {"expression": [{"seconds": 0.25, "value": 0.1}]}
    expected = color(carrier, gains) * envelope
    actual = performance_color(carrier * envelope, gains, [note])
    np.testing.assert_allclose(actual, expected, atol=1e-13)
    assert np.max(abs(color(carrier * envelope, gains) - expected)) > 0.05


def test_expression_guide_retains_long_crescendo_and_is_gain_independent():
    time = np.arange(RATE * 6) / RATE
    audio = np.sin(2 * np.pi * 220 * time) * np.exp((time - 6) * 0.5)
    controls = expression_curve(audio, 5.37)
    assert controls == expression_curve(audio * 0.01, 5.37)
    assert controls[0]["value"] < controls[-1]["value"] * 0.4
    assert all(0.08 <= point["value"] <= 1 for point in controls)
    assert max(point["seconds"] for point in controls) < 5.37


def test_compact_lock_identity_survives_json_tuple_conversion():
    import json

    manifest = {"notes": [{"pitch_bounds_hz": (205.0, 235.0),
                            "bends": [{"seconds": 0.0, "value": 0.0}]}]}
    prepared = compact(manifest)
    serialized = json.loads(json.dumps(prepared))
    assert manifest_digest(prepared) == manifest_digest(serialized)
    assert serialized["notes"][0]["bends_points"] == 1
    assert "bends" not in serialized["notes"][0]
