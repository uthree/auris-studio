"""Tests for the spectrogram / mel / energy front-end."""

from __future__ import annotations

import math

import pytest
import torch

from auris_singer.utils.audio import (
    frame_energy,
    mel_spectrogram,
    num_frames,
    spectrogram,
)

SAMPLE_RATE = 48_000
HOP = 480
N_FFT = 2048
WIN = 2048


@pytest.mark.parametrize("n_samples", [HOP * 10, HOP * 37, HOP * 100])
def test_frame_count_is_exactly_length_over_hop(n_samples):
    wav = torch.randn(2, n_samples) * 0.1
    spec = spectrogram(wav, N_FFT, HOP, WIN)
    assert spec.shape == (2, N_FFT // 2 + 1, num_frames(n_samples, HOP))
    assert frame_energy(wav, N_FFT, HOP, WIN).shape == (2, n_samples // HOP)


def test_spectrogram_accepts_unbatched_input():
    wav = torch.randn(HOP * 12) * 0.1
    assert spectrogram(wav, N_FFT, HOP, WIN).shape == (N_FFT // 2 + 1, 12)
    assert mel_spectrogram(wav, SAMPLE_RATE, N_FFT, HOP, WIN, 80).shape == (80, 12)


def test_one_hop_clip_uses_safe_edge_padding():
    wav = torch.randn(1, HOP) * 0.1

    assert spectrogram(wav, N_FFT, HOP, WIN).shape == (1, N_FFT // 2 + 1, 1)
    assert frame_energy(wav, N_FFT, HOP, WIN).shape == (1, 1)


def test_spectrogram_peaks_at_the_tone_frequency():
    freq = 1000.0
    t = torch.arange(HOP * 40, dtype=torch.float32) / SAMPLE_RATE
    wav = torch.sin(2 * math.pi * freq * t).unsqueeze(0)
    spec = spectrogram(wav, N_FFT, HOP, WIN)
    peak_bin = int(spec[0, :, 20].argmax())
    assert peak_bin == pytest.approx(freq * N_FFT / SAMPLE_RATE, abs=1.0)


def test_frame_energy_tracks_amplitude():
    wav = torch.ones(1, HOP * 20) * 0.5
    energy = frame_energy(wav, N_FFT, HOP, WIN)
    # Interior frames see only the constant signal; edges include reflection
    # padding of the same constant, so every frame should read 0.5.
    assert torch.allclose(energy, torch.full_like(energy, 0.5), atol=1e-4)

    quiet = frame_energy(wav * 0.1, N_FFT, HOP, WIN)
    assert torch.allclose(quiet, energy * 0.1, atol=1e-4)


def test_spectrogram_runs_in_float32_for_reduced_precision_input():
    """FFT kernels have no bfloat16 support, so the transform must upcast."""
    wav = (torch.randn(1, HOP * 12) * 0.1).to(torch.bfloat16)
    spec = spectrogram(wav, N_FFT, HOP, WIN)
    assert spec.dtype == torch.float32
    assert torch.isfinite(spec).all()
    assert mel_spectrogram(wav, SAMPLE_RATE, N_FFT, HOP, WIN, 80).dtype == torch.float32


def test_mel_spectrogram_is_log_compressed():
    wav = torch.randn(1, HOP * 30) * 0.1
    linear = mel_spectrogram(wav, SAMPLE_RATE, N_FFT, HOP, WIN, 80, log=False)
    log = mel_spectrogram(wav, SAMPLE_RATE, N_FFT, HOP, WIN, 80, log=True)
    assert torch.allclose(log, torch.log(linear.clamp(min=1e-5)), atol=1e-5)
