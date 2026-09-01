"""Tests for the speaker-conditional discriminator ensemble."""

from __future__ import annotations

import torch

from auris_singer.modules.discriminator import (
    Discriminator,
    MultiPeriodDiscriminator,
    MultiResolutionSTFTDiscriminator,
)

RESOLUTIONS = ((512, 120, 512), (1024, 240, 1024))


def test_ensemble_returns_one_output_per_sub_discriminator():
    disc = Discriminator(
        n_speakers=3,
        periods=(2, 3),
        resolutions=RESOLUTIONS,
        period_kwargs={"channels": (4, 8, 16)},
        stft_kwargs={"channels": 4, "n_layers": 2},
    )
    wav = torch.randn(2, 1, 12_000) * 0.1
    speakers = torch.tensor([0, 2])
    outputs, fmaps = disc(wav, speakers)

    assert len(outputs) == 2 + len(RESOLUTIONS)
    assert len(fmaps) == len(outputs)
    for out, fmap in zip(outputs, fmaps):
        assert out.dim() == 2 and out.size(0) == 2
        assert all(feature.size(0) == 2 for feature in fmap)


def test_period_discriminator_handles_lengths_not_divisible_by_period():
    mpd = MultiPeriodDiscriminator(periods=(3, 7), n_speakers=0, channels=(4, 8))
    outputs, _ = mpd(torch.randn(1, 1, 1_001) * 0.1)
    assert len(outputs) == 2


def test_speaker_conditioning_changes_the_logits():
    torch.manual_seed(0)
    disc = MultiResolutionSTFTDiscriminator(
        resolutions=RESOLUTIONS, n_speakers=4, channels=4, n_layers=2
    )
    # Projection embeddings are zero-initialized, so give them content.
    for sub in disc.discriminators:
        torch.nn.init.normal_(sub.projection.embedding.weight, std=1.0)

    wav = torch.randn(1, 1, 12_000) * 0.1
    with torch.no_grad():
        first, _ = disc(wav, torch.tensor([0]))
        second, _ = disc(wav, torch.tensor([3]))
    assert not torch.allclose(first[0], second[0], atol=1e-5)


def test_conditioning_is_inert_at_initialization():
    """Zero-initialized projection means training starts unconditioned."""
    torch.manual_seed(0)
    disc = MultiResolutionSTFTDiscriminator(
        resolutions=RESOLUTIONS, n_speakers=4, channels=4, n_layers=2
    )
    wav = torch.randn(1, 1, 12_000) * 0.1
    with torch.no_grad():
        conditioned, _ = disc(wav, torch.tensor([2]))
        unconditioned, _ = disc(wav, None)
    assert torch.allclose(conditioned[0], unconditioned[0], atol=1e-6)


def test_stft_discriminator_accepts_reduced_precision_input():
    """FFT kernels have no bfloat16 support, so the STFT must run in float32."""
    disc = MultiResolutionSTFTDiscriminator(
        resolutions=RESOLUTIONS, n_speakers=0, channels=4, n_layers=2
    )
    wav = (torch.randn(1, 1, 12_000) * 0.1).to(torch.bfloat16)
    outputs, _ = disc(wav, None)
    assert torch.isfinite(outputs[0].float()).all()


def test_gradients_reach_the_input_waveform():
    disc = Discriminator(
        n_speakers=2,
        periods=(2,),
        resolutions=(RESOLUTIONS[0],),
        period_kwargs={"channels": (4, 8)},
        stft_kwargs={"channels": 4, "n_layers": 2},
    )
    wav = (torch.randn(1, 1, 12_000) * 0.1).requires_grad_(True)
    outputs, _ = disc(wav, torch.tensor([1]))
    sum(out.square().mean() for out in outputs).backward()
    assert wav.grad is not None and torch.isfinite(wav.grad).all()
