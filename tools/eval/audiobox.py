"""Run Meta's Audiobox Aesthetics on whole songs.

Four 1-10 axes from the official predictor: content enjoyment (CE), content
usefulness (CU), production complexity (PC) and production quality (PQ).  The
predictor scores consecutive 10-second windows and weights their mean by the
length of each window.  PC measures complexity, not quality, so it is never a
target to maximize.  Silence alone already gets a PQ near 6.7.

Audio is decoded with librosa and handed over as a tensor, which avoids the
current torchaudio file loader; the official file and tensor paths agree to
about 1e-6.  Run it with the interpreter of the pinned environment in
``.local-evaluations``.
"""

from __future__ import annotations

import argparse
import os
import time
from pathlib import Path
from typing import Any

from songeval import _json, digest, load_manifest

AXES = ("CE", "CU", "PC", "PQ")
SCHEMA_VERSION = 1


def run(args: argparse.Namespace) -> dict[str, Any]:
    """Score the manifest and write an auditable JSON report."""
    manifest = load_manifest(Path(args.manifest).resolve() if args.manifest else None, args.audio)
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["OMP_NUM_THREADS"] = str(args.threads)
    from importlib import metadata

    import librosa
    import torch
    from audiobox_aesthetics.infer import initialize_predictor

    torch.set_num_threads(args.threads)
    if torch.cuda.is_available():
        # The predictor takes the GPU whenever one is visible. On an RX 9070 XT (gfx1201) under
        # ROCm 7.14 its attention fails with hipErrorInvalidValue in the memory-efficient and
        # flash kernels, as Demucs does in song_aesthetics.py; the math kernel works.
        torch.backends.cuda.enable_mem_efficient_sdp(False)
        torch.backends.cuda.enable_flash_sdp(False)
    predictor = initialize_predictor()
    runtime = {
        "device": str(predictor.device),
        "torch_threads": torch.get_num_threads(),
        "upstream": "https://github.com/facebookresearch/audiobox-aesthetics",
        "packages": {n: metadata.version(n) for n in ("torch", "torchaudio", "librosa", "audiobox_aesthetics")},
        "preprocessing": "librosa decode at native rate; official resample to 16 kHz mono and 10 s windows",
    }
    rows: list[dict[str, Any]] = []

    def save() -> dict[str, Any]:
        report = {"schema_version": SCHEMA_VERSION, "model": "audiobox-aesthetics",
                  "axes": {"CE": "content enjoyment", "CU": "content usefulness",
                           "PC": "production complexity (not a quality)", "PQ": "production quality"},
                  "policy": {"automatic_selection": "disabled"}, "runtime": runtime, "rows": rows}
        _json(Path(args.output).resolve(), report)
        return report

    save()
    for row in manifest:
        start = time.perf_counter()
        current = dict(row)
        try:
            wav, rate = librosa.load(current["path"], sr=None, mono=False)
            tensor = torch.from_numpy(wav if wav.ndim == 2 else wav[None]).float()
            result = predictor.forward([{"path": tensor, "sample_rate": rate}])[0]
            current.update({"scores": {axis: round(float(result[axis]), 4) for axis in AXES}, "status": "scored"})
        except (OSError, RuntimeError, ValueError) as error:
            current.update({"status": "failed", "error": str(error)})
        current["elapsed_seconds"] = round(time.perf_counter() - start, 2)
        current["sha256_after"] = digest(Path(current["path"]))
        rows.append(current)
        save()
    return save()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("audio_paths", nargs="*", help="Audio files when --manifest is omitted")
    parser.add_argument("--manifest")
    parser.add_argument("--output", required=True)
    parser.add_argument("--threads", type=int, default=4)
    args = parser.parse_args()
    args.audio = args.audio_paths or None
    if bool(args.manifest) == bool(args.audio):
        parser.error("provide exactly one of --manifest or positional audio paths")
    report = run(args)
    if any(row.get("status") != "scored" for row in report["rows"]):
        raise SystemExit(2)


if __name__ == "__main__":
    main()
