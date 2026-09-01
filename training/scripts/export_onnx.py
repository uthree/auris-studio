#!/usr/bin/env python
"""Export a trained checkpoint to ONNX, for runtimes like onnxruntime / ort.

The exported graph is the inference path as a pure function: alongside the
control curves it takes the noise as explicit inputs (``z_noise`` standard
normal, ``source_noise`` uniform on [-1, 1]), so a caller that seeds its own
generator gets reproducible renders. The phoneme table, the speaker map and
the audio parameters ride along inside the file's metadata and in a ``.json``
sidecar.

Example:
    uv run python scripts/export_onnx.py \
        --checkpoint runs/base/checkpoints/last.ckpt --output runs/base/model.onnx

Needs the ``export`` extra: ``uv pip install -e '.[export]'``.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from auris_singer.export import export_onnx, load_portrait, verify_onnx  # noqa: E402
from auris_singer.lightning_module import AurisSingerModule  # noqa: E402


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--checkpoint", required=True)
    parser.add_argument("--output", required=True, help="output .onnx path")
    parser.add_argument("--opset", type=int, default=18)
    parser.add_argument(
        "--no-verify",
        action="store_true",
        help="skip the onnxruntime comparison against PyTorch",
    )
    parser.add_argument(
        "--voice-card",
        type=Path,
        help="JSON file with presentational fields shown by a host UI "
        "(name, description, author, license, credits, ...)",
    )
    parser.add_argument(
        "--portrait",
        type=Path,
        help="character image embedded into the voice card (png/jpeg/webp, <=8 MB)",
    )
    parser.add_argument(
        "--phoneme-durations",
        type=Path,
        help="JSON file from scripts/measure_phoneme_durations.py: how long each "
        "consonant should be given to this voice",
    )
    args = parser.parse_args()

    module = AurisSingerModule.load_from_checkpoint(args.checkpoint, map_location="cpu")
    metadata = dict(module.hparams.get("metadata") or {})

    voice = {}
    if args.voice_card:
        voice = json.loads(args.voice_card.read_text(encoding="utf-8"))
        if not isinstance(voice, dict):
            raise SystemExit("--voice-card must contain a JSON object")
    if args.portrait:
        voice["portrait"] = load_portrait(args.portrait)

    durations = None
    if args.phoneme_durations:
        durations = json.loads(args.phoneme_durations.read_text(encoding="utf-8"))
        if not isinstance(durations, dict) or not isinstance(durations.get("seconds"), dict):
            raise SystemExit("--phoneme-durations must contain a JSON object with a 'seconds' map")

    output = Path(args.output)
    export_onnx(
        module.model,
        output,
        metadata=metadata,
        opset=args.opset,
        voice=voice or None,
        phoneme_durations=durations,
    )
    size_mb = output.stat().st_size / 1e6
    print(f"wrote {output} ({size_mb:.1f} MB) and {output.with_suffix('.json').name}")

    if not args.no_verify:
        errors = verify_onnx(module.model, output)
        print(
            "verified against PyTorch: "
            f"unvoiced max diff {errors['unvoiced_max_diff']:.2e}, "
            f"voiced max diff {errors['voiced_max_diff']:.2e} (same excitation)"
        )


if __name__ == "__main__":
    main()
