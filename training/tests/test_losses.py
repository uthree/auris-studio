"""Tests for the training objectives."""

from __future__ import annotations

import math

import pytest
import torch

from auris_singer.losses import (
    EnvelopeLoss,
    MultiParamMelLoss,
    discriminator_loss,
    feature_matching_loss,
    generator_adversarial_loss,
    kl_loss,
)


def test_envelope_loss_is_zero_for_identical_waveforms():
    wav = torch.randn(2, 1, 16_000) * 0.2
    assert EnvelopeLoss()(wav, wav).item() == pytest.approx(0.0, abs=1e-7)


def test_envelope_loss_detects_amplitude_mismatch():
    """A mel loss is largely blind to a pure gain change; this one is not."""
    torch.manual_seed(0)
    t = torch.arange(16_000, dtype=torch.float32) / 48_000
    wav = torch.sin(2 * math.pi * 220 * t).view(1, 1, -1) * 0.5
    loss = EnvelopeLoss()
    assert loss(wav, wav * 0.5).item() > loss(wav, wav * 0.95).item() > 0


def test_envelope_loss_skips_windows_longer_than_the_signal():
    short = torch.randn(1, 1, 100) * 0.1
    assert torch.isfinite(EnvelopeLoss(kernel_sizes=(64, 4096))(short, short))


def test_multi_param_mel_loss_is_zero_for_identical_waveforms():
    wav = torch.randn(2, 1, 16_000) * 0.2
    loss = MultiParamMelLoss(params=((512, 120, 512, 40), (1024, 240, 1024, 80)))
    assert loss(wav, wav).item() == pytest.approx(0.0, abs=1e-6)


def test_multi_param_mel_loss_accepts_2d_and_3d_inputs():
    loss = MultiParamMelLoss(params=((512, 120, 512, 40),))
    wav_a = torch.randn(2, 8_000) * 0.2
    wav_b = torch.randn(2, 8_000) * 0.2
    assert torch.allclose(
        loss(wav_a, wav_b), loss(wav_a.unsqueeze(1), wav_b.unsqueeze(1))
    )


def test_multi_param_mel_loss_skips_resolutions_that_do_not_fit():
    loss = MultiParamMelLoss(params=((512, 120, 512, 40), (4096, 960, 4096, 160)))
    short = torch.randn(1, 1, 2_000) * 0.1
    assert torch.isfinite(loss(short, short * 0.5))


def test_kl_loss_vanishes_in_expectation_when_prior_matches_posterior():
    """The estimator is stochastic, so only its expectation is 0."""
    torch.manual_seed(0)
    b, c, t = 8, 64, 200
    m = torch.randn(b, c, t)
    logs = torch.zeros(b, c, t)
    mask = torch.ones(b, 1, t)
    z_p = m + torch.randn_like(m)  # a genuine sample from the matching q
    value = kl_loss(z_p, logs, m, logs, mask).item() / c
    assert value == pytest.approx(0.0, abs=0.02)


def test_kl_loss_grows_with_prior_mismatch():
    b, c, t = 1, 4, 10
    z_p = torch.zeros(b, c, t)
    logs_q = torch.zeros(b, c, t)
    logs_p = torch.zeros(b, c, t)
    mask = torch.ones(b, 1, t)
    near = kl_loss(z_p, logs_q, torch.full_like(z_p, 0.5), logs_p, mask)
    far = kl_loss(z_p, logs_q, torch.full_like(z_p, 2.0), logs_p, mask)
    assert far.item() > near.item()


def test_kl_loss_is_normalized_per_frame_not_per_element():
    """The weight scale follows VITS: summed over channels, averaged over frames."""
    b, c, t = 1, 4, 10
    ones = torch.ones(b, c, t)
    value = kl_loss(torch.zeros(b, c, t), torch.zeros(b, c, t), ones, torch.zeros(b, c, t), torch.ones(b, 1, t))
    # per element: -0.5 + 0.5 * 1 = 0; summed over c=4 channels it stays 0,
    # so use an asymmetric case to expose the factor.
    value = kl_loss(torch.zeros(b, c, t), torch.zeros(b, c, t), 2 * ones, torch.zeros(b, c, t), torch.ones(b, 1, t))
    assert value.item() == pytest.approx(c * (-0.5 + 0.5 * 4.0), rel=1e-5)


def test_kl_loss_ignores_masked_frames():
    b, c, t = 1, 4, 10
    mask = torch.ones(b, 1, t)
    mask[..., 5:] = 0
    z_p = torch.randn(b, c, t)
    logs = torch.zeros(b, c, t)
    m_p = torch.randn(b, c, t)
    polluted = m_p.clone()
    polluted[..., 5:] = 100.0  # garbage in the padded region

    assert kl_loss(z_p, logs, m_p, logs, mask).item() == pytest.approx(
        kl_loss(z_p, logs, polluted, logs, mask).item(), abs=1e-6
    )


def test_adversarial_losses_reward_the_expected_direction():
    perfect_real = [torch.ones(2, 4)]
    perfect_fake = [torch.zeros(2, 4)]
    loss, real_parts, fake_parts = discriminator_loss(perfect_real, perfect_fake)
    assert loss.item() == pytest.approx(0.0, abs=1e-6)
    assert real_parts == pytest.approx([0.0]) and fake_parts == pytest.approx([0.0])

    # The generator wants the discriminator to output 1 on its samples.
    fooled, _ = generator_adversarial_loss([torch.ones(2, 4)])
    caught, _ = generator_adversarial_loss([torch.zeros(2, 4)])
    assert fooled.item() < caught.item()


def test_feature_matching_loss_is_zero_for_identical_feature_maps():
    fmap = [[torch.randn(2, 3, 5), torch.randn(2, 3, 5)]]
    assert feature_matching_loss(fmap, fmap).item() == pytest.approx(0.0, abs=1e-7)


def test_feature_matching_does_not_backpropagate_into_the_real_branch():
    real = [[torch.randn(2, 3, 5, requires_grad=True)]]
    fake = [[torch.randn(2, 3, 5, requires_grad=True)]]
    feature_matching_loss(real, fake).backward()
    assert real[0][0].grad is None
    assert fake[0][0].grad is not None


def test_free_bits_stops_the_kl_from_being_driven_to_zero():
    """Without a floor, matching the prior exactly costs nothing."""
    b, c, t = 2, 8, 20
    logs = torch.zeros(b, c, t)
    mask = torch.ones(b, 1, t)
    matched = torch.zeros(b, c, t)  # z_p == m_p, logs_q == logs_p

    plain = kl_loss(matched, logs, matched, logs, mask)
    floored = kl_loss(matched, logs, matched, logs, mask, free_bits=0.5)
    assert plain.item() < 0
    # Every channel is clamped at the floor, so the total is c * free_bits.
    assert floored.item() == pytest.approx(c * 0.5, rel=1e-5)


def test_free_bits_leaves_channels_above_the_floor_untouched():
    b, c, t = 1, 4, 10
    logs = torch.zeros(b, c, t)
    mask = torch.ones(b, 1, t)
    z_p = torch.zeros(b, c, t)
    m_p = torch.zeros(b, c, t)
    m_p[:, 0] = 3.0  # one channel is far from the prior: KL = -0.5 + 4.5 = 4.0

    floored = kl_loss(z_p, logs, m_p, logs, mask, free_bits=0.5)
    # 4.0 for the informative channel, the floor for the other three.
    assert floored.item() == pytest.approx(4.0 + 3 * 0.5, rel=1e-5)


def test_free_bits_zero_matches_the_plain_estimator():
    torch.manual_seed(0)
    b, c, t = 2, 6, 15
    args = (torch.randn(b, c, t), torch.zeros(b, c, t), torch.randn(b, c, t),
            torch.zeros(b, c, t), torch.ones(b, 1, t))
    assert kl_loss(*args, free_bits=0.0).item() == pytest.approx(
        kl_loss(*args).item(), rel=1e-6
    )
