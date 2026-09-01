"""Tests for mask and segment-slicing helpers."""

from __future__ import annotations

import torch

from auris_singer.utils.masks import (
    rand_slice_segments,
    sequence_mask,
    slice_segments,
)


def test_sequence_mask_marks_valid_positions():
    mask = sequence_mask(torch.tensor([3, 1, 0]), 4)
    assert mask.tolist() == [
        [True, True, True, False],
        [True, False, False, False],
        [False, False, False, False],
    ]


def test_sequence_mask_infers_max_length():
    assert sequence_mask(torch.tensor([2, 5])).shape == (2, 5)


def test_slice_segments_takes_the_requested_window():
    x = torch.arange(24, dtype=torch.float32).view(1, 2, 12)
    out = slice_segments(x, torch.tensor([3]), 4)
    assert out.shape == (1, 2, 4)
    assert out[0, 0].tolist() == [3.0, 4.0, 5.0, 6.0]
    assert out[0, 1].tolist() == [15.0, 16.0, 17.0, 18.0]


def test_rand_slice_segments_stays_inside_each_sequence():
    torch.manual_seed(0)
    x = torch.arange(3 * 40, dtype=torch.float32).view(3, 1, 40)
    lengths = torch.tensor([40, 20, 8])
    for _ in range(30):
        _, start = rand_slice_segments(x, lengths, 8)
        assert torch.all(start >= 0)
        assert torch.all(start + 8 <= lengths.clamp(min=8))


def test_rand_slice_segments_handles_sequences_shorter_than_the_segment():
    x = torch.randn(2, 3, 10)
    sliced, start = rand_slice_segments(x, torch.tensor([10, 4]), 16)
    assert sliced.shape == (2, 3, 16)
    assert start.tolist() == [0, 0]
