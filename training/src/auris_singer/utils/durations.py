"""Validation for inference-time phoneme durations."""

from __future__ import annotations

import numpy as np

__all__ = [
    "MAX_INFERENCE_BATCH_FRAMES",
    "MAX_INFERENCE_BATCH_SIZE",
    "MAX_INFERENCE_FRAMES",
    "MAX_INFERENCE_PHONEMES",
    "validate_inference_extent",
    "validated_duration_array",
]

# The Rust host chunks longer scores at this boundary. Keeping every Python
# expansion behind the same limit prevents hostile duration values from
# sizing lists, attention paths, or decoder intermediates without bound.
MAX_INFERENCE_FRAMES = 2_000
# Self-attention is quadratic in this axis, so a frame bound alone is not a
# memory bound when zero-duration phonemes are allowed.
MAX_INFERENCE_PHONEMES = 512
MAX_INFERENCE_BATCH_SIZE = 32
MAX_INFERENCE_BATCH_FRAMES = 16_000


def validate_inference_extent(phoneme_count: int, frame_count: int | None = None) -> None:
    """Reject sequence extents before attention or frame expansion begins."""
    if phoneme_count > MAX_INFERENCE_PHONEMES:
        raise ValueError(f"an utterance may contain at most {MAX_INFERENCE_PHONEMES} phonemes")
    if frame_count is not None and frame_count > MAX_INFERENCE_FRAMES:
        raise ValueError(f"an utterance may contain at most {MAX_INFERENCE_FRAMES} frames")


def validated_duration_array(durations: list[int] | np.ndarray, expected_count: int) -> np.ndarray:
    """Return int64 durations after checking them without expanding frames."""
    validate_inference_extent(expected_count)
    try:
        supplied_count = len(durations)
    except TypeError:
        supplied_count = None
    if supplied_count is not None and supplied_count != expected_count:
        raise ValueError(
            f"durations has {supplied_count} entries but there are {expected_count} phonemes"
        )
    values = np.asarray(durations)
    if values.ndim != 1 or values.size != expected_count:
        raise ValueError(
            f"durations has {values.size} entries but there are {expected_count} phonemes"
        )
    if np.issubdtype(values.dtype, np.bool_) or np.issubdtype(values.dtype, np.complexfloating):
        raise ValueError("durations must contain whole frame counts")
    try:
        numeric = values.astype(np.float64)
    except (TypeError, ValueError, OverflowError) as error:
        raise ValueError("durations must contain finite whole frame counts") from error
    if not np.isfinite(numeric).all():
        raise ValueError("durations must be finite")
    if not np.equal(numeric, np.trunc(numeric)).all():
        raise ValueError("durations must contain whole frame counts")
    if (numeric < 0).any():
        raise ValueError("durations must be non-negative")
    if (numeric > MAX_INFERENCE_FRAMES).any():
        raise ValueError(f"an utterance may contain at most {MAX_INFERENCE_FRAMES} frames")
    integer = numeric.astype(np.int64)
    total = sum(int(value) for value in integer)
    if total <= 0:
        raise ValueError("durations must require at least one frame")
    if total > MAX_INFERENCE_FRAMES:
        raise ValueError(f"an utterance may contain at most {MAX_INFERENCE_FRAMES} frames")
    validate_inference_extent(expected_count, total)
    return integer
