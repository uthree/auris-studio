"""Smoke tests for the Lightning training loop and the inference API."""

from __future__ import annotations

import ast
import copy
import pickle
import runpy
from pathlib import Path
from unittest import mock

import lightning as L
import pytest
import torch

from auris_singer.data import SingingDataModule
from auris_singer.infer import Synthesizer
from auris_singer.lightning_module import AurisSingerModule
from auris_singer.text import DEFAULT_PHONEME_TABLE


def _write_marker(path):
    Path(path).write_text("checkpoint code ran", encoding="utf-8")


class _CheckpointPayload:
    def __init__(self, marker):
        self.marker = marker

    def __reduce__(self):
        return _write_marker, (self.marker,)


AUDIO = {
    "sample_rate": 48_000,
    "n_fft": 2048,
    "hop_length": 480,
    "win_length": 2048,
    "n_mels": 80,
}
LOSS = {"mel_params": [[512, 120, 512, 40]], "envelope_kernel_sizes": [128, 256]}


@pytest.fixture
def module(tiny_model_config, tiny_discriminator_config):
    torch.manual_seed(0)
    return AurisSingerModule(
        model=tiny_model_config,
        discriminator=tiny_discriminator_config,
        audio=AUDIO,
        loss=LOSS,
        optimizer={"learning_rate": 1e-4},
        metadata={
            "symbols": DEFAULT_PHONEME_TABLE.symbols,
            "speaker_to_id": {"alice": 0, "bob": 1},
        },
    )


@pytest.fixture
def datamodule(processed_dataset):
    dm = SingingDataModule(
        processed_dataset,
        batch_size=2,
        num_workers=0,
        val_size=2,
        bucket_boundaries=[0, 200],
        pin_memory=False,
    )
    dm.setup()
    return dm


def test_configure_optimizers_returns_two_optimizers(module):
    optimizers, schedulers = module.configure_optimizers()
    assert len(optimizers) == 2 and len(schedulers) == 2
    generator_params = {id(p) for p in module.model.parameters()}
    assert all(
        id(p) in generator_params for group in optimizers[0].param_groups for p in group["params"]
    )


def test_training_updates_both_networks(module, datamodule):
    before_g = [p.detach().clone() for p in module.model.parameters()]
    before_d = [p.detach().clone() for p in module.discriminator.parameters()]

    trainer = L.Trainer(
        max_steps=2,
        accelerator="cpu",
        devices=1,
        logger=False,
        enable_checkpointing=False,
        enable_progress_bar=False,
        num_sanity_val_steps=0,
        limit_val_batches=0,
        use_distributed_sampler=False,
    )
    trainer.fit(module, datamodule=datamodule)

    assert trainer.global_step == 2
    assert any(not torch.equal(a, b) for a, b in zip(before_g, module.model.parameters())), (
        "generator did not update"
    )
    assert any(
        not torch.equal(a, b) for a, b in zip(before_d, module.discriminator.parameters())
    ), "discriminator did not update"
    assert all(torch.isfinite(p).all() for p in module.model.parameters())


def test_validation_runs_the_full_inference_path(module, datamodule):
    trainer = L.Trainer(
        max_steps=1,
        accelerator="cpu",
        devices=1,
        logger=False,
        enable_checkpointing=False,
        enable_progress_bar=False,
        num_sanity_val_steps=0,
        limit_val_batches=2,
        val_check_interval=1,  # validate after the single training step
        use_distributed_sampler=False,
    )
    trainer.fit(module, datamodule=datamodule)
    assert torch.isfinite(trainer.callback_metrics["val/mel"])


def test_checkpoint_roundtrip_synthesizes_audio(module, datamodule, tmp_path):
    trainer = L.Trainer(
        max_steps=1,
        accelerator="cpu",
        devices=1,
        logger=False,
        enable_checkpointing=False,
        enable_progress_bar=False,
        num_sanity_val_steps=0,
        limit_val_batches=0,
        use_distributed_sampler=False,
    )
    trainer.fit(module, datamodule=datamodule)

    checkpoint = tmp_path / "model.ckpt"
    trainer.save_checkpoint(checkpoint)

    synthesizer = Synthesizer.from_checkpoint(checkpoint)
    assert synthesizer.resolve_speaker("bob") == 1
    phonemes = ["<sil>", "k", "o", "ɴ", "i"]
    durations = [4, 5, 6, 3, 4]
    n_frames = sum(durations)
    wav = synthesizer.synthesize(
        phonemes=phonemes,
        durations=durations,
        f0=[220.0] * n_frames,
        energy=[0.1] * n_frames,
        speaker="alice",
    )
    assert wav.shape == (n_frames * AUDIO["hop_length"],)
    assert wav.dtype.name == "float32"


def test_a_run_can_start_from_another_runs_weights(
    module, datamodule, tmp_path, tiny_model_config, tiny_discriminator_config
):
    trainer = L.Trainer(
        max_steps=1,
        accelerator="cpu",
        devices=1,
        logger=False,
        enable_checkpointing=False,
        enable_progress_bar=False,
        num_sanity_val_steps=0,
        limit_val_batches=0,
        use_distributed_sampler=False,
    )
    trainer.fit(module, datamodule=datamodule)
    checkpoint = tmp_path / "pretrained.ckpt"
    trainer.save_checkpoint(checkpoint)

    torch.manual_seed(1)
    fresh = AurisSingerModule(
        model=tiny_model_config,
        discriminator=tiny_discriminator_config,
        audio=AUDIO,
        loss=LOSS,
        optimizer={"learning_rate": 1e-4},
        metadata={
            "symbols": DEFAULT_PHONEME_TABLE.symbols,
            "speaker_to_id": {"carol": 0, "dave": 1},
        },
    )
    name, before = next(iter(fresh.model.state_dict().items()))
    assert not torch.equal(before, module.model.state_dict()[name]), "a different draw"
    fresh_speaker = {
        key: value.clone()
        for key, value in fresh.state_dict().items()
        if "speaker" in key or ".projection." in key
    }
    fresh.load_weights(checkpoint)
    for key, value in module.model.state_dict().items():
        if "speaker" not in key:
            assert torch.equal(fresh.model.state_dict()[key], value), key
    for key, value in module.discriminator.state_dict().items():
        if ".projection." not in key:
            assert torch.equal(fresh.discriminator.state_dict()[key], value), key
    for key, value in fresh_speaker.items():
        assert torch.equal(fresh.state_dict()[key], value), key
    assert fresh.hparams["metadata"]["speaker_to_id"] == {"carol": 0, "dave": 1}, (
        "the new corpus's speakers"
    )

    # Another number of speakers is not a refusal: the speaker tensors start afresh and the
    # rest still loads.
    more = AurisSingerModule(
        model={**tiny_model_config, "n_speakers": 5},
        discriminator={**tiny_discriminator_config, "n_speakers": 5},
        audio=AUDIO,
        loss=LOSS,
        metadata={
            "symbols": DEFAULT_PHONEME_TABLE.symbols,
            "speaker_to_id": {f"new-{index}": index for index in range(5)},
        },
    )
    more.load_weights(checkpoint)
    for key, value in module.model.state_dict().items():
        if "speaker" not in key:
            assert torch.equal(more.model.state_dict()[key], value), key

    # A different shape is refused outright, naming the tensor.
    other = AurisSingerModule(
        model={**tiny_model_config, "inter_channels": tiny_model_config["inter_channels"] + 4},
        discriminator=tiny_discriminator_config,
        audio=AUDIO,
        loss=LOSS,
        metadata={
            "symbols": DEFAULT_PHONEME_TABLE.symbols,
            "speaker_to_id": {"alice": 0, "bob": 1},
        },
    )
    with pytest.raises(ValueError, match="is .* here"):
        other.load_weights(checkpoint)


def test_init_from_refuses_checkpoint_pickle_code(module, tmp_path):
    marker = tmp_path / "executed"
    checkpoint = tmp_path / "malicious.ckpt"
    torch.save({"state_dict": {"payload": _CheckpointPayload(marker)}}, checkpoint)

    with pytest.raises(pickle.UnpicklingError):
        module.load_weights(checkpoint)

    assert not marker.exists()


def test_inference_refuses_checkpoint_pickle_code(tmp_path):
    marker = tmp_path / "executed"
    checkpoint = tmp_path / "malicious.ckpt"
    torch.save({"state_dict": {"payload": _CheckpointPayload(marker)}}, checkpoint)

    with pytest.raises(pickle.UnpicklingError):
        Synthesizer.from_checkpoint(checkpoint)

    assert not marker.exists()


def test_every_read_only_lightning_loader_forces_safe_unpickling():
    root = Path(__file__).parents[1]
    paths = [
        root / "scripts" / "export_onnx.py",
        root / "scripts" / "check_source_control.py",
        root / "src" / "auris_singer" / "infer.py",
        root / "src" / "auris_singer" / "host_eval.py",
    ]
    for path in paths:
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        calls = [
            node
            for node in ast.walk(tree)
            if isinstance(node, ast.Call)
            and isinstance(node.func, ast.Attribute)
            and node.func.attr == "load_from_checkpoint"
        ]
        assert calls, f"{path} contains no checkpoint loader"
        for call in calls:
            weights_only = next(
                (keyword.value for keyword in call.keywords if keyword.arg == "weights_only"),
                None,
            )
            assert isinstance(weights_only, ast.Constant) and weights_only.value is True, (
                f"{path}:{call.lineno} may execute checkpoint pickle code"
            )


def test_init_from_requires_the_same_clock_and_phoneme_identity(module, tmp_path):
    checkpoint = {
        "state_dict": module.state_dict(),
        "hyper_parameters": copy.deepcopy(dict(module.hparams)),
    }
    reordered = copy.deepcopy(checkpoint)
    reordered["hyper_parameters"]["metadata"]["symbols"] = list(
        reversed(DEFAULT_PHONEME_TABLE.symbols)
    )
    reordered_path = tmp_path / "reordered.ckpt"
    torch.save(reordered, reordered_path)
    with pytest.raises(ValueError, match="phoneme symbols differ"):
        module.load_weights(reordered_path)

    another_clock = copy.deepcopy(checkpoint)
    another_clock["hyper_parameters"]["model"]["sample_rate"] = 44_100
    another_clock["hyper_parameters"]["audio"]["sample_rate"] = 44_100
    clock_path = tmp_path / "another-clock.ckpt"
    torch.save(another_clock, clock_path)
    with pytest.raises(ValueError, match="do not share a clock"):
        module.load_weights(clock_path)


def test_resume_requires_identical_clock_and_metadata(module, tmp_path):
    checkpoint = {
        "state_dict": module.state_dict(),
        "hyper_parameters": copy.deepcopy(dict(module.hparams)),
    }
    checkpoint["hyper_parameters"]["metadata"]["speaker_to_id"] = {
        "carol": 0,
        "dave": 1,
    }
    metadata_path = tmp_path / "other-speakers.ckpt"
    torch.save(checkpoint, metadata_path)
    with pytest.raises(ValueError, match="metadata differs"):
        module.validate_resume_checkpoint(metadata_path)

    checkpoint["hyper_parameters"]["metadata"] = copy.deepcopy(dict(module.hparams["metadata"]))
    checkpoint["hyper_parameters"]["model"]["hop_length"] = 240
    checkpoint["hyper_parameters"]["audio"]["hop_length"] = 240
    clock_path = tmp_path / "other-clock.ckpt"
    torch.save(checkpoint, clock_path)
    with pytest.raises(ValueError, match="do not share a clock"):
        module.validate_resume_checkpoint(clock_path)


def test_resume_uses_lightnings_safe_checkpoint_loader():
    trainer = mock.Mock()
    module = mock.Mock()
    datamodule = object()
    train_script = runpy.run_path(Path(__file__).parents[1] / "scripts" / "train.py")

    train_script["fit"](trainer, module, datamodule, "run.ckpt")

    module.validate_resume_checkpoint.assert_called_once_with("run.ckpt")
    trainer.fit.assert_called_once_with(
        module,
        datamodule=datamodule,
        ckpt_path="run.ckpt",
        weights_only=True,
    )


class _FakeExperiment:
    def __init__(self):
        self.calls: list[str] = []

    def add_audio(self, tag, audio, step, sample_rate):
        self.calls.append(tag)


class _FakeLogger:
    def __init__(self):
        self.experiment = _FakeExperiment()


def test_reference_audio_is_logged_once_regardless_of_epoch(module):
    """Step-based validation means epoch 0 is long gone on the first pass."""
    logger = _FakeLogger()
    wav = torch.zeros(1, 480)
    with mock.patch.object(
        type(module), "logger", new_callable=mock.PropertyMock, return_value=logger
    ):
        module._log_audio(0, wav, wav)
        module._log_audio(0, wav, wav)
        module._log_audio(1, wav, wav)

    tags = logger.experiment.calls
    assert tags.count("val/0/reference") == 1, "reference must not be re-logged"
    assert tags.count("val/0/generated") == 2, "generated audio changes every pass"
    assert "val/1/reference" in tags


def test_synthesize_validates_its_inputs(module):
    synthesizer = Synthesizer(module)
    with pytest.raises(ValueError, match="durations has"):
        synthesizer.synthesize(["a", "i"], [3], [220.0] * 3, [0.1] * 3)
    with pytest.raises(ValueError, match="sum\\(durations\\)"):
        synthesizer.synthesize(["a", "i"], [3, 3], [220.0] * 5, [0.1] * 5)
    with pytest.raises(ValueError, match="not in the table"):
        synthesizer.synthesize(["a", "zzz"], [2, 2], [220.0] * 4, [0.1] * 4)
    with pytest.raises(KeyError, match="unknown speaker"):
        synthesizer.resolve_speaker("nobody")
    for speaker in (-1, module.model.n_speakers):
        with pytest.raises(ValueError, match="speaker id must be between"):
            synthesizer.resolve_speaker(speaker)
    with pytest.raises(ValueError, match="f0 must be a 1D curve"):
        synthesizer.synthesize(["a"], [2], [[220.0, 220.0]], [0.1, 0.1])
    with pytest.raises(ValueError, match="energy must be a 1D curve"):
        synthesizer.synthesize(["a"], [2], [220.0, 220.0], [[0.1, 0.1]])
    with pytest.raises(ValueError, match="voiced must be a 1D curve"):
        synthesizer.synthesize(["a"], [2], [220.0, 220.0], [0.1, 0.1], voiced=[[1.0, 1.0]])
    with pytest.raises(ValueError, match="f0 must contain finite"):
        synthesizer.synthesize(["a"], [1], [float("nan")], [0.1])
    with pytest.raises(ValueError, match="noise_scale must be a finite"):
        synthesizer.synthesize(["a"], [1], [220.0], [0.1], noise_scale=float("inf"))


@pytest.mark.parametrize(
    ("durations", "message"),
    [
        ([1.5], "whole frame counts"),
        ([float("nan")], "finite"),
        ([-1], "non-negative"),
        ([2_001], "at most"),
    ],
)
def test_synthesize_rejects_hostile_durations_without_expanding_them(module, durations, message):
    synthesizer = Synthesizer(module)
    with pytest.raises(ValueError, match=message):
        synthesizer.synthesize(["a"], durations, [220.0], [0.1])


def test_synthesize_rejects_an_oversized_duration_total_before_expansion(module):
    synthesizer = Synthesizer(module)
    with pytest.raises(ValueError, match="at most"):
        synthesizer.synthesize(["a", "i"], [1_001, 1_000], [], [])


def test_training_audio_clock_must_match_the_model_and_dataset(
    tiny_model_config, tiny_discriminator_config
):
    common = {
        "discriminator": tiny_discriminator_config,
        "loss": LOSS,
    }
    with pytest.raises(ValueError, match="model sample_rate=44100 disagrees"):
        AurisSingerModule(
            model={**tiny_model_config, "sample_rate": 44_100},
            audio=AUDIO,
            **common,
        )
    with pytest.raises(ValueError, match="dataset hop_length=240 disagrees"):
        AurisSingerModule(
            model=tiny_model_config,
            audio=AUDIO,
            metadata={"audio": {"sample_rate": 48_000, "hop_length": 240}},
            **common,
        )
    with pytest.raises(ValueError, match="metadata must be a mapping"):
        AurisSingerModule(
            model=tiny_model_config,
            audio=AUDIO,
            metadata={"audio": []},
            **common,
        )
    with pytest.raises(ValueError, match="spec_channels=.*disagrees with audio n_fft"):
        AurisSingerModule(
            model={**tiny_model_config, "spec_channels": 513},
            audio=AUDIO,
            **common,
        )
    with pytest.raises(ValueError, match="dataset n_fft=1024 disagrees with audio n_fft=2048"):
        AurisSingerModule(
            model=tiny_model_config,
            audio=AUDIO,
            metadata={"audio": {**AUDIO, "n_fft": 1024}},
            **common,
        )
    with pytest.raises(
        ValueError, match="dataset win_length=1024 disagrees with audio win_length=2048"
    ):
        AurisSingerModule(
            model=tiny_model_config,
            audio=AUDIO,
            metadata={"audio": {**AUDIO, "win_length": 1024}},
            **common,
        )


def test_kl_warmup_ramps_from_zero_to_one(tiny_model_config, tiny_discriminator_config):
    module = AurisSingerModule(
        model=tiny_model_config,
        discriminator=tiny_discriminator_config,
        audio=AUDIO,
        loss={**LOSS, "kl_warmup_steps": 100},
    )
    for step, expected in [(0, 0.0), (25, 0.25), (100, 1.0), (500, 1.0)]:
        with mock.patch.object(
            type(module), "global_step", new_callable=mock.PropertyMock, return_value=step
        ):
            assert module.kl_scale() == pytest.approx(expected)


def test_kl_warmup_disabled_is_always_one(tiny_model_config, tiny_discriminator_config):
    module = AurisSingerModule(
        model=tiny_model_config,
        discriminator=tiny_discriminator_config,
        audio=AUDIO,
        loss={**LOSS, "kl_warmup_steps": 0},
    )
    with mock.patch.object(
        type(module), "global_step", new_callable=mock.PropertyMock, return_value=0
    ):
        assert module.kl_scale() == 1.0


def test_validation_reports_latent_usage(module, datamodule):
    """Guards the posterior-collapse failure mode this metric exists to catch."""
    trainer = L.Trainer(
        max_steps=1,
        accelerator="cpu",
        devices=1,
        logger=False,
        enable_checkpointing=False,
        enable_progress_bar=False,
        num_sanity_val_steps=0,
        limit_val_batches=2,
        val_check_interval=1,
        use_distributed_sampler=False,
    )
    trainer.fit(module, datamodule=datamodule)
    assert torch.isfinite(trainer.callback_metrics["val/latent_usage"])


def test_latent_usage_is_zero_when_the_decoder_ignores_z(module, datamodule):
    """A decoder blind to z must score 0 -- that is what collapse looks like."""
    batch = next(iter(datamodule.val_dataloader()))
    out = module.model(
        phonemes=batch["phonemes"],
        phoneme_lengths=batch["phoneme_lengths"],
        spec=batch["spec"],
        spec_lengths=batch["spec_lengths"],
        f0=batch["f0"],
        energy=batch["energy"],
        voiced=batch["voiced"],
        speaker_ids=batch["speaker_ids"],
    )
    # Force the latent to a constant: shuffling it in time then changes nothing.
    # The excitation must be held fixed too, or its noise alone moves the score.
    out["z_slice"] = torch.zeros_like(out["z_slice"])
    generated, _ = module.model.generator(
        out["z_slice"],
        out["f0_slice"],
        out["energy_slice"],
        out["voiced_slice"],
        g=out["g"],
        source=out["source"],
    )
    out["wav_hat"] = generated
    assert module._latent_usage(batch, out).abs().item() == pytest.approx(0.0, abs=1e-5)


def test_training_expands_by_the_labels_when_the_batch_carries_them(
    module, datamodule, monkeypatch
):
    """With durations in the batch the alignment search is never run: the path is the labels'."""
    from auris_singer.data.dataset import SingingDataset

    plain = SingingDataset.__getitem__

    def labelled(self, index):
        item = plain(self, index)
        s, t = item["phonemes"].numel(), item["spec"].size(-1)
        durations = torch.full((s,), t // s, dtype=torch.long)
        durations[-1] += t - int(durations.sum())
        return dict(item, durations=durations)

    monkeypatch.setattr(SingingDataset, "__getitem__", labelled)
    batch = next(iter(datamodule.train_dataloader()))
    assert "durations" in batch and torch.equal(batch["durations"].sum(1), batch["spec_lengths"])

    with mock.patch.object(
        module.model, "_search_alignment", side_effect=AssertionError("searched")
    ):
        out = module.model(
            phonemes=batch["phonemes"],
            phoneme_lengths=batch["phoneme_lengths"],
            spec=batch["spec"],
            spec_lengths=batch["spec_lengths"],
            f0=batch["f0"],
            energy=batch["energy"],
            voiced=batch["voiced"],
            speaker_ids=batch["speaker_ids"],
            durations=batch["durations"],
        )
        assert torch.equal(out["durations"].round().long(), batch["durations"])
        trainer = L.Trainer(
            max_steps=1,
            accelerator="cpu",
            devices=1,
            logger=False,
            enable_checkpointing=False,
            enable_progress_bar=False,
            num_sanity_val_steps=0,
            limit_val_batches=1,
            val_check_interval=1,
            use_distributed_sampler=False,
        )
        trainer.fit(module, datamodule=datamodule)
    assert torch.isfinite(trainer.callback_metrics["val/mel"])

    # Switched off, the same corpus trains by the search again.
    monkeypatch.undo()
    off = SingingDataset(datamodule.root, use_durations=False)
    assert "durations" not in off[0]
