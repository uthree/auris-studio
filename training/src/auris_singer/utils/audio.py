"""Spectrogram / mel / energy front-end.

All framing follows the VITS convention: the waveform is reflection-padded by
``(n_fft - hop_length) // 2`` on both sides and analysed with ``center=False``,
so a waveform of ``L`` samples yields exactly ``L // hop_length`` frames.  This
keeps spectrograms, f0 and energy on a single shared frame grid.
"""

from __future__ import annotations

import functools

import torch
import torchaudio

__all__ = [
    "spectrogram",
    "spec_to_mel",
    "mel_spectrogram",
    "frame_energy",
    "dynamic_range_compression",
    "num_frames",
]


def num_frames(num_samples: int, hop_length: int) -> int:
    """Number of analysis frames produced for ``num_samples`` samples."""
    return num_samples // hop_length


def dynamic_range_compression(x: torch.Tensor, clip_val: float = 1e-5) -> torch.Tensor:
    """Log compression used for mel spectrograms."""
    return torch.log(torch.clamp(x, min=clip_val))


@functools.lru_cache(maxsize=32)
def _hann_window(win_length: int, device: str, dtype: torch.dtype) -> torch.Tensor:
    return torch.hann_window(win_length, device=torch.device(device), dtype=dtype)


@functools.lru_cache(maxsize=32)
def _mel_basis(
    sample_rate: int,
    n_fft: int,
    n_mels: int,
    f_min: float,
    f_max: float | None,
    device: str,
    dtype: torch.dtype,
) -> torch.Tensor:
    fb = torchaudio.functional.melscale_fbanks(
        n_freqs=n_fft // 2 + 1,
        f_min=f_min,
        f_max=f_max if f_max is not None else sample_rate / 2,
        n_mels=n_mels,
        sample_rate=sample_rate,
        norm="slaney",
        mel_scale="slaney",
    )
    # (n_mels, n_freqs)
    return fb.T.to(device=torch.device(device), dtype=dtype)


def _pad_for_stft(wav: torch.Tensor, n_fft: int, hop_length: int) -> torch.Tensor:
    pad = (n_fft - hop_length) // 2
    return torch.nn.functional.pad(wav.unsqueeze(1), (pad, pad), mode="reflect").squeeze(1)


def spectrogram(
    wav: torch.Tensor,
    n_fft: int,
    hop_length: int,
    win_length: int,
    power: float = 1.0,
) -> torch.Tensor:
    """Linear-frequency magnitude spectrogram.

    Args:
        wav: ``(B, L)`` or ``(L,)`` waveform in ``[-1, 1]``.

    Returns:
        ``(B, n_fft // 2 + 1, L // hop_length)``.
    """
    squeeze = wav.dim() == 1
    if squeeze:
        wav = wav.unsqueeze(0)
    # cuFFT has no half/bfloat16 kernels, so the transform always runs in
    # float32 regardless of the surrounding autocast context.
    padded = _pad_for_stft(wav.float(), n_fft, hop_length)
    window = _hann_window(win_length, str(wav.device), torch.float32)
    spec = torch.stft(
        padded,
        n_fft=n_fft,
        hop_length=hop_length,
        win_length=win_length,
        window=window,
        center=False,
        pad_mode="reflect",
        normalized=False,
        onesided=True,
        return_complex=True,
    )
    mag = torch.clamp(spec.real.pow(2) + spec.imag.pow(2), min=1e-9)
    mag = mag.pow(power / 2.0)
    return mag.squeeze(0) if squeeze else mag


def spec_to_mel(
    spec: torch.Tensor,
    sample_rate: int,
    n_fft: int,
    n_mels: int,
    f_min: float = 0.0,
    f_max: float | None = None,
    log: bool = True,
) -> torch.Tensor:
    """Project a magnitude spectrogram ``(B, F, T)`` onto the mel scale."""
    basis = _mel_basis(sample_rate, n_fft, n_mels, f_min, f_max, str(spec.device), spec.dtype)
    mel = torch.matmul(basis, spec)
    return dynamic_range_compression(mel) if log else mel


def mel_spectrogram(
    wav: torch.Tensor,
    sample_rate: int,
    n_fft: int,
    hop_length: int,
    win_length: int,
    n_mels: int,
    f_min: float = 0.0,
    f_max: float | None = None,
    log: bool = True,
) -> torch.Tensor:
    """Log-mel spectrogram of ``(B, L)`` waveforms."""
    spec = spectrogram(wav, n_fft, hop_length, win_length, power=1.0)
    if spec.dim() == 2:
        spec = spec.unsqueeze(0)
        mel = spec_to_mel(spec, sample_rate, n_fft, n_mels, f_min, f_max, log)
        return mel.squeeze(0)
    return spec_to_mel(spec, sample_rate, n_fft, n_mels, f_min, f_max, log)


def frame_energy(
    wav: torch.Tensor,
    n_fft: int,
    hop_length: int,
    win_length: int,
) -> torch.Tensor:
    """Per-frame RMS energy on the same frame grid as :func:`spectrogram`.

    Returns:
        ``(B, T)`` (or ``(T,)`` for a 1D input) of non-negative RMS values.
    """
    squeeze = wav.dim() == 1
    if squeeze:
        wav = wav.unsqueeze(0)
    padded = _pad_for_stft(wav, n_fft, hop_length)
    frames = padded.unfold(-1, n_fft, hop_length)  # (B, T, n_fft)
    offset = (n_fft - win_length) // 2
    frames = frames[..., offset : offset + win_length]
    energy = frames.pow(2).mean(-1).clamp(min=1e-12).sqrt()
    return energy.squeeze(0) if squeeze else energy
