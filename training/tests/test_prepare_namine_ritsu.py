"""Tests for the Namine Ritsu database preparation script."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

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
    path.write_text("garbage\nstart 1000000 i\n0 1000000 a\n1 2\n", encoding="utf-8")
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


def test_the_script_writes_labels_beside_every_phrase(tmp_path, monkeypatch):
    """A phrase's ``dur/`` line has one number per transcript token and sums to the clip."""
    import sys

    import numpy as np
    import soundfile as sf

    song = tmp_path / "DATABASE" / "song1"
    song.mkdir(parents=True)
    sr = 44_100
    # pau a k a pau, three seconds, at 100 ns label units.
    labels = [
        (0, 5_000_000, "pau"),
        (5_000_000, 12_000_000, "a"),
        (12_000_000, 13_000_000, "k"),
        (13_000_000, 25_000_000, "a"),
        (25_000_000, 30_000_000, "pau"),
    ]
    (song / "song1.lab").write_text("\n".join(f"{a} {b} {p}" for a, b, p in labels) + "\n")
    t = np.arange(int(3.0 * sr)) / sr
    sf.write(song / "song1.wav", (0.3 * np.sin(2 * np.pi * 220 * t)).astype(np.float32), sr)
    out = tmp_path / "out"
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "prepare_namine_ritsu.py",
            "--db-dir",
            str(song.parent),
            "--output",
            str(out),
            "--min-seconds",
            "1",
            "--min-clip-seconds",
            "0.5",
        ],
    )
    prepare.main()
    paths = sorted((out / "wav").rglob("*.wav"))
    assert paths, "one phrase at least"
    for path in paths:
        relative = path.relative_to(out / "wav").with_suffix(".txt")
        tokens = (out / "text" / relative).read_text(encoding="utf-8").split()
        seconds = [float(x) for x in (out / "dur" / relative).read_text(encoding="utf-8").split()]
        assert len(seconds) == len(tokens), (tokens, seconds)
        clip = sf.info(path).duration
        assert sum(seconds) == pytest.approx(clip, abs=0.002)

    with pytest.raises(FileExistsError, match="output already exists"):
        prepare.main()


def test_equal_wav_stems_in_separate_song_folders_do_not_overwrite(tmp_path, monkeypatch):
    import sys

    import numpy as np
    import soundfile as sf

    database = tmp_path / "DATABASE"
    labels = [
        (0, 5_000_000, "pau"),
        (5_000_000, 25_000_000, "a"),
        (25_000_000, 30_000_000, "pau"),
    ]
    sr = 44_100
    t = np.arange(int(3.0 * sr)) / sr
    for folder in ("first", "second"):
        song = database / folder
        song.mkdir(parents=True)
        (song / "song.lab").write_text(
            "\n".join(f"{start} {end} {symbol}" for start, end, symbol in labels) + "\n"
        )
        sf.write(
            song / "song.wav",
            (0.3 * np.sin(2 * np.pi * 220 * t)).astype(np.float32),
            sr,
        )

    output = tmp_path / "out"
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "prepare_namine_ritsu.py",
            "--db-dir",
            str(database),
            "--output",
            str(output),
            "--min-seconds",
            "1",
            "--min-clip-seconds",
            "0.5",
        ],
    )
    prepare.main()

    clips = sorted((output / "wav").rglob("song_000.wav"))
    assert len(clips) == 2
    assert {clip.parent.name for clip in clips} == {"first", "second"}


def test_missing_database_directory_is_refused_before_output_is_reserved(tmp_path, monkeypatch):
    output = tmp_path / "out"
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "prepare_namine_ritsu.py",
            "--db-dir",
            str(tmp_path / "missing"),
            "--output",
            str(output),
        ],
    )

    with pytest.raises(SystemExit, match="database directory does not exist"):
        prepare.main()
    assert not output.exists()


def test_all_missing_labels_fail_and_keep_the_reserved_output(tmp_path, monkeypatch):
    database = tmp_path / "DATABASE"
    song = database / "song"
    song.mkdir(parents=True)
    # Missing labels are rejected before audio decoding.
    (song / "song.wav").write_bytes(b"stand-in")
    output = tmp_path / "out"
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "prepare_namine_ritsu.py",
            "--db-dir",
            str(database),
            "--output",
            str(output),
        ],
    )

    with pytest.raises(SystemExit, match="no phrases were prepared"):
        prepare.main()
    assert output.is_dir(), "failed preparation remains available for inspection"
