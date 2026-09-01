"""RefineGAN-style excitation ("template") signal generator.

Following RefineGAN (https://arxiv.org/abs/2111.00962) the vocoder is driven by
an explicit source signal rather than by f0/energy embeddings added to the
acoustic features:

* voiced frames  -> an impulse train at the instantaneous f0
* unvoiced frames -> uniform random noise
* both branches are scaled by the frame-level RMS energy of the reference
  audio, so the excitation already carries the intended loudness envelope.

Because f0 and energy only ever enter the model through this signal, pitch and
loudness at synthesis time are controlled entirely by the source.
"""

from __future__ import annotations

import torch
import torch.nn as nn
import torch.nn.functional as F

__all__ = ["SourceSignalGenerator"]


def _upsample_curve(x: torch.Tensor, hop_length: int, mode: str) -> torch.Tensor:
    """Upsample a ``(B, 1, T)`` frame-rate curve to ``(B, 1, T * hop_length)``."""
    if mode == "nearest":
        return x.repeat_interleave(hop_length, dim=-1)
    return F.interpolate(x, scale_factor=hop_length, mode="linear", align_corners=False)


class SourceSignalGenerator(nn.Module):
    """Turn ``(f0, energy, voiced)`` curves into a sample-rate excitation signal.

    Args:
        sample_rate: waveform sample rate.
        hop_length: frame hop of the f0/energy curves.
        noise_amplitude: amplitude of the unvoiced uniform noise, relative to
            the energy envelope.
        voiced_noise_amplitude: small amount of noise kept in voiced frames; it
            gives the network a stochastic component for breathiness.
        f0_min: f0 values below this are treated as unvoiced.
        normalize_impulse: scale impulses by ``sqrt(sample_rate / f0)`` so the
            impulse train has unit RMS regardless of pitch. Without it, the
            excitation would get quieter as f0 drops.
    """

    def __init__(
        self,
        sample_rate: int,
        hop_length: int,
        noise_amplitude: float = 1.0,
        voiced_noise_amplitude: float = 0.03,
        f0_min: float = 40.0,
        normalize_impulse: bool = True,
        random_phase: bool = True,
    ):
        super().__init__()
        self.sample_rate = sample_rate
        self.hop_length = hop_length
        self.noise_amplitude = noise_amplitude
        self.voiced_noise_amplitude = voiced_noise_amplitude
        self.f0_min = f0_min
        self.normalize_impulse = normalize_impulse
        self.random_phase = random_phase

    def forward(
        self,
        f0: torch.Tensor,
        energy: torch.Tensor,
        voiced: torch.Tensor | None = None,
        noise: torch.Tensor | None = None,
    ) -> torch.Tensor:
        """
        Args:
            f0: ``(B, 1, T)`` fundamental frequency in Hz; 0 (or ``< f0_min``)
                marks unvoiced frames.
            energy: ``(B, 1, T)`` linear RMS energy per frame.
            voiced: ``(B, 1, T)`` binary voiced flag; derived from ``f0`` when
                omitted.
            noise: ``(B, 1, T * hop_length)`` uniform noise on ``[-1, 1]``,
                drawn fresh when omitted. Passing it in makes the excitation a
                pure function of its inputs, which is what a seedable render
                and the ONNX export need.

        Returns:
            ``(B, 1, T * hop_length)`` excitation signal.
        """
        if voiced is None:
            voiced = (f0 >= self.f0_min).to(f0.dtype)

        f0_up = _upsample_curve(f0, self.hop_length, "linear")
        env_up = _upsample_curve(energy, self.hop_length, "linear")
        voiced_up = _upsample_curve(voiced, self.hop_length, "nearest")

        f0_up = f0_up.clamp(min=self.f0_min)

        # Instantaneous phase in cycles; an impulse is emitted whenever the
        # accumulated phase crosses an integer boundary.
        phase_inc = f0_up / self.sample_rate
        phase = torch.cumsum(phase_inc, dim=-1)
        if self.random_phase and self.training:
            phase = phase + torch.rand(
                f0.size(0), 1, 1, device=f0.device, dtype=f0.dtype
            )
        wrapped = torch.floor(phase)
        impulse = (wrapped - F.pad(wrapped, (1, 0))[..., :-1] > 0).to(f0.dtype)

        if self.normalize_impulse:
            impulse = impulse * torch.sqrt(self.sample_rate / f0_up)

        if noise is None:
            noise = torch.rand_like(impulse) * 2.0 - 1.0
        harmonic = impulse + noise * self.voiced_noise_amplitude
        source = harmonic * voiced_up + noise * self.noise_amplitude * (1.0 - voiced_up)
        return source * env_up
