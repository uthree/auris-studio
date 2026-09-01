"""Sequence mask helpers."""

from __future__ import annotations

import torch

__all__ = ["sequence_mask", "generate_path", "rand_slice_segments", "slice_segments"]


def sequence_mask(lengths: torch.Tensor, max_length: int | None = None) -> torch.Tensor:
    """Boolean mask ``(B, T)`` that is True for positions ``< lengths``."""
    if max_length is None:
        max_length = int(lengths.max().item())
    positions = torch.arange(max_length, device=lengths.device, dtype=lengths.dtype)
    return positions.unsqueeze(0) < lengths.unsqueeze(1)


def generate_path(duration: torch.Tensor, mask: torch.Tensor) -> torch.Tensor:
    """Convert per-token durations into a hard monotonic alignment path.

    Args:
        duration: ``(B, 1, S)`` integer durations per input token.
        mask: ``(B, 1, T, S)`` validity mask of the attention matrix.

    Returns:
        ``(B, 1, T, S)`` binary path where ``path[b, 0, t, s] == 1`` iff frame
        ``t`` is assigned to token ``s``.
    """
    b, _, t_y, t_x = mask.shape
    cum_duration = torch.cumsum(duration, dim=-1).view(b * t_x)
    path = sequence_mask(cum_duration, t_y).to(mask.dtype)
    path = path.view(b, t_x, t_y)
    path = path - torch.nn.functional.pad(path, [0, 0, 1, 0])[:, :-1]
    path = path.unsqueeze(1).transpose(2, 3)
    return path * mask


def rand_slice_segments(
    x: torch.Tensor, x_lengths: torch.Tensor, segment_size: int
) -> tuple[torch.Tensor, torch.Tensor]:
    """Randomly slice ``segment_size`` frames from ``x`` ``(B, C, T)``."""
    b = x.size(0)
    max_start = (x_lengths - segment_size).clamp(min=0)
    start = (torch.rand(b, device=x.device) * (max_start + 1)).long().clamp(max=max_start)
    return slice_segments(x, start, segment_size), start


def slice_segments(x: torch.Tensor, start: torch.Tensor, segment_size: int) -> torch.Tensor:
    """Gather ``segment_size`` frames starting at ``start`` from ``x`` ``(B, C, T)``."""
    idx = start.unsqueeze(1) + torch.arange(segment_size, device=x.device).unsqueeze(0)
    idx = idx.clamp(max=x.size(-1) - 1)
    idx = idx.unsqueeze(1).expand(-1, x.size(1), -1)
    return torch.gather(x, 2, idx)
