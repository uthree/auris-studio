"""Discriminators with speaker conditioning.

The HiFi-GAN multi-scale discriminator is replaced by a multi-resolution STFT
discriminator, which supervises the complex spectrum at several time/frequency
trade-offs instead of several waveform scales.  The multi-period discriminator
is kept.

Both are conditional in the projection sense (Miyato & Koyama, 2018): the
speaker embedding is projected onto the final feature map and its inner product
is added to the logit map, which conditions the discriminator without giving it
an easy shortcut.
"""

from __future__ import annotations

import torch
import torch.nn as nn
import torch.nn.functional as F
from torch.nn.utils.parametrizations import weight_norm

__all__ = [
    "PeriodDiscriminator",
    "MultiPeriodDiscriminator",
    "STFTDiscriminator",
    "MultiResolutionSTFTDiscriminator",
    "Discriminator",
]


def _get_padding(kernel_size: int, dilation: int = 1) -> int:
    return (kernel_size * dilation - dilation) // 2


class _ProjectionConditioning(nn.Module):
    """Add ``<embedding(speaker), feature_map>`` to a logit map."""

    def __init__(self, n_speakers: int, channels: int):
        super().__init__()
        self.enabled = n_speakers > 0
        if self.enabled:
            self.embedding = nn.Embedding(n_speakers, channels)
            nn.init.zeros_(self.embedding.weight)

    def forward(
        self, logits: torch.Tensor, features: torch.Tensor, speaker_ids: torch.Tensor | None
    ) -> torch.Tensor:
        if not self.enabled or speaker_ids is None:
            return logits
        emb = self.embedding(speaker_ids)  # (B, C)
        emb = emb.view(emb.size(0), emb.size(1), *([1] * (features.dim() - 2)))
        return logits + (features * emb).sum(dim=1, keepdim=True)


class PeriodDiscriminator(nn.Module):
    """HiFi-GAN period discriminator, conditioned on the speaker."""

    def __init__(
        self,
        period: int,
        n_speakers: int = 0,
        kernel_size: int = 5,
        stride: int = 3,
        channels: tuple[int, ...] = (32, 128, 512, 1024, 1024),
        leaky_slope: float = 0.1,
    ):
        super().__init__()
        self.period = period
        self.leaky_slope = leaky_slope
        convs = []
        in_ch = 1
        for i, out_ch in enumerate(channels):
            is_last = i == len(channels) - 1
            convs.append(
                weight_norm(
                    nn.Conv2d(
                        in_ch,
                        out_ch,
                        (kernel_size, 1),
                        (1 if is_last else stride, 1),
                        padding=(_get_padding(kernel_size), 0),
                    )
                )
            )
            in_ch = out_ch
        self.convs = nn.ModuleList(convs)
        self.conv_post = weight_norm(nn.Conv2d(in_ch, 1, (3, 1), padding=(1, 0)))
        self.projection = _ProjectionConditioning(n_speakers, in_ch)

    def forward(
        self, x: torch.Tensor, speaker_ids: torch.Tensor | None = None
    ) -> tuple[torch.Tensor, list[torch.Tensor]]:
        b, c, t = x.shape
        if t % self.period != 0:
            x = F.pad(x, (0, self.period - (t % self.period)), mode="reflect")
            t = x.size(-1)
        h = x.view(b, c, t // self.period, self.period)

        fmap = []
        for conv in self.convs:
            h = F.leaky_relu(conv(h), self.leaky_slope)
            fmap.append(h)
        logits = self.projection(self.conv_post(h), h, speaker_ids)
        fmap.append(logits)
        return logits.flatten(1), fmap


class MultiPeriodDiscriminator(nn.Module):
    def __init__(
        self,
        periods: tuple[int, ...] = (2, 3, 5, 7, 11),
        n_speakers: int = 0,
        **kwargs,
    ):
        super().__init__()
        self.discriminators = nn.ModuleList(
            PeriodDiscriminator(p, n_speakers=n_speakers, **kwargs) for p in periods
        )

    def forward(
        self, x: torch.Tensor, speaker_ids: torch.Tensor | None = None
    ) -> tuple[list[torch.Tensor], list[list[torch.Tensor]]]:
        outputs, fmaps = [], []
        for disc in self.discriminators:
            out, fmap = disc(x, speaker_ids)
            outputs.append(out)
            fmaps.append(fmap)
        return outputs, fmaps


class STFTDiscriminator(nn.Module):
    """Discriminator over the complex STFT at one resolution."""

    def __init__(
        self,
        n_fft: int,
        hop_length: int,
        win_length: int,
        n_speakers: int = 0,
        channels: int = 32,
        n_layers: int = 4,
        leaky_slope: float = 0.1,
    ):
        super().__init__()
        self.n_fft = n_fft
        self.hop_length = hop_length
        self.win_length = win_length
        self.leaky_slope = leaky_slope
        self.register_buffer("window", torch.hann_window(win_length), persistent=False)

        convs = [
            weight_norm(nn.Conv2d(2, channels, (3, 9), padding=(1, 4)))
        ]
        for _ in range(n_layers - 1):
            convs.append(
                weight_norm(
                    nn.Conv2d(channels, channels, (3, 9), stride=(1, 2), padding=(1, 4))
                )
            )
        convs.append(weight_norm(nn.Conv2d(channels, channels, (3, 3), padding=(1, 1))))
        self.convs = nn.ModuleList(convs)
        self.conv_post = weight_norm(nn.Conv2d(channels, 1, (3, 3), padding=(1, 1)))
        self.projection = _ProjectionConditioning(n_speakers, channels)

    def _stft(self, x: torch.Tensor) -> torch.Tensor:
        # cuFFT has no half/bfloat16 kernels, so the transform always runs in
        # float32; under autocast the following convolutions cast it back.
        x = x.squeeze(1).float()
        pad = (self.n_fft - self.hop_length) // 2
        x = F.pad(x.unsqueeze(1), (pad, pad), mode="reflect").squeeze(1)
        spec = torch.stft(
            x,
            n_fft=self.n_fft,
            hop_length=self.hop_length,
            win_length=self.win_length,
            window=self.window.float(),
            center=False,
            return_complex=True,
        )
        # (B, 2, F, T)
        return torch.stack([spec.real, spec.imag], dim=1)

    def forward(
        self, x: torch.Tensor, speaker_ids: torch.Tensor | None = None
    ) -> tuple[torch.Tensor, list[torch.Tensor]]:
        h = self._stft(x)
        fmap = []
        for conv in self.convs:
            h = F.leaky_relu(conv(h), self.leaky_slope)
            fmap.append(h)
        logits = self.projection(self.conv_post(h), h, speaker_ids)
        fmap.append(logits)
        return logits.flatten(1), fmap


class MultiResolutionSTFTDiscriminator(nn.Module):
    """Replacement for HiFi-GAN's multi-scale discriminator."""

    def __init__(
        self,
        resolutions: tuple[tuple[int, int, int], ...] = (
            (512, 120, 512),
            (1024, 240, 1024),
            (2048, 480, 2048),
            (4096, 960, 4096),
        ),
        n_speakers: int = 0,
        **kwargs,
    ):
        super().__init__()
        self.discriminators = nn.ModuleList(
            STFTDiscriminator(n_fft, hop, win, n_speakers=n_speakers, **kwargs)
            for n_fft, hop, win in resolutions
        )

    def forward(
        self, x: torch.Tensor, speaker_ids: torch.Tensor | None = None
    ) -> tuple[list[torch.Tensor], list[list[torch.Tensor]]]:
        outputs, fmaps = [], []
        for disc in self.discriminators:
            out, fmap = disc(x, speaker_ids)
            outputs.append(out)
            fmaps.append(fmap)
        return outputs, fmaps


class Discriminator(nn.Module):
    """Multi-period + multi-resolution-STFT discriminator ensemble."""

    def __init__(
        self,
        n_speakers: int = 0,
        periods: tuple[int, ...] = (2, 3, 5, 7, 11),
        resolutions: tuple[tuple[int, int, int], ...] = (
            (512, 120, 512),
            (1024, 240, 1024),
            (2048, 480, 2048),
            (4096, 960, 4096),
        ),
        period_kwargs: dict | None = None,
        stft_kwargs: dict | None = None,
    ):
        super().__init__()
        self.mpd = MultiPeriodDiscriminator(
            periods=tuple(periods), n_speakers=n_speakers, **(period_kwargs or {})
        )
        self.mrd = MultiResolutionSTFTDiscriminator(
            resolutions=tuple(tuple(r) for r in resolutions),
            n_speakers=n_speakers,
            **(stft_kwargs or {}),
        )

    def forward(
        self, x: torch.Tensor, speaker_ids: torch.Tensor | None = None
    ) -> tuple[list[torch.Tensor], list[list[torch.Tensor]]]:
        """
        Args:
            x: ``(B, 1, L)`` waveform.
            speaker_ids: ``(B,)`` speaker indices for projection conditioning.
        """
        out_p, fmap_p = self.mpd(x, speaker_ids)
        out_s, fmap_s = self.mrd(x, speaker_ids)
        return out_p + out_s, fmap_p + fmap_s
