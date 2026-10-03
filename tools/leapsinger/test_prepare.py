"""Check vocabulary preservation without importing PyTorch or downloading voice models."""

import pytest
from prepare import phoneme_dictionary


def test_dictionary_keeps_checkpoint_order():
    assert (
        phoneme_dictionary({"n_phonemes": 3, "phonemes": ["pau", "u", "a"]})
        == "pau\nu\na\n"
    )


@pytest.mark.parametrize(
    "config",
    [
        {"n_phonemes": 2},
        {"n_phonemes": 2, "phonemes": ["a", "pau"]},
        {"n_phonemes": 3, "phonemes": ["pau", "a"]},
        {"n_phonemes": 3, "phonemes": ["pau", "a", "a"]},
        {"n_phonemes": 2, "phonemes": ["pau", "a i"]},
        {"n_phonemes": 2, "phonemes": ["pau", 1]},
        {"n_phonemes": 2, "phonemes": ["pau", "a#comment"]},
        {"n_phonemes": 2, "phonemes": ["pau", "a\n"]},
        {"n_phonemes": 2, "phonemes": ["pau", " a"]},
    ],
)
def test_dictionary_rejects_ambiguous_token_ids(config):
    with pytest.raises(ValueError):
        phoneme_dictionary(config)
