# /// script
# requires-python = ">=3.11"
# dependencies = [
#   "laion-clap==1.1.7", "torch==2.6.0", "torchaudio==2.6.0",
#   "torchvision==0.21.0", "transformers>=4.44,<5", "huggingface-hub<1",
#   "soundfile>=0.12", "numpy<2", "scipy>=1.11",
# ]
# ///
"""Local LAION CLAP text/audio similarity for final rendered WAVs.

Uses the official music + AudioSet HTSAT-base checkpoint, not the smaller general
audio model. Scores are cosine similarities and positive-minus-contrast margins,
not probabilities, aesthetic ratings, or evidence that a composition improved.
The fixed prompt manifest describes preset identity and must be frozen before A/B.

    python tools/eval/clap.py target/before --json before-clap.json
    python tools/eval/clap.py target/after --baseline before-clap.json --json after-clap.json

All audio inference is local. Only public model/tokenizer files are downloaded.
Three deterministic ten-second windows span each file by default; this does not
measure full-song development. Short audio is repeat-padded as in native CLAP.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import json
import math
import os
import platform
import re
import statistics
from pathlib import Path

import numpy as np

RATE = 48_000
WINDOW = 10 * RATE
MODEL_REPO = "lukewys/laion_clap"
MODEL_REVISION = "b3708341862f581175dba5c356a4ebf74a9b6651"
MODEL_FILE = "music_audioset_epoch_15_esc_90.14.pt"
MODEL_SHA256 = "fae3e9c087f2909c28a09dc31c8dfcdacbc42ba44c70e972b58c1bd1caf6dedd"
TOKENIZER_REVISION = "e2da8e2f811d1448a5b465c236feacd80ffbac7b"
MANIFEST = Path(__file__).with_name("clap_prompts.json")
METRICS = ("positive_cosine", "contrast_cosine", "contrast_margin")


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
    """Repeat complete short clips, then zero-pad the remainder, like CLAP."""
    if not 0 < len(data) <= WINDOW:
        raise ValueError("expected a nonempty excerpt of at most ten seconds")
    repeated = np.tile(data, WINDOW // len(data))
    return np.pad(repeated, (0, WINDOW - len(repeated))).astype(np.float32)


def mono_resample(data: np.ndarray, rate: int) -> np.ndarray:
    """Average channels, then bandlimited polyphase resample to 48 kHz."""
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
        # Native CLAP truncates to signed int16. Very quiet nonzero float WAVs
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
            if text_embeddings is None:
                text_embeddings = backend.text_embeddings(profile)
            audio = backend.audio_embedding(repeat_pad(excerpt))
            try:
                segment.update(
                    cosine_scores(audio, text_embeddings, len(profile["positive"]))
                )
            except ValueError as error:
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


class NativeClap:
    """Official LAION Python implementation with the published music checkpoint."""

    def __init__(self, cache: Path, device: str, threads: int):
        # LAION's constructor also loads roberta-base; keep those public artifacts
        # in this cache and record the actual files below, including tokenizer data.
        os.environ["HF_HUB_CACHE"] = str(cache.resolve())
        os.environ["HF_HUB_DISABLE_TELEMETRY"] = "1"
        import laion_clap
        import torch
        from huggingface_hub import hf_hub_download
        from transformers import RobertaTokenizer

        torch.set_num_threads(threads)
        torch.manual_seed(0)
        np.random.seed(0)
        torch.use_deterministic_algorithms(True)
        checkpoint = Path(
            hf_hub_download(
                MODEL_REPO, MODEL_FILE, revision=MODEL_REVISION, cache_dir=str(cache)
            )
        )
        actual_hash = sha256(checkpoint)
        if actual_hash != MODEL_SHA256:
            raise ValueError("official checkpoint SHA-256 mismatch")
        self.model = laion_clap.CLAP_Module(
            enable_fusion=False, amodel="HTSAT-base", device=device
        )
        self.model.load_ckpt(str(checkpoint), verbose=False)
        # The native constructor initializes RoBERTa before the complete CLAP
        # checkpoint replaces its weights. Explicitly pin the tokenizer too.
        self.model.tokenize = RobertaTokenizer.from_pretrained(
            "roberta-base", revision=TOKENIZER_REVISION, cache_dir=str(cache)
        )
        self.model.eval()
        self.torch = torch
        self.text_cache = {}
        tokenizer_files = {}
        snapshot = cache / "models--roberta-base" / "snapshots" / TOKENIZER_REVISION
        for file in snapshot.iterdir():
            if file.is_file() and file.suffix in (".json", ".txt"):
                tokenizer_files[file.name] = sha256(file)
        self.provenance = {
            "implementation": "native laion-clap",
            "architecture": "HTSAT-base, non-fusion, RoBERTa",
            "repository": MODEL_REPO,
            "revision": MODEL_REVISION,
            "checkpoint": MODEL_FILE,
            "checkpoint_sha256": actual_hash,
            "reference": "https://github.com/LAION-AI/CLAP",
            "tokenizer_artifacts": tokenizer_files,
            "tokenizer_repository": "roberta-base",
            "tokenizer_revision": TOKENIZER_REVISION,
            "device": device,
            "threads": threads,
            "python": platform.python_version(),
            "platform": platform.platform(),
            "packages": {
                name: importlib.metadata.version(name)
                for name in (
                    "laion-clap",
                    "torch",
                    "torchaudio",
                    "transformers",
                    "soundfile",
                    "numpy",
                    "scipy",
                )
            },
        }

    def text_embeddings(self, profile: dict) -> np.ndarray:
        """Cache the fixed text embeddings once per prompt profile."""
        prompts = tuple(
            row["text"] for row in profile["positive"] + profile["contrast"]
        )
        if prompts not in self.text_cache:
            with self.torch.inference_mode():
                self.text_cache[prompts] = self.model.get_text_embedding(list(prompts))
        return self.text_cache[prompts]

    def audio_embedding(self, excerpt: np.ndarray) -> np.ndarray:
        """Exactly ten seconds avoids native CLAP's random long-audio cropping."""
        with self.torch.inference_mode():
            return self.model.get_audio_embedding_from_data(
                excerpt[None, :], use_tensor=False
            )[0]


def compare(report: dict, baseline: dict) -> dict:
    """Reject changed measurement conditions and compare only matching labels."""
    for key in ("schema_version", "prompts_sha256", "preprocessing"):
        if report[key] != baseline.get(key):
            raise ValueError(f"baseline uses different {key}")
    for key in ("checkpoint_sha256", "packages", "tokenizer_artifacts", "device"):
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
    parser.add_argument("wavs", nargs="+", type=Path, help="WAV files or folders")
    parser.add_argument(
        "--json", required=True, type=Path, help="save complete measurement report"
    )
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--prompts", type=Path, default=MANIFEST)
    parser.add_argument("--preset", help="prompt profile for arbitrary input filenames")
    parser.add_argument("--segments", type=int, default=3)
    parser.add_argument("--threads", type=int, default=4)
    parser.add_argument("--device", default="cpu", choices=("cpu", "cuda"))
    parser.add_argument(
        "--cache", type=Path, default=Path("target/composition-eval/models")
    )
    args = parser.parse_args()
    if args.segments < 1 or args.threads < 1:
        parser.error("--segments and --threads must be positive")
    wavs = []
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
        presets = [preset_for(path, profiles, args.preset) for path in wavs]
    except ValueError as error:
        parser.error(str(error))
    backend = NativeClap(args.cache, args.device, args.threads)
    report = {
        "schema_version": 1,
        "interpretation": "Cosine text/audio identity similarity; not probability or musical quality.",
        "prompts_sha256": sha256(args.prompts),
        "prompts": manifest,
        "model": backend.provenance,
        "preprocessing": {
            "sample_rate": RATE,
            "channels": "arithmetic mean before resampling",
            "resampler": "scipy.signal.resample_poly default Kaiser window",
            "normalization": "none; native CLAP clips to [-1,1] and quantizes to int16",
            "window_samples": WINDOW,
            "window_selection": "equally spaced first-to-last complete window; one segment uses center",
            "requested_segments": args.segments,
            "short_audio": "repeat complete copies, zero-pad remainder",
            "silent_rms_threshold": 1e-7,
            "aggregation": "unweighted mean of valid excerpt cosines",
        },
        "files": {},
    }
    for path, preset in zip(wavs, presets, strict=True):
        row = score_file(path, preset, profiles[preset], backend, args.segments)
        report["files"][path.stem] = row
        if row["aggregate"]:
            scores = row["aggregate"]
            print(
                f"{path.stem:24} {row['status']:8} cosine={scores['positive_cosine']:.4f} margin={scores['contrast_margin']:+.4f}",
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
