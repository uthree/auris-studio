"""NSF-HiFi-GAN waveform decoder (48 kHz).

Differences from the HiFi-GAN used by VITS:

* the excitation produced by :class:`~auris_singer.modules.source.SourceSignalGenerator`
  is injected at every upsampling stage (neural source filter);
* every activation is SiLU instead of LeakyReLU;
* the upsampling schedule is configured for a 480-sample hop at 48 kHz.

No pitch or energy embedding is added to the acoustic features — f0 and energy
reach the decoder only through the source signal.
"""

from __future__ import annotations

import math

import torch
import torch.nn as nn
import torch.nn.functional as F
from torch.nn.utils.parametrizations import weight_norm
from torch.nn.utils.parametrize import remove_parametrizations

from auris_singer.modules.source import SourceSignalGenerator

__all__ = ["ResBlock", "NsfHifiGanGenerator", "get_padding"]


def get_padding(kernel_size: int, dilation: int = 1) -> int:
    return (kernel_size * dilation - dilation) // 2


def _match_length(x: torch.Tensor, length: int) -> torch.Tensor:
    """Trim or right-pad the last dimension of ``x`` to exactly ``length``."""
    if x.size(-1) == length:
        return x
    if x.size(-1) > length:
        return x[..., :length]
    return F.pad(x, (0, length - x.size(-1)))


class ResBlock(nn.Module):
    """HiFi-GAN residual block (type 1) with SiLU activations."""

    def __init__(
        self,
        channels: int,
        kernel_size: int = 3,
        dilations: tuple[int, ...] = (1, 3, 5),
    ):
        super().__init__()
        self.convs1 = nn.ModuleList(
            weight_norm(
                nn.Conv1d(
                    channels,
                    channels,
                    kernel_size,
                    dilation=d,
                    padding=get_padding(kernel_size, d),
                )
            )
            for d in dilations
        )
        self.convs2 = nn.ModuleList(
            weight_norm(
                nn.Conv1d(
                    channels,
                    channels,
                    kernel_size,
                    dilation=1,
                    padding=get_padding(kernel_size, 1),
                )
            )
            for _ in dilations
        )

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        for conv1, conv2 in zip(self.convs1, self.convs2):
            h = conv1(F.silu(x))
            h = conv2(F.silu(h))
            x = x + h
        return x

    def remove_weight_norm(self) -> None:
        for conv in list(self.convs1) + list(self.convs2):
            remove_parametrizations(conv, "weight", leave_parametrized=True)


class NsfHifiGanGenerator(nn.Module):
    """Source-filter waveform decoder.

    Args:
        in_channels: width of the latent ``z`` produced by the flow/posterior.
        upsample_rates: per-stage upsampling factors; their product must equal
            ``hop_length``.
        resblock_kernel_sizes / resblock_dilations: multi-receptive-field fusion
            configuration.
        cond_channels: speaker conditioning width (0 disables it).
    """

    def __init__(
        self,
        in_channels: int,
        sample_rate: int = 48_000,
        hop_length: int = 480,
        upsample_initial_channel: int = 512,
        upsample_rates: tuple[int, ...] = (6, 5, 4, 4),
        upsample_kernel_sizes: tuple[int, ...] = (12, 10, 8, 8),
        resblock_kernel_sizes: tuple[int, ...] = (3, 7, 11),
        resblock_dilations: tuple[tuple[int, ...], ...] = ((1, 3, 5), (1, 3, 5), (1, 3, 5)),
        cond_channels: int = 0,
        pre_kernel_size: int = 7,
        post_kernel_size: int = 7,
        source_noise_amplitude: float = 1.0,
        source_voiced_noise_amplitude: float = 0.03,
        f0_min: float = 40.0,
    ):
        super().__init__()
        if math.prod(upsample_rates) != hop_length:
            raise ValueError(
                f"prod(upsample_rates)={math.prod(upsample_rates)} must equal "
                f"hop_length={hop_length}"
            )
        if len(upsample_rates) != len(upsample_kernel_sizes):
            raise ValueError("upsample_rates and upsample_kernel_sizes must be the same length")

        self.sample_rate = sample_rate
        self.hop_length = hop_length
        self.num_upsamples = len(upsample_rates)
        self.num_kernels = len(resblock_kernel_sizes)
        self.upsample_rates = tuple(upsample_rates)

        self.source_generator = SourceSignalGenerator(
            sample_rate=sample_rate,
            hop_length=hop_length,
            noise_amplitude=source_noise_amplitude,
            voiced_noise_amplitude=source_voiced_noise_amplitude,
            f0_min=f0_min,
        )

        self.conv_pre = weight_norm(
            nn.Conv1d(
                in_channels,
                upsample_initial_channel,
                pre_kernel_size,
                padding=pre_kernel_size // 2,
            )
        )
        self.cond = (
            nn.Conv1d(cond_channels, upsample_initial_channel, 1) if cond_channels > 0 else None
        )

        self.ups = nn.ModuleList()
        self.source_convs = nn.ModuleList()
        self.resblocks = nn.ModuleList()
        self.source_strides: list[int] = []

        for i, (rate, kernel) in enumerate(zip(upsample_rates, upsample_kernel_sizes)):
            in_ch = upsample_initial_channel // (2**i)
            out_ch = upsample_initial_channel // (2 ** (i + 1))
            if kernel < rate:
                raise ValueError(
                    f"upsample kernel {kernel} must be >= its rate {rate}"
                )
            # Chosen so the transposed convolution outputs exactly `rate` times
            # its input length, for both even and odd `kernel - rate`.
            padding = (kernel - rate + 1) // 2
            output_padding = rate + 2 * padding - kernel
            if output_padding >= rate:
                raise ValueError(
                    f"upsample kernel {kernel} and rate {rate} require invalid "
                    f"output_padding={output_padding}; use rate > 1 or an odd kernel"
                )
            self.ups.append(
                weight_norm(
                    nn.ConvTranspose1d(
                        in_ch,
                        out_ch,
                        kernel,
                        rate,
                        padding=padding,
                        output_padding=output_padding,
                    )
                )
            )
            # Downsample the full-rate excitation to this stage's resolution.
            stride = math.prod(upsample_rates[i + 1 :])
            self.source_strides.append(stride)
            if stride > 1:
                self.source_convs.append(
                    nn.Conv1d(1, out_ch, kernel_size=stride * 2, stride=stride, padding=stride // 2)
                )
            else:
                self.source_convs.append(nn.Conv1d(1, out_ch, kernel_size=1))

            for kernel_size, dilations in zip(resblock_kernel_sizes, resblock_dilations):
                self.resblocks.append(ResBlock(out_ch, kernel_size, tuple(dilations)))

        final_ch = upsample_initial_channel // (2**self.num_upsamples)
        self.conv_post = weight_norm(
            nn.Conv1d(final_ch, 1, post_kernel_size, padding=post_kernel_size // 2, bias=False)
        )

    def forward(
        self,
        x: torch.Tensor,
        f0: torch.Tensor,
        energy: torch.Tensor,
        voiced: torch.Tensor | None = None,
        g: torch.Tensor | None = None,
        source: torch.Tensor | None = None,
    ) -> tuple[torch.Tensor, torch.Tensor]:
        """
        Args:
            x: ``(B, in_channels, T)`` latent acoustic features.
            f0: ``(B, 1, T)`` f0 in Hz.
            energy: ``(B, 1, T)`` linear RMS energy.
            voiced: ``(B, 1, T)`` binary voiced flag.
            g: ``(B, cond_channels, 1)`` speaker condition.
            source: precomputed ``(B, 1, T * hop_length)`` excitation. The
                generator is stochastic — unvoiced frames and the breathiness
                component of voiced frames are fresh noise on every call — so
                two runs with identical arguments do not produce identical
                audio. Pass a source back in to compare two runs that differ
                only in ``x``; ``f0``, ``energy`` and ``voiced`` are then unused.

        Returns:
            ``(waveform, source)`` of shape ``(B, 1, T * hop_length)``.
        """
        if source is None:
            source = self.source_generator(f0, energy, voiced)

        h = self.conv_pre(x)
        if self.cond is not None and g is not None:
            h = h + self.cond(g)

        length = x.size(-1)
        for i in range(self.num_upsamples):
            length *= self.upsample_rates[i]
            h = _match_length(self.ups[i](F.silu(h)), length)
            s = _match_length(self.source_convs[i](source), length)
            h = h + s

            acc = None
            for j in range(self.num_kernels):
                out = self.resblocks[i * self.num_kernels + j](h)
                acc = out if acc is None else acc + out
            h = acc / self.num_kernels

        h = self.conv_post(F.silu(h))
        return torch.tanh(h), source

    def remove_weight_norm(self) -> None:
        for module in [self.conv_pre, self.conv_post, *self.ups]:
            remove_parametrizations(module, "weight", leave_parametrized=True)
        for block in self.resblocks:
            block.remove_weight_norm()
