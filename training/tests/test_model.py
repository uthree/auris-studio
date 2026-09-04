"""Tests for the assembled generator-side model."""

from __future__ import annotations

import pytest
import torch

from auris_singer.losses import kl_loss
from auris_singer.model import AurisSinger

HOP = 480


@pytest.fixture
def model(tiny_model_config):
    torch.manual_seed(0)
    return AurisSinger(**tiny_model_config).eval()


def make_batch(batch_size=2, n_phonemes=9, n_frames=48, spec_channels=1025):
    phonemes = torch.randint(1, 30, (batch_size, n_phonemes))
    phoneme_lengths = torch.tensor([n_phonemes, n_phonemes - 2][:batch_size])
    spec = torch.randn(batch_size, spec_channels, n_frames).abs()
    spec_lengths = torch.tensor([n_frames, n_frames - 9][:batch_size])
    f0 = torch.rand(batch_size, n_frames) * 300 + 100
    energy = torch.rand(batch_size, n_frames) * 0.2
    voiced = (torch.rand(batch_size, n_frames) > 0.3).float()
    speaker_ids = torch.tensor([0, 1][:batch_size])
    return dict(
        phonemes=phonemes,
        phoneme_lengths=phoneme_lengths,
        spec=spec,
        spec_lengths=spec_lengths,
        f0=f0,
        energy=energy,
        voiced=voiced,
        speaker_ids=speaker_ids,
    )


def test_forward_shapes(model, tiny_model_config):
    batch = make_batch()
    out = model(**batch)
    segment = tiny_model_config["segment_size"]

    assert out["wav_hat"].shape == (2, 1, segment * HOP)
    assert out["source"].shape == out["wav_hat"].shape
    assert out["z_p"].shape == out["m_p"].shape == out["logs_p"].shape
    assert out["m_p"].shape[-1] == batch["spec"].size(-1)
    assert out["attn"].shape == (2, batch["phonemes"].size(1), batch["spec"].size(-1))


def test_mas_durations_cover_every_frame(model):
    batch = make_batch()
    out = model(**batch)
    durations = out["durations"]
    # Each utterance's durations must sum to its own frame count.
    assert durations.sum(1).tolist() == batch["spec_lengths"].float().tolist()
    # Padded phonemes must receive no frames.
    assert durations[1, batch["phoneme_lengths"][1] :].sum() == 0
    # Every real phoneme gets at least one frame.
    assert torch.all(durations[0, : batch["phoneme_lengths"][0]] >= 1)


def test_supplied_durations_bypass_alignment_search(model):
    batch = make_batch(batch_size=1, n_phonemes=6, n_frames=30)
    durations = torch.tensor([[5, 5, 5, 5, 5, 5]])
    out = model(**batch, durations=durations)
    assert out["durations"].tolist() == [[5.0] * 6]


def test_infer_produces_the_requested_length(model):
    durations = torch.tensor([[4, 6, 5, 3, 2, 4, 6, 5, 5]])
    n_frames = int(durations.sum())
    wav = model.infer(
        phonemes=torch.randint(1, 30, (1, 9)),
        phoneme_lengths=torch.tensor([9]),
        durations=durations,
        f0=torch.rand(1, n_frames) * 200 + 150,
        energy=torch.rand(1, n_frames) * 0.2,
        speaker_ids=torch.tensor([0]),
    )
    assert wav.shape == (1, 1, n_frames * HOP)
    assert wav.abs().max() <= 1.0


def test_infer_defaults_to_the_first_speaker(model):
    durations = torch.tensor([[3, 3, 3]])
    wav = model.infer(
        phonemes=torch.randint(1, 30, (1, 3)),
        phoneme_lengths=torch.tensor([3]),
        durations=durations,
        f0=torch.full((1, 9), 220.0),
        energy=torch.full((1, 9), 0.1),
    )
    assert wav.shape == (1, 1, 9 * HOP)


def test_infer_rejects_control_curves_that_do_not_fit_durations(model):
    with pytest.raises(ValueError, match="f0 has 8 frames but durations require 9"):
        model.infer(
            phonemes=torch.randint(1, 30, (1, 3)),
            phoneme_lengths=torch.tensor([3]),
            durations=torch.tensor([[3, 3, 3]]),
            f0=torch.full((1, 8), 220.0),
            energy=torch.full((1, 8), 0.1),
        )


def test_f0_and_energy_change_the_synthesized_waveform(model):
    durations = torch.tensor([[4, 4, 4]])
    common = dict(
        phonemes=torch.randint(1, 30, (1, 3)),
        phoneme_lengths=torch.tensor([3]),
        durations=durations,
        noise_scale=0.0,  # remove prior sampling noise so only f0/energy differ
    )
    n_frames = 12
    base = model.infer(f0=torch.full((1, n_frames), 220.0), energy=torch.full((1, n_frames), 0.1), **common)
    higher = model.infer(f0=torch.full((1, n_frames), 440.0), energy=torch.full((1, n_frames), 0.1), **common)
    louder = model.infer(f0=torch.full((1, n_frames), 220.0), energy=torch.full((1, n_frames), 0.4), **common)
    assert not torch.allclose(base, higher, atol=1e-4)
    assert not torch.allclose(base, louder, atol=1e-4)


def test_backward_pass_reaches_every_submodule(tiny_model_config):
    torch.manual_seed(0)
    model = AurisSinger(**tiny_model_config)
    out = model(**make_batch())
    # The decoder is fed by the posterior, so the text/prior encoders are only
    # reachable through the KL terms — both parts of the loss are needed here.
    loss = (
        out["wav_hat"].square().mean()
        + kl_loss(out["z_p"], out["logs_q"], out["m_p"], out["logs_p"], out["y_mask"])
        + kl_loss(
            out["z_p"], out["logs_q"], out["m_p0_frame"], out["logs_p0_frame"], out["y_mask"]
        )
    )
    loss.backward()

    for name in ["text_encoder", "posterior_encoder", "flow", "prior_encoder", "generator"]:
        module = getattr(model, name)
        grads = [p.grad for p in module.parameters() if p.grad is not None]
        assert grads, f"no gradient reached {name}"
        assert all(torch.isfinite(g).all() for g in grads), f"non-finite grad in {name}"
