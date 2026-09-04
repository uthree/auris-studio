"""Tests for the NSF-HiFi-GAN waveform decoder."""

from __future__ import annotations

import pytest
import torch

from auris_singer.modules.generator import NsfHifiGanGenerator


def build(**overrides) -> NsfHifiGanGenerator:
    kwargs = dict(
        in_channels=8,
        sample_rate=48_000,
        hop_length=480,
        upsample_initial_channel=16,
        cond_channels=4,
    )
    kwargs.update(overrides)
    return NsfHifiGanGenerator(**kwargs)


@pytest.mark.parametrize("n_frames", [8, 17, 33])
def test_output_length_is_exactly_frames_times_hop(n_frames):
    gen = build().eval()
    x = torch.randn(2, 8, n_frames)
    f0 = torch.full((2, 1, n_frames), 220.0)
    energy = torch.full((2, 1, n_frames), 0.1)
    voiced = torch.ones(2, 1, n_frames)

    with torch.no_grad():
        wav, source = gen(x, f0, energy, voiced, g=torch.randn(2, 4, 1))

    assert wav.shape == (2, 1, n_frames * 480)
    assert source.shape == (2, 1, n_frames * 480)
    assert wav.abs().max() <= 1.0


@pytest.mark.parametrize(
    "rates,kernels",
    [((6, 5, 4, 4), (12, 10, 8, 8)), ((8, 6, 5, 2), (16, 12, 10, 4)), ((10, 8, 6), (20, 16, 12))],
)
def test_alternative_upsample_schedules_keep_exact_lengths(rates, kernels):
    hop = 1
    for rate in rates:
        hop *= rate
    gen = build(hop_length=hop, upsample_rates=rates, upsample_kernel_sizes=kernels).eval()
    n_frames = 11
    with torch.no_grad():
        wav, _ = gen(
            torch.randn(1, 8, n_frames),
            torch.full((1, 1, n_frames), 200.0),
            torch.full((1, 1, n_frames), 0.1),
            torch.ones(1, 1, n_frames),
        )
    assert wav.size(-1) == n_frames * hop


def test_upsample_rates_must_multiply_to_hop_length():
    with pytest.raises(ValueError, match="hop_length"):
        build(upsample_rates=(6, 5, 4), upsample_kernel_sizes=(12, 10, 8))


def test_stride_one_rejects_an_invalid_output_padding():
    with pytest.raises(ValueError, match="output_padding"):
        build(hop_length=1, upsample_rates=(1,), upsample_kernel_sizes=(2,))


def test_f0_changes_the_output():
    """f0 reaches the decoder only through the source signal, so it must matter."""
    torch.manual_seed(0)
    gen = build().eval()
    n_frames = 16
    x = torch.randn(1, 8, n_frames)
    energy = torch.full((1, 1, n_frames), 0.1)
    voiced = torch.ones(1, 1, n_frames)
    with torch.no_grad():
        low, _ = gen(x, torch.full((1, 1, n_frames), 110.0), energy, voiced)
        high, _ = gen(x, torch.full((1, 1, n_frames), 440.0), energy, voiced)
    assert not torch.allclose(low, high, atol=1e-4)


def test_remove_weight_norm_preserves_output():
    torch.manual_seed(0)
    gen = build().eval()
    n_frames = 12
    args = (
        torch.randn(1, 8, n_frames),
        torch.full((1, 1, n_frames), 220.0),
        torch.full((1, 1, n_frames), 0.1),
        torch.ones(1, 1, n_frames),
    )
    # The excitation contains noise, so both calls need the same RNG state.
    with torch.no_grad():
        torch.manual_seed(1)
        before, _ = gen(*args)
        gen.remove_weight_norm()
        torch.manual_seed(1)
        after, _ = gen(*args)
    # Folding the norm into the weights changes the order of operations, so
    # only float-accumulation-level differences are expected.
    assert torch.allclose(before, after, atol=1e-5)


def test_a_supplied_source_makes_the_generator_deterministic():
    """The excitation is stochastic; reusing it isolates the effect of z."""
    torch.manual_seed(0)
    gen = build(cond_channels=0)
    z = torch.randn(1, 8, 6)
    f0 = torch.full((1, 1, 6), 220.0)
    energy = torch.full((1, 1, 6), 0.1)
    voiced = torch.ones(1, 1, 6)

    first, source = gen(z, f0, energy, voiced)
    again, _ = gen(z, f0, energy, voiced)
    reused, echoed = gen(z, f0, energy, voiced, source=source)

    assert not torch.allclose(first, again), "a fresh excitation should differ"
    assert torch.equal(echoed, source)
    assert torch.allclose(first, reused, atol=1e-6)
