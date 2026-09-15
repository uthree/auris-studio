"""Shared sample-rate and frame-clock contracts."""

from __future__ import annotations

__all__ = ["require_same_audio_clock"]


def require_same_audio_clock(
    left_name: str,
    left_sample_rate: int,
    left_hop_length: int,
    right_name: str,
    right_sample_rate: int,
    right_hop_length: int,
) -> None:
    """Reject measurements that would compare two different audio clocks."""
    if (left_sample_rate, left_hop_length) != (right_sample_rate, right_hop_length):
        raise ValueError(
            f"the {left_name} is at {left_sample_rate} Hz / hop {left_hop_length} "
            f"and the {right_name} at {right_sample_rate} Hz / hop {right_hop_length}; "
            "they do not share a clock"
        )
