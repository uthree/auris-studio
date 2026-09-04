"""End-to-end test of the preprocessing pipeline.

The ``ipa`` front-end is used so the test does not need the jpreprocess
dictionary; f0 extraction runs the real FCPE model on CPU.
"""

from __future__ import annotations

import json
import math

import numpy as np
import pytest
import soundfile as sf
import torch
from omegaconf import OmegaConf

from auris_singer.preprocess import collect_utterances, run_preprocess

SAMPLE_RATE = 48_000
HOP = 480


def write_corpus(root, n_utterances=3, source_rate=44_100):
    """Write wav/text pairs at a rate that forces resampling."""
    wav_dir = root / "raw" / "singer" / "wav"
    text_dir = root / "raw" / "singer" / "text"
    wav_dir.mkdir(parents=True)
    text_dir.mkdir(parents=True)

    for i in range(n_utterances):
        seconds = 1.0 + 0.5 * i
        t = np.arange(int(seconds * source_rate)) / source_rate
        f0 = 200.0 + 30.0 * i
        wav = sum(np.sin(2 * math.pi * f0 * k * t) / k for k in range(1, 6))
        wav = (wav / np.abs(wav).max() * 0.5).astype(np.float32)
        sf.write(wav_dir / f"utt{i}.wav", wav, source_rate)
        (text_dir / f"utt{i}.txt").write_text("k o ɴ ɲ i tɕ i w a", encoding="utf-8")
    return wav_dir, text_dir


def build_config(root, out_dir):
    return OmegaConf.create(
        {
            "audio": {
                "sample_rate": SAMPLE_RATE,
                "n_fft": 2048,
                "hop_length": HOP,
                "win_length": 2048,
                "peak_normalize": True,
                "peak": 0.95,
                "min_seconds": 0.2,
                "max_seconds": 20.0,
            },
            "f0": {"device": "cpu", "f0_min": 40.0, "f0_max": 1600.0},
            "text": {"language": "ipa", "phoneme_table": None},
            "dataset": {
                "output_dir": str(out_dir),
                "sources": [
                    {
                        "name": "singer",
                        "wav_dir": str(root / "raw" / "singer" / "wav"),
                        "text_dir": str(root / "raw" / "singer" / "text"),
                    }
                ],
            },
            "num_workers": 2,
        }
    )


def test_collect_utterances_pairs_wav_and_text(tmp_path):
    write_corpus(tmp_path, n_utterances=2)
    utterances = collect_utterances(
        [
            {
                "name": "singer",
                "wav_dir": str(tmp_path / "raw" / "singer" / "wav"),
                "text_dir": str(tmp_path / "raw" / "singer" / "text"),
            }
        ]
    )
    assert len(utterances) == 2
    assert all(u.text_path is not None for u in utterances)
    assert all(u.speaker == "singer" for u in utterances)


def test_missing_wav_dir_is_reported(tmp_path):
    with pytest.raises(FileNotFoundError, match="wav_dir"):
        collect_utterances([{"name": "x", "wav_dir": str(tmp_path / "nope")}])


@pytest.mark.slow
def test_pipeline_writes_a_loadable_dataset(tmp_path):
    write_corpus(tmp_path)
    out_dir = tmp_path / "processed"
    summary = run_preprocess(build_config(tmp_path, out_dir))
    assert summary["processed"] == 3

    records = [json.loads(line) for line in (out_dir / "metadata.jsonl").read_text().splitlines()]
    assert len(records) == 3
    assert json.loads((out_dir / "speakers.json").read_text()) == {"singer": 0}
    assert json.loads((out_dir / "audio_config.json").read_text())["hop_length"] == HOP

    for record in records:
        with np.load(out_dir / record["path"]) as data:
            n_frames = record["n_frames"]
            # Every frame-level feature lives on the same grid as the waveform.
            assert data["wav"].shape == (n_frames * HOP,)
            assert data["f0"].shape == (n_frames,)
            assert data["energy"].shape == (n_frames,)
            assert data["voiced"].shape == (n_frames,)
            assert data["phonemes"].shape == (record["n_phonemes"],)
            assert data["wav"].dtype == np.int16
            # A harmonic tone must be detected as voiced with a plausible pitch.
            voiced = data["voiced"].astype(bool)
            assert voiced.mean() > 0.5
            assert 150.0 < float(data["f0"][voiced].mean()) < 350.0
            # f0 is exactly zero wherever the frame is unvoiced.
            assert np.all(data["f0"][~voiced] == 0.0)


@pytest.mark.slow
def test_pipeline_output_loads_into_the_dataset(tmp_path):
    from auris_singer.data import SingingDataset, collate_batch

    write_corpus(tmp_path)
    out_dir = tmp_path / "processed"
    run_preprocess(build_config(tmp_path, out_dir))

    dataset = SingingDataset(out_dir, min_frames=1, max_frames=10_000)
    batch = collate_batch([dataset[i] for i in range(len(dataset))])
    assert batch["spec"].shape[1] == 1025
    assert torch.isfinite(batch["spec"]).all()


def test_utterances_without_a_transcript_are_skipped(tmp_path):
    write_corpus(tmp_path, n_utterances=2)
    (tmp_path / "raw" / "singer" / "text" / "utt0.txt").unlink()
    utterances = collect_utterances(
        [
            {
                "name": "singer",
                "wav_dir": str(tmp_path / "raw" / "singer" / "wav"),
                "text_dir": str(tmp_path / "raw" / "singer" / "text"),
            }
        ]
    )
    assert sum(u.text_path is None for u in utterances) == 1


@pytest.mark.parametrize("broken", ["transcript", "audio"])
def test_decode_errors_skip_only_the_broken_utterance(tmp_path, monkeypatch, broken):
    """A bad corpus file must not orphan features already written for good files."""
    write_corpus(tmp_path, n_utterances=2)
    if broken == "transcript":
        (tmp_path / "raw" / "singer" / "text" / "utt1.txt").write_bytes(b"\xff\xfe")
    else:
        (tmp_path / "raw" / "singer" / "wav" / "utt1.wav").write_bytes(b"not a wav")

    class FakeExtractor:
        def __init__(self, **_kwargs):
            pass

        def __call__(self, _wav, _sample_rate, n_frames):
            return torch.zeros(n_frames), torch.zeros(n_frames, dtype=torch.bool)

    monkeypatch.setattr("auris_singer.preprocess.pipeline.FcpeExtractor", FakeExtractor)
    out_dir = tmp_path / "processed"
    summary = run_preprocess(build_config(tmp_path, out_dir))

    assert summary["processed"] == 1
    assert summary["skipped"] == 1
    assert sum(value for key, value in summary.items() if "decode error" in key) == 1
    records = [json.loads(line) for line in (out_dir / "metadata.jsonl").read_text().splitlines()]
    assert [record["id"] for record in records] == ["singer/utt0"]
    assert (out_dir / records[0]["path"]).is_file()


def test_labelled_seconds_become_frames_that_sum_exactly():
    from auris_singer.preprocess.pipeline import seconds_to_frames

    assert seconds_to_frames([0.1, 0.2, 0.7], 100) == [10, 20, 70]
    frames = seconds_to_frames([0.333, 0.333, 0.334], 100)
    assert sum(frames) == 100 and min(frames) >= 1
    # The rounding residue lands on the longest phoneme, and nothing goes below one frame.
    frames = seconds_to_frames([0.001, 0.001, 0.998], 10)
    assert frames == [1, 1, 8]
    with pytest.raises(ValueError, match="cannot share"):
        seconds_to_frames([0.5, 0.5], 1)
    with pytest.raises(ValueError, match="nothing"):
        seconds_to_frames([0.0, 0.0], 10)


def test_a_source_with_labels_stores_frames_per_phoneme(tmp_path):
    write_corpus(tmp_path, n_utterances=2)
    dur_dir = tmp_path / "raw" / "singer" / "dur"
    dur_dir.mkdir()
    for text in (tmp_path / "raw" / "singer" / "text").glob("*.txt"):
        tokens = text.read_text(encoding="utf-8").split()
        (dur_dir / text.name).write_text(" ".join("0.1" for _ in tokens), encoding="utf-8")
    # One label that does not line up: the utterance is skipped, not guessed at.
    first = sorted(dur_dir.glob("*.txt"))[0]
    first.write_text("0.1 0.1", encoding="utf-8")

    config = build_config(tmp_path, tmp_path / "processed")
    config.dataset.sources[0]["duration_dir"] = str(dur_dir)
    config.f0.device = "cpu"
    summary = run_preprocess(config)
    assert summary["processed"] == 1 and summary["skipped"] == 1
    records = [json.loads(line) for line in (tmp_path / "processed" / "metadata.jsonl").read_text().splitlines()]
    assert records[0]["has_durations"] is True
    with np.load(tmp_path / "processed" / records[0]["path"]) as data:
        assert data["durations"].sum() == records[0]["n_frames"]
        assert data["durations"].shape == (records[0]["n_phonemes"],)

    from auris_singer.data import SingingDataset, collate_batch

    dataset = SingingDataset(tmp_path / "processed", min_frames=1, max_frames=10_000)
    batch = collate_batch([dataset[0]])
    assert batch["durations"].shape == (1, records[0]["n_phonemes"])
    assert int(batch["durations"].sum()) == records[0]["n_frames"]
    without = SingingDataset(tmp_path / "processed", min_frames=1, max_frames=10_000, use_durations=False)
    assert "durations" not in collate_batch([without[0]])
