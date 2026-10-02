"""Numerical checks for causal modes and independent, paired evaluation."""

import os
from pathlib import Path

import numpy as np
import pytest
from fit_physical_extensions import coordinate, paired_summary
from fit_physical_mel import Renderer, body_profiles
from fit_physical_trajectories import performance_color
from physical_extensions import (
    MODE_HZ,
    gesture,
    mode_basis,
    mode_sections,
    register_params,
    resonant_body,
)


def test_modes_have_stable_poles_and_peak_at_their_physical_frequencies():
    from scipy.signal import freqz

    for rate in (24000, 48000, 96000):
        for hz, section in zip(MODE_HZ, mode_sections(rate), strict=True):
            assert np.max(abs(np.roots(section[3:]))) < 1
            _, response = freqz(section[:3], section[3:], worN=[0, 2 * np.pi * hz / rate])
            assert abs(response[0]) < 1e-10
            assert abs(response[1]) == pytest.approx(1, abs=0.001)


def test_modes_are_causal_and_zero_gains_preserve_the_signal():
    audio = np.zeros((1, 24000))
    audio[0, 500] = 1
    assert np.all(mode_basis(audio)[:, :, :500] == 0)
    np.testing.assert_array_equal(resonant_body(audio, [0] * 6, [{}]), audio)


def test_register_extremes_stay_in_the_actual_worker_parameter_ranges():
    defaults = {"hardness": 0.7, "decay": 11.296, "damping": 0.000108}
    for pitch in (0, 60, 127):
        params = register_params(defaults, "piano", pitch, [0.2, 0.8, 0.08])
        assert 0 <= params["hardness"] <= 1
        assert 0.1 <= params["decay"] <= 12
        assert 0 <= params["damping"] <= 1
    assert register_params(defaults, "piano", 60, [0.2, 0.8, 0.08]) == defaults


def test_automatic_gesture_uses_duration_without_the_recorded_expression():
    note = {"hold": 7.07}
    assert gesture(note, 0.5) == gesture(note | {"expression": [{"seconds": 0, "value": 0.01}]}, 0.5)
    points = gesture(note, 0.5)
    assert max(np.float32(p["seconds"]) for p in points) < np.float32(note["hold"])
    assert all(0.5 <= p["value"] <= 1 for p in points)


def test_search_keeps_the_unchanged_model_when_candidates_are_worse():
    vector, loss = coordinate(lambda x: float(np.sum(x**2)), [0, 0], [(-1, 1)] * 2, [0.1])
    assert vector == [0, 0]
    assert loss == 0


def test_paired_report_clusters_overlapping_excerpts_and_rejects_unpaired_notes():
    before = [{"id": str(i), "source": "same-capture", "mel": 0.2, "envelope_db": 1} for i in range(3)]
    after = [row | {"mel": 0.1} for row in before]
    report = paired_summary(before, after)
    assert report["source_clusters"] == 1
    assert report["relative_reduction"] == pytest.approx(0.5)
    assert report["mean_delta_ci95"] == pytest.approx([-0.1, -0.1])
    with pytest.raises(ValueError, match="unpaired"):
        paired_summary(before, after[::-1])


@pytest.mark.parametrize("rate", [24000, 48000])
@pytest.mark.parametrize("model", ["piano", "guitar", "violin"])
def test_causal_surrogate_agrees_with_actual_worker_and_moving_expression(model, rate):
    """Enable with PHYSICAL_RENDERER after building the real Rust example."""
    executable = os.environ.get("PHYSICAL_RENDERER")
    if not executable:
        pytest.skip("set PHYSICAL_RENDERER to exercise actual Rust PCM")
    renderer = Renderer(Path(executable).resolve())
    notes = [{"pitch": pitch, "velocity": 0.65, "hold": 0.8,
              "expression": [{"seconds": 0, "value": 0.8},
                             {"seconds": 0.213, "value": 0.2},
                             {"seconds": 0.537, "value": 0.9}]}
             for pitch in (48, 60, 72)]
    try:
        actual = renderer.render(model, notes, {}, 1.2, 0.8, rate)
        dry = renderer.render(model, notes, {"body": 0}, 1.2, 0.8, rate)
        response = (renderer.performance["expression_response_fraction"]
                    * renderer.defaults[model]["bow_response"]) if model == "violin" else 0
        surrogate = performance_color(dry, body_profiles()[model], notes, rate=rate,
                    response=response, modes=renderer.radiation.get(model, ()),
                    exponent=renderer.performance.get("expression_exponent", 1) if model == "violin" else 1)
        assert np.max(abs(actual - surrogate)) < 2e-4
    finally:
        renderer.close()
