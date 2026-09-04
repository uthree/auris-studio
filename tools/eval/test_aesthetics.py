from pathlib import Path

from aesthetics import score_labels


def test_same_named_wavs_keep_distinct_score_keys(tmp_path: Path) -> None:
    first = tmp_path / "take-a" / "mix.wav"
    second = tmp_path / "take-b" / "mix.wav"

    labels = score_labels([first, second])

    assert len(set(labels)) == 2
    assert all(label.endswith("mix.wav") for label in labels)


def test_unique_wavs_keep_the_compact_stem_label() -> None:
    assert score_labels([Path("render.wav")]) == ["render"]
