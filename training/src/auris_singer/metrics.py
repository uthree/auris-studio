"""Validation metrics for source-control fidelity.

f0 and energy reach the decoder only through the excitation signal, so the
useful question at validation time is not just "does this sound like speech"
but "did the output actually follow the pitch and loudness it was told to
follow".  These metrics answer that by re-analysing the generated waveform and
comparing it against the curves that were fed in.

All functions take ``(B, T)`` tensors on a shared frame grid plus a ``(B, T)``
validity mask, and return scalar tensors.  A metric that has no frames to
average over returns NaN rather than 0, so an undefined metric is visibly
undefined instead of looking like a perfect score.
"""

from __future__ import annotations

import torch

__all__ = ["hz_to_cents", "pearson_correlation", "pitch_metrics", "energy_metrics"]

_NAN = float("nan")


def hz_to_cents(f0: torch.Tensor, reference: float = 10.0) -> torch.Tensor:
    """Convert Hz to cents relative to ``reference`` Hz (1200 cents = 1 octave)."""
    return 1200.0 * torch.log2(f0.clamp(min=1e-5) / reference)


def _masked_mean(values: torch.Tensor, mask: torch.Tensor) -> torch.Tensor:
    total = mask.sum()
    if total < 1:
        return torch.tensor(_NAN, device=values.device)
    return (values * mask).sum() / total


def pearson_correlation(
    a: torch.Tensor, b: torch.Tensor, mask: torch.Tensor
) -> torch.Tensor:
    """Pearson correlation of ``a`` and ``b`` over the frames selected by ``mask``."""
    count = mask.sum()
    if count < 2:
        return torch.tensor(_NAN, device=a.device)
    a_mean = (a * mask).sum() / count
    b_mean = (b * mask).sum() / count
    a_centered = (a - a_mean) * mask
    b_centered = (b - b_mean) * mask
    denominator = a_centered.pow(2).sum().sqrt() * b_centered.pow(2).sum().sqrt()
    if denominator < 1e-8:
        return torch.tensor(_NAN, device=a.device)
    return (a_centered * b_centered).sum() / denominator


def pitch_metrics(
    target_f0: torch.Tensor,
    target_voiced: torch.Tensor,
    pred_f0: torch.Tensor,
    pred_voiced: torch.Tensor,
    valid: torch.Tensor,
    tolerance_cents: float = 50.0,
) -> dict[str, torch.Tensor]:
    """Compare the f0 asked for against the f0 actually produced.

    Args:
        target_f0: ``(B, T)`` requested f0 in Hz (0 on unvoiced frames).
        target_voiced: ``(B, T)`` requested voicing flag.
        pred_f0: ``(B, T)`` f0 measured on the generated waveform.
        pred_voiced: ``(B, T)`` voicing measured on the generated waveform.
        valid: ``(B, T)`` frames that are not padding.
        tolerance_cents: threshold for ``f0_accuracy``. 50 cents is a quarter
            tone — audibly in tune.

    Returns:
        ``f0_rmse_cent`` — pitch error on frames both sides call voiced;
        ``f0_accuracy`` — fraction of those frames within ``tolerance_cents``;
        ``f0_corr`` — correlation of the two pitch contours in cents;
        ``vuv_error`` — fraction of valid frames whose voicing disagrees;
        ``voiced_ratio_error`` — signed difference in overall voiced fraction,
        which distinguishes "too breathy" from "too buzzy".
    """
    valid = valid.float()
    target_voiced = target_voiced.float() * valid
    pred_voiced = pred_voiced.float() * valid
    both_voiced = target_voiced * pred_voiced

    target_cents = hz_to_cents(target_f0)
    pred_cents = hz_to_cents(pred_f0)
    error = (pred_cents - target_cents) * both_voiced

    rmse = _masked_mean(error.pow(2), both_voiced).sqrt()
    accuracy = _masked_mean((error.abs() <= tolerance_cents).float(), both_voiced)
    correlation = pearson_correlation(target_cents, pred_cents, both_voiced)
    vuv_error = _masked_mean((target_voiced != pred_voiced).float(), valid)
    voiced_ratio_error = _masked_mean(pred_voiced, valid) - _masked_mean(
        target_voiced, valid
    )

    return {
        "f0_rmse_cent": rmse,
        "f0_accuracy": accuracy,
        "f0_corr": correlation,
        "vuv_error": vuv_error,
        "voiced_ratio_error": voiced_ratio_error,
    }


def energy_metrics(
    target_energy: torch.Tensor,
    pred_energy: torch.Tensor,
    valid: torch.Tensor,
    floor: float = 1e-4,
) -> dict[str, torch.Tensor]:
    """Compare the loudness envelope asked for against the one produced.

    Args:
        target_energy: ``(B, T)`` requested per-frame linear RMS.
        pred_energy: ``(B, T)`` RMS measured on the generated waveform.
        valid: ``(B, T)`` frames that are not padding.
        floor: frames whose target energy is below this are excluded — they
            carry no loudness information and would dominate a dB metric.

    Returns:
        ``energy_rmse_db`` — RMS of the level error in dB;
        ``energy_bias_db`` — signed mean level error (systematically loud or
        quiet), which an RMSE alone hides;
        ``energy_corr`` — correlation of the two envelopes in dB.
    """
    valid = valid.float()
    audible = valid * (target_energy > floor).float()

    target_db = 20.0 * torch.log10(target_energy.clamp(min=floor))
    pred_db = 20.0 * torch.log10(pred_energy.clamp(min=floor))
    error = (pred_db - target_db) * audible

    return {
        "energy_rmse_db": _masked_mean(error.pow(2), audible).sqrt(),
        "energy_bias_db": _masked_mean(error, audible),
        "energy_corr": pearson_correlation(target_db, pred_db, audible),
    }
