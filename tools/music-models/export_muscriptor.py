# /// script
# requires-python = ">=3.11,<3.13"
# dependencies = ["muscriptor==0.3.0", "torch==2.5.1", "torchaudio==2.5.1",
#                 "onnx==1.17.0", "onnxruntime==1.20.1", "numpy>=1.26,<3"]
# ///
"""Convert a user's local MuScriptor Small checkpoint; never download or redistribute weights.

The exported graphs retain the checkpoint's CC BY-NC 4.0 model-use restrictions.
Python/PyTorch is needed only for this explicit preparation step, not by Auris inference.
"""

import argparse
import hashlib
import importlib.metadata
import json
import os
import tempfile
from pathlib import Path

CONFIG = {
    "model_type": "muscriptor",
    "variant": "small",
    "dim": 768,
    "num_heads": 12,
    "num_layers": 14,
    "card": 1393,
}


def check_input(checkpoint, consent):
    """Fail before optional imports, model loading or output creation."""
    if not consent:
        raise ValueError("MuScriptor weights are CC BY-NC 4.0: noncommercial use only")
    checkpoint = checkpoint.resolve(strict=True)
    if checkpoint.suffix != ".safetensors" or checkpoint.stat().st_size > 512 * 1024**2:
        raise ValueError("expected a local MuScriptor Small safetensors checkpoint")
    config = checkpoint.with_name("config.json")
    if config.stat().st_size > 4096 or json.loads(config.read_text()) != CONFIG:
        raise ValueError("expected the original MuScriptor Small config.json")
    return checkpoint


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def wrappers(torch, model, conditions):
    """Pure tensor equivalents of the pinned upstream streaming model (see NOTICE)."""
    from torch import nn
    from torch.nn import functional as F

    class Audio(nn.Module):
        def __init__(self):
            super().__init__()
            self.mel = model.condition_provider.conditioners["self_wav"]
            # LM.forward prepends each condition in iteration order, hence reverse here.
            text = [v[0] for k, v in conditions.items() if k != "self_wav"]
            self.register_buffer("text", torch.cat(list(reversed(text)), dim=1))
            self.register_buffer("mask", (torch.arange(501) < 500).view(1, 501, 1))

        def forward(self, waveform):
            spec = torch.stft(
                waveform,
                2048,
                hop_length=160,
                window=self.mel.mel_spec_transform.spectrogram.window,
                center=True,
                pad_mode="reflect",
                return_complex=False,
            )
            magnitude = torch.sqrt(torch.sum(spec * spec, dim=-1))
            mel = magnitude.transpose(1, 2) @ self.mel.mel_spec_transform.mel_scale.fb
            audio = self.mel.output_proj(torch.log(mel + 1e-6)) * self.mask
            return torch.cat([audio, self.text], dim=1)

    class Decoder(nn.Module):
        def __init__(self):
            super().__init__()
            self.emb = model.emb
            self.layers = model.transformer.layers
            self.norm = model.out_norm
            self.linear = model.linear
            self.register_buffer("period", 10000 ** (torch.arange(384).float() / 383))

        def forward(self, tokens, prefix, past):
            x = torch.cat([prefix, self.emb(tokens)], dim=1)
            count = x.shape[1]
            offset = past.shape[4]
            positions = torch.arange(count) + offset
            phase = positions.float().view(1, -1, 1) / self.period
            x = x + torch.cat([torch.cos(phase), torch.sin(phase)], dim=-1)
            mask = torch.arange(offset + count).view(1, -1) <= positions.view(-1, 1)
            present = []
            for i, layer in enumerate(self.layers):
                packed = F.linear(layer.norm1(x), layer.self_attn.in_proj_weight)
                packed = packed.reshape(1, -1, 3, 12, 64).permute(2, 0, 3, 1, 4)
                q, k, v = packed.unbind(0)
                k = torch.cat([past[i, 0], k], dim=2)
                v = torch.cat([past[i, 1], v], dim=2)
                weights = (q @ k.transpose(-1, -2)) * 0.125
                weights = torch.softmax(
                    weights.masked_fill(~mask, float("-inf")), dim=-1
                )
                attended = (weights @ v).transpose(1, 2).reshape(1, -1, 768)
                x = x + layer.self_attn.out_proj(attended)
                x = x + layer.linear2(F.gelu(layer.linear1(layer.norm2(x))))
                present.append(torch.stack([k, v]))
            return self.linear(self.norm(x[:, -1:])).squeeze(1), torch.stack(present)

    return Audio().eval(), Decoder().eval()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--checkpoint", type=Path, required=True)
    parser.add_argument("--output-directory", type=Path, required=True)
    parser.add_argument("--acknowledge-noncommercial", action="store_true")
    args = parser.parse_args()
    checkpoint = check_input(args.checkpoint, args.acknowledge_noncommercial)
    destination = args.output_directory.resolve()
    if destination.exists():
        raise ValueError("output directory already exists; choose a new directory")
    if importlib.metadata.version("muscriptor") != "0.3.0":
        raise ValueError("use muscriptor==0.3.0")
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["CUDA_VISIBLE_DEVICES"] = "-1"
    import numpy as np
    import onnx
    import onnxruntime as ort
    import torch
    from muscriptor.modules.streaming import increment_steps, init_states
    from muscriptor.transcription_model import TranscriptionModel

    torch.set_num_threads(2)
    torch.set_num_interop_threads(1)
    transcription = TranscriptionModel.load_model(
        checkpoint, device="cpu", dtype="float32"
    )
    model = transcription._model
    wav = torch.zeros(1, 80000)
    conditions = model.condition_provider(
        model.condition_provider.tokenize(transcription._build_conditions(wav))
    )
    audio, decoder = wrappers(torch, model, conditions)
    prefix = audio(wav)
    tokens = torch.tensor([[1393]])
    past = torch.zeros(14, 2, 1, 12, 0, 64)
    options = ort.SessionOptions()
    options.intra_op_num_threads = 2
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix="muscriptor-export-", dir=destination.parent
    ) as tmp:
        root = Path(tmp)
        with torch.inference_mode():
            torch.onnx.export(
                audio,
                (wav,),
                root / "audio.onnx",
                opset_version=17,
                input_names=["waveform"],
                output_names=["prefix"],
            )
            torch.onnx.export(
                decoder,
                (tokens, prefix, past),
                root / "decoder.onnx",
                opset_version=17,
                input_names=["tokens", "prefix", "past"],
                output_names=["logits", "present"],
                dynamic_axes={
                    "tokens": {1: "tokens"},
                    "prefix": {1: "prefix_length"},
                    "past": {4: "past_length"},
                    "present": {4: "total_length"},
                },
            )
        sessions = {}
        for name in ["audio", "decoder"]:
            path = root / f"{name}.onnx"
            graph = onnx.load(path)
            onnx.helper.set_model_props(
                graph,
                {
                    "auris.muscriptor": "small-v1",
                    "role": name,
                    "license": "CC-BY-NC-4.0",
                },
            )
            onnx.checker.check_model(graph)
            onnx.save(graph, path)
            sessions[name] = ort.InferenceSession(
                str(path), options, providers=["CPUExecutionProvider"]
            )
        checks = []
        rng = np.random.default_rng(91827)
        fixtures = [
            np.zeros((1, 80000), np.float32),
            rng.uniform(-0.1, 0.1, (1, 80000)).astype(np.float32),
            (np.sin(np.arange(80000) * (2 * np.pi * 440 / 16000)) * 0.3)
            .astype(np.float32)
            .reshape(1, -1),
        ]
        for waveform in fixtures:
            with torch.inference_mode():
                official_conditions = model.condition_provider(
                    model.condition_provider.tokenize(
                        transcription._build_conditions(torch.from_numpy(waveform))
                    )
                )
                expected_prefix = torch.cat(
                    [v[0] for v in reversed(official_conditions.values())], 1
                )
                actual_prefix = sessions["audio"].run(None, {"waveform": waveform})[0]
                np.testing.assert_allclose(
                    actual_prefix, expected_prefix.numpy(), atol=3e-3, rtol=3e-3
                )
                state = init_states(model, batch_size=1, sequence_length=2600)
                cache = np.zeros((14, 2, 1, 12, 0, 64), np.float32)
                sequence = np.array([[1393]], np.int64)
                errors = []
                for step in range(8):
                    expected = model(
                        torch.from_numpy(sequence),
                        official_conditions,
                        first_step=step == 0,
                        model_state=state,
                    )[:, -1].numpy()
                    actual, cache = sessions["decoder"].run(
                        None,
                        {
                            "tokens": sequence,
                            "prefix": actual_prefix
                            if step == 0
                            else np.zeros((1, 0, 768), np.float32),
                            "past": cache,
                        },
                    )
                    np.testing.assert_allclose(actual, expected, atol=5e-3, rtol=5e-3)
                    if int(actual.argmax()) != int(expected.argmax()):
                        raise ValueError("greedy token differs from the official model")
                    errors.append(float(np.abs(actual - expected).max()))
                    increment_steps(
                        model.transformer, state, increment=504 if step == 0 else 1
                    )
                    sequence = actual.argmax(-1).astype(np.int64).reshape(1, 1)
                checks.append(
                    {
                        "prefix_max_error": float(
                            np.abs(actual_prefix - expected_prefix.numpy()).max()
                        ),
                        "logits_max_error": max(errors),
                        "greedy_steps_checked": 8,
                    }
                )
        manifest = {
            "format": "auris-muscriptor-small-v1",
            "license": "CC-BY-NC-4.0",
            "source_package": "muscriptor==0.3.0",
            "checkpoint_sha256": digest(checkpoint),
            "files": {
                name: digest(root / name) for name in ["audio.onnx", "decoder.onnx"]
            },
            "vocabulary": [
                {"type": e.type, "value": e.value}
                for e in transcription._tokenizer._vocab
            ],
            "programs": [transcription._instrument_for_program(p) for p in range(130)],
            "checks": checks,
        }
        (root / "muscriptor.json").write_text(
            json.dumps(manifest, indent=2), encoding="utf-8"
        )
        # Publish only after all checks pass; no model is bundled with the repository.
        destination.mkdir()
        for path in root.iterdir():
            path.rename(destination / path.name)
    print(json.dumps({"output": str(destination), "checks": checks}))


if __name__ == "__main__":
    main()
