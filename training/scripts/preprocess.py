#!/usr/bin/env python
"""Preprocess a dataset into the cached feature format used for training.

Example:
    uv run python scripts/preprocess.py \
        --config configs/preprocess/generic_wav_text.yml \
        dataset.output_dir=data/processed
"""

from __future__ import annotations

import argparse
import logging
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from auris_singer.preprocess import run_preprocess  # noqa: E402
from auris_singer.utils.config import load_config  # noqa: E402


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", required=True, help="preprocessing YAML config")
    parser.add_argument(
        "overrides",
        nargs="*",
        help="dotlist overrides, e.g. dataset.output_dir=data/other",
    )
    parser.add_argument("--log-level", default="INFO")
    args = parser.parse_args()

    logging.basicConfig(
        level=args.log_level.upper(),
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )

    config = load_config(args.config, args.overrides)
    summary = run_preprocess(config)
    print(
        f"done: {summary['processed']} utterances written to "
        f"{config.dataset.output_dir} ({summary['skipped']} skipped)"
    )


if __name__ == "__main__":
    main()
