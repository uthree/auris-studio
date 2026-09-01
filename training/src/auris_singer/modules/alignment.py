"""Monotonic alignment search (MAS).

Durations are supplied explicitly at inference time, so this model has no
duration predictor.  During training on ``(waveform, text)`` pairs the
phoneme/frame alignment is still unknown, and — exactly as in VITS — it is
recovered by searching for the monotonic surjective alignment that maximizes
the prior likelihood of the flow output.

A numba kernel is used when available; otherwise an equivalent NumPy
implementation runs the same dynamic program.
"""

from __future__ import annotations

import numpy as np
import torch

__all__ = ["maximum_path"]

_NEG_INF = -1e9

try:  # pragma: no cover - depends on the environment
    from numba import njit, prange

    _HAS_NUMBA = True
except ImportError:  # pragma: no cover
    _HAS_NUMBA = False
    prange = range  # type: ignore[assignment]


def _mas_kernel(paths, values, t_xs, t_ys):
    """Viterbi forward/backtrace for one batch element set.

    ``values`` is modified in place and holds the accumulated score.
    Shapes: ``paths``/``values`` ``(B, S, T)``, ``t_xs``/``t_ys`` ``(B,)``.
    """
    for b in prange(values.shape[0]):
        t_x = t_xs[b]
        t_y = t_ys[b]
        value = values[b]
        path = paths[b]
        for y in range(t_y):
            lo = max(0, t_x + y - t_y)
            hi = min(t_x, y + 1)
            for x in range(lo, hi):
                if x == y:
                    v_cur = _NEG_INF
                else:
                    v_cur = value[x, y - 1]
                if x == 0:
                    v_prev = 0.0 if y == 0 else _NEG_INF
                else:
                    v_prev = value[x - 1, y - 1]
                value[x, y] += max(v_cur, v_prev)

        index = t_x - 1
        for y in range(t_y - 1, -1, -1):
            path[index, y] = 1.0
            if index != 0 and (
                index == y or value[index, y - 1] < value[index - 1, y - 1]
            ):
                index -= 1


if _HAS_NUMBA:  # pragma: no cover - compiled variant
    _mas_kernel_compiled = njit(nogil=True, parallel=True, cache=True)(_mas_kernel)
else:  # pragma: no cover
    _mas_kernel_compiled = _mas_kernel


@torch.no_grad()
def maximum_path(
    neg_cent: torch.Tensor, mask: torch.Tensor
) -> torch.Tensor:
    """Find the most likely monotonic alignment.

    Args:
        neg_cent: ``(B, S, T)`` log-likelihood of frame ``t`` under token ``s``.
        mask: ``(B, S, T)`` binary mask of admissible cells.

    Returns:
        ``(B, S, T)`` binary alignment path with the same dtype as ``neg_cent``.
    """
    device, dtype = neg_cent.device, neg_cent.dtype
    values = neg_cent.detach().to(torch.float32).cpu().numpy().copy()
    mask_np = mask.detach().cpu().numpy()
    paths = np.zeros_like(values)

    t_xs = mask_np.sum(axis=1)[:, 0].astype(np.int32)
    t_ys = mask_np.sum(axis=2)[:, 0].astype(np.int32)

    _mas_kernel_compiled(paths, values, t_xs, t_ys)
    return torch.from_numpy(paths).to(device=device, dtype=dtype)
