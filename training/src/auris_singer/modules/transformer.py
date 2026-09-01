"""Modernized Transformer building blocks.

Every 1D sequence module of the original VITS (text encoder, posterior encoder,
normalizing flow, ...) is built on top of the encoder defined here instead of the
original WaveNet/1D-CNN stacks.  The block follows current practice:

* pre-norm residual blocks with :class:`RMSNorm`
* :class:`SwiGLUFeedForward` instead of a ReLU/GELU MLP
* rotary position embeddings (RoPE) instead of absolute/relative positions
* QK normalization for attention-logit stability
* ``F.scaled_dot_product_attention`` so PyTorch dispatches to the fused
  (Flash / memory-efficient) kernels

Tensors are channel-first ``(B, C, T)`` at the module boundary to stay
drop-in compatible with the rest of the VITS code base; the transpose to
``(B, T, C)`` happens internally.
"""

from __future__ import annotations

import torch
import torch.nn as nn
import torch.nn.functional as F

__all__ = [
    "RMSNorm",
    "SwiGLUFeedForward",
    "RotaryEmbedding",
    "SelfAttention",
    "TransformerBlock",
    "TransformerEncoder",
]


class RMSNorm(nn.Module):
    """Root-mean-square layer normalization (Zhang & Sennrich, 2019)."""

    def __init__(self, dim: int, eps: float = 1e-6, elementwise_affine: bool = True):
        super().__init__()
        self.eps = eps
        self.weight = nn.Parameter(torch.ones(dim)) if elementwise_affine else None

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        dtype = x.dtype
        x_fp32 = x.float()
        normed = x_fp32 * torch.rsqrt(x_fp32.pow(2).mean(-1, keepdim=True) + self.eps)
        normed = normed.to(dtype)
        if self.weight is not None:
            normed = normed * self.weight
        return normed


class SwiGLUFeedForward(nn.Module):
    """SwiGLU feed-forward network (Shazeer, 2020)."""

    def __init__(
        self,
        dim: int,
        hidden_dim: int | None = None,
        multiple_of: int = 64,
        dropout: float = 0.0,
    ):
        super().__init__()
        if hidden_dim is None:
            # 8/3 * dim keeps the parameter count of a 4*dim ReLU MLP.
            hidden_dim = int(8 * dim / 3)
        hidden_dim = multiple_of * ((hidden_dim + multiple_of - 1) // multiple_of)
        self.hidden_dim = hidden_dim
        self.w_in = nn.Linear(dim, 2 * hidden_dim, bias=False)
        self.w_out = nn.Linear(hidden_dim, dim, bias=False)
        self.dropout = nn.Dropout(dropout)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        gate, value = self.w_in(x).chunk(2, dim=-1)
        return self.w_out(self.dropout(F.silu(gate) * value))


class RotaryEmbedding(nn.Module):
    """Rotary position embedding (Su et al., 2021), GPT-NeoX half-split layout."""

    def __init__(self, head_dim: int, base: float = 10_000.0):
        super().__init__()
        if head_dim % 2 != 0:
            raise ValueError(f"head_dim must be even for RoPE, got {head_dim}")
        self.head_dim = head_dim
        self.base = base
        inv_freq = 1.0 / (base ** (torch.arange(0, head_dim, 2).float() / head_dim))
        self.register_buffer("inv_freq", inv_freq, persistent=False)

    def forward(self, seq_len: int, device: torch.device) -> tuple[torch.Tensor, torch.Tensor]:
        t = torch.arange(seq_len, device=device, dtype=self.inv_freq.dtype)
        freqs = torch.outer(t, self.inv_freq.to(device))
        emb = torch.cat((freqs, freqs), dim=-1)
        # (1, 1, T, head_dim) so it broadcasts over (B, H, T, D)
        return emb.cos()[None, None], emb.sin()[None, None]


def _rotate_half(x: torch.Tensor) -> torch.Tensor:
    x1, x2 = x.chunk(2, dim=-1)
    return torch.cat((-x2, x1), dim=-1)


def apply_rotary(
    x: torch.Tensor, cos: torch.Tensor, sin: torch.Tensor
) -> torch.Tensor:
    """Apply RoPE to ``x`` of shape ``(B, H, T, D)``."""
    cos = cos.to(x.dtype)
    sin = sin.to(x.dtype)
    return x * cos + _rotate_half(x) * sin


class SelfAttention(nn.Module):
    """Multi-head self attention with RoPE, QK-Norm and fused SDPA."""

    def __init__(
        self,
        dim: int,
        n_heads: int,
        dropout: float = 0.0,
        qk_norm: bool = True,
        bias: bool = False,
    ):
        super().__init__()
        if dim % n_heads != 0:
            raise ValueError(f"dim ({dim}) must be divisible by n_heads ({n_heads})")
        self.n_heads = n_heads
        self.head_dim = dim // n_heads
        self.dropout = dropout
        self.qkv = nn.Linear(dim, 3 * dim, bias=bias)
        self.proj = nn.Linear(dim, dim, bias=bias)
        if qk_norm:
            self.q_norm: nn.Module = RMSNorm(self.head_dim)
            self.k_norm: nn.Module = RMSNorm(self.head_dim)
        else:
            self.q_norm = nn.Identity()
            self.k_norm = nn.Identity()

    def forward(
        self,
        x: torch.Tensor,
        cos: torch.Tensor,
        sin: torch.Tensor,
        attn_mask: torch.Tensor | None = None,
    ) -> torch.Tensor:
        b, t, _ = x.shape
        qkv = self.qkv(x).view(b, t, 3, self.n_heads, self.head_dim)
        q, k, v = qkv.permute(2, 0, 3, 1, 4).unbind(0)  # each (B, H, T, D)

        q = self.q_norm(q)
        k = self.k_norm(k)
        q = apply_rotary(q, cos, sin)
        k = apply_rotary(k, cos, sin)

        out = F.scaled_dot_product_attention(
            q, k, v, attn_mask=attn_mask, dropout_p=self.dropout if self.training else 0.0
        )
        # The head dimension is written out rather than inferred with -1: a -1 in
        # a traced view becomes an ONNX Reshape with allowzero=1 whose shape
        # tensor holds -1, and onnxruntime's DirectML provider rejects that
        # combination outright (see doc/inference.md).
        out = out.transpose(1, 2).reshape(b, t, self.n_heads * self.head_dim)
        return self.proj(out)


class TransformerBlock(nn.Module):
    """Pre-norm Transformer block with optional global (speaker) conditioning.

    When ``cond_dim`` is given the block is modulated adaLN-style: the
    conditioning vector produces per-channel scale/shift for both sub-layer
    norms.  The projection is zero-initialized, so the block starts out exactly
    equal to the unconditioned one.
    """

    def __init__(
        self,
        dim: int,
        n_heads: int,
        ffn_dim: int | None = None,
        dropout: float = 0.0,
        qk_norm: bool = True,
        cond_dim: int = 0,
    ):
        super().__init__()
        self.norm_attn = RMSNorm(dim)
        self.attn = SelfAttention(dim, n_heads, dropout=dropout, qk_norm=qk_norm)
        self.norm_ffn = RMSNorm(dim)
        self.ffn = SwiGLUFeedForward(dim, ffn_dim, dropout=dropout)
        self.dropout = nn.Dropout(dropout)

        self.cond_dim = cond_dim
        if cond_dim > 0:
            self.modulation = nn.Linear(cond_dim, 4 * dim)
            nn.init.zeros_(self.modulation.weight)
            nn.init.zeros_(self.modulation.bias)

    def forward(
        self,
        x: torch.Tensor,
        cos: torch.Tensor,
        sin: torch.Tensor,
        attn_mask: torch.Tensor | None = None,
        g: torch.Tensor | None = None,
    ) -> torch.Tensor:
        if self.cond_dim > 0 and g is not None:
            scale_a, shift_a, scale_f, shift_f = self.modulation(g).unsqueeze(1).chunk(4, dim=-1)
        else:
            scale_a = shift_a = scale_f = shift_f = None

        h = self.norm_attn(x)
        if scale_a is not None:
            h = h * (1.0 + scale_a) + shift_a
        x = x + self.dropout(self.attn(h, cos, sin, attn_mask))

        h = self.norm_ffn(x)
        if scale_f is not None:
            h = h * (1.0 + scale_f) + shift_f
        x = x + self.dropout(self.ffn(h))
        return x


class TransformerEncoder(nn.Module):
    """Stack of :class:`TransformerBlock` operating on channel-first tensors.

    Args:
        channels: model width.
        n_layers: number of blocks.
        n_heads: attention heads.
        ffn_dim: SwiGLU inner width; ``None`` picks ``8/3 * channels``.
        cond_channels: width of the global conditioning vector (0 disables it).
    """

    def __init__(
        self,
        channels: int,
        n_layers: int,
        n_heads: int,
        ffn_dim: int | None = None,
        dropout: float = 0.0,
        qk_norm: bool = True,
        cond_channels: int = 0,
        rope_base: float = 10_000.0,
    ):
        super().__init__()
        self.channels = channels
        self.cond_channels = cond_channels
        self.rope = RotaryEmbedding(channels // n_heads, base=rope_base)
        self.blocks = nn.ModuleList(
            TransformerBlock(
                channels,
                n_heads,
                ffn_dim=ffn_dim,
                dropout=dropout,
                qk_norm=qk_norm,
                cond_dim=cond_channels,
            )
            for _ in range(n_layers)
        )
        self.norm_out = RMSNorm(channels)

    def forward(
        self,
        x: torch.Tensor,
        x_mask: torch.Tensor | None = None,
        g: torch.Tensor | None = None,
    ) -> torch.Tensor:
        """
        Args:
            x: ``(B, C, T)``
            x_mask: ``(B, 1, T)`` float mask, 1 for valid frames.
            g: ``(B, cond_channels, 1)`` or ``(B, cond_channels)`` global condition.

        Returns:
            ``(B, C, T)``, masked.
        """
        h = x.transpose(1, 2)  # (B, T, C)
        cos, sin = self.rope(h.shape[1], h.device)

        attn_mask = None
        if x_mask is not None:
            # (B, 1, 1, T) boolean key-padding mask; True keeps the key.
            attn_mask = x_mask.bool().unsqueeze(1)
            h = h * x_mask.transpose(1, 2)

        if g is not None and g.dim() == 3:
            g = g.squeeze(-1)

        for block in self.blocks:
            h = block(h, cos, sin, attn_mask=attn_mask, g=g)
            if x_mask is not None:
                h = h * x_mask.transpose(1, 2)

        h = self.norm_out(h)
        out = h.transpose(1, 2)
        if x_mask is not None:
            out = out * x_mask
        return out
