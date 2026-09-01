"""Tests for config loading and the shipped YAML configs."""

from __future__ import annotations

import math
from pathlib import Path

import pytest
from omegaconf import OmegaConf

from auris_singer.utils.config import load_config, save_config

CONFIG_DIR = Path(__file__).resolve().parents[1] / "configs"


def test_dotlist_overrides_are_applied(tmp_path):
    (tmp_path / "a.yml").write_text("foo:\n  bar: 1\n  baz: 2\n")
    config = load_config(tmp_path / "a.yml", ["foo.bar=9", "foo.new=3"])
    assert config.foo.bar == 9 and config.foo.baz == 2 and config.foo.new == 3


def test_defaults_merge_a_preset_at_the_top_level(tmp_path):
    (tmp_path / "presets.yml").write_text("small:\n  model:\n    width: 16\n    depth: 2\n")
    (tmp_path / "train.yml").write_text(
        "defaults:\n  - presets.yml: small\nmodel:\n  depth: 4\nseed: 7\n"
    )
    config = load_config(tmp_path / "train.yml")
    assert config.model.width == 16
    assert config.model.depth == 4, "the including file must win over the preset"
    assert config.seed == 7


def test_defaults_can_target_a_nested_key(tmp_path):
    (tmp_path / "presets.yml").write_text("small:\n  width: 16\n")
    (tmp_path / "train.yml").write_text("defaults:\n  - presets.yml@model: small\n")
    config = load_config(tmp_path / "train.yml")
    assert config.model.width == 16


def test_missing_preset_key_is_reported(tmp_path):
    (tmp_path / "presets.yml").write_text("small:\n  width: 16\n")
    (tmp_path / "train.yml").write_text("defaults:\n  - presets.yml: enormous\n")
    with pytest.raises(KeyError, match="enormous"):
        load_config(tmp_path / "train.yml")


def test_save_config_roundtrip(tmp_path):
    config = OmegaConf.create({"a": {"b": 1}})
    save_config(config, tmp_path / "nested" / "out.yaml")
    assert OmegaConf.load(tmp_path / "nested" / "out.yaml") == config


@pytest.mark.parametrize("name", ["small.yml", "base.yml"])
def test_shipped_training_configs_are_self_consistent(name):
    config = load_config(CONFIG_DIR / "train" / name)

    hop = config.audio.hop_length
    assert config.model.hop_length == hop
    assert config.model.sample_rate == config.audio.sample_rate
    assert config.model.spec_channels == config.audio.n_fft // 2 + 1
    assert math.prod(config.model.generator.upsample_rates) == hop

    rates = config.model.generator.upsample_rates
    kernels = config.model.generator.upsample_kernel_sizes
    assert len(rates) == len(kernels)
    assert all(k >= r for k, r in zip(kernels, rates))

    resblock_kernels = config.model.generator.resblock_kernel_sizes
    assert len(resblock_kernels) == len(config.model.generator.resblock_dilations)

    for section in ["text_encoder", "posterior_encoder", "flow", "prior_encoder"]:
        heads = config.model[section].n_heads
        assert config.model.hidden_channels % heads == 0, section

    assert config.data.max_frames <= config.data.bucket_boundaries[-1]
    assert config.data.min_frames >= config.model.segment_size


def test_preprocess_config_matches_the_training_frame_grid():
    preprocess = load_config(CONFIG_DIR / "preprocess" / "generic_wav_text.yml")
    train = load_config(CONFIG_DIR / "train" / "base.yml")
    for key in ["sample_rate", "n_fft", "hop_length", "win_length"]:
        assert preprocess.audio[key] == train.audio[key], key
