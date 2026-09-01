"""Tests for the per-phoneme consonant width table and the script that builds it."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest

from auris_singer.phoneme_durations import (
    DEFAULT_SECONDS,
    METADATA_FIELD,
    STRETCHED,
    measure,
    summarize,
)

SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "measure_phoneme_durations.py"
_spec = importlib.util.spec_from_file_location("measure_phoneme_durations", SCRIPT)
script = importlib.util.module_from_spec(_spec)
sys.modules["measure_phoneme_durations"] = script
_spec.loader.exec_module(script)


def utterance(*spans: tuple[str, float]) -> list[tuple[float, float, str]]:
    """Build ``(start, end, symbol)`` triples from ``(symbol, duration)`` pairs."""
    out, at = [], 0.0
    for symbol, duration in spans:
        out.append((at, at + duration, symbol))
        at += duration
    return out


def test_only_medial_consonants_are_counted():
    # The leading k is phrase-initial: its label span has no closure in it, so
    # counting it would drag the median toward a length that never occurs
    # between two vowels.
    counted = measure([utterance(("<sil>", 0.5), ("k", 0.02), ("a", 0.3), ("k", 0.09), ("a", 0.3))])
    assert counted["k"] == [pytest.approx(0.09)]


def test_stretched_phonemes_get_no_entry():
    counted = measure(
        [utterance(("<sil>", 0.5), ("a", 0.3), ("k", 0.09), ("a", 0.4), ("ɴ", 0.2), ("o", 0.3))]
    )
    assert set(counted) == {"k"}
    assert "ɴ" in STRETCHED and "a" in STRETCHED


def test_devoiced_vowels_are_treated_as_consonants():
    # A whispered vowel between voiceless neighbours takes a slot of its own,
    # so it belongs in the table rather than with the stretching vowels.
    assert "i̥" not in STRETCHED
    counted = measure([utterance(("<sil>", 0.5), ("s", 0.1), ("i̥", 0.03), ("k", 0.09), ("i", 0.3))])
    assert set(counted) == {"i̥", "k"}


def test_pause_breaks_the_medial_chain():
    counted = measure(
        [utterance(("a", 0.3), ("<pau>", 0.4), ("k", 0.09), ("a", 0.3), ("s", 0.1), ("a", 0.3))]
    )
    assert set(counted) == {"s"}, "the k after the pause is phrase-initial again"


def test_zero_length_spans_are_ignored():
    counted = measure([[(0.0, 0.3, "a"), (0.3, 0.3, "k"), (0.3, 0.6, "a")]])
    assert counted == {}


def test_summarize_keeps_only_lengthenings_with_enough_samples():
    block = summarize(
        {
            "s": [0.104] * 100,      # long enough, sampled enough
            "ɾ": [0.036] * 100,      # sampled enough but shorter than the default
            "ts": [0.119] * 10,      # long enough but barely sampled
        },
        measured_from="unit test",
        min_samples=90,
    )
    assert set(block["seconds"]) == {"s"}
    assert block["seconds"]["s"] == pytest.approx(0.104)
    assert block["counts"]["s"] == 100
    assert block["default"] == DEFAULT_SECONDS
    assert block["unit"] == "seconds"
    assert block["measured_from"] == "unit test"


def test_summarize_orders_longest_first():
    block = summarize(
        {"s": [0.104] * 100, "ts": [0.119] * 100, "k": [0.091] * 100},
        measured_from="unit test",
    )
    assert list(block["seconds"]) == ["ts", "s", "k"]


def test_summarize_is_json_round_trippable():
    block = summarize({"s": [0.104] * 100}, measured_from="unit test")
    assert json.loads(json.dumps(block, ensure_ascii=False)) == block


def test_metadata_field_name_is_stable():
    # Consumers key off this string; doc/inference.md documents it.
    assert METADATA_FIELD == "phoneme_durations"


def test_script_reads_mono_labels(tmp_path):
    path = tmp_path / "song.lab"
    path.write_text("0 5000000 pau\n5000000 5900000 k\n5900000 9000000 a\n", encoding="utf-8")
    assert script.read_timed_phonemes(path) == [
        (pytest.approx(0.0), pytest.approx(0.5), "pau"),
        (pytest.approx(0.5), pytest.approx(0.59), "k"),
        (pytest.approx(0.59), pytest.approx(0.9), "a"),
    ]


def test_script_reads_hts_labels(tmp_path):
    path = tmp_path / "song.lab"
    path.write_text(
        "0 5000000 xx^xx-sil+k=a_xx\n5000000 5900000 xx^sil-k+a=i_xx\n",
        encoding="utf-8",
    )
    assert [s for _, _, s in script.read_timed_phonemes(path)] == ["sil", "k"]


def test_script_normalizes_enunu_symbols(tmp_path):
    path = tmp_path / "song.lab"
    path.write_text("0 1000000 GlottalStop\n1000000 2000000 br\n", encoding="utf-8")
    assert [s for _, _, s in script.read_timed_phonemes(path)] == ["cl", "pau"]


def test_script_maps_to_ipa_without_dropping_unknowns():
    mapped = script.to_ipa([(0.0, 0.1, "k"), (0.1, 0.2, "zzz"), (0.2, 0.3, "a")])
    assert [s for _, _, s in mapped] == ["k", "zzz", "a"]


def test_script_end_to_end_writes_the_block(tmp_path, capsys):
    labels = tmp_path / "labels"
    labels.mkdir()
    # 100 intervocalic /s/ of 104 ms each, enough to clear the sample threshold.
    line, at = [], 0
    step = 3_000_000
    line.append(f"0 {step} pau")
    at = step
    for _ in range(100):
        for symbol, ticks in (("a", 3_000_000), ("s", 1_040_000)):
            line.append(f"{at} {at + ticks} {symbol}")
            at += ticks
    line.append(f"{at} {at + step} a")
    (labels / "song.lab").write_text("\n".join(line) + "\n", encoding="utf-8")

    out = tmp_path / "durations.json"
    sys.argv = [
        "measure_phoneme_durations.py",
        "--label-dir", str(labels),
        "--output", str(out),
        "--measured-from", "synthetic",
    ]
    script.main()

    block = json.loads(out.read_text(encoding="utf-8"))
    assert block["seconds"] == {"s": pytest.approx(0.104)}
    assert block["counts"] == {"s": 100}
    assert block["measured_from"] == "synthetic"
    assert "104 ms" in capsys.readouterr().out


def test_script_reports_symbols_outside_the_phoneme_table(tmp_path, capsys):
    labels = tmp_path / "labels"
    labels.mkdir()
    (labels / "song.lab").write_text(
        "0 3000000 pau\n3000000 6000000 a\n6000000 7000000 zzz\n7000000 9000000 a\n",
        encoding="utf-8",
    )
    out = tmp_path / "durations.json"
    sys.argv = [
        "measure_phoneme_durations.py",
        "--label-dir", str(labels), "--output", str(out),
    ]
    script.main()
    assert "zzz" in capsys.readouterr().out
    assert json.loads(out.read_text(encoding="utf-8"))["seconds"] == {}
