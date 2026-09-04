"""The full generator-side model.

``AurisSinger`` is VITS with the modifications described in ``doc/architecture.md``:
Transformer sequence modules, an NSF-HiFi-GAN decoder driven by an explicit
source signal, and no duration predictor — durations are an input.

Training still has to align phonemes to frames, which monotonic alignment
search does exactly as in VITS.  Because MAS needs a *phoneme-level* Gaussian
while the pitch/energy curves are *frame-level*, the prior is factored in two:

1. ``TextEncoder`` emits a phoneme-level Gaussian used for the alignment search
   (and kept as an auxiliary KL target so search and training agree);
2. ``PriorEncoder`` refines the duration-expanded states with the f0/energy
   curves and emits the prior used for the main KL term.
"""

from __future__ import annotations

import math
from typing import Any

import torch
import torch.nn as nn

from auris_singer.modules.alignment import maximum_path
from auris_singer.modules.encoders import PosteriorEncoder, PriorEncoder, TextEncoder
from auris_singer.modules.flow import ResidualCouplingBlock
from auris_singer.modules.generator import NsfHifiGanGenerator
from auris_singer.utils.masks import (
    generate_path,
    rand_slice_segments,
    sequence_mask,
    slice_segments,
)

__all__ = ["AurisSinger"]


class AurisSinger(nn.Module):
    """VITS-based singing voice synthesizer.

    Args:
        n_vocab: size of the IPA phoneme table.
        spec_channels: number of linear spectrogram bins (``n_fft // 2 + 1``).
        inter_channels: width of the latent ``z``.
        hidden_channels: width of the Transformer encoders.
        n_speakers: number of speakers (always >= 1; the model is multi-speaker).
        gin_channels: speaker embedding width.
        segment_size: number of frames decoded per training step.
        text_encoder / posterior_encoder / flow / prior_encoder / generator:
            per-module keyword overrides.
    """

    def __init__(
        self,
        n_vocab: int,
        spec_channels: int = 1025,
        inter_channels: int = 192,
        hidden_channels: int = 192,
        n_speakers: int = 1,
        gin_channels: int = 256,
        segment_size: int = 32,
        sample_rate: int = 48_000,
        hop_length: int = 480,
        text_encoder: dict[str, Any] | None = None,
        posterior_encoder: dict[str, Any] | None = None,
        flow: dict[str, Any] | None = None,
        prior_encoder: dict[str, Any] | None = None,
        generator: dict[str, Any] | None = None,
    ):
        super().__init__()
        self.n_vocab = n_vocab
        self.inter_channels = inter_channels
        self.segment_size = segment_size
        self.sample_rate = sample_rate
        self.hop_length = hop_length
        self.n_speakers = n_speakers

        self.speaker_embedding = nn.Embedding(n_speakers, gin_channels)

        self.text_encoder = TextEncoder(
            n_vocab,
            out_channels=inter_channels,
            hidden_channels=hidden_channels,
            cond_channels=gin_channels,
            **(text_encoder or {}),
        )
        self.posterior_encoder = PosteriorEncoder(
            spec_channels,
            out_channels=inter_channels,
            hidden_channels=hidden_channels,
            cond_channels=gin_channels,
            **(posterior_encoder or {}),
        )
        self.flow = ResidualCouplingBlock(
            inter_channels,
            hidden_channels=hidden_channels,
            cond_channels=gin_channels,
            **(flow or {}),
        )
        self.prior_encoder = PriorEncoder(
            in_channels=hidden_channels,
            out_channels=inter_channels,
            hidden_channels=hidden_channels,
            cond_channels=gin_channels,
            **(prior_encoder or {}),
        )
        self.generator = NsfHifiGanGenerator(
            in_channels=inter_channels,
            sample_rate=sample_rate,
            hop_length=hop_length,
            cond_channels=gin_channels,
            **(generator or {}),
        )

    # ------------------------------------------------------------------
    # alignment
    # ------------------------------------------------------------------
    @staticmethod
    @torch.no_grad()
    def _search_alignment(
        z_p: torch.Tensor,
        m_p: torch.Tensor,
        logs_p: torch.Tensor,
        x_mask: torch.Tensor,
        y_mask: torch.Tensor,
    ) -> torch.Tensor:
        """Monotonic alignment search.

        Args:
            z_p: ``(B, C, T)`` flow output.
            m_p, logs_p: ``(B, C, S)`` phoneme-level prior.
            x_mask: ``(B, 1, S)``; y_mask: ``(B, 1, T)``.

        Returns:
            ``(B, S, T)`` hard alignment.
        """
        s_p_sq_r = torch.exp(-2.0 * logs_p)  # (B, C, S)
        # log N(z_p; m_p, s_p) decomposed so it can be computed with matmuls.
        term1 = torch.sum(-0.5 * math.log(2.0 * math.pi) - logs_p, dim=1, keepdim=True)
        term2 = torch.matmul(-0.5 * (z_p**2).transpose(1, 2), s_p_sq_r)
        term3 = torch.matmul(z_p.transpose(1, 2), m_p * s_p_sq_r)
        term4 = torch.sum(-0.5 * (m_p**2) * s_p_sq_r, dim=1, keepdim=True)
        neg_cent = term1 + term2 + term3 + term4  # (B, T, S)

        attn_mask = x_mask.transpose(1, 2) * y_mask  # (B, S, T)
        return maximum_path(neg_cent.transpose(1, 2).contiguous(), attn_mask)

    @staticmethod
    def _path_from_durations(
        durations: torch.Tensor, x_mask: torch.Tensor, y_mask: torch.Tensor
    ) -> torch.Tensor:
        """Expand integer durations ``(B, S)`` into a ``(B, S, T)`` alignment."""
        attn_mask = y_mask.unsqueeze(-1) * x_mask.unsqueeze(2)  # (B, 1, T, S)
        path = generate_path(durations.unsqueeze(1), attn_mask)  # (B, 1, T, S)
        return path.squeeze(1).transpose(1, 2)

    # ------------------------------------------------------------------
    # training
    # ------------------------------------------------------------------
    def forward(
        self,
        phonemes: torch.Tensor,
        phoneme_lengths: torch.Tensor,
        spec: torch.Tensor,
        spec_lengths: torch.Tensor,
        f0: torch.Tensor,
        energy: torch.Tensor,
        voiced: torch.Tensor,
        speaker_ids: torch.Tensor,
        durations: torch.Tensor | None = None,
    ) -> dict[str, torch.Tensor]:
        """Training forward pass.

        Args:
            phonemes: ``(B, S)`` phoneme ids.
            phoneme_lengths: ``(B,)``.
            spec: ``(B, spec_channels, T)`` linear spectrogram.
            spec_lengths: ``(B,)``.
            f0, energy, voiced: ``(B, T)`` frame-level curves.
            speaker_ids: ``(B,)``.
            durations: optional ``(B, S)`` integer durations; MAS is used when
                they are not provided.

        Returns:
            A dict with the decoded segment, the slice index and everything the
            losses need.
        """
        g = self.speaker_embedding(speaker_ids).unsqueeze(-1)  # (B, gin, 1)

        x, m_p0, logs_p0, x_mask = self.text_encoder(phonemes, phoneme_lengths, g=g)
        z, m_q, logs_q, y_mask = self.posterior_encoder(spec, spec_lengths, g=g)
        z_p = self.flow(z, y_mask, g=g)

        if durations is None:
            attn = self._search_alignment(z_p, m_p0, logs_p0, x_mask, y_mask)
        else:
            attn = self._path_from_durations(durations, x_mask, y_mask)
        w = attn.sum(dim=2)  # (B, S) frames per phoneme

        # Duration-expanded phoneme-level quantities.
        x_frame = torch.matmul(x, attn)  # (B, hidden, T)
        m_p0_frame = torch.matmul(m_p0, attn)
        logs_p0_frame = torch.matmul(logs_p0, attn)

        f0 = f0.unsqueeze(1)
        energy = energy.unsqueeze(1)
        voiced = voiced.unsqueeze(1)

        m_p, logs_p = self.prior_encoder(
            x_frame, y_mask, f0=f0, energy=energy, voiced=voiced, g=g
        )

        z_slice, slice_ids = rand_slice_segments(z, spec_lengths, self.segment_size)
        f0_slice = slice_segments(f0, slice_ids, self.segment_size)
        energy_slice = slice_segments(energy, slice_ids, self.segment_size)
        voiced_slice = slice_segments(voiced, slice_ids, self.segment_size)

        wav_hat, source = self.generator(
            z_slice, f0_slice, energy_slice, voiced_slice, g=g
        )

        return {
            "wav_hat": wav_hat,
            "source": source,
            "slice_ids": slice_ids,
            # The sliced decoder inputs are returned so callers can re-run the
            # decoder on a modified latent (see the latent-usage diagnostic in
            # the Lightning module) without repeating the encoder pass.
            "z_slice": z_slice,
            "f0_slice": f0_slice,
            "energy_slice": energy_slice,
            "voiced_slice": voiced_slice,
            "g": g,
            "z": z,
            "z_p": z_p,
            "m_q": m_q,
            "logs_q": logs_q,
            "m_p": m_p,
            "logs_p": logs_p,
            "m_p0_frame": m_p0_frame,
            "logs_p0_frame": logs_p0_frame,
            "attn": attn,
            "durations": w,
            "y_mask": y_mask,
            "x_mask": x_mask,
        }

    # ------------------------------------------------------------------
    # inference
    # ------------------------------------------------------------------
    @torch.inference_mode()
    def infer(
        self,
        phonemes: torch.Tensor,
        phoneme_lengths: torch.Tensor,
        durations: torch.Tensor,
        f0: torch.Tensor,
        energy: torch.Tensor,
        voiced: torch.Tensor | None = None,
        speaker_ids: torch.Tensor | None = None,
        noise_scale: float = 0.667,
    ) -> torch.Tensor:
        """Synthesize a waveform from explicit control curves.

        Args:
            phonemes: ``(B, S)`` phoneme ids.
            phoneme_lengths: ``(B,)``.
            durations: ``(B, S)`` integer frame counts per phoneme.
            f0, energy: ``(B, T)`` with ``T == durations.sum(1).max()``.
            voiced: ``(B, T)``; derived from ``f0`` when omitted.
            speaker_ids: ``(B,)``; defaults to speaker 0.
            noise_scale: standard deviation multiplier for the prior sample.

        Returns:
            ``(B, 1, T * hop_length)`` waveform.
        """
        device = phonemes.device
        batch = phonemes.size(0)
        if speaker_ids is None:
            speaker_ids = torch.zeros(batch, dtype=torch.long, device=device)
        g = self.speaker_embedding(speaker_ids).unsqueeze(-1)

        x, _, _, x_mask = self.text_encoder(phonemes, phoneme_lengths, g=g)

        durations = durations.to(torch.long) * x_mask.squeeze(1).long()
        y_lengths = durations.sum(dim=1).clamp(min=1)
        y_max = int(y_lengths.max().item())
        curves = {"f0": f0, "energy": energy}
        if voiced is not None:
            curves["voiced"] = voiced
        for name, curve in curves.items():
            if curve.size(-1) != y_max:
                raise ValueError(
                    f"{name} has {curve.size(-1)} frames but durations require {y_max}"
                )
        y_mask = sequence_mask(y_lengths, y_max).unsqueeze(1).to(x.dtype)

        attn = self._path_from_durations(durations, x_mask, y_mask)
        x_frame = torch.matmul(x, attn)

        f0 = f0[..., :y_max].unsqueeze(1)
        energy = energy[..., :y_max].unsqueeze(1)
        if voiced is None:
            voiced = (f0 >= self.generator.source_generator.f0_min).to(f0.dtype)
        else:
            voiced = voiced[..., :y_max].unsqueeze(1)

        m_p, logs_p = self.prior_encoder(
            x_frame, y_mask, f0=f0, energy=energy, voiced=voiced, g=g
        )
        z_p = m_p + torch.randn_like(m_p) * torch.exp(logs_p) * noise_scale
        z = self.flow(z_p, y_mask, g=g, reverse=True)

        wav, _ = self.generator(z * y_mask, f0, energy, voiced, g=g)
        return wav

    def remove_weight_norm(self) -> None:
        """Fold weight normalization into the weights (for export/inference)."""
        self.generator.remove_weight_norm()
