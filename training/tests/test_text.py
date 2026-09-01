"""Tests for the IPA table and the Japanese front-end."""

from __future__ import annotations

import pytest

from auris_singer.text import (
    DEFAULT_PHONEME_TABLE,
    PAD,
    SIL,
    UNK,
    JapaneseFrontend,
    PhonemeTable,
    get_frontend,
    parse_ipa,
)
from auris_singer.text.japanese import OPENJTALK_TO_IPA, openjtalk_to_ipa


def test_pad_is_index_zero_and_table_has_no_duplicates():
    table = DEFAULT_PHONEME_TABLE
    assert table.pad_id == 0 and table.symbols[0] == PAD
    assert len(set(table.symbols)) == len(table.symbols)


def test_encode_decode_roundtrip():
    table = DEFAULT_PHONEME_TABLE
    phonemes = [SIL, "k", "o", "ɴ", "ɲ", "i", "tɕ", "i", "w", "a", SIL]
    assert table.decode(table.encode(phonemes)) == phonemes


def test_unknown_symbols_map_to_unk_and_are_reported():
    table = DEFAULT_PHONEME_TABLE
    ids = table.encode(["a", "!!not-ipa!!"])
    assert ids[1] == table.unk_id
    assert table.decode([ids[1]]) == [UNK]
    assert table.unknown_symbols(["a", "!!not-ipa!!"]) == ["!!not-ipa!!"]


def test_duplicate_symbols_are_rejected():
    with pytest.raises(ValueError, match="duplicate"):
        PhonemeTable(["a", "b", "a"])


def test_table_save_and_load_roundtrip(tmp_path):
    path = tmp_path / "phonemes.json"
    DEFAULT_PHONEME_TABLE.save(path)
    assert PhonemeTable.load(path).symbols == DEFAULT_PHONEME_TABLE.symbols


def test_every_openjtalk_phoneme_maps_into_the_table():
    missing = [
        f"{src}->{dst}"
        for src, dst in OPENJTALK_TO_IPA.items()
        if dst not in DEFAULT_PHONEME_TABLE
    ]
    assert missing == []


def test_openjtalk_translation_and_unknown_handling():
    assert openjtalk_to_ipa(["k", "o", "N", "ch", "i"]) == ["k", "o", "ɴ", "tɕ", "i"]
    assert openjtalk_to_ipa(["k", "???"]) == ["k"]
    assert openjtalk_to_ipa(["k", "???"], keep_unknown=True) == ["k", "???"]


def test_parse_ipa_splits_on_whitespace():
    # IPA symbols are multi-character, so they must be space separated.
    assert parse_ipa("k o ɴ ɲ i tɕ i") == ["k", "o", "ɴ", "ɲ", "i", "tɕ", "i"]


def test_ipa_frontend_passes_through():
    frontend = get_frontend("ipa")
    assert frontend("tɕ i") == ["tɕ", "i"]


def test_get_frontend_rejects_unknown_languages():
    with pytest.raises(ValueError, match="unsupported language"):
        get_frontend("klingon")


@pytest.mark.slow
def test_japanese_frontend_produces_table_symbols():
    """Needs the jpreprocess dictionary (downloaded on first use)."""
    try:
        frontend = JapaneseFrontend()
        phonemes = frontend.g2p("こんにちは、歌声合成です。")
    except Exception as exc:  # pragma: no cover - offline environments
        pytest.skip(f"jpreprocess unavailable: {exc}")

    assert phonemes[0] == SIL and phonemes[-1] == SIL
    assert DEFAULT_PHONEME_TABLE.unknown_symbols(phonemes) == []
    # こんにちは -> k o N n i ch i w a
    assert phonemes[1:10] == ["k", "o", "ɴ", "n", "i", "tɕ", "i", "w", "a"]


def test_every_voiceless_symbol_is_in_the_table_it_classifies():
    """Two spellings of one glyph are two strings to a set. The devoiced /a/ is ``a`` +
    U+0325 in the inventory and in what the front-end emits, and was the precomposed U+1E01
    in ``VOICELESS`` — and, by the contract, in the host — so the one devoiced vowel the
    data actually contains was never counted as voiceless on either side."""
    from auris_singer.text.ipa import IPA_SYMBOLS, SPECIAL_SYMBOLS, VOICELESS, is_voiceless

    table = set(IPA_SYMBOLS) | set(SPECIAL_SYMBOLS)
    assert VOICELESS <= table, sorted(VOICELESS - table)
    assert "a\u0325" in IPA_SYMBOLS and "\u1e01" not in IPA_SYMBOLS
    assert is_voiceless("a\u0325") and not is_voiceless("\u1e01")


def test_the_front_ends_devoiced_vowels_are_in_the_table():
    from auris_singer.text.ipa import IPA_SYMBOLS
    from auris_singer.text.japanese import openjtalk_to_ipa

    for symbol in openjtalk_to_ipa(["A", "I", "U", "E", "O"]):
        assert symbol in IPA_SYMBOLS, [hex(ord(c)) for c in symbol]
