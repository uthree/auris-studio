"""The host evaluation: its arithmetic without a host, and the whole run with one.

Everything that can be checked without a Rust toolchain is checked without one. The two
end-to-end tests at the bottom drive a real ``auris`` and are marked ``slow`` and skipped
wherever none can be found, which is CI's training job: pytest must not need cargo.
"""

from __future__ import annotations

import json
import math

import numpy as np
import pytest
import torch

from auris_singer.host import Host
from auris_singer.host_eval import (
    METRICS,
    Analyst,
    Settings,
    evaluate,
    evaluate_score,
    format_report,
    phonemes_of_frames,
    summarize,
    validation_records,
    voice_info,
)


class Parroting:
    """A listener that hears one thing whatever it is played, for runs without a model."""

    def __init__(self, phonemes: list[str]):
        self.phonemes = phonemes
        self.heard = 0

    def hear(self, wav, sample_rate):
        from auris_singer.asr import Heard

        self.heard += 1
        return Heard(text=" ".join(self.phonemes), phonemes=list(self.phonemes))


def test_a_frames_file_spells_the_phonemes_a_listener_is_held_to():
    tokens = ["sil", "sil", "k", "a", "a", "a", "sil", "a", "a", "i̥", "sil"]
    # Runs collapse — the host run-length-encodes them — rests go, devoicing goes.
    assert phonemes_of_frames(tokens) == ["k", "a", "a", "i"]
    assert phonemes_of_frames([]) == []


def test_summary_is_a_mean_over_the_rows_that_answered():
    rows = [
        {"mel_l1": 1.0, "f0_rmse_cent": 10.0, "peak": 0.5},
        {"mel_l1": 3.0, "f0_rmse_cent": float("nan"), "peak": 0.7},
        {"mel_l1": 2.0, "peak": 0.6},
    ]
    summary = summarize(rows)
    assert summary["mel_l1"] == pytest.approx(2.0)
    assert summary["f0_rmse_cent"] == pytest.approx(10.0), "NaN is left out, not counted as 0"
    assert "f0_accuracy" not in summary, "a metric nobody answered is absent, not zero"
    assert list(summary) == [m for m in METRICS if m in summary], "the table's order"


def test_the_validation_split_is_the_data_modules(processed_dataset):
    from auris_singer.data import SingingDataModule

    dm = SingingDataModule(processed_dataset, batch_size=2, num_workers=0, val_size=3, seed=7)
    dm.setup()
    theirs = [r["id"] for r in dm.val_dataset.records]
    ours = [r["id"] for r in validation_records(processed_dataset, seed=7, val_size=3)]
    assert ours == theirs


def test_the_analyst_measures_a_render_against_its_curves():
    sr, hop, n_fft, win = 48_000, 480, 2048, 2048
    n_frames = 50
    t = np.arange(n_frames * hop) / sr
    wav = (0.3 * np.sin(2 * math.pi * 220.0 * t)).astype(np.float32)
    f0 = np.full(n_frames, 220.0, dtype=np.float32)
    voiced = np.ones(n_frames, dtype=np.float32)
    energy = np.full(n_frames, 0.3 / math.sqrt(2), dtype=np.float32)
    analyst = Analyst(sr, n_fft, hop, win, n_mels=32, pitch=False)

    same = analyst.measure(wav, f0, energy, voiced, reference=wav)
    assert same["mel_l1"] == pytest.approx(0.0)
    assert abs(same["energy_bias_db"]) < 0.5, "a sine at the asked RMS sits within half a dB"
    assert same["peak"] == pytest.approx(0.3, abs=1e-3)
    assert "f0_rmse_cent" not in same, "no extractor, no pitch metrics"

    quiet = analyst.measure(wav * 0.5, f0, energy, voiced, reference=wav)
    assert quiet["mel_l1"] > 0.1
    assert quiet["energy_bias_db"] == pytest.approx(-6.02, abs=0.3)

    without = analyst.measure(wav, f0, energy, voiced)
    assert "mel_l1" not in without, "no recording, no spectral distance"

    with pytest.raises(ValueError, match="samples"):
        analyst.measure(wav[: hop * 10], f0, energy, voiced)


def test_the_table_reads_every_column_it_is_given():
    report = {
        "voice": {"path": "v.onnx", "name": "Voice"},
        "summary": {
            "host": {"mel_l1": 0.5, "peak": 0.8},
            "reference": {"mel_l1": 0.4, "peak": 0.9},
            "song": {"mel_l1": 0.55, "peak": 0.8},
            "timing": {
                "audio_seconds": 10.0,
                "render_seconds": 1.0,
                "rtf": 0.1,
                "load_seconds_mean": 0.3,
                "chunks": 3,
                "on_gpu": False,
                "reference_seconds": 2.0,
                "song": {"seconds_of_frames": 12.0, "chunks": 1, "render_seconds": 1.2},
            },
        },
    }
    text = format_report(report)
    assert "mel_l1" in text and "peak" in text
    assert "+0.100" in text, "host − reference"
    assert "+0.050" in text, "song − host"
    assert "10× realtime" in text
    assert "f0_rmse_cent" not in text, "a metric no column holds is not a row"

    baseline = {"summary": {"host": {"mel_l1": 0.6}}}
    with_baseline = format_report(report, baseline)
    assert "-0.100" in with_baseline, "host against the baseline's host"

    # A listener adds a row and the recording's ceiling as a column, and reports its time.
    report["summary"]["host"]["per"] = 0.9
    report["summary"]["recording"] = {"per": 0.1}
    report["summary"]["timing"]["asr_seconds"] = 3.0
    heard = format_report(report)
    assert "recording" in heard and "per" in heard and "listener took 3.00 s" in heard
    # The tally prints under the table for the main column, worst phoneme first.
    report["summary"]["confusions"] = {"host": [["a", "a", 5], ["ɕ", "s", 3], ["k", "", 1]]}
    tallied = format_report(report)
    lines = tallied.split("\n")
    start = next(at for at, line in enumerate(lines) if line.startswith("what the listener heard, host column"))
    assert [line.split()[0] for line in lines[start + 2 : start + 5]] == ["ɕ", "k", "a"], "sorted by errors"

    score = {
        "voice": {"path": "v.onnx", "name": ""},
        "summary": {
            "score": {"energy_bias_db": -1.0},
            "timing": {"audio_seconds": 4.0, "wall_seconds": 2.0, "wall_rtf": 0.5},
        },
    }
    assert "wall RTF" in format_report(score)


# ----------------------------------------------------------------------------------------------
# with a host
# ----------------------------------------------------------------------------------------------
needs_host = pytest.mark.skipif(not Host.available(), reason="no `auris` to drive")


@pytest.fixture
def exported_voice(tmp_path, tiny_model_config, tiny_discriminator_config, processed_dataset):
    """A one-step checkpoint and its ONNX export, the pair a real run has."""
    pytest.importorskip("onnxruntime")
    import lightning as L

    from auris_singer.data import SingingDataModule
    from auris_singer.export import export_onnx
    from auris_singer.lightning_module import AurisSingerModule

    dm = SingingDataModule(
        processed_dataset, batch_size=2, num_workers=0, val_size=2,
        bucket_boundaries=[0, 200], pin_memory=False,
    )
    torch.manual_seed(0)
    module = AurisSingerModule(
        model=tiny_model_config,
        discriminator=tiny_discriminator_config,
        audio={"sample_rate": 48_000, "n_fft": 2048, "hop_length": 480, "win_length": 2048, "n_mels": 80},
        loss={"mel_params": [[512, 120, 512, 40]], "envelope_kernel_sizes": [128, 256]},
        optimizer={"learning_rate": 1e-4},
        metadata={"symbols": dm.phoneme_table.symbols, "speaker_to_id": dm.speaker_to_id, "audio": dm.audio_config},
    )
    trainer = L.Trainer(
        max_steps=1, accelerator="cpu", devices=1, logger=False, enable_checkpointing=False,
        enable_progress_bar=False, num_sanity_val_steps=0, limit_val_batches=0,
        use_distributed_sampler=False,
    )
    trainer.fit(module, datamodule=dm)
    checkpoint = tmp_path / "tiny.ckpt"
    trainer.save_checkpoint(checkpoint)
    voice = tmp_path / "tiny.onnx"
    export_onnx(module.model, voice, metadata=dict(module.hparams["metadata"]), voice={"name": "Tiny"})
    return voice, checkpoint


@needs_host
@pytest.mark.slow
def test_the_host_sings_the_corpus_and_every_column_is_measured(exported_voice, processed_dataset, tmp_path):
    voice, checkpoint = exported_voice
    info = voice_info(voice)
    assert info.name == "Tiny" and info.hop_seconds == pytest.approx(0.01)

    # `all` rather than `val`: twelve synthetic utterances make a one-utterance validation
    # split, and the song column needs two.
    settings = Settings(split="all", utterances=2, pitch=False, song_gap_seconds=0.2, take_seeds=2)
    listener = Parroting(["a", "i", "k", "o"])
    report = evaluate(
        voice, processed_dataset, checkpoint, Host.find(), tmp_path / "work", settings, listener=listener
    )

    assert report["kind"] == "corpus"
    assert len(report["utterances"]) == 2
    for row in report["utterances"]:
        for column in ("host", "reference", "song"):
            assert math.isfinite(row[column]["mel_l1"]), (column, row)
            assert row[column]["peak"] < 1.0
        facts = row["timing"]
        assert facts["frames"] == row["n_frames"]
        assert facts["chunks"] >= 1
        assert facts["seconds"] == pytest.approx(row["seconds"] * facts["takes"], abs=0.011), "added over takes"
        assert facts["wall_seconds"] >= facts["render_seconds"] > 0
    summary = report["summary"]
    assert set(summary) >= {"host", "reference", "song", "recording", "timing"}
    # The listener heard every take of every column of every utterance, and the recording.
    assert listener.heard == 2 * (3 * 2 + 1)
    assert report["utterances"][0]["seeds"] == [0, 1]
    assert report["summary"]["timing"]["takes"] == 2
    assert report["utterances"][0]["timing"]["takes"] == 2
    for row in report["utterances"]:
        assert row["asked"].startswith("a i k o"), "the synthetic transcript, made hearable"
        for column in ("host", "reference", "song", "recording"):
            assert math.isfinite(row[column]["per"]) and row[column]["heard"] == "a i k o"
    assert 0 <= summary["recording"]["per"] < 1
    assert set(summary["confusions"]) == {"host", "reference", "song", "recording"}
    assert sum(n for _, _, n in summary["confusions"]["host"]) >= 2 * 2 * 4, "every take, every phoneme"
    assert summary["timing"]["asr_seconds"] >= 0
    assert summary["timing"]["song"]["seconds_of_frames"] > sum(r["seconds"] for r in report["utterances"])
    assert 0 < summary["timing"]["rtf"] < 1000

    # Every file that crossed the boundary is kept, and the table reads.
    kept = {p.name for p in (tmp_path / "work").iterdir()}
    assert "song.frames.json" in kept and "song.s0.host.wav" in kept and "song.s1.host.wav" in kept
    assert any(name.endswith(".s1.host.wav") and not name.startswith("song") for name in kept)
    text = format_report(report)
    assert "mel_l1" in text and "realtime" in text
    json.dumps(report)  # the report is plain data


@needs_host
@pytest.mark.slow
def test_the_host_sings_a_score_the_way_a_person_would(exported_voice, tmp_path):
    voice, _ = exported_voice
    settings = Settings(pitch=False)
    listener = Parroting(["s", "a", "k", "ɯ", "ɾ", "a"])
    report = evaluate_score(voice, Host.find(), tmp_path / "score", settings=settings, listener=listener)

    assert report["kind"] == "score"
    assert report["frames"]["count"] > 0
    assert report["frames"]["inventory"][0] == "sil"
    assert (tmp_path / "score" / "score.asong").is_file(), "the built-in verse was written out"
    take = report["take"]
    assert take.endswith(".wav") and "Audio" in take
    metrics = report["score"]
    assert "mel_l1" not in metrics, "nothing to hold a spectral distance against"
    assert math.isfinite(metrics["energy_rmse_db"])
    assert report["summary"]["timing"]["wall_seconds"] > 0
    assert listener.heard == 1 and math.isfinite(metrics["per"])
    assert report["asked"].startswith("s a k ɯ ɾ a"), "さくら, as the frames spelt it"
    assert report["summary"]["confusions"]["score"][0][2] >= 1, "the tally rode along"
    assert "wall RTF" in format_report(report)


def test_a_log_line_carries_the_numbers_and_not_what_was_heard():
    from auris_singer.host_eval import _one_line

    line = _one_line({"mel_l1": 0.5, "per": 1.0, "heard": "パンがパーンがたい", "asr_seconds": 0.4})
    assert line == "mel_l1=0.500  per=1.000  asr_seconds=0.400"


def test_several_takes_average_into_one_row():
    from auris_singer.host_eval import average_rows, sum_facts

    rows = [
        {"mel_l1": 1.0, "per": 0.5, "heard": "first", "f0_rmse_cent": 10.0},
        {"mel_l1": 3.0, "per": 0.7, "heard": "second", "f0_rmse_cent": float("nan")},
    ]
    row = average_rows(rows)
    assert row["mel_l1"] == pytest.approx(2.0) and row["per"] == pytest.approx(0.6)
    assert row["f0_rmse_cent"] == pytest.approx(10.0), "NaN is left out of the mean"
    assert row["heard"] == "first"
    facts = sum_facts([
        {"seconds": 1.0, "chunks": 1, "render_seconds": 0.5, "load_seconds": 0.3, "wall_seconds": 2.0, "on_gpu": True, "sample_rate": 48_000, "frames": 100},
        {"seconds": 1.0, "chunks": 1, "render_seconds": 0.4, "load_seconds": 0.0, "wall_seconds": 1.5, "on_gpu": False, "sample_rate": 48_000, "frames": 100},
    ])
    assert facts["takes"] == 2 and facts["seconds"] == 2.0 and facts["render_seconds"] == pytest.approx(0.9)
    assert facts["on_gpu"] is False and facts["frames"] == 100
