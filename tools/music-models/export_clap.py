# /// script
# requires-python = ">=3.11,<3.13"
# dependencies = ["torch==2.6.0", "transformers==4.57.3", "onnx==1.17.0", "onnxruntime==1.20.1", "numpy>=1.26,<3"]
# ///
"""Prepare the official unfused CLAP checkpoint for offline native Auris inference.

Run with uv run --python 3.12 tools/music-models/export_clap.py --output MODEL_DIR.
This explicit preparation command downloads model data, never remote Python code.
"""

import argparse
import hashlib
import json
import re
from pathlib import Path

MODEL_ID = "laion/clap-htsat-unfused"
REVISION = "8fa0f1c6d0433df6e97c127f64b2a1d6c0dcda8a"
FORMAT = "auris-clap-htsat-unfused-v1"
FILES = ("audio.onnx", "text.onnx", "tokenizer.json", "preprocess.bin")
TEXT_LENGTH = 77
PROMPT = "A bright instrumental melody with a steady electronic rhythm."


def validate_revision(revision: str) -> str:
    """Require an immutable source commit rather than a moving branch or tag."""
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("revision must be a lowercase 40-character commit hash")
    return revision


def token_inputs(tokenizer, prompt: str) -> dict:
    """Reject empty or overlong prompts before applying fixed right padding."""
    if not prompt.strip():
        raise ValueError("prompt must not be empty")
    tokens = tokenizer(prompt, truncation=False, padding=False)
    if len(tokens["input_ids"]) > TEXT_LENGTH:
        raise ValueError(
            f"prompt exceeds the {TEXT_LENGTH}-token limit including BOS/EOS"
        )
    return tokenizer(
        prompt,
        truncation=False,
        padding="max_length",
        max_length=TEXT_LENGTH,
        return_tensors="pt",
    )


def file_digest(path: Path) -> str:
    """Hash large graph files without reading a second copy into memory."""
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def write_manifest(directory: Path, revision: str) -> dict:
    """Publish the complete, verified package last as an offline load contract."""
    manifest = {
        "format": FORMAT,
        "model_id": MODEL_ID,
        "source_revision": validate_revision(revision),
        "license": "Apache-2.0",
        "files": {name: file_digest(directory / name) for name in FILES},
    }
    with (directory / "manifest.json").open("x", encoding="utf-8") as output:
        json.dump(manifest, output, indent=2)
    return manifest


def export_model(directory: Path, revision: str, cache: Path | None = None) -> dict:
    """Export fixed inputs, then verify both graphs against the real checkpoint."""
    import numpy as np
    import onnx
    import onnxruntime as ort
    import torch
    from transformers import AutoTokenizer, ClapFeatureExtractor, ClapModel
    from transformers.audio_utils import window_function

    revision = validate_revision(revision)
    directory.mkdir(parents=True, exist_ok=False)
    torch.set_num_threads(2)
    torch.set_num_interop_threads(1)
    source = {"revision": revision, "cache_dir": cache, "trust_remote_code": False}
    print(f"Loading {MODEL_ID}@{revision}", flush=True)
    model = ClapModel.from_pretrained(MODEL_ID, **source).float().eval()
    processor = ClapFeatureExtractor.from_pretrained(MODEL_ID, **source)
    tokenizer = AutoTokenizer.from_pretrained(MODEL_ID, use_fast=True, **source)
    if model.config.audio_config.enable_fusion:
        raise ValueError("the unfused CLAP export does not support feature fusion")
    if not tokenizer.is_fast or tokenizer.pad_token_id != 1:
        raise ValueError(
            "expected the official fast RoBERTa tokenizer with pad token 1"
        )
    tokenizer.padding_side = "right"
    tokenizer.backend_tokenizer.no_padding()
    tokenizer.backend_tokenizer.no_truncation()
    tokenizer.backend_tokenizer.save(str(directory / "tokenizer.json"))

    coefficients = np.concatenate(
        [window_function(1024, "hann").ravel(), processor.mel_filters_slaney.ravel()]
    ).astype("<f8")
    if coefficients.size != 33856 or not np.isfinite(coefficients).all():
        raise ValueError("unexpected CLAP preprocessing coefficient dimensions")
    coefficients.tofile(directory / "preprocess.bin")

    # A deterministic mixture exercises quiet, transient and tonal feature bins.
    time = np.arange(480000, dtype=np.float64) / 48000
    waveform = (
        0.18 * np.sin(2 * np.pi * 220 * time)
        + 0.07 * np.sin(2 * np.pi * 659.25 * time)
        + 0.025 * np.random.default_rng(51773).standard_normal(time.size)
    )
    waveform *= 0.25 + 0.75 * np.exp(-np.mod(time, 0.5) * 7)
    waveform = waveform.astype("<f4")
    features = processor(waveform, sampling_rate=48000, return_tensors="pt")[
        "input_features"
    ]
    if tuple(features.shape) != (1, 1, 1001, 64):
        raise ValueError(f"unexpected CLAP feature shape: {features.shape}")
    text = token_inputs(tokenizer, PROMPT)

    class Audio(torch.nn.Module):
        def __init__(self):
            super().__init__()
            self.model = model

        def forward(self, input_features):
            return self.model.get_audio_features(input_features=input_features)

    class Text(torch.nn.Module):
        def __init__(self):
            super().__init__()
            self.model = model

        def forward(self, input_ids, attention_mask):
            return self.model.get_text_features(
                input_ids=input_ids, attention_mask=attention_mask
            )

    checks = {}
    reference_embeddings = {}
    options = ort.SessionOptions()
    options.intra_op_num_threads = 2
    options.inter_op_num_threads = 1
    for role, wrapper, inputs, names in [
        ("audio", Audio(), (features,), ["input_features"]),
        (
            "text",
            Text(),
            (text["input_ids"], text["attention_mask"]),
            ["input_ids", "attention_mask"],
        ),
    ]:
        # The wrappers share a checkpoint: export restores the wrapper's train
        # state recursively, so it must already be eval before tracing.
        wrapper.eval()
        print(f"Exporting {role} encoder", flush=True)
        graph_path = directory / f"{role}.onnx"
        with torch.inference_mode():
            expected = wrapper(*inputs).numpy()
            torch.onnx.export(
                wrapper,
                inputs,
                str(graph_path),
                input_names=names,
                output_names=["embedding"],
                opset_version=17,
                do_constant_folding=True,
                dynamo=False,
                external_data=False,
            )
            np.testing.assert_array_equal(wrapper(*inputs).numpy(), expected)
        graph = onnx.load(str(graph_path), load_external_data=False)
        onnx.helper.set_model_props(
            graph,
            {
                "auris.clap": "htsat-unfused-v1",
                "role": role,
                "model_id": MODEL_ID,
                "source_revision": revision,
            },
        )
        # Fixed shape export may leave symbolic output dimensions after Reshape.
        # The graph is checked below by actual inference before publication.
        output_shape = graph.graph.output[0].type.tensor_type.shape
        for dimension, size in zip(output_shape.dim, (1, 512), strict=True):
            dimension.ClearField("dim_param")
            dimension.dim_value = size
        onnx.checker.check_model(graph)
        onnx.save(graph, str(graph_path), save_as_external_data=False)
        del graph
        session = ort.InferenceSession(
            str(graph_path), sess_options=options, providers=["CPUExecutionProvider"]
        )
        actual = session.run(
            None, {name: tensor.numpy() for name, tensor in zip(names, inputs)}
        )[0]
        if actual.shape != (1, 512) or not np.isfinite(actual).all():
            raise ValueError(f"invalid {role} encoder output")
        np.testing.assert_allclose(actual, expected, atol=2e-4, rtol=2e-3)
        np.testing.assert_allclose(np.linalg.norm(actual, axis=-1), 1, atol=1e-5)
        reference_embeddings[role] = expected.astype("<f4")
        checks[role] = {
            "max_absolute_error": float(np.max(np.abs(actual - expected))),
            "cosine_similarity": float(np.sum(actual * expected)),
        }
        # Different content catches value-dependent tracing or accidental dropout.
        if role == "audio":
            impulse = np.zeros(480000, dtype=np.float32)
            impulse[::24000] = 0.8
            cases = {
                "silence": np.zeros(480000, dtype=np.float32),
                "impulses": impulse,
            }
            extra_inputs = {
                name: (
                    processor(wave, sampling_rate=48000, return_tensors="pt")[
                        "input_features"
                    ],
                )
                for name, wave in cases.items()
            }
        else:
            cases = {
                "short": "Music.",
                "distinct": "Distant thunder and heavy rainfall.",
            }
            extra_inputs = {}
            for name, prompt in cases.items():
                tokens = token_inputs(tokenizer, prompt)
                extra_inputs[name] = (tokens["input_ids"], tokens["attention_mask"])
        checks[role]["additional_cases"] = {}
        for case, case_inputs in extra_inputs.items():
            with torch.inference_mode():
                case_expected = wrapper(*case_inputs).numpy()
            case_actual = session.run(
                None, {name: tensor.numpy() for name, tensor in zip(names, case_inputs)}
            )[0]
            np.testing.assert_allclose(case_actual, case_expected, atol=2e-4, rtol=2e-3)
            checks[role]["additional_cases"][case] = float(
                np.max(np.abs(case_actual - case_expected))
            )
        print(f"Verified {role}: {checks[role]}", flush=True)
        del session

    parity = directory / "parity"
    parity.mkdir()
    waveform.tofile(parity / "waveform.f32")
    features.numpy().astype("<f4").tofile(parity / "features.f32")
    reference_embeddings["audio"].tofile(parity / "audio_embedding.f32")
    (parity / "text.json").write_text(
        json.dumps(
            {
                "prompt": PROMPT,
                "input_ids": text["input_ids"].ravel().tolist(),
                "attention_mask": text["attention_mask"].ravel().tolist(),
                "embedding": reference_embeddings["text"].ravel().tolist(),
            },
            indent=2,
        ),
        encoding="utf-8",
    )
    verification = {
        "model_id": MODEL_ID,
        "source_revision": revision,
        "checks": checks,
        "versions": {
            "torch": torch.__version__,
            "onnx": onnx.__version__,
            "onnxruntime": ort.__version__,
            "transformers": "4.57.3",
        },
    }
    (directory / "verification.json").write_text(
        json.dumps(verification, indent=2), encoding="utf-8"
    )
    write_manifest(directory, revision)
    return verification


def main() -> None:
    """Download and export into a new directory without replacing existing models."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--revision", default=REVISION, type=validate_revision)
    parser.add_argument("--cache", type=Path)
    args = parser.parse_args()
    if args.output.exists():
        parser.error("output already exists; choose a new directory")
    print(json.dumps(export_model(args.output, args.revision, args.cache), indent=2))


if __name__ == "__main__":
    main()
