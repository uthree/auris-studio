"""Encoders of the VITS variational framework, rebuilt on Transformers.

* :class:`TextEncoder` — phoneme sequence -> hidden states + a phoneme-level
  prior used only by monotonic alignment search.
* :class:`PriorEncoder` — duration-expanded hidden states conditioned on the
  frame-level f0/energy curves -> the prior actually used for the KL term.
* :class:`PosteriorEncoder` — linear spectrogram -> posterior ``q(z|x)``.
"""

from __future__ import annotations

import math

import torch
import torch.nn as nn

from auris_singer.modules.transformer import TransformerEncoder
from auris_singer.utils.masks import sequence_mask

__all__ = ["TextEncoder", "PriorEncoder", "PosteriorEncoder", "PitchEnergyEmbedding"]


class TextEncoder(nn.Module):
    """Phoneme encoder.

    Returns both the hidden states (expanded by duration later) and a
    phoneme-level Gaussian ``(m, logs)``.  The latter is the statistic MAS
    scores frames against; it is also kept as an auxiliary KL target so the
    alignment objective and the training objective agree.
    """

    def __init__(
        self,
        n_vocab: int,
        out_channels: int,
        hidden_channels: int,
        n_layers: int = 6,
        n_heads: int = 2,
        ffn_dim: int | None = None,
        dropout: float = 0.1,
        cond_channels: int = 0,
    ):
        super().__init__()
        self.hidden_channels = hidden_channels
        self.out_channels = out_channels
        self.embedding = nn.Embedding(n_vocab, hidden_channels)
        nn.init.normal_(self.embedding.weight, 0.0, hidden_channels**-0.5)
        self.encoder = TransformerEncoder(
            hidden_channels,
            n_layers=n_layers,
            n_heads=n_heads,
            ffn_dim=ffn_dim,
            dropout=dropout,
            cond_channels=cond_channels,
        )
        self.proj = nn.Conv1d(hidden_channels, out_channels * 2, 1)

    def forward(
        self,
        phonemes: torch.Tensor,
        phoneme_lengths: torch.Tensor,
        g: torch.Tensor | None = None,
    ) -> tuple[torch.Tensor, torch.Tensor, torch.Tensor, torch.Tensor]:
        """
        Args:
            phonemes: ``(B, S)`` phoneme ids.
            phoneme_lengths: ``(B,)``.
            g: ``(B, cond_channels, 1)`` speaker condition.

        Returns:
            ``(x, m, logs, x_mask)`` with ``x`` ``(B, hidden, S)``,
            ``m``/``logs`` ``(B, out_channels, S)`` and ``x_mask`` ``(B, 1, S)``.
        """
        x_mask = sequence_mask(phoneme_lengths, phonemes.size(1)).unsqueeze(1).to(
            self.proj.weight.dtype
        )
        x = self.embedding(phonemes) * math.sqrt(self.hidden_channels)
        x = x.transpose(1, 2) * x_mask
        x = self.encoder(x, x_mask, g=g)
        stats = self.proj(x) * x_mask
        m, logs = stats.chunk(2, dim=1)
        return x, m, logs, x_mask


class PitchEnergyEmbedding(nn.Module):
    """Embed the frame-level f0 / voiced-flag / energy curves.

    f0 and energy enter in the log domain, which is both perceptually sensible
    and keeps the inputs in a well-scaled range for the Transformer.
    """

    def __init__(self, channels: int, f0_ref: float = 220.0, energy_ref: float = 0.1):
        super().__init__()
        self.f0_ref = f0_ref
        self.energy_ref = energy_ref
        self.proj = nn.Conv1d(3, channels, 1)

    def forward(
        self, f0: torch.Tensor, energy: torch.Tensor, voiced: torch.Tensor
    ) -> torch.Tensor:
        """
        Args:
            f0: ``(B, 1, T)`` in Hz (0 where unvoiced).
            energy: ``(B, 1, T)`` linear RMS.
            voiced: ``(B, 1, T)`` binary voiced flag.
        """
        log_f0 = torch.log(f0.clamp(min=1e-3) / self.f0_ref) * voiced
        log_energy = torch.log(energy.clamp(min=1e-5) / self.energy_ref)
        return self.proj(torch.cat([log_f0, log_energy, voiced], dim=1))


class PriorEncoder(nn.Module):
    """Frame-level prior ``p(z|text, f0, energy, speaker)``.

    The duration-expanded text hidden states are combined with the pitch and
    energy curves and refined by a Transformer before the Gaussian projection.
    """

    def __init__(
        self,
        in_channels: int,
        out_channels: int,
        hidden_channels: int,
        n_layers: int = 4,
        n_heads: int = 2,
        ffn_dim: int | None = None,
        dropout: float = 0.1,
        cond_channels: int = 0,
        f0_ref: float = 220.0,
        energy_ref: float = 0.1,
    ):
        super().__init__()
        self.pre = nn.Conv1d(in_channels, hidden_channels, 1)
        self.cond = PitchEnergyEmbedding(hidden_channels, f0_ref, energy_ref)
        self.encoder = TransformerEncoder(
            hidden_channels,
            n_layers=n_layers,
            n_heads=n_heads,
            ffn_dim=ffn_dim,
            dropout=dropout,
            cond_channels=cond_channels,
        )
        self.proj = nn.Conv1d(hidden_channels, out_channels * 2, 1)

    def forward(
        self,
        x: torch.Tensor,
        x_mask: torch.Tensor,
        f0: torch.Tensor,
        energy: torch.Tensor,
        voiced: torch.Tensor,
        g: torch.Tensor | None = None,
    ) -> tuple[torch.Tensor, torch.Tensor]:
        """
        Args:
            x: ``(B, in_channels, T)`` duration-expanded text states.
            x_mask: ``(B, 1, T)``.

        Returns:
            ``(m_p, logs_p)``, each ``(B, out_channels, T)``.
        """
        h = (self.pre(x) + self.cond(f0, energy, voiced)) * x_mask
        h = self.encoder(h, x_mask, g=g)
        stats = self.proj(h) * x_mask
        return stats.chunk(2, dim=1)


class PosteriorEncoder(nn.Module):
    """Approximate posterior ``q(z|spectrogram, speaker)``."""

    def __init__(
        self,
        in_channels: int,
        out_channels: int,
        hidden_channels: int,
        n_layers: int = 6,
        n_heads: int = 2,
        ffn_dim: int | None = None,
        dropout: float = 0.0,
        cond_channels: int = 0,
    ):
        super().__init__()
        self.pre = nn.Conv1d(in_channels, hidden_channels, 1)
        self.encoder = TransformerEncoder(
            hidden_channels,
            n_layers=n_layers,
            n_heads=n_heads,
            ffn_dim=ffn_dim,
            dropout=dropout,
            cond_channels=cond_channels,
        )
        self.proj = nn.Conv1d(hidden_channels, out_channels * 2, 1)
        self.out_channels = out_channels

    def forward(
        self,
        spec: torch.Tensor,
        spec_lengths: torch.Tensor,
        g: torch.Tensor | None = None,
    ) -> tuple[torch.Tensor, torch.Tensor, torch.Tensor, torch.Tensor]:
        """
        Returns:
            ``(z, m, logs, mask)`` where ``z`` is a reparameterized sample.
        """
        mask = sequence_mask(spec_lengths, spec.size(2)).unsqueeze(1).to(spec.dtype)
        h = self.pre(spec) * mask
        h = self.encoder(h, mask, g=g)
        stats = self.proj(h) * mask
        m, logs = stats.chunk(2, dim=1)
        z = (m + torch.randn_like(m) * torch.exp(logs)) * mask
        return z, m, logs, mask
