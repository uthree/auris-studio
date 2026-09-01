"""Shared utilities: audio front-end, masks and configuration helpers."""

from auris_singer.utils.audio import (
    frame_energy,
    mel_spectrogram,
    num_frames,
    spec_to_mel,
    spectrogram,
)
from auris_singer.utils.masks import (
    generate_path,
    rand_slice_segments,
    sequence_mask,
    slice_segments,
)

__all__ = [
    "frame_energy",
    "mel_spectrogram",
    "num_frames",
    "spec_to_mel",
    "spectrogram",
    "generate_path",
    "rand_slice_segments",
    "sequence_mask",
    "slice_segments",
]
