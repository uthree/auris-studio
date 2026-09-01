"""Whether the words survive: what a render did to its consonants, and to its phonemes.

Pitch and loudness reach the decoder through the excitation, and :mod:`auris_singer.metrics`
asks whether they arrived. The *words* reach it through the latent, and nothing asked. A
render can track its f0 to the cent and sing every syllable as the same vowel — the
regression `crates/auris-singer`'s real-voice test guards against by ear-shaped assertion,
and the one the consonant-width study found by measuring: at the wrong width the model does
not form a sibilant at all, and the spectral distance to the recording halves once it does.

Two instruments, in order of cost:

* **Class-resolved spectral distance** (:func:`class_spectral_metrics`). The mel distance
  the corpus run already computes, split by the manner class of the phoneme on each frame,
  which the alignment already knows. A vowel distance and a consonant distance side by side
  say *which* half of the inventory a change touched; the sibilant tilt — the energy above
  4 kHz against the energy below it, on sibilant frames, render against recording — says
  outright whether the /s/ was formed. Deterministic, dependency-free, and needs a
  recording to hold the render against.
* **Phoneme error rate** — what a listener would have heard, by way of a recogniser. The
  second instrument, and the next unit of work.
"""

from __future__ import annotations

import math

import numpy as np
import torch

from auris_singer.text.ipa import SIBILANTS, phoneme_class

__all__ = [
    "SIBILANT_SPLIT_HZ",
    "CLASS_METRICS",
    "frame_classes",
    "class_spectral_metrics",
]

#: Where the sibilant tilt splits the spectrum. Sibilant identity lives above it; the
#: consonant-width study measured the ratio at this split against the real recordings.
SIBILANT_SPLIT_HZ = 4000.0

#: The metrics :func:`class_spectral_metrics` answers, in table order.
CLASS_METRICS = ("mel_l1_vowel", "mel_l1_consonant", "mel_l1_sibilant", "sibilant_tilt_db")

#: The manner classes that count as consonants for the split.
_CONSONANTS = frozenset({"nasal", "plosive", "affricate", "fricative", "approximant"})


def frame_classes(tokens: list[str]) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """Per frame: is it a vowel, a consonant, a sibilant — from the phoneme on the frame.

    A frames file spells silence its own way (``sil``); it is neither, like every special.
    """
    classes = [phoneme_class(t) for t in tokens]
    vowel = np.asarray([c == "vowel" for c in classes], dtype=bool)
    consonant = np.asarray([c in _CONSONANTS for c in classes], dtype=bool)
    sibilant = np.asarray([t in SIBILANTS for t in tokens], dtype=bool)
    return vowel, consonant, sibilant


def _masked_mean(values: torch.Tensor, mask: np.ndarray) -> float:
    if not mask.any():
        return math.nan
    return float(values[torch.from_numpy(mask)].mean())


def _band_energy(spec: torch.Tensor, split_bin: int, mask: np.ndarray) -> tuple[float, float]:
    """Energy below and above ``split_bin`` over the frames ``mask`` selects, from a power
    spectrogram ``(F, T)``."""
    frames = spec[:, torch.from_numpy(mask)]
    return float(frames[:split_bin].sum()), float(frames[split_bin:].sum())


def class_spectral_metrics(
    mel_pred: torch.Tensor,
    mel_real: torch.Tensor,
    power_pred: torch.Tensor,
    power_real: torch.Tensor,
    tokens: list[str],
    sample_rate: int,
    n_fft: int,
    split_hz: float = SIBILANT_SPLIT_HZ,
) -> dict[str, float]:
    """The spectral distance by phoneme class, and the sibilant tilt.

    Args:
        mel_pred, mel_real: log-mel spectrograms ``(n_mels, T)`` of the render and the
            recording, on one frame grid.
        power_pred, power_real: power spectrograms ``(n_fft // 2 + 1, T)`` of the same.
        tokens: the phoneme on each of the ``T`` frames.
        sample_rate, n_fft: what the spectrograms were made with, to place ``split_hz``.

    Returns:
        ``mel_l1_vowel``, ``mel_l1_consonant``, ``mel_l1_sibilant`` — the mean absolute
        log-mel distance over the frames of each class, NaN where the utterance has none;
        ``sibilant_tilt_db`` — the render's high-to-low band energy ratio on sibilant frames
        minus the recording's, in dB. Zero is a sibilant formed as the singer formed it;
        well below zero is a hiss the model never made.
    """
    if not (mel_pred.shape == mel_real.shape and power_pred.shape == power_real.shape):
        raise ValueError("the render and the recording must be on one frame grid")
    n_frames = mel_pred.shape[-1]
    if len(tokens) != n_frames or power_pred.shape[-1] != n_frames:
        raise ValueError(f"{len(tokens)} tokens for {n_frames} frames")
    vowel, consonant, sibilant = frame_classes(tokens)
    per_frame = (mel_pred - mel_real).abs().mean(dim=0)
    out = {
        "mel_l1_vowel": _masked_mean(per_frame, vowel),
        "mel_l1_consonant": _masked_mean(per_frame, consonant),
        "mel_l1_sibilant": _masked_mean(per_frame, sibilant),
        "sibilant_tilt_db": math.nan,
    }
    if sibilant.any():
        split_bin = int(round(split_hz / (sample_rate / 2) * (n_fft // 2)))
        low_p, high_p = _band_energy(power_pred, split_bin, sibilant)
        low_r, high_r = _band_energy(power_real, split_bin, sibilant)
        floor = 1e-12
        tilt_pred = 10.0 * math.log10(max(high_p, floor) / max(low_p, floor))
        tilt_real = 10.0 * math.log10(max(high_r, floor) / max(low_r, floor))
        out["sibilant_tilt_db"] = tilt_pred - tilt_real
    return out
