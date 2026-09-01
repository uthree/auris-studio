"""Tests for monotonic alignment search and duration expansion."""

from __future__ import annotations

import torch

from auris_singer.modules.alignment import maximum_path
from auris_singer.utils.masks import generate_path, sequence_mask


def _mask(s_lengths: torch.Tensor, t_lengths: torch.Tensor, s: int, t: int):
    x_mask = sequence_mask(s_lengths, s).float()  # (B, S)
    y_mask = sequence_mask(t_lengths, t).float()  # (B, T)
    return x_mask.unsqueeze(2) * y_mask.unsqueeze(1)  # (B, S, T)


def test_path_is_monotonic_and_surjective():
    torch.manual_seed(0)
    b, s, t = 3, 7, 25
    s_lengths = torch.tensor([7, 5, 4])
    t_lengths = torch.tensor([25, 18, 9])
    mask = _mask(s_lengths, t_lengths, s, t)
    scores = torch.randn(b, s, t)

    path = maximum_path(scores, mask)

    for i in range(b):
        valid = path[i, : s_lengths[i], : t_lengths[i]]
        # Exactly one phoneme per frame.
        assert torch.all(valid.sum(0) == 1)
        # Every phoneme gets at least one frame.
        assert torch.all(valid.sum(1) >= 1)
        # Frame assignments are non-decreasing in phoneme index.
        assignment = valid.argmax(0)
        assert torch.all(assignment[1:] - assignment[:-1] >= 0)
        assert assignment[0] == 0 and assignment[-1] == s_lengths[i] - 1
        # Nothing outside the valid region.
        assert path[i].sum() == t_lengths[i]


def test_path_prefers_high_scoring_cells():
    """A strongly preferred diagonal should be recovered exactly."""
    s, t = 4, 8
    mask = torch.ones(1, s, t)
    scores = torch.full((1, s, t), -10.0)
    expected = [0, 0, 1, 1, 2, 2, 3, 3]
    for frame, phoneme in enumerate(expected):
        scores[0, phoneme, frame] = 10.0

    path = maximum_path(scores, mask)
    assert path[0].argmax(0).tolist() == expected


def test_generate_path_matches_durations():
    durations = torch.tensor([[[2, 3, 1]]])  # (B, 1, S)
    mask = torch.ones(1, 1, 6, 3)
    path = generate_path(durations, mask)
    assert path.shape == (1, 1, 6, 3)
    assert path.squeeze(1).sum(1).tolist() == [[2.0, 3.0, 1.0]]
    assert path.squeeze(1)[0].argmax(1).tolist() == [0, 0, 1, 1, 1, 2]
