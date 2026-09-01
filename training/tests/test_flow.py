"""Tests for the Transformer-based normalizing flow."""

from __future__ import annotations

import pytest
import torch

from auris_singer.modules.flow import ResidualCouplingBlock, TransformerCouplingLayer


@pytest.mark.parametrize("mean_only", [True, False])
def test_flow_is_invertible(mean_only):
    torch.manual_seed(0)
    flow = ResidualCouplingBlock(
        16, hidden_channels=16, n_flows=3, n_layers=1, n_heads=2,
        cond_channels=8, mean_only=mean_only,
    ).eval()
    # Zero-initialized coupling layers start as the identity, so perturb them.
    for parameter in flow.parameters():
        parameter.data.add_(torch.randn_like(parameter) * 0.05)

    x = torch.randn(2, 16, 24)
    mask = torch.ones(2, 1, 24)
    g = torch.randn(2, 8, 1)

    with torch.no_grad():
        z = flow(x, mask, g=g)
        recovered = flow(z, mask, g=g, reverse=True)

    assert not torch.allclose(z, x, atol=1e-3), "flow should not be the identity"
    assert torch.allclose(recovered, x, atol=1e-4)


def test_flow_respects_the_mask():
    flow = ResidualCouplingBlock(8, hidden_channels=8, n_flows=2, n_layers=1, n_heads=2).eval()
    x = torch.randn(1, 8, 12)
    mask = torch.ones(1, 1, 12)
    mask[..., 8:] = 0
    with torch.no_grad():
        out = flow(x, mask)
    assert torch.count_nonzero(out[..., 8:]) == 0


def test_mean_only_layer_is_volume_preserving():
    layer = TransformerCouplingLayer(8, 8, n_layers=1, n_heads=2, mean_only=True)
    _, logdet = layer(torch.randn(2, 8, 10), torch.ones(2, 1, 10))
    assert torch.allclose(logdet, torch.zeros_like(logdet))


def test_layer_starts_as_identity():
    layer = TransformerCouplingLayer(8, 8, n_layers=1, n_heads=2).eval()
    x = torch.randn(2, 8, 10)
    with torch.no_grad():
        out, _ = layer(x, torch.ones(2, 1, 10))
    assert torch.allclose(out, x, atol=1e-6)


def test_odd_channel_count_is_rejected():
    with pytest.raises(ValueError):
        TransformerCouplingLayer(7, 8)
