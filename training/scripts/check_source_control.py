#!/usr/bin/env python
"""Check that synthesis actually responds to the source curves it is given.

The ``val/f0_*`` and ``val/energy_*`` metrics logged during training compare the
output against the *ground-truth* curves of the reference audio. That is a
necessary check but not a sufficient one: a model that ignored the excitation
entirely and reconstructed the utterance from the latent alone would still
score well, because the reference audio happens to have exactly that pitch.

This script closes the gap. It re-synthesizes one utterance several times with
**modified** curves — transposed pitch, rescaled energy — and measures whether
the output followed the modification. A model that memorized rather than
learned to be controlled shows a large error as soon as the curve moves away
from the reference.

Durations come from monotonic alignment search on the reference, so no manually
aligned input is needed.

Example:
    uv run python scripts/check_source_control.py \\
        --checkpoint runs/base/checkpoints/last.ckpt \\
        --dataset data/processed/jsut_song \\
        --output-dir runs/base/control_check
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

import numpy as np  # noqa: E402
import soundfile as sf  # noqa: E402
import torch  # noqa: E402

from auris_singer.lightning_module import AurisSingerModule  # noqa: E402
from auris_singer.metrics import energy_metrics, pitch_metrics  # noqa: E402
from auris_singer.preprocess.f0 import FcpeExtractor  # noqa: E402
from auris_singer.utils.audio import frame_energy, spectrogram  # noqa: E402


def build_conditions(
    semitones: list[float], energy_scales: list[float]
) -> list[tuple[str, float, float]]:
    """``(name, pitch ratio, energy scale)`` for each condition to render."""
    conditions = [("reference", 1.0, 1.0)]
    for shift in semitones:
        if shift == 0:
            continue
        sign = "up" if shift > 0 else "down"
        conditions.append((f"pitch_{sign}_{abs(shift):g}st", 2 ** (shift / 12), 1.0))
    for scale in energy_scales:
        if scale == 1.0:
            continue
        conditions.append((f"energy_x{scale:g}", 1.0, scale))
    return conditions


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--checkpoint", required=True)
    parser.add_argument("--dataset", required=True, help="preprocessed dataset dir")
    parser.add_argument("--output-dir", default=None, help="where to write the wavs")
    parser.add_argument("--index", type=int, default=0, help="utterance index")
    parser.add_argument("--device", default="cuda" if torch.cuda.is_available() else "cpu")
    parser.add_argument("--noise-scale", type=float, default=0.667)
    parser.add_argument(
        "--semitones", type=float, nargs="*", default=[-5.0, -2.0, 3.0, 7.0]
    )
    parser.add_argument("--energy-scales", type=float, nargs="*", default=[0.5, 2.0])
    parser.add_argument("--f0-min", type=float, default=40.0)
    parser.add_argument("--f0-max", type=float, default=1600.0)
    args = parser.parse_args()

    device = torch.device(args.device)
    module = AurisSingerModule.load_from_checkpoint(
        args.checkpoint, map_location="cpu"
    ).to(device)
    module.eval()
    model = module.model
    hop, sample_rate = module.hop_length, module.sample_rate
    extractor = FcpeExtractor(device=str(device), f0_min=args.f0_min, f0_max=args.f0_max)

    root = Path(args.dataset)
    records = [
        json.loads(line)
        for line in (root / "metadata.jsonl").read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    record = records[args.index % len(records)]
    data = np.load(root / record["path"])

    wav = torch.from_numpy(data["wav"].astype(np.float32) / 32768.0)
    n_frames = wav.numel() // hop
    phonemes = torch.from_numpy(data["phonemes"].astype(np.int64))[None].to(device)
    phoneme_lengths = torch.tensor([phonemes.shape[1]], device=device)
    f0 = torch.from_numpy(data["f0"].astype(np.float32))[None].to(device)
    energy = torch.from_numpy(data["energy"].astype(np.float32))[None].to(device)
    voiced = torch.from_numpy(data["voiced"].astype(np.float32))[None].to(device)
    speaker = torch.tensor([record.get("speaker_id", 0)], device=device)

    with torch.no_grad():
        spec = spectrogram(wav, module.n_fft, hop, module.win_length)[None].to(device)
        out = model(
            phonemes,
            phoneme_lengths,
            spec,
            torch.tensor([n_frames], device=device),
            f0,
            energy,
            voiced,
            speaker,
        )
    durations = out["durations"].round().long()

    output_dir = Path(args.output_dir) if args.output_dir else None
    if output_dir:
        output_dir.mkdir(parents=True, exist_ok=True)

    print(
        f"utterance {record['id']}: {n_frames} frames, "
        f"{phonemes.shape[1]} phonemes, speaker {record.get('speaker', 0)}"
    )
    print(
        f"{'condition':<20}{'f0 err (cent)':>15}{'f0 acc':>9}{'f0 corr':>9}"
        f"{'energy bias (dB)':>19}{'energy corr':>13}"
    )

    for name, pitch_ratio, energy_scale in build_conditions(
        args.semitones, args.energy_scales
    ):
        target_f0 = f0 * pitch_ratio
        target_energy = energy * energy_scale
        with torch.no_grad():
            generated = model.infer(
                phonemes,
                phoneme_lengths,
                durations,
                target_f0,
                target_energy,
                (target_f0 > 0).float(),
                speaker,
                noise_scale=args.noise_scale,
            )
        length = generated.size(-1) // hop
        mono = generated.squeeze(1)
        measured_f0, measured_voiced = extractor(mono, sample_rate, length)
        measured_energy = frame_energy(mono, module.n_fft, hop, module.win_length)
        valid = torch.ones(1, length, device=device)

        pitch = pitch_metrics(
            target_f0[:, :length],
            (target_f0[:, :length] > 0).float(),
            measured_f0.to(device),
            measured_voiced.to(device),
            valid,
        )
        loudness = energy_metrics(target_energy[:, :length], measured_energy, valid)
        print(
            f"{name:<20}{pitch['f0_rmse_cent']:>15.1f}{pitch['f0_accuracy']:>9.3f}"
            f"{pitch['f0_corr']:>9.4f}{loudness['energy_bias_db']:>19.2f}"
            f"{loudness['energy_corr']:>13.3f}"
        )
        if output_dir:
            sf.write(
                output_dir / f"{name}.wav",
                mono.squeeze(0).float().cpu().numpy(),
                sample_rate,
            )

    if output_dir:
        print(f"\nwrote {output_dir}")


if __name__ == "__main__":
    main()
