"""Run the pinned multi-stem song aesthetics model on whole songs.

The model (Lv et al., "Song Aesthetics Evaluation with Multi-Stem Attention and
Hierarchical Uncertainty Modeling", ISMIR 2026) scores a song from aligned
mixture, vocal and accompaniment segments: 10 seconds long, 5 seconds apart,
mono at 24 kHz, each embedded with MuQ.  It reports five 0-100 scores.  Its
authors separate the stems with a source separator; this adapter does the same
with Demucs ``htdemucs`` for every input, rendered or recorded, so both kinds go
through one front end.

Segments whose vocal or accompaniment is mostly silent are skipped, as in the
official inference, so an instrumental has nothing to score and is reported as
failed rather than given a number.

Like ``songeval.py`` this keeps model imports out of module import; run it with
the interpreter of the pinned environment in ``.local-evaluations``.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

from songeval import _json, digest, load_manifest

AXES = ("Overall", "Singing", "Melody", "Arrange", "AudioQ")
SCHEMA_VERSION = 1
RATE = 24000
SEGMENT_SECONDS = 10
HOP_SECONDS = 5


def segments(length: int, rate: int = RATE) -> list[tuple[int, int]]:
    """Return the sample ranges of every whole segment the official data layout uses."""
    size, step = SEGMENT_SECONDS * rate, HOP_SECONDS * rate
    return [(start, start + size) for start in range(0, length - size + 1, step)]


def run(args: argparse.Namespace) -> dict[str, Any]:
    """Separate, segment and score the manifest, writing an auditable JSON report."""
    manifest = load_manifest(Path(args.manifest).resolve() if args.manifest else None, args.audio)
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["TRANSFORMERS_OFFLINE"] = "1"
    os.environ["OMP_NUM_THREADS"] = str(args.threads)
    os.environ["MKL_NUM_THREADS"] = str(args.threads)
    upstream = Path(args.upstream).resolve()
    sys.path.insert(0, str(upstream))
    if args.fast_transformers:
        sys.path.insert(0, str(Path(args.fast_transformers).resolve()))
    from importlib import metadata

    import librosa
    import numpy as np
    import pandas as pd
    import torch
    from core.model import AestheticScoringModel
    from core.utils import is_silent
    from demucs.apply import apply_model
    from demucs.pretrained import get_model
    from inference import Infer
    from muq import MuQ

    torch.set_num_threads(args.threads)
    torch.set_num_interop_threads(1)
    torch.manual_seed(0)
    checkpoint = upstream / "ckpt" / "model.pt"
    muq_path = Path(args.muq).resolve()
    revision = subprocess.run(
        ["git", "-C", str(upstream), "rev-parse", "HEAD"], check=True, capture_output=True, text=True
    ).stdout.strip()
    if revision != args.upstream_revision:
        raise ValueError(f"source revision mismatch: expected {args.upstream_revision}, got {revision}")
    model = AestheticScoringModel(device="cpu").to("cpu").eval()
    state = torch.load(checkpoint, map_location="cpu")
    model.load_state_dict(state.get("state_dict", state), strict=True)
    muq = MuQ.from_pretrained(str(muq_path), local_files_only=True).to("cpu").eval()
    separator = get_model("htdemucs").to("cpu").eval()
    vocals = separator.sources.index("vocals")
    runtime = {
        "device": "cpu",
        "torch_threads": torch.get_num_threads(),
        "upstream": "https://github.com/yisan33/song-aesthetics-evaluation",
        "upstream_revision": revision,
        "artifacts": {
            "checkpoint": digest(checkpoint),
            "muq": {n: digest(muq_path / n) for n in ("model.safetensors",) if (muq_path / n).is_file()},
            "source": {n: digest(upstream / n) for n in ("inference.py", "core/model.py", "core/utils.py")},
        },
        "packages": {n: metadata.version(n) for n in ("torch", "torchaudio", "librosa", "muq", "numpy", "soundfile", "transformers", "demucs")},
        "preprocessing_version": "song-aesthetics-adapter-v1",
        "preprocessing": "Demucs htdemucs at 44.1 kHz stereo (vocals; accompaniment = the other stems); "
        "mono 24 kHz; whole 10 s segments every 5 s; official silence skip, interval aggregation and MuQ hidden states",
    }
    rows: list[dict[str, Any]] = []
    report_path = Path(args.output).resolve()

    def save() -> dict[str, Any]:
        report = {
            "schema_version": SCHEMA_VERSION,
            "model": "song-aesthetics-evaluation",
            "axes": {axis: "Official output; 0-100, uncalibrated for rendered songs" for axis in AXES},
            "policy": {"automatic_selection": "disabled"},
            "runtime": runtime,
            "rows": rows,
        }
        _json(report_path, report)
        return report

    def embed(wav: np.ndarray) -> torch.Tensor:
        hidden = muq(torch.from_numpy(wav).float().unsqueeze(0), output_hidden_states=True).hidden_states
        return torch.stack(hidden).squeeze(1)

    save()
    for row in manifest:
        start = time.perf_counter()
        current = dict(row)
        try:
            info_seconds = librosa.get_duration(path=current["path"])
            if info_seconds > args.max_seconds:
                raise ValueError(f"audio is {info_seconds:.0f} s, longer than --max-seconds")
            stereo, _ = librosa.load(current["path"], sr=separator.samplerate, mono=False)
            if stereo.ndim == 1:
                stereo = np.stack([stereo, stereo])
            wav = torch.from_numpy(stereo[:2]).float()
            reference = wav.mean(0)
            mean, std = reference.mean(), reference.std() + 1e-8
            with torch.no_grad():
                stems = apply_model(separator, ((wav - mean) / std)[None], device="cpu", progress=False)[0]
            stems = stems * std + mean
            vocal = stems[vocals].mean(0).numpy()
            accompaniment = (stems.sum(0) - stems[vocals]).mean(0).numpy()
            mixture = wav.mean(0).numpy()
            mono = [librosa.resample(x, orig_sr=separator.samplerate, target_sr=RATE) for x in (mixture, vocal, accompaniment)]
            spans = segments(min(len(x) for x in mono))
            kept = [(a, b) for a, b in spans if not is_silent(mono[1][a:b]) and not is_silent(mono[2][a:b])]
            if not kept:
                raise ValueError("no segment has both a sounding vocal and accompaniment")
            batches = []
            with torch.no_grad():
                for i in range(0, len(kept), 32):
                    chunk = kept[i : i + 32]
                    batch = [torch.stack([embed(part[a:b]) for a, b in chunk]) for part in mono]
                    batches.append({"embeddings": batch[0], "embeddings_vocal": batch[1], "embeddings_inst": batch[2],
                                    "music_id": [current["id"]] * len(chunk)})
                with tempfile.TemporaryDirectory() as scratch:
                    Infer(model=model, dataset=batches, device="cpu", estim_dir=scratch).process()
                    table = pd.read_excel(Path(scratch) / f"{current['id']}.xlsx")
            scores = {axis: round(float(table.iloc[0][axis]), 4) for axis in AXES}
            if not all(np.isfinite(list(scores.values()))):
                raise ValueError("model produced a non-finite score")
            current.update({"scores": scores, "status": "scored", "segments": len(kept), "segments_total": len(spans)})
        except (OSError, RuntimeError, ValueError) as error:
            current.update({"status": "failed", "error": str(error)})
        current["elapsed_seconds"] = round(time.perf_counter() - start, 2)
        rows.append(current)
        save()
    return save()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("audio_paths", nargs="*", help="Audio files when --manifest is omitted")
    parser.add_argument("--manifest")
    parser.add_argument("--output", required=True)
    parser.add_argument("--upstream", required=True, help="Checkout of the official repository")
    parser.add_argument("--upstream-revision", required=True)
    parser.add_argument("--muq", required=True, help="Local OpenMuQ/MuQ-large-msd-iter snapshot")
    parser.add_argument("--fast-transformers", help="Directory holding an importable fast_transformers")
    parser.add_argument("--threads", type=int, default=4)
    parser.add_argument("--max-seconds", type=float, default=600.0)
    args = parser.parse_args()
    args.audio = args.audio_paths or None
    if bool(args.manifest) == bool(args.audio):
        parser.error("provide exactly one of --manifest or positional audio paths")
    if not 1 <= args.threads <= 16:
        parser.error("--threads must be between 1 and 16")
    report = run(args)
    if any(row.get("status") != "scored" for row in report["rows"]):
        raise SystemExit(2)


if __name__ == "__main__":
    main()
