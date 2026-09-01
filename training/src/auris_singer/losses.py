"""Training objectives.

Beyond the standard VITS losses (LSGAN adversarial loss, feature matching, KL),
the vocoder is supervised with the two objectives proposed by RefineGAN
(https://arxiv.org/abs/2111.00962):

* :class:`EnvelopeLoss` — matches the upper and lower amplitude envelopes of
  the waveform at several time scales, which constrains loudness dynamics that
  a mel loss is largely blind to.
* :class:`MultiParamMelLoss` — the multi-parameter mel-spectrogram loss: the
  same L1 mel objective evaluated under several STFT parameterizations, so
  neither fine spectral detail nor temporal resolution is traded away.
"""

from __future__ import annotations

import torch
import torch.nn as nn
import torch.nn.functional as F

from auris_singer.utils.audio import mel_spectrogram

__all__ = [
    "discriminator_loss",
    "generator_adversarial_loss",
    "feature_matching_loss",
    "kl_loss",
    "EnvelopeLoss",
    "MultiParamMelLoss",
]


def discriminator_loss(
    real_outputs: list[torch.Tensor], fake_outputs: list[torch.Tensor]
) -> tuple[torch.Tensor, list[float], list[float]]:
    """Least-squares GAN loss for the discriminator."""
    total = torch.zeros((), device=real_outputs[0].device, dtype=real_outputs[0].dtype)
    real_losses, fake_losses = [], []
    for real, fake in zip(real_outputs, fake_outputs):
        r_loss = torch.mean((1.0 - real.float()) ** 2)
        f_loss = torch.mean(fake.float() ** 2)
        total = total + (r_loss + f_loss).to(total.dtype)
        real_losses.append(r_loss.item())
        fake_losses.append(f_loss.item())
    return total, real_losses, fake_losses


def generator_adversarial_loss(
    fake_outputs: list[torch.Tensor],
) -> tuple[torch.Tensor, list[float]]:
    """Least-squares GAN loss for the generator."""
    total = torch.zeros((), device=fake_outputs[0].device, dtype=fake_outputs[0].dtype)
    losses = []
    for fake in fake_outputs:
        loss = torch.mean((1.0 - fake.float()) ** 2)
        total = total + loss.to(total.dtype)
        losses.append(loss.item())
    return total, losses


def feature_matching_loss(
    real_fmaps: list[list[torch.Tensor]], fake_fmaps: list[list[torch.Tensor]]
) -> torch.Tensor:
    """L1 distance between intermediate discriminator activations."""
    total = torch.zeros(
        (), device=real_fmaps[0][0].device, dtype=real_fmaps[0][0].dtype
    )
    for real_maps, fake_maps in zip(real_fmaps, fake_fmaps):
        for real, fake in zip(real_maps, fake_maps):
            total = total + torch.mean(torch.abs(real.detach() - fake))
    return total * 2.0


def kl_loss(
    z_p: torch.Tensor,
    logs_q: torch.Tensor,
    m_p: torch.Tensor,
    logs_p: torch.Tensor,
    mask: torch.Tensor,
    free_bits: float = 0.0,
) -> torch.Tensor:
    """KL[ q(z|x) || p(z|c) ] evaluated at the flow output ``z_p``.

    This is the single-sample estimator used by VITS: the ``E[eps^2] / 2`` term
    of ``log q`` is replaced by its expectation, so the value is 0 *in
    expectation* when the two distributions match, and can be negative for an
    individual sample.

    The result is normalized per frame (summed over channels), matching VITS,
    so the ``kl`` loss weight keeps its usual scale.

    Args:
        z_p: ``(B, C, T)`` posterior sample pushed through the flow.
        logs_q: ``(B, C, T)`` posterior log-scale.
        m_p, logs_p: ``(B, C, T)`` prior statistics.
        mask: ``(B, 1, T)``.
        free_bits: per-channel KL floor in nats (Kingma et al., 2016). Channels
            whose KL is already below the floor stop being penalized, so the
            optimizer cannot buy loss by driving the posterior all the way onto
            the prior. 0 disables it and reproduces the VITS objective.

            This matters more here than in VITS: the decoder can read pitch and
            loudness off the excitation signal, so collapsing ``z`` to noise is
            a cheap local optimum that still produces plausible audio. See
            ``doc/architecture.md``.
    """
    z_p, logs_q = z_p.float(), logs_q.float()
    m_p, logs_p, mask = m_p.float(), logs_p.float(), mask.float()

    kl = logs_p - logs_q - 0.5
    kl = kl + 0.5 * ((z_p - m_p) ** 2) * torch.exp(-2.0 * logs_p)

    n_frames = torch.sum(mask).clamp(min=1.0)
    if free_bits > 0.0:
        # Average each channel over frames first, floor it, then sum over
        # channels so the result stays on the same scale as the plain estimator.
        per_channel = torch.sum(kl * mask, dim=[0, 2]) / n_frames
        return torch.clamp(per_channel, min=free_bits).sum()
    return torch.sum(kl * mask) / n_frames


class EnvelopeLoss(nn.Module):
    """RefineGAN envelope loss.

    The upper envelope is obtained with a max-pool over the waveform and the
    lower envelope with a max-pool over its negation.  Matching both at several
    window sizes forces the generated waveform to reproduce the amplitude
    dynamics of the reference, not only its average spectrum.
    """

    def __init__(self, kernel_sizes: tuple[int, ...] = (128, 256, 512, 1024)):
        super().__init__()
        self.kernel_sizes = tuple(kernel_sizes)

    @staticmethod
    def _envelopes(x: torch.Tensor, kernel_size: int) -> tuple[torch.Tensor, torch.Tensor]:
        stride = kernel_size // 2
        upper = F.max_pool1d(x, kernel_size, stride=stride, ceil_mode=True)
        lower = -F.max_pool1d(-x, kernel_size, stride=stride, ceil_mode=True)
        return upper, lower

    def forward(self, real: torch.Tensor, fake: torch.Tensor) -> torch.Tensor:
        """
        Args:
            real, fake: ``(B, 1, L)`` waveforms.
        """
        total = torch.zeros((), device=fake.device, dtype=torch.float32)
        for kernel_size in self.kernel_sizes:
            if real.size(-1) < kernel_size:
                continue
            r_up, r_low = self._envelopes(real.float(), kernel_size)
            f_up, f_low = self._envelopes(fake.float(), kernel_size)
            total = total + F.l1_loss(f_up, r_up) + F.l1_loss(f_low, r_low)
        return total / max(len(self.kernel_sizes), 1)


class MultiParamMelLoss(nn.Module):
    """Multi-parameter mel-spectrogram L1 loss.

    Args:
        sample_rate: waveform sample rate.
        params: one ``(n_fft, hop_length, win_length, n_mels)`` tuple per
            resolution.
        f_min / f_max: mel filter bank range. ``f_max=None`` uses Nyquist.
    """

    def __init__(
        self,
        sample_rate: int = 48_000,
        params: tuple[tuple[int, int, int, int], ...] = (
            (512, 120, 512, 40),
            (1024, 240, 1024, 80),
            (2048, 480, 2048, 128),
            (4096, 960, 4096, 160),
        ),
        f_min: float = 0.0,
        f_max: float | None = None,
    ):
        super().__init__()
        self.sample_rate = sample_rate
        self.params = tuple(tuple(p) for p in params)
        self.f_min = f_min
        self.f_max = f_max

    def forward(self, real: torch.Tensor, fake: torch.Tensor) -> torch.Tensor:
        """
        Args:
            real, fake: ``(B, 1, L)`` or ``(B, L)`` waveforms.
        """
        if real.dim() == 3:
            real = real.squeeze(1)
        if fake.dim() == 3:
            fake = fake.squeeze(1)
        real, fake = real.float(), fake.float()

        total = torch.zeros((), device=fake.device, dtype=torch.float32)
        used = 0
        for n_fft, hop_length, win_length, n_mels in self.params:
            if real.size(-1) < n_fft:
                continue
            kwargs = dict(
                sample_rate=self.sample_rate,
                n_fft=n_fft,
                hop_length=hop_length,
                win_length=win_length,
                n_mels=n_mels,
                f_min=self.f_min,
                f_max=self.f_max,
            )
            mel_real = mel_spectrogram(real, **kwargs)
            mel_fake = mel_spectrogram(fake, **kwargs)
            total = total + F.l1_loss(mel_fake, mel_real)
            used += 1
        return total / max(used, 1)
