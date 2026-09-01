"""What this project and the host that plays its exports must agree on.

An exported ``.onnx`` is a contract between two code bases in two languages:
this one writes the file and ``crates/auris-singer`` reads it, with
``crates/auris-vocal`` deciding which phonemes go into it. Several things are
therefore written down twice — the ``metadata_props`` key, the metadata format
version, the reserved symbols, and the phoneme table down to which symbols are
voiceless — and while the two halves lived in separate repositories nothing
could check that the copies agreed. A comment in
``crates/auris-vocal/src/phoneme.rs`` went as far as asserting that its
voiceless list "matches the ``VOICELESS`` table in auris-singer's training
pipeline symbol for symbol": true when written, unverified ever after, and one
careless edit from being false. These tests are that comment, executable.

The Rust sources are read as *text*, never run — ``uv run pytest`` must not
need a Rust toolchain, and the host's own ``cargo test`` must not need a Python
one. That makes every test here a parser, and a parser that quietly stops
finding anything is a test that quietly stops testing; so each one checks that
its parse found something before it compares, and the array parsers check what
they found against the length Rust declares in the type.

When one of these fails, the fix is a decision rather than an edit to whichever
side the assertion happened to name. Either the export has moved on and the
host must be taught to read it — which is what bumping ``FORMAT_VERSION`` on
both sides declares — or the host is right and the export is wrong.
"""

from __future__ import annotations

import re
from pathlib import Path
from types import SimpleNamespace

import pytest

from auris_singer.export import FORMAT_VERSION, METADATA_KEY, metadata_block
from auris_singer.phoneme_durations import METADATA_FIELD, summarize
from auris_singer.text.ipa import (
    IPA_SYMBOLS,
    SIL,
    SPECIAL_SYMBOLS,
    UNK,
    VOICELESS,
    PhonemeTable,
)

#: The repository root: this file is ``training/tests/`` two levels down.
REPO_ROOT = Path(__file__).resolve().parents[2]

METADATA_RS = "crates/auris-singer/src/metadata.rs"
SCORE_RS = "crates/auris-singer/src/score.rs"
PHONEME_RS = "crates/auris-vocal/src/phoneme.rs"
OPENJTALK_RS = "crates/auris-vocal/src/openjtalk.rs"
KANA_RS = "crates/auris-vocal/src/kana.rs"


def rust(relative: str) -> str:
    """The Rust source at ``relative``, without comments or its test module.

    Both are dropped because both are full of the very strings these tests look
    for: a doc comment naming ``<sil>`` is prose, and a test's fixture table is
    not the vocabulary. Stripping ``//`` to end of line would also cut through a
    string literal containing it — no source read here has one, and a URL would
    only ever appear in a comment, which is going anyway.
    """
    path = REPO_ROOT / relative
    if not path.is_file():
        raise AssertionError(
            f"{relative} is missing. This test reads the host's own sources, so it wants the "
            "whole Auris Studio repository, not `training/` on its own."
        )
    text = path.read_text(encoding="utf-8")
    cut = text.find("#[cfg(test)]")
    if cut >= 0:
        text = text[:cut]
    return re.sub(r"//[^\n]*", "", text)


def rust_str_const(source: str, name: str) -> str:
    """The value of a ``const NAME: &str = "...";``."""
    found = re.search(rf'const {name}: &str = "([^"]*)"', rust(source))
    assert found, f"no `const {name}: &str` in {source} — the parser or the constant has moved"
    return found.group(1)


def rust_u32_const(source: str, name: str) -> int:
    """The value of a ``const NAME: u32 = N;``."""
    found = re.search(rf"const {name}: u32 = (\d+)", rust(source))
    assert found, f"no `const {name}: u32` in {source} — the parser or the constant has moved"
    return int(found.group(1))


def rust_str_array(source: str, name: str) -> list[str]:
    """The items of a ``const NAME: [&str; N] = [...];``, checked against ``N``."""
    found = re.search(rf'const {name}: \[&str; (\d+)\] = \[(.*?)\];', rust(source), re.S)
    assert found, f"no `const {name}: [&str; N]` in {source} — the parser or the array has moved"
    declared, items = int(found.group(1)), re.findall(r'"([^"]*)"', found.group(2))
    assert len(items) == declared, (
        f"{name} in {source} declares {declared} items and the parse found {len(items)}"
    )
    return items


def rust_struct_fields(source: str, name: str) -> dict[str, bool]:
    """A struct's fields, each mapped to whether serde gives it a default.

    A field without one is a field the host *requires*: an export that omits it
    is refused outright rather than read with a zero in its place.
    """
    text = rust(source)
    at = text.find(f"pub struct {name} {{")
    assert at >= 0, f"no `pub struct {name}` in {source}"
    body = text[at : text.index("\n}", at)]
    fields: dict[str, bool] = {}
    defaulted = False
    for line in body.splitlines():
        line = line.strip()
        if line.startswith("#[") and "serde(default" in line:
            defaulted = True
            continue
        field = re.match(r"pub (\w+):", line)
        if field:
            fields[field.group(1)] = defaulted
            defaulted = False
    assert fields, f"parsed no fields out of {name} in {source}"
    return fields


def rust_ipa_arms(source: str) -> set[str]:
    """Every IPA token on the right of a ``=> &[...]`` match arm."""
    text = rust(source)
    symbols: set[str] = set()
    for arm in re.finditer(r"=>\s*&\[([^\]]*)\]", text):
        symbols |= set(re.findall(r'"([^"]*)"', arm.group(1)))
    assert symbols, f"parsed no phoneme arms out of {source}"
    return symbols


def kana_symbols() -> set[str]:
    """Every IPA token the kana walker can produce.

    Two shapes carry them, and the arm shape alone is not enough: the single-kana
    table answers ``&["k", "a"]`` directly while the ゃゅょ rows wrap the same
    array in ``Some(tokens(&[...]))``, so every ``&[...]`` in the file is read
    rather than only the ones an arm points straight at. The digraph rows add a
    third shape — the onset handed to ``palatal``/``with_vowel``, whose vowel
    comes from the small kana. Nothing else in the file is a ``&[&str]`` with
    literals in it; a fourth shape written later would be missed rather than
    mistaken, which is the safe direction, because this set is only ever asked
    whether it is *inside* the phoneme table.
    """
    text = rust(KANA_RS)
    symbols: set[str] = set()
    for array in re.finditer(r"&\[([^\]]*)\]", text):
        symbols |= set(re.findall(r'"([^"]*)"', array.group(1)))
    symbols |= set(re.findall(r'(?:palatal|with_vowel)\("([^"]*)"\)', text))
    assert symbols, f"parsed no phonemes out of {KANA_RS}"
    return symbols


@pytest.fixture(scope="module")
def phoneme_table() -> set[str]:
    """Every symbol a default export's phoneme table holds."""
    return set(PhonemeTable().symbols)


def test_the_metadata_key_is_spelt_the_same_on_both_sides():
    assert METADATA_KEY == rust_str_const(METADATA_RS, "METADATA_KEY"), (
        "the key an export stores its JSON under is not the key the host looks it up by, so "
        "every exported voice would read as 'not a voice'"
    )


def test_the_format_version_is_the_same_on_both_sides():
    host = rust_u32_const(METADATA_RS, "FORMAT_VERSION")
    assert FORMAT_VERSION == host, (
        f"this project stamps format {FORMAT_VERSION} and the host reads up to {host}. "
        "Raising the exporter alone makes every voice it writes refuse to load; the host has "
        "to learn the new shape in the same change."
    )


def test_a_real_export_carries_every_field_the_host_requires(phoneme_table):
    """The block an export ships, held against the fields the host demands.

    Built the way ``scripts/export_onnx.py`` builds it — the model's own
    parameters, plus the ``metadata`` a checkpoint carries from
    ``scripts/train.py``. A stand-in stands in for the model because none of
    this needs weights, only the numbers an export copies out of them.
    """
    model = SimpleNamespace(
        sample_rate=48_000,
        hop_length=256,
        inter_channels=192,
        n_speakers=1,
        generator=SimpleNamespace(source_generator=SimpleNamespace(f0_min=40.0)),
    )
    block = metadata_block(
        model,
        {"symbols": sorted(phoneme_table), "speaker_to_id": {"voice": 0}, "audio": {}},
    )
    fields = rust_struct_fields(METADATA_RS, "VoiceInfo")
    required = {field for field, defaulted in fields.items() if not defaulted}
    missing = sorted(required - set(block))
    assert not missing, (
        f"the host requires {missing} and an export does not write them. A field with no "
        "`#[serde(default)]` is one the host refuses a file for lacking."
    )


def test_the_hosts_reserved_symbols_are_the_ones_this_project_reserves():
    """``<sil>`` and ``<unk>`` are hard-coded on the host side and looked for by name."""
    assert SIL == rust_str_const(SCORE_RS, "MODEL_SILENCE")
    assert UNK == rust_str_const(SCORE_RS, "MODEL_UNKNOWN")


def test_the_reserved_symbols_are_in_every_exported_table(phoneme_table):
    """The host refuses a voice whose table lacks either, so the default must hold both."""
    assert {SIL, UNK} <= phoneme_table


def test_the_voiceless_tables_match_symbol_for_symbol():
    """The claim ``phoneme.rs`` makes about this project, checked.

    The host's list is phonemes only; this project's also carries the special
    symbols, which are not sounds at all and so trivially unvoiced. Comparing
    the sounds is comparing the thing both sides actually decide with — a
    frame's voiced flag, which is read from the phoneme class because f0 is
    written as a contour straight through the consonants.
    """
    host = set(rust_str_array(PHONEME_RS, "VOICELESS"))
    ours = set(VOICELESS) - set(SPECIAL_SYMBOLS)
    assert ours == host, (
        f"only this project calls {sorted(ours - host)} voiceless, and only the host calls "
        f"{sorted(host - ours)} voiceless. A disagreement here silences sung frames or hums "
        "through unsung ones, depending on which way it falls."
    )


def test_the_hosts_vowels_are_in_the_phoneme_table(phoneme_table):
    """What the host stretches to fill a note, this project must be able to embed."""
    vowels = set(rust_str_array(PHONEME_RS, "VOWELS"))
    assert vowels <= phoneme_table, sorted(vowels - phoneme_table)


def test_every_phoneme_the_host_can_produce_is_in_the_phoneme_table(phoneme_table):
    """No lyric may reach a voice as ``<unk>``.

    The host has two Japanese front-ends — the dictionary path, whose OpenJTalk
    names are translated by ``openjtalk.rs``, and the kana walker that needs no
    dictionary installed. Between them they are the whole vocabulary a sung
    lyric arrives in, and a symbol missing from the table here is a syllable the
    model was never trained on and cannot sing.
    """
    produced = rust_ipa_arms(OPENJTALK_RS) | kana_symbols()
    missing = sorted(produced - phoneme_table)
    assert not missing, (
        f"the host can produce {missing}, and this project has no id for them. Add them to "
        "IPA_SYMBOLS — but note that a table that has grown is a table a trained checkpoint no "
        "longer matches, since ids are positional."
    )


def test_the_hosts_two_japanese_paths_share_one_inventory():
    """Dictionary and kana must reach the same symbols, exhaustively.

    The host checks this itself, but by spot-testing a handful of syllables. The
    tables parse cleanly enough to compare whole, and it is worth comparing
    whole: a syllable spelt two ways is two symbols the model has to learn were
    one sound, and only some of them would be caught by a sample.
    """
    dictionary, kana = rust_ipa_arms(OPENJTALK_RS), kana_symbols()
    assert dictionary == kana, (
        f"only the dictionary path produces {sorted(dictionary - kana)}, and only the kana "
        f"path produces {sorted(kana - dictionary)}"
    )


def test_the_duration_table_is_named_and_measured_as_the_host_expects():
    """The consonant widths an export measures, as the host asks for them."""
    fields = rust_struct_fields(METADATA_RS, "VoiceInfo")
    assert METADATA_FIELD in fields, (
        f"an export ships its widths under {METADATA_FIELD!r} and `VoiceInfo` has no such field"
    )

    block = summarize({}, measured_from="the contract test")
    unit = re.search(r'durations\.unit != "([^"]*)"', rust(METADATA_RS))
    assert unit, "the host's unit check has moved; this test cannot see what it accepts"
    assert block["unit"] == unit.group(1), (
        f"this project measures in {block['unit']!r} and the host reads {unit.group(1)!r} — "
        "a table of frames read as seconds is wrong by two orders of magnitude"
    )

    durations = rust_struct_fields(METADATA_RS, "PhonemeDurations")
    required = {field for field, defaulted in durations.items() if not defaulted}
    assert required <= set(block), sorted(required - set(block))


def test_the_ipa_table_holds_no_duplicates():
    """Ids are positional, so a repeat would quietly shift every symbol after it."""
    symbols = list(SPECIAL_SYMBOLS) + list(IPA_SYMBOLS)
    assert len(set(symbols)) == len(symbols)
