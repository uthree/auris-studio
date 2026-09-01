#!/usr/bin/env python
"""Train the model.

Example:
    uv run python scripts/train.py --config configs/train/base.yml \
        data.root=data/processed data.batch_size=8

Long runs should be started inside tmux so they survive a disconnect:
    tmux new-session -d -s train "uv run python scripts/train.py --config configs/train/base.yml"
"""

from __future__ import annotations

import argparse
import logging
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

import lightning as L  # noqa: E402
from lightning.pytorch.callbacks import LearningRateMonitor, ModelCheckpoint  # noqa: E402
from lightning.pytorch.loggers import TensorBoardLogger  # noqa: E402
from omegaconf import OmegaConf  # noqa: E402

from auris_singer.data import SingingDataModule  # noqa: E402
from auris_singer.lightning_module import AurisSingerModule  # noqa: E402
from auris_singer.utils.config import load_config, save_config  # noqa: E402


def build(config):
    """Build the data module and the Lightning module from a config."""
    datamodule = SingingDataModule(**OmegaConf.to_container(config.data, resolve=True))
    datamodule.setup()

    model_config = OmegaConf.to_container(config.model, resolve=True)
    model_config["n_vocab"] = datamodule.n_vocab
    model_config["n_speakers"] = datamodule.n_speakers

    module = AurisSingerModule(
        model=model_config,
        discriminator=OmegaConf.to_container(config.discriminator, resolve=True),
        audio=OmegaConf.to_container(config.audio, resolve=True),
        loss=OmegaConf.to_container(config.loss, resolve=True),
        optimizer=OmegaConf.to_container(config.optimizer, resolve=True),
        validation=OmegaConf.to_container(config.get("validation", {}), resolve=True),
        metadata={
            "symbols": datamodule.phoneme_table.symbols,
            "speaker_to_id": datamodule.speaker_to_id,
            "audio": datamodule.audio_config,
        },
    )
    return datamodule, module


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", required=True, help="training YAML config")
    parser.add_argument("--resume", default=None, help="checkpoint path to resume from")
    parser.add_argument("overrides", nargs="*", help="dotlist config overrides")
    args = parser.parse_args()

    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s: %(message)s")
    config = load_config(args.config, args.overrides)
    L.seed_everything(int(config.get("seed", 1234)), workers=True)

    datamodule, module = build(config)
    n_params = sum(p.numel() for p in module.model.parameters())
    logging.info(
        "generator %.1fM params | %d phonemes | %d speakers | %d train / %d val utterances",
        n_params / 1e6,
        datamodule.n_vocab,
        datamodule.n_speakers,
        len(datamodule.train_dataset),
        len(datamodule.val_dataset),
    )

    checkpoint_config = OmegaConf.to_container(config.checkpoint, resolve=True)
    callbacks = [
        ModelCheckpoint(monitor="val/mel", mode="min", **checkpoint_config),
        LearningRateMonitor(logging_interval="epoch"),
    ]
    logger = TensorBoardLogger(
        save_dir=str(config.get("log_dir", "runs/logs")),
        name=str(config.get("run_name", "auris-singer")),
    )
    save_config(config, Path(logger.log_dir) / "config.yaml")

    trainer_config = OmegaConf.to_container(config.trainer, resolve=True)
    trainer = L.Trainer(
        logger=logger,
        callbacks=callbacks,
        # The train loader supplies its own distribution-aware bucket sampler.
        use_distributed_sampler=False,
        **trainer_config,
    )
    trainer.fit(module, datamodule=datamodule, ckpt_path=args.resume)


if __name__ == "__main__":
    main()
