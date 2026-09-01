"""Inference API.

The model is not given a score: it is given a phoneme sequence, an integer
duration per phoneme, and frame-level f0 and energy curves.  Turning a score
into those curves is the DAW front-end's job.
"""

from __future__ import annotations

from pathlib import Path

import numpy as np
import torch

from auris_singer.lightning_module import AurisSingerModule
from auris_singer.text import DEFAULT_PHONEME_TABLE, PhonemeTable, is_voiceless

__all__ = ["Synthesizer", "frame_voicing"]


def frame_voicing(
    phonemes: list[str], durations: list[int] | np.ndarray, f0: list[float] | np.ndarray
) -> np.ndarray:
    """Per-frame voiced flags from the phoneme class and the f0 contour.

    A frame is voiced iff its phoneme is not voiceless and its f0 is nonzero.
    The phoneme class is the primary signal — a score front-end writes pitch
    as a contour across consonants, so ``f0 > 0`` alone would voice every
    /k/ and /s/; the f0 term only clears frames with no pitch at all.
    """
    per_phoneme = np.asarray([0.0 if is_voiceless(p) else 1.0 for p in phonemes])
    expanded = np.repeat(per_phoneme, np.asarray(durations, dtype=np.int64))
    return (expanded * (np.asarray(f0, dtype=np.float32) > 0.0)).astype(np.float32)


class Synthesizer:
    """Wraps a trained checkpoint for waveform synthesis.

    Args:
        module: a trained :class:`AurisSingerModule`.
        device: device to run on.
        phoneme_table: overrides the table stored in the checkpoint.
        speaker_to_id: overrides the speaker map stored in the checkpoint.
    """

    def __init__(
        self,
        module: AurisSingerModule,
        device: str | torch.device = "cpu",
        phoneme_table: PhonemeTable | None = None,
        speaker_to_id: dict[str, int] | None = None,
    ):
        self.device = torch.device(device)
        self.module = module.to(self.device).eval()
        self.model = self.module.model
        self.sample_rate = self.module.sample_rate
        self.hop_length = self.module.hop_length

        metadata = dict(module.hparams.get("metadata") or {})
        if phoneme_table is None:
            symbols = metadata.get("symbols")
            phoneme_table = PhonemeTable(symbols) if symbols else DEFAULT_PHONEME_TABLE
        self.phoneme_table = phoneme_table
        self.speaker_to_id: dict[str, int] = speaker_to_id or metadata.get(
            "speaker_to_id", {}
        )

    @classmethod
    def from_checkpoint(
        cls, path: str | Path, device: str | torch.device = "cpu", **kwargs
    ) -> Synthesizer:
        """Load a Lightning checkpoint written by ``scripts/train.py``."""
        module = AurisSingerModule.load_from_checkpoint(str(path), map_location="cpu")
        return cls(module, device=device, **kwargs)

    def resolve_speaker(self, speaker: str | int | None) -> int:
        """Map a speaker name (or index) to a speaker id."""
        if speaker is None:
            return 0
        if isinstance(speaker, int):
            return speaker
        if speaker not in self.speaker_to_id:
            raise KeyError(
                f"unknown speaker {speaker!r}; known speakers: "
                f"{sorted(self.speaker_to_id)}"
            )
        return self.speaker_to_id[speaker]

    @torch.inference_mode()
    def synthesize(
        self,
        phonemes: list[str],
        durations: list[int] | np.ndarray,
        f0: list[float] | np.ndarray,
        energy: list[float] | np.ndarray,
        speaker: str | int | None = None,
        voiced: list[float] | np.ndarray | None = None,
        noise_scale: float = 0.667,
    ) -> np.ndarray:
        """Synthesize one utterance.

        Args:
            phonemes: IPA symbols.
            durations: frames per phoneme; must be the same length as
                ``phonemes`` and sum to ``len(f0)``.
            f0: per-frame f0 in Hz; 0 marks an unvoiced frame.
            energy: per-frame linear RMS energy.
            speaker: speaker name or id.
            voiced: optional explicit voiced flags. When omitted, a frame is
                voiced iff its phoneme is not voiceless **and** its ``f0`` is
                nonzero — never from ``f0`` alone, because a score front-end
                writes pitch as a contour across consonants, and voicing those
                frames would swallow every /k/ and /s/.
            noise_scale: prior sampling temperature.

        Returns:
            A 1D float32 waveform at ``self.sample_rate``.
        """
        if len(durations) != len(phonemes):
            raise ValueError(
                f"durations has {len(durations)} entries but there are "
                f"{len(phonemes)} phonemes"
            )
        durations_t = torch.as_tensor(np.asarray(durations), dtype=torch.long)
        total_frames = int(durations_t.sum().item())
        f0_t = torch.as_tensor(np.asarray(f0), dtype=torch.float32)
        energy_t = torch.as_tensor(np.asarray(energy), dtype=torch.float32)
        if f0_t.numel() != total_frames or energy_t.numel() != total_frames:
            raise ValueError(
                f"f0 ({f0_t.numel()}) and energy ({energy_t.numel()}) must both "
                f"have sum(durations) = {total_frames} frames"
            )

        unknown = self.phoneme_table.unknown_symbols(phonemes)
        if unknown:
            raise ValueError(f"phonemes not in the table: {unknown}")

        ids = torch.tensor(
            self.phoneme_table.encode(phonemes), dtype=torch.long, device=self.device
        ).unsqueeze(0)
        lengths = torch.tensor([len(phonemes)], dtype=torch.long, device=self.device)
        durations_t = durations_t.unsqueeze(0).to(self.device)
        f0_t = f0_t.unsqueeze(0).to(self.device)
        energy_t = energy_t.unsqueeze(0).to(self.device)
        if voiced is None:
            voiced = frame_voicing(phonemes, durations, f0)
        voiced_t = (
            torch.as_tensor(np.asarray(voiced), dtype=torch.float32)
            .unsqueeze(0)
            .to(self.device)
        )
        speaker_t = torch.tensor(
            [self.resolve_speaker(speaker)], dtype=torch.long, device=self.device
        )

        wav = self.model.infer(
            phonemes=ids,
            phoneme_lengths=lengths,
            durations=durations_t,
            f0=f0_t,
            energy=energy_t,
            voiced=voiced_t,
            speaker_ids=speaker_t,
            noise_scale=noise_scale,
        )
        return wav.squeeze().float().cpu().numpy()
