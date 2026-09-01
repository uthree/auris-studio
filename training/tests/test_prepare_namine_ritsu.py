"""Tests for the Namine Ritsu database preparation script."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "prepare_namine_ritsu.py"
_spec = importlib.util.spec_from_file_location("prepare_namine_ritsu", SCRIPT)
prepare = importlib.util.module_from_spec(_spec)
sys.modules["prepare_namine_ritsu"] = prepare
_spec.loader.exec_module(prepare)


def test_read_mono_label_parses_times_and_symbols(tmp_path):
    path = tmp_path / "song.lab"
    path.write_text(
        "0 10000000 pau\n10000000 15000000 k\n15000000 30000000 a\n",
        encoding="utf-8",
    )
    phonemes = prepare.read_mono_label(path)
    assert [p.symbol for p in phonemes] == ["pau", "k", "a"]
    assert phonemes[0].start == 0.0
    assert phonemes[0].end == 1.0
    assert phonemes[2].duration == 1.5


def test_enunu_extensions_are_normalized(tmp_path):
    path = tmp_path / "song.lab"
    path.write_text(
        "0 1000000 GlottalStop\n1000000 2000000 a\n"
        "2000000 3000000 br\n3000000 4000000 Edge\n4000000 5000000 i\n",
        encoding="utf-8",
    )
    symbols = [p.symbol for p in prepare.read_mono_label(path)]
    assert symbols == ["cl", "a", "pau", "cl", "i"]


def test_malformed_lines_are_skipped(tmp_path):
    path = tmp_path / "song.lab"
    path.write_text("garbage\n0 1000000 a\n1 2\n", encoding="utf-8")
    assert [p.symbol for p in prepare.read_mono_label(path)] == ["a"]


def test_every_ritsu_label_symbol_is_mapped():
    """Every symbol the database's own inventory lists must survive into IPA.

    The inventory below is the label list from the Ver2 database's
    ``output.txt`` (plus the ENUNU extensions); a symbol that neither the
    normalization nor the OpenJTalk mapping knows would silently disappear
    from transcripts.
    """
    from auris_singer.text.japanese import OPENJTALK_TO_IPA

    inventory = (
        "a i u e o N A I U E O pau sil br GlottalStop Edge cl "
        "k s sh t ch ts n h f m y r w g z d j b p "
        "ky ty ny hy my ry gy by py dy fy"
    ).split()
    for symbol in inventory:
        normalized = prepare.ENUNU_TO_OPENJTALK.get(symbol, symbol)
        assert normalized in OPENJTALK_TO_IPA, symbol
