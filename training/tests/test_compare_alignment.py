"""The alignment comparison's arithmetic, without a checkpoint."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "compare_alignment.py"
_spec = importlib.util.spec_from_file_location("compare_alignment", SCRIPT)
compare = importlib.util.module_from_spec(_spec)
sys.modules["compare_alignment"] = compare
_spec.loader.exec_module(compare)


def test_label_reader_stays_on_the_generation_the_corpus_opened(tmp_path):
    from auris_singer.dataset_layout import CURRENT_FILE, GENERATIONS_DIR, resolve_dataset_root

    root = tmp_path / "processed"
    old = root / GENERATIONS_DIR / "old"
    new = root / GENERATIONS_DIR / "new"
    old.mkdir(parents=True)
    new.mkdir()
    (old / "metadata.jsonl").write_text(
        json.dumps({"id": "old", "has_durations": True}) + "\n", encoding="utf-8"
    )
    (new / "metadata.jsonl").write_text(
        json.dumps({"id": "new", "has_durations": True}) + "\n", encoding="utf-8"
    )
    (root / CURRENT_FILE).write_text("old\n", encoding="utf-8")
    corpus = SimpleNamespace(root=resolve_dataset_root(root))

    # A writer publishes after Corpus resolved CURRENT but before labels are read.
    (root / CURRENT_FILE).write_text("new\n", encoding="utf-8")

    assert [record["id"] for record in compare._labelled_records(corpus)] == ["old"]


def test_alignment_comparison_rejects_a_checkpoint_on_another_clock():
    corpus = SimpleNamespace(sample_rate=44_100, hop_length=441)
    aligner = SimpleNamespace(synthesizer=SimpleNamespace(sample_rate=48_000, hop_length=480))

    with pytest.raises(ValueError, match="do not share a clock"):
        compare._require_matching_clock(corpus, aligner)


def test_the_table_reads_each_class_against_its_labels():
    rows = [
        ("a", 30.0, 28),
        ("a", 40.0, 41),
        ("s", 16.0, 12),
        ("s", 18.0, 2),
        ("k", 14.0, 15),
        ("<sil>", 20.0, 20),
    ]
    table = {row["class"]: row for row in compare.alignment_table(rows)}
    assert set(table) == {"vowel", "sibilant", "plosive", "special"}
    assert table["vowel"]["count"] == 2 and table["vowel"]["ratio"] == pytest.approx(69 / 70)
    assert table["sibilant"]["searched_at_most_two"] == pytest.approx(0.5), (
        "one ɕ in two got two frames"
    )
    assert table["sibilant"]["ratio"] == pytest.approx(14 / 34)
    assert table["sibilant"]["mean_abs_diff"] == pytest.approx((4 + 16) / 2)
    assert table["special"]["mean_abs_diff"] == 0.0
    text = compare.format_table(compare.alignment_table(rows))
    assert "sibilant" in text and "|diff|" in text
    assert [r["class"] for r in compare.alignment_table(rows)][0] in {"vowel", "sibilant"}, (
        "largest class first"
    )


def test_the_symbol_table_keeps_the_obstruents_that_occur_enough():
    rows = (
        [("s", 16.0, 12)] * 3 + [("ɕ", 17.0, 2)] * 2 + [("a", 30.0, 30)] * 5 + [("k", 14.0, 15)] * 2
    )
    table = {row["class"]: row for row in compare.symbol_table(rows, at_least=2)}
    assert set(table) == {"s", "ɕ", "k"}, "vowels are not obstruents; each obstruent is its own row"
    assert table["ɕ"]["searched_at_most_two"] == 1.0
    assert compare.symbol_table(rows, at_least=3) and set(
        r["class"] for r in compare.symbol_table(rows, at_least=3)
    ) == {"s"}
