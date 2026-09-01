"""Lightning data module for preprocessed singing datasets."""

from __future__ import annotations

import json
import random
from pathlib import Path

import lightning as L
from torch.utils.data import DataLoader

from auris_singer.data.dataset import (
    DistributedBucketSampler,
    SingingDataset,
    collate_batch,
    read_metadata,
)
from auris_singer.text import PhonemeTable

__all__ = ["SingingDataModule"]


class SingingDataModule(L.LightningDataModule):
    """Loads a preprocessed dataset directory and splits it into train/val.

    The split is deterministic given ``seed``, so resuming a run — or starting
    a second one with a different model size — sees the same validation set.
    ``use_durations`` hands training the labelled frames-per-phoneme where the
    preprocessor stored them; off, the alignment search runs as for a corpus
    without labels, which is how the two are compared on one dataset.

    Note:
        The train loader uses :class:`DistributedBucketSampler`, which is
        already distribution-aware, so the trainer must be created with
        ``use_distributed_sampler=False``.
    """

    def __init__(
        self,
        root: str | Path,
        batch_size: int = 16,
        num_workers: int = 4,
        val_size: int = 8,
        min_frames: int = 32,
        max_frames: int = 1600,
        bucket_boundaries: list[int] | None = None,
        seed: int = 1234,
        pin_memory: bool = True,
        use_durations: bool = True,
    ):
        super().__init__()
        self.root = Path(root)
        self.batch_size = batch_size
        self.num_workers = num_workers
        self.val_size = val_size
        self.min_frames = min_frames
        self.max_frames = max_frames
        self.bucket_boundaries = bucket_boundaries
        self.seed = seed
        self.pin_memory = pin_memory
        self.use_durations = use_durations

        self.train_dataset: SingingDataset | None = None
        self.val_dataset: SingingDataset | None = None

        self.speaker_to_id: dict[str, int] = json.loads(
            (self.root / "speakers.json").read_text(encoding="utf-8")
        )
        self.phoneme_table = PhonemeTable.load(self.root / "phonemes.json")
        self.audio_config: dict = json.loads(
            (self.root / "audio_config.json").read_text(encoding="utf-8")
        )

    @property
    def n_speakers(self) -> int:
        return len(self.speaker_to_id)

    @property
    def n_vocab(self) -> int:
        return len(self.phoneme_table)

    def setup(self, stage: str | None = None) -> None:
        if self.train_dataset is not None:
            return
        records = read_metadata(self.root)
        rng = random.Random(self.seed)
        rng.shuffle(records)

        val_size = min(self.val_size, max(len(records) // 10, 1))
        val_records = records[:val_size]
        train_records = records[val_size:] or records

        common = dict(
            min_frames=self.min_frames,
            max_frames=self.max_frames,
            use_durations=self.use_durations,
        )
        self.train_dataset = SingingDataset(self.root, train_records, **common)
        self.val_dataset = SingingDataset(self.root, val_records, **common)

    def train_dataloader(self) -> DataLoader:
        assert self.train_dataset is not None, "call setup() first"
        sampler = DistributedBucketSampler(
            self.train_dataset,
            batch_size=self.batch_size,
            boundaries=self.bucket_boundaries,
        )
        return DataLoader(
            self.train_dataset,
            batch_sampler=sampler,
            num_workers=self.num_workers,
            collate_fn=collate_batch,
            pin_memory=self.pin_memory,
            persistent_workers=self.num_workers > 0,
        )

    def val_dataloader(self) -> DataLoader:
        assert self.val_dataset is not None, "call setup() first"
        return DataLoader(
            self.val_dataset,
            batch_size=1,
            shuffle=False,
            num_workers=min(self.num_workers, 2),
            collate_fn=collate_batch,
            pin_memory=self.pin_memory,
        )
