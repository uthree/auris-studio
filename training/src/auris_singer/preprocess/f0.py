"""f0 extraction with FCPE.

FCPE (Fast Context-based Pitch Estimation) is run once during preprocessing;
training and inference only ever see the resulting curve.
"""

from __future__ import annotations

import torch

__all__ = ["FcpeExtractor"]


class FcpeExtractor:
    """Wrapper around ``torchfcpe``'s bundled inference model.

    Args:
        device: torch device string.
        f0_min / f0_max: search range in Hz. The default upper bound is high
            enough for soprano singing.
        threshold: FCPE voicing threshold; frames below it are marked unvoiced.
        decoder_mode: FCPE decoder (``"local_argmax"`` is the recommended one).
    """

    def __init__(
        self,
        device: str = "cpu",
        f0_min: float = 40.0,
        f0_max: float = 1600.0,
        threshold: float = 0.006,
        decoder_mode: str = "local_argmax",
    ):
        self.device = torch.device(device)
        self.f0_min = f0_min
        self.f0_max = f0_max
        self.threshold = threshold
        self.decoder_mode = decoder_mode
        self._model = None

    @property
    def model(self):
        if self._model is None:
            from torchfcpe import spawn_bundled_infer_model

            self._model = spawn_bundled_infer_model(device=str(self.device))
        return self._model

    @torch.no_grad()
    def __call__(
        self, wav: torch.Tensor, sample_rate: int, n_frames: int
    ) -> tuple[torch.Tensor, torch.Tensor]:
        """Extract f0 on a fixed frame grid.

        Args:
            wav: ``(L,)`` or ``(B, L)`` waveform.
            sample_rate: sample rate of ``wav``.
            n_frames: number of frames to interpolate the f0 curve onto; use
                ``len(wav) // hop_length`` to stay on the spectrogram grid.

        Returns:
            ``(f0, voiced)``, each ``(n_frames,)`` (or ``(B, n_frames)``).
            ``f0`` is 0 on unvoiced frames.
        """
        squeeze = wav.dim() == 1
        if squeeze:
            wav = wav.unsqueeze(0)
        x = wav.to(self.device).float().unsqueeze(-1)  # (B, L, 1)

        f0, uv = self.model.infer(
            x,
            sr=sample_rate,
            decoder_mode=self.decoder_mode,
            threshold=self.threshold,
            f0_min=self.f0_min,
            f0_max=self.f0_max,
            interp_uv=False,
            output_interp_target_length=n_frames,
            retur_uv=True,
        )
        f0 = f0.squeeze(-1).cpu()
        # torchfcpe's second output flags *unvoiced* frames (1 = unvoiced).
        uv = uv.squeeze(-1).cpu().float()
        voiced = ((uv < 0.5) & (f0 >= self.f0_min)).float()
        # FCPE still emits a (meaningless) pitch on unvoiced frames; zero it so
        # downstream code can rely on "f0 == 0 means unvoiced".
        f0 = f0 * voiced

        if squeeze:
            return f0.squeeze(0), voiced.squeeze(0)
        return f0, voiced
