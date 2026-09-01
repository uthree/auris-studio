"""Tests for the source-control validation metrics."""

from __future__ import annotations

import math

import pytest
import torch

from auris_singer.metrics import (
    energy_metrics,
    hz_to_cents,
    pearson_correlation,
    pitch_metrics,
)


def test_hz_to_cents_is_1200_per_octave():
    cents = hz_to_cents(torch.tensor([110.0, 220.0, 440.0]))
    assert (cents[1] - cents[0]).item() == pytest.approx(1200.0, abs=1e-3)
    assert (cents[2] - cents[1]).item() == pytest.approx(1200.0, abs=1e-3)


def test_pearson_correlation_extremes():
    mask = torch.ones(1, 8)
    a = torch.arange(8, dtype=torch.float32).unsqueeze(0)
    assert pearson_correlation(a, a * 3 + 1, mask).item() == pytest.approx(1.0, abs=1e-5)
    assert pearson_correlation(a, -a, mask).item() == pytest.approx(-1.0, abs=1e-5)


def test_pearson_correlation_is_nan_without_variance():
    mask = torch.ones(1, 8)
    constant = torch.ones(1, 8)
    assert math.isnan(pearson_correlation(constant, constant, mask).item())


def test_perfect_pitch_tracking_scores_perfectly():
    f0 = torch.tensor([[0.0, 220.0, 233.1, 246.9, 0.0]])
    voiced = torch.tensor([[0.0, 1.0, 1.0, 1.0, 0.0]])
    valid = torch.ones_like(voiced)

    metrics = pitch_metrics(f0, voiced, f0, voiced, valid)
    assert metrics["f0_rmse_cent"].item() == pytest.approx(0.0, abs=1e-3)
    assert metrics["f0_accuracy"].item() == pytest.approx(1.0)
    assert metrics["f0_corr"].item() == pytest.approx(1.0, abs=1e-4)
    assert metrics["vuv_error"].item() == pytest.approx(0.0)
    assert metrics["voiced_ratio_error"].item() == pytest.approx(0.0)


def test_constant_pitch_offset_is_measured_in_cents():
    target = torch.full((1, 10), 220.0)
    voiced = torch.ones(1, 10)
    semitone_up = target * (2 ** (1 / 12))

    metrics = pitch_metrics(target, voiced, semitone_up, voiced, voiced)
    assert metrics["f0_rmse_cent"].item() == pytest.approx(100.0, abs=1e-2)
    # 100 cents is outside the 50-cent tolerance.
    assert metrics["f0_accuracy"].item() == pytest.approx(0.0)


def test_pitch_tolerance_threshold():
    target = torch.full((1, 10), 220.0)
    voiced = torch.ones(1, 10)
    slightly_sharp = target * (2 ** (30 / 1200))  # 30 cents

    metrics = pitch_metrics(target, voiced, slightly_sharp, voiced, voiced)
    assert metrics["f0_accuracy"].item() == pytest.approx(1.0)
    assert metrics["f0_rmse_cent"].item() == pytest.approx(30.0, abs=1e-2)


def test_voicing_disagreement_is_reported():
    target_voiced = torch.tensor([[1.0, 1.0, 1.0, 0.0]])
    pred_voiced = torch.tensor([[1.0, 1.0, 0.0, 0.0]])
    f0 = torch.full((1, 4), 220.0)
    valid = torch.ones(1, 4)

    metrics = pitch_metrics(f0, target_voiced, f0, pred_voiced, valid)
    assert metrics["vuv_error"].item() == pytest.approx(0.25)
    # One requested voiced frame came out unvoiced: too breathy.
    assert metrics["voiced_ratio_error"].item() == pytest.approx(-0.25)


def test_pitch_metrics_ignore_padded_frames():
    f0 = torch.tensor([[220.0, 220.0, 999.0, 999.0]])
    voiced = torch.tensor([[1.0, 1.0, 1.0, 1.0]])
    valid = torch.tensor([[1.0, 1.0, 0.0, 0.0]])
    predicted = torch.tensor([[220.0, 220.0, 100.0, 100.0]])

    metrics = pitch_metrics(f0, voiced, predicted, voiced, valid)
    assert metrics["f0_rmse_cent"].item() == pytest.approx(0.0, abs=1e-3)
    assert metrics["vuv_error"].item() == pytest.approx(0.0)


def test_pitch_metrics_are_nan_when_nothing_is_voiced():
    silent = torch.zeros(1, 6)
    metrics = pitch_metrics(silent, silent, silent, silent, torch.ones(1, 6))
    assert math.isnan(metrics["f0_rmse_cent"].item())
    assert math.isnan(metrics["f0_accuracy"].item())
    # Voicing agreement is still well defined.
    assert metrics["vuv_error"].item() == pytest.approx(0.0)


def test_perfect_energy_tracking_scores_perfectly():
    energy = torch.tensor([[0.01, 0.1, 0.3, 0.2]])
    valid = torch.ones_like(energy)
    metrics = energy_metrics(energy, energy, valid)
    assert metrics["energy_rmse_db"].item() == pytest.approx(0.0, abs=1e-4)
    assert metrics["energy_bias_db"].item() == pytest.approx(0.0, abs=1e-4)
    assert metrics["energy_corr"].item() == pytest.approx(1.0, abs=1e-4)


def test_energy_bias_has_a_sign():
    target = torch.tensor([[0.1, 0.2, 0.3]])
    valid = torch.ones_like(target)
    half = energy_metrics(target, target * 0.5, valid)
    double = energy_metrics(target, target * 2.0, valid)

    assert half["energy_bias_db"].item() == pytest.approx(-6.02, abs=0.05)
    assert double["energy_bias_db"].item() == pytest.approx(6.02, abs=0.05)
    # RMSE cannot tell the two apart; the bias term is what distinguishes them.
    assert half["energy_rmse_db"].item() == pytest.approx(
        double["energy_rmse_db"].item(), abs=1e-3
    )


def test_energy_metrics_exclude_silent_frames():
    target = torch.tensor([[1e-6, 1e-6, 0.1, 0.2]])
    predicted = torch.tensor([[0.5, 0.5, 0.1, 0.2]])  # wrong only where silent
    valid = torch.ones_like(target)
    metrics = energy_metrics(target, predicted, valid, floor=1e-4)
    assert metrics["energy_rmse_db"].item() == pytest.approx(0.0, abs=1e-4)


def test_energy_metrics_are_nan_when_everything_is_silent():
    silent = torch.full((1, 5), 1e-8)
    metrics = energy_metrics(silent, silent, torch.ones(1, 5))
    assert math.isnan(metrics["energy_rmse_db"].item())


def test_metrics_handle_batches():
    f0 = torch.tensor([[220.0, 220.0], [440.0, 440.0]])
    voiced = torch.ones(2, 2)
    valid = torch.ones(2, 2)
    metrics = pitch_metrics(f0, voiced, f0 * (2 ** (50 / 1200)), voiced, valid)
    assert metrics["f0_rmse_cent"].item() == pytest.approx(50.0, abs=1e-2)
    assert metrics["f0_accuracy"].item() == pytest.approx(1.0)
