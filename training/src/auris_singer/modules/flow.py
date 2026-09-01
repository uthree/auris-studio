"""Transformer-based normalizing flow.

VITS conditions the prior with a stack of affine coupling layers whose coupling
function is a WaveNet.  The coupling structure is kept — it is what makes the
map invertible — but the WaveNet is replaced by a modernized Transformer
encoder.
"""

from __future__ import annotations

import torch
import torch.nn as nn

from auris_singer.modules.transformer import TransformerEncoder

__all__ = ["TransformerCouplingLayer", "Flip", "ResidualCouplingBlock"]


class Flip(nn.Module):
    """Channel reversal, so consecutive coupling layers transform both halves."""

    def forward(
        self,
        x: torch.Tensor,
        x_mask: torch.Tensor,
        g: torch.Tensor | None = None,
        reverse: bool = False,
    ) -> torch.Tensor | tuple[torch.Tensor, torch.Tensor]:
        x = torch.flip(x, [1])
        if reverse:
            return x
        return x, torch.zeros(x.size(0), device=x.device, dtype=x.dtype)


class TransformerCouplingLayer(nn.Module):
    """Affine coupling layer with a Transformer coupling function.

    With ``mean_only=True`` the layer is volume preserving (log-determinant is
    zero) which matches the VITS default and trains more stably.
    """

    def __init__(
        self,
        channels: int,
        hidden_channels: int,
        n_layers: int = 2,
        n_heads: int = 2,
        ffn_dim: int | None = None,
        dropout: float = 0.0,
        cond_channels: int = 0,
        mean_only: bool = True,
    ):
        super().__init__()
        if channels % 2 != 0:
            raise ValueError(f"channels must be even, got {channels}")
        self.half_channels = channels // 2
        self.mean_only = mean_only

        self.pre = nn.Conv1d(self.half_channels, hidden_channels, 1)
        self.enc = TransformerEncoder(
            hidden_channels,
            n_layers=n_layers,
            n_heads=n_heads,
            ffn_dim=ffn_dim,
            dropout=dropout,
            cond_channels=cond_channels,
        )
        self.post = nn.Conv1d(
            hidden_channels, self.half_channels * (1 if mean_only else 2), 1
        )
        # Zero init => the layer starts as the identity transform.
        nn.init.zeros_(self.post.weight)
        nn.init.zeros_(self.post.bias)

    def forward(
        self,
        x: torch.Tensor,
        x_mask: torch.Tensor,
        g: torch.Tensor | None = None,
        reverse: bool = False,
    ) -> torch.Tensor | tuple[torch.Tensor, torch.Tensor]:
        x0, x1 = x.chunk(2, dim=1)
        h = self.pre(x0) * x_mask
        h = self.enc(h, x_mask, g=g)
        stats = self.post(h) * x_mask

        if self.mean_only:
            m = stats
            logs = torch.zeros_like(m)
        else:
            m, logs = stats.chunk(2, dim=1)

        if not reverse:
            x1 = (m + x1 * torch.exp(logs)) * x_mask
            logdet = torch.sum(logs * x_mask, dim=[1, 2])
            return torch.cat([x0, x1], dim=1), logdet
        x1 = ((x1 - m) * torch.exp(-logs)) * x_mask
        return torch.cat([x0, x1], dim=1)


class ResidualCouplingBlock(nn.Module):
    """Stack of coupling layers interleaved with channel flips."""

    def __init__(
        self,
        channels: int,
        hidden_channels: int,
        n_flows: int = 4,
        n_layers: int = 2,
        n_heads: int = 2,
        ffn_dim: int | None = None,
        dropout: float = 0.0,
        cond_channels: int = 0,
        mean_only: bool = True,
    ):
        super().__init__()
        self.flows = nn.ModuleList()
        for _ in range(n_flows):
            self.flows.append(
                TransformerCouplingLayer(
                    channels,
                    hidden_channels,
                    n_layers=n_layers,
                    n_heads=n_heads,
                    ffn_dim=ffn_dim,
                    dropout=dropout,
                    cond_channels=cond_channels,
                    mean_only=mean_only,
                )
            )
            self.flows.append(Flip())

    def forward(
        self,
        x: torch.Tensor,
        x_mask: torch.Tensor,
        g: torch.Tensor | None = None,
        reverse: bool = False,
    ) -> torch.Tensor:
        if not reverse:
            for flow in self.flows:
                x, _ = flow(x, x_mask, g=g, reverse=False)
        else:
            for flow in reversed(self.flows):
                x = flow(x, x_mask, g=g, reverse=True)
        return x
