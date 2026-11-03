"""Run the pinned local SongEval checkpoint on a reproducible audio manifest.

This adapter deliberately keeps model dependencies out of module import.  The
normal evaluation environment is the pinned SongEval virtual environment in
``.local-evaluations``; callers can invoke this file with that interpreter.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

AXES = ("Coherence", "Musicality", "Memorability", "Clarity", "Naturalness")
SCHEMA_VERSION = 1


def digest(path: Path) -> str:
    """Return the SHA-256 digest of *path*."""
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def _json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def load_manifest(path: Path, audio: list[str] | None = None) -> list[dict[str, Any]]:
    """Load and validate an input manifest, assigning collision-free IDs.

    A manifest is a JSON array of objects with ``path`` and optional ``id``,
    ``condition`` and ``group`` fields.  ``--audio`` is a convenience equivalent
    that creates entries with the file stem as their initial identity.
    """
    if audio:
        raw = [{"path": item} for item in audio]
    else:
        payload = json.loads(path.read_text(encoding="utf-8"))
        raw = payload.get("inputs", payload) if isinstance(payload, dict) else payload
        if not isinstance(raw, list):
            raise ValueError("manifest must be a JSON array or an object containing inputs")
    result: list[dict[str, Any]] = []
    used: set[str] = set()
    for index, item in enumerate(raw):
        if not isinstance(item, dict) or not isinstance(item.get("path"), str):
            raise TypeError(f"manifest item {index} must contain a string path")
        source = Path(item["path"]).expanduser().resolve(strict=True)
        if source.suffix.lower() not in {".wav", ".mp3"}:
            raise ValueError(f"unsupported audio extension: {source}")
        identity = str(item.get("id") or source.stem)
        base = identity
        suffix = 2
        while identity in used:
            identity = f"{base}-{suffix}"
            suffix += 1
        used.add(identity)
        row = dict(item)
        row.update({"id": identity, "path": str(source), "sha256": digest(source)})
        result.append(row)
    if not result:
        raise ValueError("manifest contains no audio inputs")
    return result


def audio_diagnostic(path: Path, max_seconds: float = 120.0) -> dict[str, Any]:
    """Read cheap PCM diagnostics without importing a model dependency."""
    import numpy as np
    import soundfile as sf

    info = sf.info(path)
    if info.duration > max_seconds:
        return {"seconds": float(info.duration), "sample_rate": info.samplerate,
                "channels": info.channels, "frames": info.frames, "peak": None,
                "rms": None, "admissible": False, "silence_reason": "too_long"}
    peak = 0.0
    sum_sq = 0.0
    count = 0
    with sf.SoundFile(path) as stream:
        for block in stream.blocks(blocksize=262144, dtype="float32", always_2d=True):
            if not bool(np.isfinite(block).all()):
                return {"seconds": float(info.duration), "sample_rate": info.samplerate,
                        "channels": info.channels, "frames": info.frames, "peak": None,
                        "rms": None, "admissible": False, "silence_reason": "nonfinite_pcm"}
            peak = max(peak, float(abs(block).max(initial=0.0)))
            sum_sq += float((block * block).sum())
            count += block.size
    rms = math.sqrt(sum_sq / count) if count else 0.0
    return {
        "seconds": float(info.duration),
        "sample_rate": info.samplerate,
        "channels": info.channels,
        "frames": info.frames,
        "peak": peak,
        "rms": rms,
        "admissible": bool(count and peak > 1e-6 and rms > 1e-7),
        "silence_reason": None if peak > 1e-6 and rms > 1e-7 else "silent_or_empty",
    }


def _comparisons(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    grouped: dict[str, list[dict[str, Any]]] = {}
    for row in rows:
        group = row.get("group")
        if group:
            grouped.setdefault(str(group), []).append(row)
    output = []
    for group, members in sorted(grouped.items()):
        conditions = [str(row.get("condition")) for row in members]
        if len(conditions) != len(set(conditions)):
            raise ValueError(f"comparison group {group!r} has duplicate conditions")
        by_condition = dict(zip(conditions, members))
        if not {"before", "after"}.issubset(by_condition):
            output.append({"group": group, "status": "incomplete", "members": [r["id"] for r in members]})
            continue
        before, after = by_condition["before"], by_condition["after"]
        if before.get("status") != "scored" or after.get("status") != "scored":
            output.append({"group": group, "status": "not_scored", "members": [before["id"], after["id"]]})
            continue
        output.append({
            "group": group,
            "status": "paired",
            "members": [before["id"], after["id"]],
            "delta_after_minus_before": {
                axis: round(after["scores"][axis] - before["scores"][axis], 4) for axis in AXES
            },
            "selection": "none",  # Scores are evidence, never an automatic winner.
        })
    return output


def run(args: argparse.Namespace) -> dict[str, Any]:
    """Evaluate the manifest and write an auditable JSON report."""
    manifest = load_manifest(Path(args.manifest).resolve() if args.manifest else None, args.audio)
    for row in manifest:
        try:
            row["diagnostics"] = audio_diagnostic(Path(row["path"]), args.max_seconds)
        except (OSError, RuntimeError, ValueError) as error:
            row["diagnostics"] = {"admissible": False, "silence_reason": f"diagnostic_error: {error}"}
    # Keep the environment deterministic and bounded before importing torch.
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["TRANSFORMERS_OFFLINE"] = "1"
    os.environ["OMP_NUM_THREADS"] = str(args.threads)
    os.environ["MKL_NUM_THREADS"] = str(args.threads)
    sys.path.insert(0, str(Path(args.upstream).resolve()))
    from importlib import metadata

    import librosa
    import numpy as np
    import torch
    from hydra.utils import instantiate
    from muq import MuQ
    from omegaconf import OmegaConf
    from safetensors.torch import load_file

    torch.set_num_threads(args.threads)
    torch.set_num_interop_threads(1)
    torch.manual_seed(0)
    upstream = Path(args.upstream).resolve()
    checkpoint = Path(args.checkpoint).resolve()
    muq_path = Path(args.muq).resolve()
    config = OmegaConf.load(upstream / "config.yaml")
    device = torch.device(args.device)
    model = instantiate(config.generator).to(device).eval()
    model.load_state_dict(load_file(str(checkpoint), device=str(device)), strict=True)
    muq = MuQ.from_pretrained(str(muq_path), local_files_only=True).to(device).eval()
    source_files = {
        name: digest(upstream / name) for name in ("model.py", "eval.py", "config.yaml")
    }
    git_revision = subprocess.run(
        ["git", "-C", str(upstream), "rev-parse", "HEAD"],
        check=True, capture_output=True, text=True,
    ).stdout.strip()
    if args.upstream_revision and git_revision != args.upstream_revision:
        raise ValueError(f"SongEval source revision mismatch: expected {args.upstream_revision}, got {git_revision}")
    muq_weights = {
        name: digest(muq_path / name)
        for name in ("model.safetensors", "pytorch_model.bin")
        if (muq_path / name).is_file()
    }
    runtime = {
        "device": str(device), "torch_threads": torch.get_num_threads(),
        "upstream": "https://github.com/ASLP-lab/SongEval",
        "upstream_revision": git_revision, "requested_upstream_revision": args.upstream_revision,
        "artifacts": {"checkpoint": digest(checkpoint), "muq": muq_weights,
                      "muq_config": digest(muq_path / "config.json"), "source": source_files},
        "packages": {name: metadata.version(name) for name in ("torch", "torchaudio", "librosa", "muq", "numpy", "soundfile", "transformers", "hydra-core", "huggingface-hub", "safetensors", "scipy")},
        "preprocessing_version": "songeval-adapter-v2",
        "preprocessing": "librosa mono 24 kHz; MuQ hidden_states[6]; no loudness normalization",
    }
    rows: list[dict[str, Any]] = []
    report_path = Path(args.output).resolve()
    def save_partial() -> None:
        _json(report_path, {"schema_version": SCHEMA_VERSION, "model": "SongEval",
            "axes": {axis: "Official SongEval output; uncalibrated 1-5-like regression" for axis in AXES},
            "policy": {"automatic_selection": "disabled", "reason": "Use paired deltas as diagnostics; retain before/after audio and human review."},
            "runtime": runtime, "rows": rows, "comparisons": _comparisons(rows)})
    save_partial()
    for row in manifest:
        start = time.perf_counter()
        current = dict(row)
        try:
            if not current["diagnostics"]["admissible"]:
                raise ValueError("audio rejected by silence/empty diagnostic")
            wav, _ = librosa.load(current["path"], sr=24000, mono=True)
            with torch.no_grad():
                hidden = muq(torch.tensor(wav, dtype=torch.float32, device=device).unsqueeze(0), output_hidden_states=True)["hidden_states"][6]
                values = model(hidden).squeeze(0).tolist()
            if len(values) != len(AXES):
                raise ValueError(f"SongEval returned {len(values)} scores, expected {len(AXES)}")
            scores = {axis: round(float(value), 4) for axis, value in zip(AXES, values)}
            if not all(np.isfinite(list(scores.values()))) or not all(1.0 <= value <= 5.0 for value in scores.values()):
                raise ValueError("SongEval produced a non-finite or out-of-range score")
            current.update({"scores": scores, "status": "scored"})
        except (OSError, RuntimeError, ValueError) as error:
            current.update({"status": "failed", "error": str(error)})
        current["elapsed_seconds"] = round(time.perf_counter() - start, 4)
        current["sha256_after"] = digest(Path(current["path"]))
        if current["sha256_after"] != current["sha256"]:
            current.update({"status": "failed", "error": "input changed during inference"})
        rows.append(current)
        save_partial()
    report = {
        "schema_version": SCHEMA_VERSION,
        "model": "SongEval",
        "axes": {axis: "Official SongEval output; uncalibrated 1-5-like regression" for axis in AXES},
        "policy": {
            "automatic_selection": "disabled",
            "reason": "Use paired deltas as diagnostics; retain before/after audio and human review.",
        },
        "runtime": runtime,
        "rows": rows,
        "comparisons": _comparisons(rows),
    }
    _json(Path(args.output).resolve(), report)
    return report


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("audio_paths", nargs="*", help="Audio files when --manifest is omitted")
    parser.add_argument("--config", help="JSON file containing model paths and optional input metadata")
    parser.add_argument("--manifest")
    parser.add_argument("--audio", nargs="*")
    parser.add_argument("--output", required=True)
    parser.add_argument("--upstream")
    parser.add_argument("--checkpoint")
    parser.add_argument("--muq")
    parser.add_argument("--upstream-revision")
    parser.add_argument("--threads", type=int, default=4)
    parser.add_argument("--device", default="cpu", help="Torch device, such as cpu or cuda (ROCm builds use cuda too)")
    parser.add_argument("--max-seconds", type=float, default=120.0)
    args = parser.parse_args()
    config: dict[str, Any] = {}
    if args.config:
        config = json.loads(Path(args.config).read_text(encoding="utf-8"))
        if not isinstance(config, dict):
            parser.error("--config must contain a JSON object")
        for key in ("upstream", "checkpoint", "muq", "upstream_revision", "threads"):
            if key in config and getattr(args, key) == parser.get_default(key):
                setattr(args, key, config[key])
        if args.manifest is None and args.audio is None and not args.audio_paths and config.get("inputs"):
            args.manifest = args.config
    if args.audio_paths:
        args.audio = args.audio_paths
    if bool(args.manifest) == bool(args.audio):
        parser.error("provide exactly one of --manifest, --audio, or positional audio paths")
    if not args.upstream or not args.checkpoint or not args.muq or not args.upstream_revision:
        parser.error("--upstream, --checkpoint, --muq and --upstream-revision are required (or provide them in --config)")
    if not 1 <= args.threads <= 16:
        parser.error("--threads must be between 1 and 16")
    if args.max_seconds <= 0:
        parser.error("--max-seconds must be positive")
    report = run(args)
    if any(row.get("status") != "scored" for row in report["rows"]):
        raise SystemExit(2)


if __name__ == "__main__":
    main()
