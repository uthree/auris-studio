"""Pinned, development-only TuneJury and MuQ-MuLan inference."""

from __future__ import annotations

import hashlib
import importlib.metadata
import json
import os
import subprocess
import sys
from pathlib import Path

import numpy as np

TUNEJURY_SOURCE = "9dd8ea3e4a72f358ce311a6a856194ca6932c89e"
MODELS = {
    "TuneJury/tunejury": "a36a6adaad4a9786a12c3e5fd82501f2f2dc54ff",
    "m-a-p/MERT-v1-330M": "5240c2708a5acaee1007f43fb9735c7dcd0b78c9",
    "OpenMuQ/MuQ-MuLan-large": "2e01c796b71dca71b45251384c04cd7b237c9020",
    "OpenMuQ/MuQ-large-msd-iter": "0562a57814f6f8bbd9fdea0a25921a2fce1a841a",
    "FacebookAI/xlm-roberta-base": "e73636d4f797dec63c3081bb6ed5c7b0bb3f2089",
    "FacebookAI/roberta-base": "e2da8e2f811d1448a5b465c236feacd80ffbac7b",
    "lukewys/laion_clap": "b3708341862f581175dba5c356a4ebf74a9b6651",
}
CLAP_FILE = "music_audioset_epoch_15_esc_90.14.pt"
CLAP_SHA = "fae3e9c087f2909c28a09dc31c8dfcdacbc42ba44c70e972b58c1bd1caf6dedd"


def file_hash(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


class LearnedModels:
    """Keep both official models loaded across a complete experiment.

    TuneJury uses its recommended empty-prompt/zero-text protocol for post-hoc
    genre captions. MuQ-MuLan evaluates the frozen positive/contrast captions.
    Both receive the same ten-second mono 24 kHz excerpt, at its original gain.
    """

    def __init__(self, cache: Path, device: str = "cpu", threads: int = 4):
        cache = cache.resolve()
        cache.mkdir(parents=True, exist_ok=True)
        os.environ["HF_HUB_CACHE"] = str(cache)
        os.environ["HF_HUB_DISABLE_TELEMETRY"] = "1"
        import torch
        from huggingface_hub import hf_hub_download, snapshot_download

        torch.set_num_threads(threads)
        torch.manual_seed(0)
        np.random.seed(0)
        torch.use_deterministic_algorithms(True)
        self.torch, self.device = torch, device
        artifacts = {}

        def download(repo, filename=None):
            revision = MODELS[repo]
            if filename:
                path = Path(
                    hf_hub_download(
                        repo, filename, revision=revision, cache_dir=str(cache)
                    )
                )
                files = {filename: file_hash(path)}
            else:
                path = Path(
                    snapshot_download(
                        repo,
                        revision=revision,
                        cache_dir=str(cache),
                        allow_patterns=[
                            "*.json",
                            "*.txt",
                            "*.model",
                            "*.py",
                            "*.safetensors",
                            "pytorch_model.bin",
                        ],
                    )
                )
                files = {
                    p.relative_to(path).as_posix(): file_hash(p)
                    for p in sorted(path.rglob("*"))
                    if p.is_file()
                }
            artifacts[repo] = {"revision": revision, "files": files}
            return path

        source = cache.parent / "TuneJury"
        if not source.exists():
            subprocess.run(
                [
                    "git",
                    "clone",
                    "https://github.com/yonghyunk1m/TuneJury.git",
                    str(source),
                ],
                check=True,
            )
            subprocess.run(
                ["git", "-C", str(source), "checkout", TUNEJURY_SOURCE], check=True
            )
        revision = subprocess.check_output(
            ["git", "-C", str(source), "rev-parse", "HEAD"], text=True
        ).strip()
        if revision != TUNEJURY_SOURCE:
            raise ValueError(f"TuneJury source must be at {TUNEJURY_SOURCE}")
        subprocess.run(
            ["git", "-C", str(source), "diff", "--exit-code", "HEAD", "--", "tunejury"],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        sys.path.insert(0, str(source))
        import laion_clap
        from muq import MuQMuLan
        from transformers import AutoModel, RobertaTokenizer, Wav2Vec2FeatureExtractor
        from tunejury.model import TuneJury
        from tunejury.score import Scorer

        head_path = download("TuneJury/tunejury", "tunejury.pt")
        clap_path = download("lukewys/laion_clap", CLAP_FILE)
        if file_hash(clap_path) != CLAP_SHA:
            raise ValueError("TuneJury encoder checkpoint SHA-256 mismatch")
        roberta = download("FacebookAI/roberta-base")
        mert = download("m-a-p/MERT-v1-330M")
        head = TuneJury(input_dim=2048)
        state = torch.load(head_path, map_location="cpu", weights_only=True)
        head.load_state_dict(state.get("state_dict", state))
        clap = laion_clap.CLAP_Module(
            enable_fusion=False, amodel="HTSAT-base", device=device
        )
        clap.load_ckpt(str(clap_path), verbose=False)
        clap.tokenize = RobertaTokenizer.from_pretrained(
            str(roberta), local_files_only=True
        )
        self.jury = Scorer(
            head.eval().to(device),
            clap.eval().to(device),
            Wav2Vec2FeatureExtractor.from_pretrained(str(mert), local_files_only=True),
            AutoModel.from_pretrained(
                str(mert), trust_remote_code=True, local_files_only=True
            )
            .eval()
            .to(device),
            device,
        )
        mulan_path = download("OpenMuQ/MuQ-MuLan-large")
        muq_path = download("OpenMuQ/MuQ-large-msd-iter")
        text_path = download("FacebookAI/xlm-roberta-base")
        config = json.loads((mulan_path / "config.json").read_text(encoding="utf-8"))
        config = config.get("config", config)
        config["audio_model"]["name"] = str(muq_path)
        config["text_model"]["name"] = str(text_path)
        self.mulan = MuQMuLan(config, hf_hub_cache_dir=str(cache))
        # HubMixin's local-directory branch assumes safetensors, but this pinned
        # release contains pytorch_model.bin. Load its exact state dict strictly,
        # as the remote binary loader does, without altering any model weights.
        state = torch.load(
            mulan_path / "pytorch_model.bin", map_location="cpu", weights_only=True
        )
        self.mulan.load_state_dict(state, strict=True)
        self.mulan = self.mulan.float().eval().to(device)
        self.text_cache = {}
        self.provenance = {
            "tunejury_source_revision": revision,
            "artifacts": artifacts,
            "device": device,
            "threads": threads,
            "packages": {
                name: importlib.metadata.version(name)
                for name in (
                    "torch",
                    "torchaudio",
                    "transformers",
                    "muq",
                    "laion-clap",
                    "soundfile",
                    "numpy",
                    "scipy",
                    "huggingface-hub",
                )
            },
        }

    def text_embeddings(self, profile: dict) -> np.ndarray:
        prompts = tuple(p["text"] for p in profile["positive"] + profile["contrast"])
        if prompts not in self.text_cache:
            with self.torch.inference_mode():
                self.text_cache[prompts] = self.mulan(texts=list(prompts)).cpu().numpy()
        return self.text_cache[prompts]

    def audio_embedding(self, excerpt: np.ndarray) -> np.ndarray:
        with self.torch.inference_mode():
            wav = self.torch.from_numpy(excerpt).unsqueeze(0).to(self.device)
            return self.mulan(wavs=wav).cpu().numpy()[0]

    def reward(self, excerpt: np.ndarray) -> float:
        # The official waveform API avoids torchaudio's OS-dependent decoder.
        with self.torch.inference_mode():
            return self.jury.score_waveform(excerpt, 24_000, text="")
