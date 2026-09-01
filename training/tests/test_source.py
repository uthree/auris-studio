"""Tests for the RefineGAN-style source signal generator."""

from __future__ import annotations

import pytest
import torch

from auris_singer.modules.source import SourceSignalGenerator

SAMPLE_RATE = 48_000
HOP = 480


@pytest.fixture
def generator():
    gen = SourceSignalGenerator(SAMPLE_RATE, HOP, random_phase=False)
    gen.eval()
    return gen


def test_output_length_matches_hop(generator):
    f0 = torch.full((2, 1, 30), 220.0)
    energy = torch.full((2, 1, 30), 0.1)
    voiced = torch.ones(2, 1, 30)
    assert generator(f0, energy, voiced).shape == (2, 1, 30 * HOP)


def test_voiced_frames_produce_an_impulse_train_at_f0(generator):
    f0_hz = 200.0
    n_frames = 20
    f0 = torch.full((1, 1, n_frames), f0_hz)
    energy = torch.ones(1, 1, n_frames)
    voiced = torch.ones(1, 1, n_frames)

    source = generator(f0, energy, voiced)[0, 0]
    # Impulses dominate; locate them by amplitude.
    impulse_positions = torch.nonzero(source > source.max() * 0.5).flatten()
    spacing = (impulse_positions[1:] - impulse_positions[:-1]).float()
    assert spacing.mean().item() == pytest.approx(SAMPLE_RATE / f0_hz, rel=0.02)


def test_unvoiced_frames_produce_noise_not_impulses(generator):
    n_frames = 20
    f0 = torch.zeros(1, 1, n_frames)
    energy = torch.ones(1, 1, n_frames)
    voiced = torch.zeros(1, 1, n_frames)

    source = generator(f0, energy, voiced)[0, 0]
    assert source.abs().max() <= 1.0 + 1e-5
    # Uniform noise on [-1, 1] has an RMS near 1/sqrt(3).
    assert source.pow(2).mean().sqrt().item() == pytest.approx(0.577, rel=0.1)


def test_amplitude_follows_the_energy_envelope(generator):
    n_frames = 20
    f0 = torch.full((1, 1, n_frames), 220.0)
    voiced = torch.ones(1, 1, n_frames)
    loud = generator(f0, torch.full((1, 1, n_frames), 0.4), voiced)
    quiet = generator(f0, torch.full((1, 1, n_frames), 0.1), voiced)
    ratio = loud.abs().sum() / quiet.abs().sum()
    assert ratio.item() == pytest.approx(4.0, rel=0.05)


def test_impulse_normalization_keeps_rms_flat_across_pitch(generator):
    """Without normalization a low-pitched impulse train would be much quieter."""
    energy = torch.ones(1, 1, 40)
    voiced = torch.ones(1, 1, 40)
    low = generator(torch.full((1, 1, 40), 100.0), energy, voiced)
    high = generator(torch.full((1, 1, 40), 400.0), energy, voiced)
    low_rms = low.pow(2).mean().sqrt().item()
    high_rms = high.pow(2).mean().sqrt().item()
    assert low_rms == pytest.approx(high_rms, rel=0.15)


def test_explicit_noise_makes_the_excitation_deterministic(generator):
    n_frames = 20
    f0 = torch.cat([torch.zeros(1, 1, 10), torch.full((1, 1, 10), 220.0)], dim=-1)
    energy = torch.ones(1, 1, n_frames)
    noise = torch.rand(1, 1, n_frames * HOP) * 2.0 - 1.0

    once = generator(f0, energy, noise=noise)
    twice = generator(f0, energy, noise=noise)
    assert torch.equal(once, twice)
    # Fresh internal noise differs in the unvoiced half, so the argument is
    # actually being used rather than ignored.
    assert not torch.equal(once, generator(f0, energy))


def test_voiced_flag_is_derived_from_f0_when_missing(generator):
    f0 = torch.cat([torch.zeros(1, 1, 10), torch.full((1, 1, 10), 220.0)], dim=-1)
    energy = torch.ones(1, 1, 20)
    source = generator(f0, energy)
    unvoiced_part = source[0, 0, : 10 * HOP]
    voiced_part = source[0, 0, 10 * HOP :]
    # The impulse branch has a much higher peak than uniform noise.
    assert voiced_part.abs().max() > unvoiced_part.abs().max() * 2
