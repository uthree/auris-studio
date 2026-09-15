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
from auris_singer.utils.durations import (
    MAX_INFERENCE_BATCH_FRAMES,
    MAX_INFERENCE_BATCH_SIZE,
    MAX_INFERENCE_FRAMES,
    MAX_INFERENCE_PHONEMES,
)
from auris_singer.utils.masks import (
    generate_path,
    rand_slice_segments,
    sequence_mask,
    slice_segments,
)

__all__ = ["AurisSinger", "MAX_INFERENCE_FRAMES"]


def _validated_inference_durations(
    durations: torch.Tensor,
    phonemes: torch.Tensor,
    phoneme_lengths: torch.Tensor,
) -> tuple[torch.Tensor, torch.Tensor]:
    """Validate duration values before any frame-sized model allocation."""
    if durations.ndim != 2 or durations.shape != phonemes.shape:
        raise ValueError(
            f"durations shape {tuple(durations.shape)} must match phonemes "
            f"shape {tuple(phonemes.shape)}"
        )
    if phonemes.size(0) > MAX_INFERENCE_BATCH_SIZE:
        raise ValueError(
            f"inference accepts at most {MAX_INFERENCE_BATCH_SIZE} utterances per batch"
        )
    if phonemes.size(0) == 0:
        raise ValueError("inference requires at least one utterance")
    if phonemes.size(1) > MAX_INFERENCE_PHONEMES:
        raise ValueError(f"an utterance may contain at most {MAX_INFERENCE_PHONEMES} phonemes")
    if phoneme_lengths.ndim != 1 or phoneme_lengths.numel() != phonemes.size(0):
        raise ValueError("phoneme_lengths must contain one value per batch row")
    if (
        phoneme_lengths.dtype == torch.bool
        or torch.is_floating_point(phoneme_lengths)
        or phoneme_lengths.is_complex()
    ):
        raise ValueError("phoneme_lengths must contain whole phoneme counts")
    if durations.dtype == torch.bool or durations.is_complex():
        raise ValueError("durations must contain whole frame counts")
    if torch.is_floating_point(durations):
        if not bool(torch.isfinite(durations).all().item()):
            raise ValueError("durations must be finite")
        if not bool((durations == durations.trunc()).all().item()):
            raise ValueError("durations must contain whole frame counts")
    if bool((durations < 0).any().item()):
        raise ValueError("durations must be non-negative")
    if bool((durations > MAX_INFERENCE_FRAMES).any().item()):
        raise ValueError(f"each utterance may contain at most {MAX_INFERENCE_FRAMES} frames")

    device = phonemes.device
    lengths = phoneme_lengths.to(device=device, dtype=torch.long)
    if bool(((lengths <= 0) | (lengths > phonemes.size(1))).any().item()):
        raise ValueError("phoneme_lengths must be between 1 and the phoneme width")
    if bool((lengths > MAX_INFERENCE_PHONEMES).any().item()):
        raise ValueError(f"an utterance may contain at most {MAX_INFERENCE_PHONEMES} phonemes")
    values = durations.to(device=device, dtype=torch.long)
    positions = torch.arange(phonemes.size(1), device=device).unsqueeze(0)
    values = values * (positions < lengths.unsqueeze(1))
    totals = values.sum(dim=1)
    if bool((totals <= 0).any().item()):
        raise ValueError("durations must require at least one frame per utterance")
    if bool((totals > MAX_INFERENCE_FRAMES).any().item()):
        raise ValueError(f"each utterance may contain at most {MAX_INFERENCE_FRAMES} frames")
    if int(totals.sum().item()) > MAX_INFERENCE_BATCH_FRAMES:
        raise ValueError(
            f"an inference batch may contain at most {MAX_INFERENCE_BATCH_FRAMES} frames"
        )
    return values, totals


def _validate_inference_controls(
    *,
    phonemes: torch.Tensor,
    y_lengths: torch.Tensor,
    f0: torch.Tensor,
    energy: torch.Tensor,
    voiced: torch.Tensor | None,
    speaker_ids: torch.Tensor,
    n_vocab: int,
    n_speakers: int,
    noise_scale: float,
) -> int:
    """Validate public inference tensors before an embedding or encoder runs."""
    batch = phonemes.size(0)
    if phonemes.dtype == torch.bool or torch.is_floating_point(phonemes) or phonemes.is_complex():
        raise ValueError("phonemes must contain integer ids")
    if bool(((phonemes < 0) | (phonemes >= n_vocab)).any().item()):
        raise ValueError(f"phoneme ids must be between 0 and {n_vocab - 1}")

    if speaker_ids.ndim != 1 or speaker_ids.numel() != batch:
        raise ValueError("speaker_ids must contain one id per batch row")
    if (
        speaker_ids.dtype == torch.bool
        or torch.is_floating_point(speaker_ids)
        or speaker_ids.is_complex()
    ):
        raise ValueError("speaker_ids must contain integer ids")
    if bool(((speaker_ids < 0) | (speaker_ids >= n_speakers)).any().item()):
        raise ValueError(f"speaker ids must be between 0 and {n_speakers - 1}")

    y_max = int(y_lengths.max().item())
    curves = {"f0": f0, "energy": energy}
    if voiced is not None:
        curves["voiced"] = voiced
    for name, curve in curves.items():
        if curve.ndim != 2 or tuple(curve.shape) != (batch, y_max):
            raise ValueError(f"{name} shape {tuple(curve.shape)} must be ({batch}, {y_max})")
        if not torch.is_floating_point(curve) or curve.is_complex():
            raise ValueError(f"{name} must contain floating-point values")
        if not bool(torch.isfinite(curve).all().item()):
            raise ValueError(f"{name} must contain finite values")
        if bool((curve < 0).any().item()):
            raise ValueError(f"{name} must contain non-negative values")
    if voiced is not None and bool((voiced > 1).any().item()):
        raise ValueError("voiced values must be between 0 and 1")
    if not isinstance(noise_scale, (int, float)) or isinstance(noise_scale, bool):
        raise ValueError("noise_scale must be a finite non-negative number")
    if not math.isfinite(noise_scale) or noise_scale < 0:
        raise ValueError("noise_scale must be a finite non-negative number")
    return y_max


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
        self.spec_channels = spec_channels
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

        m_p, logs_p = self.prior_encoder(x_frame, y_mask, f0=f0, energy=energy, voiced=voiced, g=g)

        z_slice, slice_ids = rand_slice_segments(z, spec_lengths, self.segment_size)
        f0_slice = slice_segments(f0, slice_ids, self.segment_size)
        energy_slice = slice_segments(energy, slice_ids, self.segment_size)
        voiced_slice = slice_segments(voiced, slice_ids, self.segment_size)

        wav_hat, source = self.generator(z_slice, f0_slice, energy_slice, voiced_slice, g=g)

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
            durations: ``(B, S)`` non-negative integer frame counts per
                phoneme; each utterance is limited to
                :data:`MAX_INFERENCE_FRAMES` frames.
            f0, energy: ``(B, T)`` with ``T == durations.sum(1).max()``.
            voiced: ``(B, T)``; derived from ``f0`` when omitted.
            speaker_ids: ``(B,)``; defaults to speaker 0.
            noise_scale: standard deviation multiplier for the prior sample.

        Returns:
            ``(B, 1, T * hop_length)`` waveform.
        """
        device = phonemes.device
        batch = phonemes.size(0)
        durations, y_lengths = _validated_inference_durations(durations, phonemes, phoneme_lengths)
        if speaker_ids is None:
            speaker_ids = torch.zeros(batch, dtype=torch.long, device=device)
        y_max = _validate_inference_controls(
            phonemes=phonemes,
            y_lengths=y_lengths,
            f0=f0,
            energy=energy,
            voiced=voiced,
            speaker_ids=speaker_ids,
            n_vocab=self.n_vocab,
            n_speakers=self.n_speakers,
            noise_scale=noise_scale,
        )
        g = self.speaker_embedding(speaker_ids).unsqueeze(-1)

        x, _, _, x_mask = self.text_encoder(phonemes, phoneme_lengths, g=g)

        durations = durations * x_mask.squeeze(1).long()
        y_mask = sequence_mask(y_lengths, y_max).unsqueeze(1).to(x.dtype)

        attn = self._path_from_durations(durations, x_mask, y_mask)
        x_frame = torch.matmul(x, attn)

        f0 = f0[..., :y_max].unsqueeze(1)
        energy = energy[..., :y_max].unsqueeze(1)
        if voiced is None:
            voiced = (f0 >= self.generator.source_generator.f0_min).to(f0.dtype)
        else:
            voiced = voiced[..., :y_max].unsqueeze(1)

        m_p, logs_p = self.prior_encoder(x_frame, y_mask, f0=f0, energy=energy, voiced=voiced, g=g)
        z_p = m_p + torch.randn_like(m_p) * torch.exp(logs_p) * noise_scale
        z = self.flow(z_p, y_mask, g=g, reverse=True)

        wav, _ = self.generator(z * y_mask, f0, energy, voiced, g=g)
        return wav

    def remove_weight_norm(self) -> None:
        """Fold weight normalization into the weights (for export/inference)."""
        self.generator.remove_weight_norm()
