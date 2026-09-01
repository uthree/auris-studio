#!/usr/bin/env python
"""Synthesize a waveform from an explicit control-curve input file.

The input is JSON produced by the DAW front-end:

    {
      "speaker": "my_singer",
      "phonemes": ["<sil>", "k", "o", "ɴ", "..."],
      "durations": [10, 6, 8, 5],
      "f0":     [0.0, 220.1, 220.4, ...],
      "energy": [0.0, 0.08, 0.09, ...]
    }

``durations`` counts frames per phoneme (100 frames per second at 48 kHz with
the default hop), and ``f0``/``energy`` must have ``sum(durations)`` entries.

Example:
    uv run python scripts/infer.py --checkpoint runs/base/checkpoints/last.ckpt \
        --input score.json --output out.wav
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

import soundfile as sf  # noqa: E402

from auris_singer.infer import Synthesizer  # noqa: E402


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--checkpoint", required=True)
    parser.add_argument("--input", required=True, help="JSON control file")
    parser.add_argument("--output", required=True, help="output .wav path")
    parser.add_argument("--device", default="cpu")
    parser.add_argument("--noise-scale", type=float, default=0.667)
    args = parser.parse_args()

    payload = json.loads(Path(args.input).read_text(encoding="utf-8"))
    synthesizer = Synthesizer.from_checkpoint(args.checkpoint, device=args.device)

    wav = synthesizer.synthesize(
        phonemes=payload["phonemes"],
        durations=payload["durations"],
        f0=payload["f0"],
        energy=payload["energy"],
        speaker=payload.get("speaker"),
        voiced=payload.get("voiced"),
        noise_scale=args.noise_scale,
    )

    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    sf.write(str(output), wav, synthesizer.sample_rate)
    print(f"wrote {output} ({wav.shape[0] / synthesizer.sample_rate:.2f} s)")


if __name__ == "__main__":
    main()
