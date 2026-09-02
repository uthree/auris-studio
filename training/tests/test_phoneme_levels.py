"""The consonant-level table: measured against the next vowel, shipped with the voice."""

from __future__ import annotations

import numpy as np
import pytest

from auris_singer.phoneme_levels import MIN_SAMPLES, measure, summarize


def utterance(spec: list[tuple[str, int, float]]) -> tuple[list[str], list[int], np.ndarray]:
    """``(symbol, frames, rms)`` triples into what the preprocessor stores."""
    phonemes = [s for s, _, _ in spec]
    durations = [n for _, n, _ in spec]
    energy = np.concatenate([np.full(n, rms, np.float32) for _, n, rms in spec])
    return phonemes, durations, energy


def test_a_consonant_is_measured_against_the_vowel_after_it():
    levels = measure([utterance([("<sil>", 5, 0.0), ("k", 4, 0.01), ("a", 20, 0.1), ("s", 6, 0.02), ("i", 15, 0.2), ("<sil>", 5, 0.0)])])
    assert levels["k"] == pytest.approx([-20.0])
    assert levels["s"] == pytest.approx([-20.0]), "against *its* vowel, not the loudest one"
    assert "a" not in levels and "<sil>" not in levels, "vowels are the reference, specials boundaries"


def test_a_consonant_with_no_vowel_after_it_or_at_the_floor_is_not_a_reading():
    levels = measure([utterance([("a", 10, 0.1), ("ɴ", 5, 0.05)]), utterance([("k", 4, 0.0), ("a", 10, 0.1)])])
    assert levels == {}


def test_the_block_ships_the_medians_that_earned_it_and_a_consonant_default():
    levels = {"k": [-22.0] * MIN_SAMPLES, "s": [-19.0, -21.0] * (MIN_SAMPLES // 2), "ɸʲ": [-15.0] * 3}
    block = summarize(levels, "a test corpus")
    assert block["unit"] == "db"
    assert block["db"] == {"k": -22.0, "s": -20.0}, "ɸʲ was seen three times and does not ship"
    assert block["counts"] == {"k": MIN_SAMPLES, "s": MIN_SAMPLES}
    assert block["default"] == pytest.approx(-21.0), "the pooled median, a consonant's level, not 0 dB"
    assert block["measured_from"] == "a test corpus"
    assert list(block["db"]) == ["k", "s"], "quietest first"
    assert summarize({}, "nothing")["default"] == 0.0


def test_the_export_carries_the_table_and_refuses_a_stranger(tiny_model_config, tmp_path):
    pytest.importorskip("onnxruntime")
    import json

    import torch

    from auris_singer.export import export_onnx
    from auris_singer.model import AurisSinger

    torch.manual_seed(0)
    model = AurisSinger(**tiny_model_config).eval()
    table = {"unit": "db", "default": -12.0, "db": {"k": -22.0}, "counts": {"k": 30}, "measured_from": "test"}
    export_onnx(model, tmp_path / "v.onnx", metadata={"symbols": ["<pad>", "<unk>", "<sil>", "k", "a"]}, phoneme_levels=table)
    stored = json.loads((tmp_path / "v.json").read_text(encoding="utf-8"))
    assert stored["phoneme_levels"] == table
    with pytest.raises(ValueError, match="phoneme_levels names symbols"):
        export_onnx(
            AurisSinger(**tiny_model_config).eval(), tmp_path / "w.onnx",
            metadata={"symbols": ["<pad>", "<unk>", "<sil>", "a"]}, phoneme_levels=table,
        )
