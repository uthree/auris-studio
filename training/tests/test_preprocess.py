"""End-to-end test of the preprocessing pipeline.

The ``ipa`` front-end is used so the test does not need the jpreprocess
dictionary; f0 extraction runs the real FCPE model on CPU.
"""

from __future__ import annotations

import json
import math
import threading
from pathlib import Path

import numpy as np
import pytest
import soundfile as sf
import torch
from omegaconf import OmegaConf

from auris_singer.dataset_layout import resolve_dataset_root
from auris_singer.preprocess import collect_utterances, run_preprocess
from auris_singer.preprocess.pipeline import seconds_to_frames

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


def test_duration_scaling_rejects_a_non_finite_sum_before_rounding():
    with pytest.raises(ValueError, match="finite sum"):
        seconds_to_frames([1e308, 1e308, 1.0], 3)


def test_staging_map_keeps_only_a_bounded_number_of_results_in_flight():
    from auris_singer.preprocess.pipeline import _bounded_map

    class Future:
        def __init__(self, owner, value):
            self.owner = owner
            self.value = value

        def result(self):
            self.owner.pending -= 1
            return self.value

    class Executor:
        pending = 0
        peak = 0

        def submit(self, function, value):
            self.pending += 1
            self.peak = max(self.peak, self.pending)
            return Future(self, function(value))

    executor = Executor()
    assert list(_bounded_map(executor, lambda value: value * 2, range(100), 3)) == [
        value * 2 for value in range(100)
    ]
    assert executor.peak == 3


def test_audio_decode_reads_only_the_configured_prefix(tmp_path, monkeypatch):
    from auris_singer.preprocess import pipeline

    observed = []

    class Audio:
        samplerate = SAMPLE_RATE
        frames = SAMPLE_RATE * 60

        def __enter__(self):
            return self

        def __exit__(self, *_args):
            return False

        def read(self, *, frames, dtype, always_2d):
            observed.append((frames, dtype, always_2d))
            return np.zeros((frames, 2), dtype=np.float32)

    monkeypatch.setattr(pipeline.sf, "SoundFile", lambda _path: Audio())
    wav = pipeline._load_audio(
        tmp_path / "huge.wav",
        SAMPLE_RATE,
        peak_normalize=False,
        peak=0.95,
        max_samples=HOP,
    )

    assert wav.shape == (HOP,)
    assert observed == [(HOP, "float32", True)]


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


def test_source_name_cannot_escape_the_output_directory(tmp_path):
    wav_dir, _ = write_corpus(tmp_path, n_utterances=1)
    with pytest.raises(ValueError, match="one path component"):
        collect_utterances([{"name": "../outside", "wav_dir": str(wav_dir)}])


@pytest.mark.slow
def test_pipeline_writes_a_loadable_dataset(tmp_path):
    write_corpus(tmp_path)
    out_dir = tmp_path / "processed"
    summary = run_preprocess(build_config(tmp_path, out_dir))
    assert summary["processed"] == 3

    generation = resolve_dataset_root(out_dir)
    records = [
        json.loads(line) for line in (generation / "metadata.jsonl").read_text().splitlines()
    ]
    assert len(records) == 3
    assert json.loads((generation / "speakers.json").read_text()) == {"singer": 0}
    assert json.loads((generation / "audio_config.json").read_text())["hop_length"] == HOP

    for record in records:
        with np.load(generation / record["path"]) as data:
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
    generation = resolve_dataset_root(out_dir)
    records = [
        json.loads(line) for line in (generation / "metadata.jsonl").read_text().splitlines()
    ]
    assert [record["id"] for record in records] == ["singer/0000/utt0"]
    assert (generation / records[0]["path"]).is_file()


def test_audio_too_short_for_reflect_padding_is_skipped(tmp_path, monkeypatch):
    """A custom zero minimum must not let reflect padding abort preprocessing."""
    wav_dir, text_dir = write_corpus(tmp_path, n_utterances=2, source_rate=SAMPLE_RATE)
    sf.write(wav_dir / "utt1.wav", np.zeros(600, dtype=np.float32), SAMPLE_RATE)
    (text_dir / "utt1.txt").write_text("a", encoding="utf-8")

    class FakeExtractor:
        def __init__(self, **_kwargs):
            pass

        def __call__(self, _wav, _sample_rate, n_frames):
            return torch.zeros(n_frames), torch.zeros(n_frames, dtype=torch.bool)

    monkeypatch.setattr("auris_singer.preprocess.pipeline.FcpeExtractor", FakeExtractor)
    out_dir = tmp_path / "processed"
    config = build_config(tmp_path, out_dir)
    config.audio.min_seconds = 0.0

    summary = run_preprocess(config)

    assert summary["processed"] == 1
    assert summary["too short"] == 1


def test_zero_length_audio_is_skipped(tmp_path, monkeypatch):
    wav_dir, text_dir = write_corpus(tmp_path, n_utterances=2, source_rate=SAMPLE_RATE)
    sf.write(wav_dir / "utt1.wav", np.zeros(0, dtype=np.float32), SAMPLE_RATE)
    (text_dir / "utt1.txt").write_text("a", encoding="utf-8")

    class FakeExtractor:
        def __init__(self, **_kwargs):
            pass

        def __call__(self, _wav, _sample_rate, n_frames):
            return torch.zeros(n_frames), torch.zeros(n_frames, dtype=torch.bool)

    monkeypatch.setattr("auris_singer.preprocess.pipeline.FcpeExtractor", FakeExtractor)
    summary = run_preprocess(build_config(tmp_path, tmp_path / "processed"))
    assert summary["processed"] == 1
    assert summary["too short"] == 1


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
    with pytest.raises(ValueError, match="finite"):
        seconds_to_frames([0.1, float("nan")], 10)
    with pytest.raises(ValueError, match="non-negative"):
        seconds_to_frames([0.1, -0.1], 10)


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
    generation = resolve_dataset_root(tmp_path / "processed")
    records = [
        json.loads(line) for line in (generation / "metadata.jsonl").read_text().splitlines()
    ]
    assert records[0]["has_durations"] is True
    with np.load(generation / records[0]["path"]) as data:
        assert data["durations"].sum() == records[0]["n_frames"]
        assert data["durations"].shape == (records[0]["n_phonemes"],)

    from auris_singer.data import SingingDataset, collate_batch

    dataset = SingingDataset(tmp_path / "processed", min_frames=1, max_frames=10_000)
    batch = collate_batch([dataset[0]])
    assert batch["durations"].shape == (1, records[0]["n_phonemes"])
    assert int(batch["durations"].sum()) == records[0]["n_frames"]
    without = SingingDataset(
        tmp_path / "processed", min_frames=1, max_frames=10_000, use_durations=False
    )
    assert "durations" not in collate_batch([without[0]])


def test_same_named_sources_publish_distinct_samples(tmp_path, monkeypatch):
    """Two roots may extend one speaker without overwriting equal filenames."""
    first = tmp_path / "first"
    second = tmp_path / "second"
    first_wav, first_text = write_corpus(first, n_utterances=1, source_rate=SAMPLE_RATE)
    second_wav, second_text = write_corpus(second, n_utterances=1, source_rate=SAMPLE_RATE)

    class FakeExtractor:
        def __init__(self, **_kwargs):
            pass

        def __call__(self, _wav, _sample_rate, n_frames):
            return torch.zeros(n_frames), torch.zeros(n_frames, dtype=torch.bool)

    monkeypatch.setattr("auris_singer.preprocess.pipeline.FcpeExtractor", FakeExtractor)
    output = tmp_path / "processed"
    config = build_config(first, output)
    config.dataset.sources = [
        {"name": "singer", "wav_dir": str(first_wav), "text_dir": str(first_text)},
        {"name": "singer", "wav_dir": str(second_wav), "text_dir": str(second_text)},
    ]

    summary = run_preprocess(config)

    generation = resolve_dataset_root(output)
    records = [
        json.loads(line) for line in (generation / "metadata.jsonl").read_text().splitlines()
    ]
    assert summary["processed"] == 2
    assert len({record["path"] for record in records}) == 2
    assert len({record["id"] for record in records}) == 2
    assert all((generation / record["path"]).is_file() for record in records)


def test_failed_preprocess_keeps_the_previous_generation_current(tmp_path, monkeypatch):
    write_corpus(tmp_path, n_utterances=2, source_rate=SAMPLE_RATE)

    class FakeExtractor:
        def __init__(self, **_kwargs):
            pass

        def __call__(self, _wav, _sample_rate, n_frames):
            return torch.zeros(n_frames), torch.zeros(n_frames, dtype=torch.bool)

    monkeypatch.setattr("auris_singer.preprocess.pipeline.FcpeExtractor", FakeExtractor)
    output = tmp_path / "processed"
    config = build_config(tmp_path, output)
    run_preprocess(config)
    old_generation = resolve_dataset_root(output)
    old_manifest = (old_generation / "metadata.jsonl").read_bytes()
    old_pointer = (output / "CURRENT").read_bytes()

    save = np.savez
    calls = 0

    def interrupt_second(path, **features):
        nonlocal calls
        calls += 1
        if calls == 2:
            raise OSError("injected preprocessing interruption")
        return save(path, **features)

    monkeypatch.setattr(np, "savez", interrupt_second)
    with pytest.raises(OSError, match="injected"):
        run_preprocess(config)

    assert resolve_dataset_root(output) == old_generation
    assert (old_generation / "metadata.jsonl").read_bytes() == old_manifest
    assert (output / "CURRENT").read_bytes() == old_pointer
    assert not list(output.glob(".staging-*"))


def test_current_pointer_failure_cannot_publish_a_partial_generation(tmp_path, monkeypatch):
    from auris_singer import dataset_layout

    root = tmp_path / "processed"
    old = root / dataset_layout.GENERATIONS_DIR / "old"
    old.mkdir(parents=True)
    (old / "metadata.jsonl").write_text('{"id":"old"}\n', encoding="utf-8")
    (root / dataset_layout.CURRENT_FILE).write_text("old\n", encoding="utf-8")
    staging = root / ".staging-new"
    staging.mkdir()
    (staging / "metadata.jsonl").write_text('{"id":"new"}\n', encoding="utf-8")

    replace = dataset_layout.os.replace

    def fail_current(source, destination):
        if Path(destination).name == dataset_layout.CURRENT_FILE:
            raise OSError("injected pointer publication failure")
        return replace(source, destination)

    monkeypatch.setattr(dataset_layout.os, "replace", fail_current)
    with pytest.raises(OSError, match="injected pointer"):
        dataset_layout.publish_dataset_generation(root, staging)

    assert resolve_dataset_root(root) == old
    assert (resolve_dataset_root(root) / "metadata.jsonl").read_text() == '{"id":"old"}\n'
    assert (root / dataset_layout.GENERATIONS_DIR / "new").is_dir()


@pytest.mark.parametrize("failure_point", ["replace-return", "directory-sync"])
def test_current_pointer_is_rolled_back_after_an_ambiguous_commit_failure(
    tmp_path, monkeypatch, failure_point
):
    from auris_singer import dataset_layout

    root = tmp_path / "processed"
    old = root / dataset_layout.GENERATIONS_DIR / "old"
    old.mkdir(parents=True)
    (old / "metadata.jsonl").write_text('{"id":"old"}\n', encoding="utf-8")
    current = root / dataset_layout.CURRENT_FILE
    current.write_text("old\n", encoding="utf-8")
    old_pointer = current.read_bytes()
    staging = root / ".staging-new"
    staging.mkdir()
    (staging / "metadata.jsonl").write_text('{"id":"new"}\n', encoding="utf-8")

    replace = dataset_layout.os.replace
    current_replacements = 0

    def interrupt_after_replace(source, destination):
        nonlocal current_replacements
        result = replace(source, destination)
        if Path(destination) == current:
            current_replacements += 1
            if failure_point == "replace-return" and current_replacements == 1:
                raise KeyboardInterrupt("after pointer rename")
        return result

    directory_sync = dataset_layout.fsync_directory
    root_syncs = 0

    def fail_first_post_replace_sync(path):
        nonlocal root_syncs
        if Path(path) == root:
            root_syncs += 1
            if failure_point == "directory-sync" and root_syncs == 2:
                raise OSError("pointer directory sync failed")
        return directory_sync(path)

    monkeypatch.setattr(dataset_layout.os, "replace", interrupt_after_replace)
    monkeypatch.setattr(dataset_layout, "fsync_directory", fail_first_post_replace_sync)

    expected = KeyboardInterrupt if failure_point == "replace-return" else OSError
    with pytest.raises(expected):
        dataset_layout.publish_dataset_generation(root, staging)

    assert current.read_bytes() == old_pointer
    assert resolve_dataset_root(root) == old


def test_failed_publisher_cannot_roll_back_a_later_success(tmp_path, monkeypatch):
    from auris_singer import dataset_layout

    root = tmp_path / "processed"
    old = root / dataset_layout.GENERATIONS_DIR / "old"
    old.mkdir(parents=True)
    (old / "metadata.jsonl").write_text('{"id":"old"}\n', encoding="utf-8")
    current = root / dataset_layout.CURRENT_FILE
    current.write_text("old\n", encoding="utf-8")
    staging_a = root / ".staging-a"
    staging_b = root / ".staging-b"
    staging_a.mkdir()
    staging_b.mkdir()
    (staging_a / "metadata.jsonl").write_text('{"id":"a"}\n', encoding="utf-8")
    (staging_b / "metadata.jsonl").write_text('{"id":"b"}\n', encoding="utf-8")

    a_waiting_to_fail = threading.Event()
    allow_a_failure = threading.Event()
    b_replaced_current = threading.Event()
    a_replaced_current = threading.Event()
    a_fault_injected = threading.Event()
    replace = dataset_layout.os.replace

    def observe_replace(source, destination):
        result = replace(source, destination)
        if Path(destination) == current:
            if threading.current_thread().name == "publisher-a":
                a_replaced_current.set()
            elif threading.current_thread().name == "publisher-b":
                b_replaced_current.set()
        return result

    sync_directory = dataset_layout.fsync_directory

    def fail_a_after_rename(path):
        if (
            Path(path) == root
            and threading.current_thread().name == "publisher-a"
            and a_replaced_current.is_set()
            and not a_fault_injected.is_set()
        ):
            a_fault_injected.set()
            a_waiting_to_fail.set()
            assert allow_a_failure.wait(5), "test did not release publisher A"
            raise OSError("publisher A directory sync failed")
        return sync_directory(path)

    monkeypatch.setattr(dataset_layout.os, "replace", observe_replace)
    monkeypatch.setattr(dataset_layout, "fsync_directory", fail_a_after_rename)
    results = {}

    def publish(label, staging):
        try:
            results[label] = dataset_layout.publish_dataset_generation(root, staging)
        except BaseException as error:
            results[label] = error

    publisher_a = threading.Thread(
        target=publish, args=("a", staging_a), name="publisher-a", daemon=True
    )
    publisher_b = threading.Thread(
        target=publish, args=("b", staging_b), name="publisher-b", daemon=True
    )
    publisher_a.start()
    assert a_waiting_to_fail.wait(5), "publisher A did not reach its commit fault"
    publisher_b.start()
    assert not b_replaced_current.wait(0.2), "publisher B bypassed the CURRENT transaction lock"
    allow_a_failure.set()
    publisher_a.join(5)
    publisher_b.join(5)

    assert not publisher_a.is_alive() and not publisher_b.is_alive()
    assert isinstance(results["a"], OSError)
    assert results["b"] == root / dataset_layout.GENERATIONS_DIR / "b"
    assert resolve_dataset_root(root) == results["b"]


def test_current_pointer_is_fsynced_before_its_atomic_replace(tmp_path, monkeypatch):
    from auris_singer import dataset_layout

    root = tmp_path / "processed"
    root.mkdir()
    staging = root / ".staging-first"
    staging.mkdir()
    (staging / "metadata.jsonl").write_text('{"id":"first"}\n', encoding="utf-8")

    events = []
    fsync = dataset_layout.os.fsync
    replace = dataset_layout.os.replace

    def observe_fsync(descriptor):
        events.append("fsync pointer")
        return fsync(descriptor)

    def observe_tree(path):
        events.append(f"sync tree {Path(path).name}")

    def observe_directory(path):
        events.append(f"sync dir {Path(path).name}")

    def observe_replace(source, destination):
        if Path(destination).name == dataset_layout.CURRENT_FILE:
            events.append("replace CURRENT")
        return replace(source, destination)

    monkeypatch.setattr(dataset_layout.os, "fsync", observe_fsync)
    monkeypatch.setattr(dataset_layout.os, "replace", observe_replace)
    monkeypatch.setattr(dataset_layout, "fsync_tree", observe_tree)
    monkeypatch.setattr(dataset_layout, "fsync_directory", observe_directory)
    published = dataset_layout.publish_dataset_generation(root, staging)

    assert events == [
        "sync tree .staging-first",
        "sync dir processed",
        "sync dir generations",
        "fsync pointer",
        "replace CURRENT",
        "sync dir processed",
    ]
    assert resolve_dataset_root(root) == published


def test_publishing_a_new_generation_retains_the_one_active_readers_resolved(tmp_path):
    from auris_singer import dataset_layout

    root = tmp_path / "processed"
    root.mkdir()
    first_staging = root / ".staging-first"
    first_staging.mkdir()
    (first_staging / "metadata.jsonl").write_text('{"id":"first"}\n', encoding="utf-8")
    first = dataset_layout.publish_dataset_generation(root, first_staging)
    reader_root = resolve_dataset_root(root)

    second_staging = root / ".staging-second"
    second_staging.mkdir()
    (second_staging / "metadata.jsonl").write_text('{"id":"second"}\n', encoding="utf-8")
    second = dataset_layout.publish_dataset_generation(root, second_staging)

    assert resolve_dataset_root(root) == second
    assert reader_root == first
    assert (reader_root / "metadata.jsonl").read_text(encoding="utf-8") == ('{"id":"first"}\n')
