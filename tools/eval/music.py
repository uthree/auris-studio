# /// script
# requires-python = ">=3.11,<3.12"
# dependencies = [
#   "torch==2.7.1", "torchaudio==2.7.1", "torchvision==0.22.1",
#   "numpy==1.23.5", "scipy==1.13.1", "librosa==0.10.2.post1",
#   "soundfile==0.13.1", "transformers==4.57.6", "huggingface-hub==0.36.2",
#   "laion-clap==1.1.7", "muq==0.1.0",
# ]
# ///
"""Development-only TuneJury preference and MuQ-MuLan music/text evaluation.

    uv run tools/eval/music.py target/before --json before.json
    uv run tools/eval/music.py target/after --baseline before.json --json after.json
    uv run tools/eval/music.py --preset all --seeds 3 --json before.json

Frozen ten-second windows, empty-prompt TuneJury, and MuQ-MuLan positive/contrast
cosines. Reports retain model/source revisions, hashes, prompts, and PCM hashes.
No learned evaluator is shipped in the desktop application.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import statistics
from pathlib import Path

import numpy as np

RATE = 24_000
WINDOW = 10 * RATE
MANIFEST = Path(__file__).with_name("music_prompts.json")
METRICS = ("tunejury_reward", "positive_cosine", "contrast_cosine", "contrast_margin")
import render_audio
from learned_models import LearnedModels


def sha256(path: Path) -> str:
    """Hash an artifact without reading a large model into memory at once."""
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def segment_starts(length: int, count: int = 3) -> list[int]:
    """Evenly span first to last complete window; deduplicate short inputs."""
    if length < 1 or count < 1:
        raise ValueError("audio length and segment count must be positive")
    last = max(0, length - WINDOW)
    if count == 1:
        return [last // 2]
    return sorted({index * last // (count - 1) for index in range(count)})


def repeat_pad(data: np.ndarray) -> np.ndarray:
    """Repeat complete short clips, then zero-pad the remainder, for the fixed ten-second model input."""
    if not 0 < len(data) <= WINDOW:
        raise ValueError("expected a nonempty excerpt of at most ten seconds")
    repeated = np.tile(data, WINDOW // len(data))
    return np.pad(repeated, (0, WINDOW - len(repeated))).astype(np.float32)


def mono_resample(data: np.ndarray, rate: int) -> np.ndarray:
    """Average channels, then bandlimited polyphase resample to 24 kHz."""
    from scipy.signal import resample_poly

    if data.ndim != 2 or not len(data) or data.shape[1] < 1 or rate < 1:
        raise ValueError("expected nonempty frames-by-channels audio and valid rate")
    if not np.isfinite(data).all():
        raise ValueError("audio contains nonfinite samples")
    mono = data.mean(axis=1, dtype=np.float64)
    if rate != RATE:
        divisor = math.gcd(rate, RATE)
        mono = resample_poly(mono, RATE // divisor, rate // divisor)
    if not np.isfinite(mono).all():
        raise ValueError("resampled audio contains nonfinite samples")
    return mono.astype(np.float32)


def load_manifest(path: Path) -> dict:
    """Validate named positive and contrasting prompts before running a model."""
    manifest = json.loads(path.read_text(encoding="utf-8"))
    if manifest.get("version") != 1 or not manifest.get("presets"):
        raise ValueError("expected version 1 prompt manifest with presets")
    for name, profile in manifest["presets"].items():
        for group in ("positive", "contrast"):
            prompts = profile.get(group, [])
            ids = [prompt.get("id") for prompt in prompts]
            if (
                not prompts
                or any(not isinstance(item, str) or not item.strip() for item in ids)
                or len(ids) != len(set(ids))
                or any(
                    not isinstance(prompt.get("text"), str)
                    or not prompt["text"].strip()
                    for prompt in prompts
                )
            ):
                raise ValueError(f"invalid {group} prompts for {name}")
    return manifest


def preset_for(path: Path, profiles: dict, override: str | None = None) -> str:
    """Recognize CLI preset filenames, including the fixed additional seeds."""
    preset = override or re.sub(r"-s\d+$", "", path.stem)
    if preset not in profiles:
        raise ValueError(f"no prompt profile for {path.name}; use --preset")
    return preset


def cosine_scores(audio: np.ndarray, texts: np.ndarray, positive_count: int) -> dict:
    """Normalize embeddings explicitly and retain each prompt's cosine."""
    if not 0 < positive_count < len(texts):
        raise ValueError("positive and contrast embeddings are both required")
    if not np.isfinite(audio).all() or not np.isfinite(texts).all():
        raise ValueError("model returned nonfinite embeddings")
    audio_norm = np.linalg.norm(audio)
    text_norms = np.linalg.norm(texts, axis=1)
    if audio_norm <= 0 or (text_norms <= 0).any():
        raise ValueError("model returned a zero embedding")
    cosines = np.clip((texts / text_norms[:, None]) @ (audio / audio_norm), -1, 1)
    positive = float(np.mean(cosines[:positive_count]))
    contrast = float(np.mean(cosines[positive_count:]))
    return {
        "positive_cosine": positive,
        "contrast_cosine": contrast,
        "contrast_margin": positive - contrast,
        "prompt_cosines": [float(value) for value in cosines],
    }


def aggregate(segments: list[dict]) -> dict | None:
    """Average valid windows equally; never turn invalid windows into zeros."""
    valid = [row for row in segments if row["status"] == "ok"]
    if not valid:
        return None
    result = {
        metric: statistics.mean(row[metric] for row in valid) for metric in METRICS
    }
    result["positive_cosine_stddev"] = statistics.pstdev(
        row["positive_cosine"] for row in valid
    )
    result["contrast_margin_stddev"] = statistics.pstdev(
        row["contrast_margin"] for row in valid
    )
    return result


def score_file(path: Path, preset: str, profile: dict, backend, count: int) -> dict:
    """Score deterministic excerpts, reporting invalid audio and silent windows."""
    import soundfile

    result = {"path": str(path.resolve()), "sha256": sha256(path), "preset": preset}
    try:
        data, rate = soundfile.read(path, dtype="float32", always_2d=True)
        mono = mono_resample(data, rate)
    except (ValueError, RuntimeError) as error:
        return {
            **result,
            "status": "invalid",
            "reason": str(error),
            "segments": [],
            "aggregate": None,
        }
    result.update(
        original_sample_rate=rate,
        original_channels=data.shape[1],
        duration_seconds=len(data) / rate,
        resampled_frames=len(mono),
    )
    segments = []
    text_embeddings = None
    for start in segment_starts(len(mono), count):
        excerpt = mono[start : start + WINDOW]
        rms = float(np.sqrt(np.mean(excerpt.astype(np.float64) ** 2)))
        # TuneJury's CLAP encoder truncates to signed int16. Very quiet nonzero float WAVs
        # may therefore become exact silence at the model's input.
        quantized_zero = not np.any(np.abs(excerpt) >= (1.0 / 32767.0))
        segment = {
            "start_seconds": start / RATE,
            "source_duration_seconds": len(excerpt) / RATE,
            "rms": rms,
            "peak": float(np.max(np.abs(excerpt))),
            "samples_outside_unit_range": int(np.count_nonzero(np.abs(excerpt) > 1)),
            "quantized_to_silence": quantized_zero,
            "status": "silent" if rms <= 1e-7 or quantized_zero else "ok",
        }
        if segment["status"] == "ok":
            try:
                if text_embeddings is None:
                    text_embeddings = backend.text_embeddings(profile)
                padded = repeat_pad(excerpt)
                audio = backend.audio_embedding(padded)
                reward = backend.reward(padded)
                if not math.isfinite(reward):
                    raise ValueError("model returned nonfinite reward")
                segment["tunejury_reward"] = reward
                segment.update(
                    cosine_scores(audio, text_embeddings, len(profile["positive"]))
                )
            except (ValueError, RuntimeError) as error:
                segment.update(status="invalid_embedding", reason=str(error))
        segments.append(segment)
    valid = sum(row["status"] == "ok" for row in segments)
    invalid = any(row["status"] == "invalid_embedding" for row in segments)
    result.update(
        status="ok"
        if valid == len(segments)
        else "partial"
        if valid
        else "invalid"
        if invalid
        else "silent",
        valid_segments=valid,
        segments=segments,
        aggregate=aggregate(segments),
    )
    return result


def compare(report: dict, baseline: dict) -> dict:
    """Reject changed measurement conditions and compare only matching labels."""
    for key in ("schema_version", "prompts_sha256", "preprocessing"):
        if report[key] != baseline.get(key):
            raise ValueError(f"baseline uses different {key}")
    for key in (
        "tunejury_source_revision",
        "artifacts",
        "packages",
        "device",
        "threads",
    ):
        if report["model"][key] != baseline.get("model", {}).get(key):
            raise ValueError(f"baseline uses different model {key}")
    differences = {}
    for label, row in report["files"].items():
        previous = baseline.get("files", {}).get(label)
        if previous is None:
            differences[label] = {"status": "missing_baseline"}
        elif previous["preset"] != row["preset"]:
            raise ValueError(f"baseline uses different prompt profile for {label}")
        elif row["status"] != "ok" or previous["status"] != "ok":
            differences[label] = {"status": "invalid_or_silent_segments"}
        else:
            differences[label] = {
                "status": "ok",
                "delta": {
                    metric: row["aggregate"][metric] - previous["aggregate"][metric]
                    for metric in METRICS
                },
                "same_excerpt_positions": [
                    item["start_seconds"] for item in row["segments"]
                ]
                == [item["start_seconds"] for item in previous["segments"]],
            }
    return differences


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("wavs", nargs="*", type=Path, help="WAV files or folders")
    parser.add_argument(
        "--preset",
        action="append",
        default=[],
        help="render these presets when no WAVs are supplied; otherwise select one prompt profile",
    )
    parser.add_argument("--seeds", type=int, default=1)
    parser.add_argument("--cli", type=Path)
    parser.add_argument(
        "--workdir", type=Path, default=Path("target/composition-eval/renders")
    )
    parser.add_argument(
        "--json", required=True, type=Path, help="save complete measurement report"
    )
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--prompts", type=Path, default=MANIFEST)
    parser.add_argument("--segments", type=int, default=3)
    parser.add_argument("--threads", type=int, default=4)
    parser.add_argument("--device", default="cpu", choices=("cpu", "cuda"))
    parser.add_argument(
        "--cache", type=Path, default=Path("target/composition-eval/models")
    )
    args = parser.parse_args()
    if args.segments < 1 or args.threads < 1 or not 1 <= args.seeds <= 8:
        parser.error("--segments and --threads must be positive")
    wavs = []
    if not args.wavs and args.preset:
        render_audio.CLI = args.cli.resolve() if args.cli else None
        presets = render_audio.preset_names() if "all" in args.preset else args.preset
        args.workdir.mkdir(parents=True, exist_ok=True)
        wavs = render_audio.render_presets(presets, args.seeds, args.workdir.resolve())
    for path in args.wavs:
        if path.is_dir():
            wavs.extend(sorted(path.rglob("*.wav")))
        elif path.is_file() and path.suffix.lower() == ".wav":
            wavs.append(path)
        else:
            parser.error(f"not a WAV or folder: {path}")
    if not wavs:
        parser.error("no WAV files found")
    labels = [path.stem for path in wavs]
    if len(labels) != len(set(labels)):
        parser.error(
            "duplicate WAV stems; score each baseline/after directory separately"
        )
    try:
        manifest = load_manifest(args.prompts)
        profiles = manifest["presets"]
        presets = [
            preset_for(
                path,
                profiles,
                args.preset[0] if args.wavs and len(args.preset) == 1 else None,
            )
            for path in wavs
        ]
    except ValueError as error:
        parser.error(str(error))
    backend = LearnedModels(args.cache, args.device, args.threads)
    report = {
        "schema_version": 2,
        "interpretation": "TuneJury uncalibrated preference reward (empty prompt); MuQ-MuLan identity cosines, not probability.",
        "prompts_sha256": sha256(args.prompts),
        "prompts": manifest,
        "model": backend.provenance,
        "preprocessing": {
            "sample_rate": RATE,
            "channels": "arithmetic mean before resampling",
            "resampler": "scipy.signal.resample_poly default Kaiser window",
            "normalization": "none; TuneJury internal CLAP clips/quantizes; MuQ-MuLan uses float32",
            "tunejury_prompt": "empty text branch (512 zeros), official waveform API",
            "window_samples": WINDOW,
            "window_selection": "equally spaced first-to-last complete window; one segment uses center",
            "requested_segments": args.segments,
            "short_audio": "repeat complete copies, zero-pad remainder",
            "silent_rms_threshold": 1e-7,
            "aggregation": "unweighted mean of valid excerpt rewards and cosines",
        },
        "files": {},
    }
    for path, preset in zip(wavs, presets, strict=True):
        row = score_file(path, preset, profiles[preset], backend, args.segments)
        report["files"][path.stem] = row
        if row["aggregate"]:
            scores = row["aggregate"]
            print(
                f"{path.stem:24} {row['status']:8} reward={scores['tunejury_reward']:.4f} cosine={scores['positive_cosine']:.4f} margin={scores['contrast_margin']:+.4f}",
                flush=True,
            )
        else:
            print(f"{path.stem:24} {row['status']}", flush=True)
    if args.baseline:
        try:
            report["comparison"] = compare(
                report, json.loads(args.baseline.read_text(encoding="utf-8"))
            )
        except ValueError as error:
            parser.error(str(error))
    args.json.parent.mkdir(parents=True, exist_ok=True)
    args.json.write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n", encoding="utf-8"
    )
    print(f"Wrote {args.json}", flush=True)
    if any(row["status"] != "ok" for row in report["files"].values()):
        raise SystemExit(2)


if __name__ == "__main__":
    main()
