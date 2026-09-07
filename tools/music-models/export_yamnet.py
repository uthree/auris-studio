# /// script
# requires-python = ">=3.11,<3.13"
# dependencies = ["tensorflow==2.16.1", "tf-keras==2.16.0", "tf2onnx==1.16.1", "onnxruntime==1.20.1", "numpy==1.26.4"]
# ///
"""Fetch pinned Google YAMNet inputs and export a fixed waveform-to-scores ONNX model.

Run with uv run --python 3.12 tools/music-models/export_yamnet.py --output MODEL.onnx.
Only this explicit preparation step uses the network; Auris inference is offline.
"""

import argparse
import hashlib
import json
import os
import sys
import urllib.request
from pathlib import Path

COMMIT = "d598fb8b23d9cd2fb26b5789b8242de3f494aca7"
SOURCE = f"https://raw.githubusercontent.com/tensorflow/models/{COMMIT}/research/audioset/yamnet"
HASHES = {
    "yamnet.py": "7ef3df32b7ecb782490b5a04d7581a23cfcf701dbf476bc4d03deefe22cdb040",
    "features.py": "e6cd53f81d072c7c43be4c7fff2b9dd0c5ccc7d64f2fbcfc85b44013d6d2ed5e",
    "params.py": "925bb1e62461016031f98aea09aeac28975dd516f5747513767de5d1b06b6145",
    "yamnet_class_map.csv": "cdf24d193e196d9e95912a2667051ae203e92a2ba09449218ccb40ef787c6df2",
    "yamnet.h5": "13c3308955bbfaef262f175ac9c40e47b134573a93984f009220dd7cc12a1744",
}


def fetch(cache: Path) -> None:
    """Verify every upstream file, including cached Python, before importing it."""
    cache.mkdir(parents=True, exist_ok=True)
    for name, digest in HASHES.items():
        path = cache / name
        if not path.exists():
            url = f"{SOURCE}/{name}"
            if name == "yamnet.h5":
                url = "https://storage.googleapis.com/audioset/yamnet.h5"
            with urllib.request.urlopen(url, timeout=120) as response:
                data = response.read(32 * 1024 * 1024)
            if hashlib.sha256(data).hexdigest() != digest:
                raise ValueError(f"Download checksum mismatch: {name}")
            path.write_bytes(data)
        if hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            raise ValueError(f"Cached checksum mismatch: {path}")


def main() -> None:
    """Export preprocessing plus classification, then compare with official STFT."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache", type=Path, default=Path("target/music-models"))
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists() or args.output.with_suffix(".verification.json").exists():
        parser.error("output already exists")
    fetch(args.cache)
    os.environ["CUDA_VISIBLE_DEVICES"] = "-1"
    os.environ["TF_ENABLE_ONEDNN_OPTS"] = "0"
    os.environ["TF_NUM_INTRAOP_THREADS"] = "2"
    os.environ["TF_NUM_INTEROP_THREADS"] = "1"
    sys.path.insert(0, str(args.cache.resolve()))
    import features
    import numpy as np
    import onnx
    import onnxruntime as ort
    import params
    import tensorflow as tf
    import tf2onnx
    import tf_keras
    import yamnet

    reference = yamnet.yamnet_frames_model(params.Params())
    reference.load_weights(str(args.cache / "yamnet.h5"))
    patch = tf_keras.layers.Input(shape=(96, 64))
    portable = tf_keras.Model(patch, yamnet.yamnet(patch, params.Params())[0])
    portable.set_weights(reference.get_weights())

    @tf.function(input_signature=[tf.TensorSpec([15600], tf.float32, name="waveform")])
    def predict(waveform):
        _, patches = features.waveform_to_log_mel_spectrogram_patches(
            waveform, params.Params(tflite_compatible=True)
        )
        return {"scores": portable(patches, training=False)}

    model, _ = tf2onnx.convert.from_function(
        predict, input_signature=predict.input_signature, opset=17
    )
    onnx.helper.set_model_props(
        model,
        {
            "auris.yamnet": "waveform-15600-v1",
            "source_commit": COMMIT,
            "weights_sha256": HASHES["yamnet.h5"],
        },
    )
    onnx.checker.check_model(model)
    session_options = ort.SessionOptions()
    session_options.intra_op_num_threads = 2
    session = ort.InferenceSession(
        model.SerializeToString(),
        sess_options=session_options,
        providers=["CPUExecutionProvider"],
    )
    rng = np.random.default_rng(51773)
    fixtures = {
        "silence": np.zeros(15600, dtype=np.float32),
        "noise": rng.uniform(-0.5, 0.5, 15600).astype(np.float32),
        "sine": (0.4 * np.sin(2 * np.pi * 440 * np.arange(15600) / 16000)).astype(
            np.float32
        ),
        "impulse": np.eye(1, 15600, dtype=np.float32).ravel(),
    }
    checks = {}
    for name, waveform in fixtures.items():
        expected = reference(waveform, training=False)[0].numpy()
        actual = session.run(None, {"waveform": waveform})[0]
        np.testing.assert_allclose(actual, expected, atol=2e-4, rtol=2e-3)
        checks[name] = {
            "max_absolute_error": float(np.max(np.abs(actual - expected))),
            "top_index": int(actual.argmax()),
            "scores": actual.ravel().tolist(),
        }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    data = model.SerializeToString()
    # Exclusive creation prevents accidental replacement of a user's model.
    with args.output.open("xb") as output:
        output.write(data)
    verification = {
        "source_commit": COMMIT,
        "weights_sha256": HASHES["yamnet.h5"],
        "onnx_sha256": hashlib.sha256(data).hexdigest(),
        "checks": checks,
    }
    with args.output.with_suffix(".verification.json").open("x") as output:
        json.dump(verification, output, indent=2)
    print(
        json.dumps(
            {
                "output": str(args.output),
                "bytes": len(data),
                "max_error": max(c["max_absolute_error"] for c in checks.values()),
            }
        )
    )


if __name__ == "__main__":
    main()
