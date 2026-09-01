#!/usr/bin/env python
"""Measure an exported voice through the host — the application, not PyTorch.

Two ways to sing, both through ``auris``, the command line frontend that drives the same
session the window does:

* **corpus** — validation utterances' own curves, sung by the host through the ``.onnx`` and
  held against the recordings and the curves; beside them the checkpoint singing the same
  curves in PyTorch, and the same utterances sung as one long *song* so the host's chunking
  and stitching are in the picture.
* **score** — notes and words through ``compose``, ``frames`` and ``sing``, the whole path a
  person walks, measured against the frames the session said it would sing.

Examples::

    uv run python scripts/evaluate_host.py --voice runs/small/model.onnx \\
        --checkpoint runs/small/checkpoints/last.ckpt --data data/processed/jsut_song \\
        --json before.json
    ... change something on either side, re-export, then ...
    uv run python scripts/evaluate_host.py --voice runs/small/model.onnx \\
        --checkpoint runs/small/checkpoints/last.ckpt --data data/processed/jsut_song \\
        --baseline before.json

    uv run python scripts/evaluate_host.py --voice runs/small/model.onnx --score
    uv run python scripts/evaluate_host.py --voice runs/small/model.onnx --score --asr

``--asr`` adds a listener: a recogniser in the voice's language (ReazonSpeech for Japanese)
transcribes every render, the transcript is turned back into IPA, and the phoneme error
rate against what was asked for is one more row — with the recording's own rate beside it
as the ceiling, in corpus mode.

The host is ``cargo run -p auris-cli`` from the repository root, or the binary ``AURIS_CLI``
names. ``--workdir`` keeps every file that crossed between the two languages — frames, WAVs,
the host's own reports — so any number in the table can be listened to.
"""

from __future__ import annotations

import argparse
import json
import logging
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from auris_singer.host import Host  # noqa: E402
from auris_singer.host_eval import Settings, evaluate, evaluate_score, format_report  # noqa: E402


def main() -> None:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--voice", required=True, type=Path, help="the exported .onnx")
    parser.add_argument("--checkpoint", type=Path, help="the checkpoint it was exported from (corpus mode)")
    parser.add_argument("--data", type=Path, help="a preprocessed dataset directory (corpus mode)")
    parser.add_argument("--score", action="store_true", help="sing notes and words instead of a corpus")
    parser.add_argument("--spec", type=Path, help="the .asong to compose in score mode (default: a built-in verse)")
    parser.add_argument("--split", choices=["val", "all"], default="val", help="which utterances (default: val)")
    parser.add_argument("--utterances", type=int, default=8, help="how many (default: 8)")
    parser.add_argument("--val-size", type=int, default=8, help="the training config's data.val_size (default: 8)")
    parser.add_argument("--seed", type=int, default=1234, help="the training config's seed, for the split (default: 1234)")
    parser.add_argument("--take-seed", type=int, default=0, help="the render's seed (default: 0)")
    parser.add_argument("--take-seeds", type=int, default=1, help="how many takes to average, seeds --take-seed onwards (default: 1)")
    parser.add_argument("--acceleration", choices=["auto", "gpu", "cpu"], default="auto")
    parser.add_argument("--no-song", action="store_true", help="skip the concatenated render")
    parser.add_argument("--no-reference", action="store_true", help="skip the PyTorch render")
    parser.add_argument("--no-pitch", action="store_true", help="skip FCPE, and so the pitch metrics")
    parser.add_argument("--gap", type=float, default=0.5, help="seconds of silence between song parts (default: 0.5)")
    parser.add_argument("--asr", action="store_true", help="also listen: the phoneme error rate, by a recogniser (needs the `asr` extra)")
    parser.add_argument("--asr-language", default="ja", help="the language the listener listens in (default: ja, ReazonSpeech)")
    parser.add_argument("--asr-precision", default="fp32", help="the recogniser's weights, fp32 or int8 (default: fp32)")
    parser.add_argument("--n-mels", type=int, default=128)
    parser.add_argument("--tolerance-cents", type=float, default=50.0)
    parser.add_argument("--device", default=None, help="torch device for alignment, reference and FCPE (default: cuda if present)")
    parser.add_argument("--release", action="store_true", help="drive cargo's release build, for honest timings")
    parser.add_argument("--workdir", type=Path, help="where the renders are kept (default: a temporary directory)")
    parser.add_argument("--json", type=Path, help="write the whole report here")
    parser.add_argument("--baseline", type=Path, help="a report to print deltas against")
    parser.add_argument("--log-level", default="INFO")
    args = parser.parse_args()

    logging.basicConfig(level=args.log_level.upper(), format="%(levelname)s %(name)s: %(message)s")
    if args.device is None:
        import torch

        args.device = "cuda" if torch.cuda.is_available() else "cpu"

    settings = Settings(
        split=args.split,
        utterances=args.utterances,
        seed=args.seed,
        val_size=args.val_size,
        take_seed=args.take_seed,
        take_seeds=args.take_seeds,
        acceleration=args.acceleration,
        song=not args.no_song,
        reference=not args.no_reference,
        pitch=not args.no_pitch,
        song_gap_seconds=args.gap,
        n_mels=args.n_mels,
        tolerance_cents=args.tolerance_cents,
        device=args.device,
        asr=args.asr,
        asr_language=args.asr_language,
        asr_options={"precision": args.asr_precision},
    )
    host = Host.find(release=args.release)

    if args.workdir is None:
        keep = tempfile.TemporaryDirectory(prefix="auris-host-eval-")
        workdir = Path(keep.name)
    else:
        keep = None
        workdir = args.workdir

    if args.score:
        report = evaluate_score(args.voice, host, workdir, spec=args.spec, settings=settings)
    else:
        if args.checkpoint is None or args.data is None:
            parser.error("corpus mode needs --checkpoint and --data (or pass --score)")
        report = evaluate(args.voice, args.data, args.checkpoint, host, workdir, settings=settings)

    baseline = json.loads(args.baseline.read_text(encoding="utf-8")) if args.baseline else None
    print(format_report(report, baseline))
    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")
        print(f"wrote {args.json}")
    if keep is not None:
        keep.cleanup()


if __name__ == "__main__":
    main()
