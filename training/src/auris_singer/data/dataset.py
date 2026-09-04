"""Dataset and batching utilities for preprocessed singing data."""

from __future__ import annotations

import json
import math
from pathlib import Path

import numpy as np
import torch
from torch.utils.data import Dataset
from torch.utils.data.distributed import DistributedSampler

from auris_singer.utils.audio import spectrogram

__all__ = ["SingingDataset", "collate_batch", "DistributedBucketSampler"]


class SingingDataset(Dataset):
    """Reads the ``.npz`` files written by the preprocessing pipeline.

    The linear spectrogram is computed on the fly from the stored waveform.

    Args:
        root: preprocessed dataset directory (contains ``metadata.jsonl``).
        records: explicit record list; read from ``metadata.jsonl`` if omitted.
        min_frames / max_frames: keep only utterances within this frame range.
            ``max_frames`` bounds the memory of a batch; ``min_frames`` avoids
            degenerate clips.
        use_durations: hand out the labelled frames-per-phoneme where the
            preprocessor stored them (``durations``), so training expands the
            phonemes by the labels instead of by alignment search. Off, the
            key is left out and the search runs as it does for a corpus that
            has no labels.
    """

    def __init__(
        self,
        root: str | Path,
        records: list[dict] | None = None,
        min_frames: int = 32,
        max_frames: int = 1600,
        n_fft: int | None = None,
        hop_length: int | None = None,
        win_length: int | None = None,
        use_durations: bool = True,
    ):
        self.root = Path(root)
        self.use_durations = use_durations
        audio_config = json.loads((self.root / "audio_config.json").read_text())
        self.sample_rate = int(audio_config["sample_rate"])
        self.n_fft = int(n_fft if n_fft is not None else audio_config["n_fft"])
        self.hop_length = int(
            hop_length if hop_length is not None else audio_config["hop_length"]
        )
        self.win_length = int(
            win_length if win_length is not None else audio_config["win_length"]
        )

        if records is None:
            records = read_metadata(self.root)
        self.records = [
            r for r in records if min_frames <= r["n_frames"] <= max_frames
        ]
        if not self.records:
            raise RuntimeError(
                f"no utterance in {self.root} has between {min_frames} and "
                f"{max_frames} frames"
            )
        self.lengths = [int(r["n_frames"]) for r in self.records]
        #: Whether each utterance hands training its labelled durations — the sampler keeps
        #: a batch all-labelled or all-searched, since one unlabelled member would send the
        #: whole batch to the search (see :func:`collate_batch`).
        self.labelled = [
            bool(self.use_durations and r.get("has_durations")) for r in self.records
        ]

    def __len__(self) -> int:
        return len(self.records)

    def __getitem__(self, index: int) -> dict[str, torch.Tensor]:
        record = self.records[index]
        with np.load(self.root / record["path"]) as data:
            wav = torch.from_numpy(data["wav"].astype(np.float32) / 32768.0)
            phonemes = torch.from_numpy(data["phonemes"].astype(np.int64))
            f0 = torch.from_numpy(data["f0"].astype(np.float32))
            energy = torch.from_numpy(data["energy"].astype(np.float32))
            voiced = torch.from_numpy(data["voiced"].astype(np.float32))
            durations = (
                torch.from_numpy(data["durations"].astype(np.int64))
                if self.use_durations and "durations" in data
                else None
            )

        n_frames = min(wav.numel() // self.hop_length, f0.numel())
        wav = wav[: n_frames * self.hop_length]
        spec = spectrogram(wav, self.n_fft, self.hop_length, self.win_length)

        item = {
            "phonemes": phonemes,
            "spec": spec,
            "wav": wav.unsqueeze(0),
            "f0": f0[:n_frames],
            "energy": energy[:n_frames],
            "voiced": voiced[:n_frames],
            "speaker_id": torch.tensor(record["speaker_id"], dtype=torch.long),
        }
        if durations is not None:
            item["durations"] = durations
        return item


def read_metadata(root: str | Path) -> list[dict]:
    """Read ``metadata.jsonl`` from a preprocessed dataset directory."""
    path = Path(root) / "metadata.jsonl"
    with path.open(encoding="utf-8") as fp:
        return [json.loads(line) for line in fp if line.strip()]


def collate_batch(batch: list[dict[str, torch.Tensor]]) -> dict[str, torch.Tensor]:
    """Pad a list of samples into a batch.

    Frame-level tensors are padded to the longest spectrogram in the batch and
    the waveform to the matching number of samples, so ``wav.size(-1)`` is
    always ``spec.size(-1) * hop_length``. ``durations`` is in the batch only
    when every item carries it — a batch half aligned by labels and half by
    search would be neither.
    """
    batch_size = len(batch)
    max_phonemes = max(int(item["phonemes"].numel()) for item in batch)
    max_frames = max(int(item["spec"].size(-1)) for item in batch)
    hop_length = batch[0]["wav"].size(-1) // batch[0]["spec"].size(-1)
    spec_channels = batch[0]["spec"].size(0)

    phonemes = torch.zeros(batch_size, max_phonemes, dtype=torch.long)
    phoneme_lengths = torch.zeros(batch_size, dtype=torch.long)
    spec = torch.zeros(batch_size, spec_channels, max_frames)
    spec_lengths = torch.zeros(batch_size, dtype=torch.long)
    wav = torch.zeros(batch_size, 1, max_frames * hop_length)
    f0 = torch.zeros(batch_size, max_frames)
    energy = torch.zeros(batch_size, max_frames)
    voiced = torch.zeros(batch_size, max_frames)
    speaker_ids = torch.zeros(batch_size, dtype=torch.long)

    for i, item in enumerate(batch):
        n_phonemes = int(item["phonemes"].numel())
        n_frames = int(item["spec"].size(-1))
        phonemes[i, :n_phonemes] = item["phonemes"]
        phoneme_lengths[i] = n_phonemes
        spec[i, :, :n_frames] = item["spec"]
        spec_lengths[i] = n_frames
        wav[i, :, : item["wav"].size(-1)] = item["wav"]
        f0[i, :n_frames] = item["f0"]
        energy[i, :n_frames] = item["energy"]
        voiced[i, :n_frames] = item["voiced"]
        speaker_ids[i] = item["speaker_id"]

    batch_out = {
        "phonemes": phonemes,
        "phoneme_lengths": phoneme_lengths,
        "spec": spec,
        "spec_lengths": spec_lengths,
        "wav": wav,
        "f0": f0,
        "energy": energy,
        "voiced": voiced,
        "speaker_ids": speaker_ids,
    }
    if all("durations" in item for item in batch):
        durations = torch.zeros(batch_size, max_phonemes, dtype=torch.long)
        for i, item in enumerate(batch):
            durations[i, : item["durations"].numel()] = item["durations"]
        batch_out["durations"] = durations
    return batch_out


class DistributedBucketSampler(DistributedSampler):
    """Length-bucketed batch sampler.

    Utterances are grouped into frame-count buckets and batches are drawn
    within a bucket, so a batch contains similar-length clips and padding waste
    stays small.  It subclasses :class:`DistributedSampler`, so it also works
    unchanged with a single process.

    A bucket is also split by whether its utterances carry labelled durations:
    :func:`collate_batch` hands training the labels only when every member has
    them, so on a corpus that mixes a labelled source with an unlabelled one —
    JSUT-song with its HTS labels beside VocalSet's bare vowels — a mixed batch
    would quietly send the labelled half back to alignment search. Kept apart,
    each batch is all-labelled or all-searched.

    Args:
        dataset: a :class:`SingingDataset` (needs a ``lengths`` attribute).
        batch_size: utterances per batch per replica.
        boundaries: bucket edges in frames.
    """

    def __init__(
        self,
        dataset: SingingDataset,
        batch_size: int,
        boundaries: list[int] | None = None,
        num_replicas: int | None = None,
        rank: int | None = None,
        shuffle: bool = True,
    ):
        if num_replicas is None or rank is None:
            # Fall back to single-process values when torch.distributed is not
            # initialized, which is what DistributedSampler would raise on.
            if not (torch.distributed.is_available() and torch.distributed.is_initialized()):
                num_replicas = 1 if num_replicas is None else num_replicas
                rank = 0 if rank is None else rank
        super().__init__(dataset, num_replicas=num_replicas, rank=rank, shuffle=shuffle)
        # Lightning advances ``batch_sampler.sampler`` at the start of every epoch.
        # This object is both the sampler and the batch sampler, so expose that
        # conventional attribute rather than leaving Lightning to find the
        # DataLoader's unrelated SequentialSampler.
        self.sampler = self
        self.batch_size = batch_size
        self.lengths = list(dataset.lengths)
        self.labelled = list(getattr(dataset, "labelled", [False] * len(self.lengths)))
        self.boundaries = boundaries or [0, 100, 200, 300, 400, 600, 800, 1200, 1600]
        self.buckets, self.num_samples_per_bucket = self._create_buckets()
        self.total_size = sum(self.num_samples_per_bucket)
        self.num_samples = self.total_size // self.num_replicas

    def _bucket_of(self, length: int) -> int:
        for i in range(len(self.boundaries) - 1):
            if self.boundaries[i] < length <= self.boundaries[i + 1]:
                return i
        return -1

    def _create_buckets(self) -> tuple[list[list[int]], list[int]]:
        # Two rows per length bucket: the labelled utterances and the searched ones.
        buckets: list[list[int]] = [[] for _ in range(2 * (len(self.boundaries) - 1))]
        for index, (length, labelled) in enumerate(zip(self.lengths, self.labelled)):
            bucket = self._bucket_of(length)
            if bucket >= 0:
                buckets[2 * bucket + int(labelled)].append(index)

        # Drop empty buckets, then pad each so it divides evenly across replicas.
        kept = [b for b in buckets if b]
        if not kept:
            raise RuntimeError(
                "no utterance falls inside the sampler boundaries; widen "
                "data.bucket_boundaries"
            )
        sizes = []
        for bucket in kept:
            per_batch = self.num_replicas * self.batch_size
            remainder = (per_batch - (len(bucket) % per_batch)) % per_batch
            sizes.append(len(bucket) + remainder)
        return kept, sizes

    def __iter__(self):
        generator = torch.Generator()
        generator.manual_seed(self.epoch)

        indices = []
        for bucket in self.buckets:
            if self.shuffle:
                order = torch.randperm(len(bucket), generator=generator).tolist()
            else:
                order = list(range(len(bucket)))
            indices.append(order)

        batches: list[list[int]] = []
        for i, bucket in enumerate(self.buckets):
            order = indices[i]
            padding = self.num_samples_per_bucket[i] - len(bucket)
            if padding:
                repeats = math.ceil(padding / len(order))
                order = order + (order * repeats)[:padding]
            selected = [bucket[j] for j in order]
            selected = selected[self.rank :: self.num_replicas]
            for start in range(0, len(selected), self.batch_size):
                batch = selected[start : start + self.batch_size]
                if len(batch) == self.batch_size:
                    batches.append(batch)

        if self.shuffle:
            order = torch.randperm(len(batches), generator=generator).tolist()
            batches = [batches[i] for i in order]
        return iter(batches)

    def __len__(self) -> int:
        return self.num_samples // self.batch_size
