"""Tests for the modernized Transformer building blocks."""

from __future__ import annotations

import pytest
import torch

from auris_singer.modules.transformer import (
    RMSNorm,
    RotaryEmbedding,
    SwiGLUFeedForward,
    TransformerEncoder,
    apply_rotary,
)


def test_rmsnorm_normalizes_to_unit_rms():
    norm = RMSNorm(8, elementwise_affine=False)
    x = torch.randn(4, 5, 8) * 7.0 + 3.0
    out = norm(x)
    rms = out.pow(2).mean(-1).sqrt()
    assert torch.allclose(rms, torch.ones_like(rms), atol=1e-3)


def test_swiglu_shape_and_hidden_multiple():
    ffn = SwiGLUFeedForward(64, multiple_of=32)
    assert ffn.hidden_dim % 32 == 0
    assert ffn(torch.randn(2, 3, 64)).shape == (2, 3, 64)


def test_rope_preserves_norm_and_encodes_relative_position():
    rope = RotaryEmbedding(16)
    cos, sin = rope(12, torch.device("cpu"))
    q = torch.randn(1, 2, 12, 16)
    rotated = apply_rotary(q, cos, sin)
    assert torch.allclose(rotated.norm(dim=-1), q.norm(dim=-1), atol=1e-5)

    # With the same content at every position, the dot product between two
    # rotated vectors depends only on the offset between their positions.
    q_const = torch.randn(1, 1, 1, 16).expand(1, 1, 12, 16)
    k_const = torch.randn(1, 1, 1, 16).expand(1, 1, 12, 16)
    scores = apply_rotary(q_const, cos, sin)[0, 0] @ apply_rotary(k_const, cos, sin)[0, 0].T
    assert scores[2, 5] == pytest.approx(scores[4, 7].item(), abs=1e-4)
    assert scores[0, 3] == pytest.approx(scores[8, 11].item(), abs=1e-4)


def test_encoder_output_shape_and_masking():
    encoder = TransformerEncoder(32, n_layers=2, n_heads=4, cond_channels=8)
    x = torch.randn(3, 32, 20)
    lengths = torch.tensor([20, 14, 7])
    mask = (torch.arange(20)[None, :] < lengths[:, None]).unsqueeze(1).float()
    g = torch.randn(3, 8, 1)

    out = encoder(x, mask, g=g)
    assert out.shape == x.shape
    # Padded positions must be exactly zero.
    assert torch.count_nonzero(out[1, :, 14:]) == 0
    assert torch.count_nonzero(out[2, :, 7:]) == 0


def test_encoder_ignores_padding_content():
    """Changing padded values must not change the valid outputs."""
    torch.manual_seed(0)
    encoder = TransformerEncoder(16, n_layers=2, n_heads=2).eval()
    lengths = torch.tensor([6])
    mask = (torch.arange(10)[None, :] < lengths[:, None]).unsqueeze(1).float()

    x = torch.randn(1, 16, 10)
    polluted = x.clone()
    polluted[..., 6:] = 42.0

    with torch.no_grad():
        a = encoder(x, mask)
        b = encoder(polluted, mask)
    assert torch.allclose(a[..., :6], b[..., :6], atol=1e-5)


def test_conditioning_is_identity_at_initialization():
    """Zero-initialized modulation means conditioning starts as a no-op."""
    torch.manual_seed(0)
    encoder = TransformerEncoder(16, n_layers=2, n_heads=2, cond_channels=4).eval()
    x = torch.randn(2, 16, 9)
    with torch.no_grad():
        without = encoder(x, None, g=None)
        with_cond = encoder(x, None, g=torch.randn(2, 4, 1))
    assert torch.allclose(without, with_cond, atol=1e-6)


def test_encoder_rejects_incompatible_head_count():
    with pytest.raises(ValueError):
        TransformerEncoder(18, n_layers=1, n_heads=4)
