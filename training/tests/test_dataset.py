"""Tests for the dataset, the collate function and the bucket sampler."""

from __future__ import annotations

import pytest
import torch

from auris_singer.data import (
    DistributedBucketSampler,
    SingingDataModule,
    SingingDataset,
    collate_batch,
    read_metadata,
)

HOP = 480


def test_sample_fields_are_consistent(processed_dataset):
    dataset = SingingDataset(processed_dataset)
    item = dataset[0]
    n_frames = item["spec"].size(-1)

    assert item["wav"].shape == (1, n_frames * HOP)
    assert item["f0"].shape == item["energy"].shape == item["voiced"].shape
    assert item["f0"].numel() == n_frames
    assert item["phonemes"].dtype == torch.long
    assert item["speaker_id"].dtype == torch.long
    assert torch.all((item["voiced"] == 0) | (item["voiced"] == 1))


def test_length_filter_drops_out_of_range_utterances(processed_dataset):
    records = read_metadata(processed_dataset)
    longest = max(r["n_frames"] for r in records)
    dataset = SingingDataset(processed_dataset, min_frames=0, max_frames=longest - 1)
    assert len(dataset) < len(records)
    assert all(r["n_frames"] <= longest - 1 for r in dataset.records)


def test_empty_selection_raises(processed_dataset):
    with pytest.raises(RuntimeError, match="no utterance"):
        SingingDataset(processed_dataset, min_frames=10_000, max_frames=20_000)


def test_collate_pads_to_the_longest_item(processed_dataset):
    dataset = SingingDataset(processed_dataset)
    batch = collate_batch([dataset[i] for i in range(4)])

    max_frames = int(batch["spec_lengths"].max())
    assert batch["spec"].shape[-1] == max_frames
    assert batch["wav"].shape == (4, 1, max_frames * HOP)
    assert batch["f0"].shape == batch["energy"].shape == (4, max_frames)
    assert batch["phonemes"].shape[-1] == int(batch["phoneme_lengths"].max())

    # Everything past a sample's own length must be zero padding.
    for i in range(4):
        n = int(batch["spec_lengths"][i])
        assert torch.count_nonzero(batch["spec"][i, :, n:]) == 0
        assert torch.count_nonzero(batch["f0"][i, n:]) == 0
        assert torch.count_nonzero(batch["wav"][i, :, n * HOP :]) == 0
        s = int(batch["phoneme_lengths"][i])
        assert torch.count_nonzero(batch["phonemes"][i, s:]) == 0


def test_bucket_sampler_groups_similar_lengths(processed_dataset):
    dataset = SingingDataset(processed_dataset)
    sampler = DistributedBucketSampler(
        dataset, batch_size=2, boundaries=[0, 70, 90, 200]
    )
    sampler.set_epoch(0)
    batches = list(sampler)
    assert batches, "sampler produced no batches"
    for batch in batches:
        assert len(batch) == 2
        lengths = [dataset.lengths[i] for i in batch]
        # Both members must come from the same bucket.
        assert sampler._bucket_of(lengths[0]) == sampler._bucket_of(lengths[1])


def test_bucket_sampler_shards_across_replicas(processed_dataset):
    dataset = SingingDataset(processed_dataset)
    common = dict(batch_size=1, boundaries=[0, 200], num_replicas=2, shuffle=False)
    first = DistributedBucketSampler(dataset, rank=0, **common)
    second = DistributedBucketSampler(dataset, rank=1, **common)
    first.set_epoch(0)
    second.set_epoch(0)

    a = {i for batch in first for i in batch}
    b = {i for batch in second for i in batch}
    assert a and b
    assert a.isdisjoint(b)


def test_datamodule_splits_deterministically(processed_dataset):
    def build():
        dm = SingingDataModule(processed_dataset, batch_size=2, num_workers=0, val_size=3)
        dm.setup()
        return dm

    first, second = build(), build()
    assert [r["id"] for r in first.val_dataset.records] == [
        r["id"] for r in second.val_dataset.records
    ]
    train_ids = {r["id"] for r in first.train_dataset.records}
    val_ids = {r["id"] for r in first.val_dataset.records}
    assert train_ids.isdisjoint(val_ids)
    assert first.n_speakers == 2
    assert first.n_vocab == len(first.phoneme_table)


def test_dataloaders_yield_usable_batches(processed_dataset):
    dm = SingingDataModule(
        processed_dataset,
        batch_size=2,
        num_workers=0,
        val_size=2,
        bucket_boundaries=[0, 200],
        pin_memory=False,
    )
    dm.setup()
    batch = next(iter(dm.train_dataloader()))
    assert batch["spec"].size(0) == 2
    val_batch = next(iter(dm.val_dataloader()))
    assert val_batch["spec"].size(0) == 1


def test_a_batch_carries_durations_only_when_every_item_does(processed_dataset):
    import torch

    from auris_singer.data import SingingDataset, collate_batch

    dataset = SingingDataset(processed_dataset)
    a, b = dataset[0], dataset[1]
    assert "durations" not in collate_batch([a, b]), "the synthetic corpus has no labels"
    a = dict(a, durations=torch.full((a["phonemes"].numel(),), 3, dtype=torch.long))
    assert "durations" not in collate_batch([a, b]), "half a batch labelled is not labelled"
    b = dict(b, durations=torch.full((b["phonemes"].numel(),), 2, dtype=torch.long))
    batch = collate_batch([a, b])
    assert batch["durations"].shape == (2, max(a["phonemes"].numel(), b["phonemes"].numel()))
    assert batch["durations"][0, : a["phonemes"].numel()].tolist() == [3] * a["phonemes"].numel()
    assert batch["durations"][1, b["phonemes"].numel() :].tolist() == [0] * (batch["durations"].shape[1] - b["phonemes"].numel())


def test_a_batch_is_all_labelled_or_all_searched(processed_dataset):
    """One unlabelled member sends a whole batch to the search, so the sampler never
    seats a labelled utterance beside an unlabelled one."""
    dataset = SingingDataset(processed_dataset)
    dataset.labelled = [i % 3 == 0 for i in range(len(dataset))]
    sampler = DistributedBucketSampler(dataset, batch_size=2, boundaries=[0, 200])
    sampler.set_epoch(0)
    batches = list(sampler)
    assert batches
    seen = set()
    for batch in batches:
        kinds = {dataset.labelled[i] for i in batch}
        assert len(kinds) == 1, batch
        seen |= kinds
    assert seen == {True, False}, "both kinds are still trained on"
