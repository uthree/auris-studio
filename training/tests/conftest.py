"""Shared fixtures.

Most tests run against a synthetic preprocessed dataset written directly to
disk, so they need neither the jpreprocess dictionary nor the FCPE model.
"""

from __future__ import annotations

import json
import math
from pathlib import Path

import numpy as np
import pytest
import torch

from auris_singer.text import DEFAULT_PHONEME_TABLE
from auris_singer.utils.audio import frame_energy

SAMPLE_RATE = 48_000
HOP_LENGTH = 480
N_FFT = 2048
WIN_LENGTH = 2048


@pytest.fixture(scope="session")
def audio_config() -> dict:
    return {
        "sample_rate": SAMPLE_RATE,
        "n_fft": N_FFT,
        "hop_length": HOP_LENGTH,
        "win_length": WIN_LENGTH,
    }


def synth_waveform(n_frames: int, f0_hz: float, seed: int) -> torch.Tensor:
    """A quiet harmonic tone with a little noise: cheap but not degenerate."""
    generator = torch.Generator().manual_seed(seed)
    n_samples = n_frames * HOP_LENGTH
    t = torch.arange(n_samples, dtype=torch.float32) / SAMPLE_RATE
    wav = torch.zeros(n_samples)
    for harmonic in range(1, 6):
        wav += torch.sin(2 * math.pi * f0_hz * harmonic * t) / harmonic
    wav = wav / wav.abs().max()
    wav = wav * 0.5 + 0.01 * torch.randn(n_samples, generator=generator)
    return wav.clamp(-1.0, 1.0)


@pytest.fixture(scope="session")
def processed_dataset(tmp_path_factory, audio_config) -> Path:
    """Build a small preprocessed dataset with two speakers."""
    root = tmp_path_factory.mktemp("processed")
    table = DEFAULT_PHONEME_TABLE
    speakers = {"alice": 0, "bob": 1}
    records = []

    rng = np.random.default_rng(0)
    for i in range(12):
        speaker = "alice" if i % 2 == 0 else "bob"
        n_frames = 60 + 10 * (i % 5)
        n_phonemes = 8 + (i % 4)
        f0_hz = 180.0 + 20.0 * (i % 6)

        wav = synth_waveform(n_frames, f0_hz, seed=i)
        energy = frame_energy(wav, N_FFT, HOP_LENGTH, WIN_LENGTH)
        voiced = (rng.random(n_frames) > 0.2).astype(np.uint8)
        f0 = (f0_hz + rng.normal(0, 2.0, n_frames)).astype(np.float32) * voiced
        phonemes = np.asarray(
            table.encode(["<sil>"] + ["a", "i", "k", "o"] * 4)[:n_phonemes],
            dtype=np.int32,
        )

        path = root / speaker / f"utt{i:03d}.npz"
        path.parent.mkdir(parents=True, exist_ok=True)
        np.savez(
            path,
            wav=(wav.numpy() * 32767.0).astype(np.int16),
            phonemes=phonemes,
            f0=f0,
            energy=energy.numpy().astype(np.float32),
            voiced=voiced,
        )
        records.append(
            {
                "id": f"{speaker}/utt{i:03d}",
                "path": str(path.relative_to(root)),
                "speaker": speaker,
                "speaker_id": speakers[speaker],
                "n_frames": n_frames,
                "n_phonemes": int(phonemes.size),
                "seconds": n_frames * HOP_LENGTH / SAMPLE_RATE,
                "text": "test",
            }
        )

    with (root / "metadata.jsonl").open("w", encoding="utf-8") as fp:
        for record in records:
            fp.write(json.dumps(record) + "\n")
    (root / "speakers.json").write_text(json.dumps(speakers))
    (root / "audio_config.json").write_text(json.dumps(audio_config))
    table.save(root / "phonemes.json")
    return root


@pytest.fixture
def tiny_model_config() -> dict:
    """A model small enough to run several steps on CPU in a test."""
    return {
        "n_vocab": len(DEFAULT_PHONEME_TABLE),
        "spec_channels": N_FFT // 2 + 1,
        "inter_channels": 16,
        "hidden_channels": 16,
        "n_speakers": 2,
        "gin_channels": 8,
        "segment_size": 8,
        "sample_rate": SAMPLE_RATE,
        "hop_length": HOP_LENGTH,
        "text_encoder": {"n_layers": 1, "n_heads": 2},
        "posterior_encoder": {"n_layers": 1, "n_heads": 2},
        "flow": {"n_flows": 2, "n_layers": 1, "n_heads": 2},
        "prior_encoder": {"n_layers": 1, "n_heads": 2},
        "generator": {"upsample_initial_channel": 16},
    }


@pytest.fixture
def tiny_discriminator_config() -> dict:
    return {
        "periods": [2, 3],
        "resolutions": [[512, 120, 512], [1024, 240, 1024]],
        "period_kwargs": {"channels": [4, 8, 16]},
        "stft_kwargs": {"channels": 4, "n_layers": 2},
    }
