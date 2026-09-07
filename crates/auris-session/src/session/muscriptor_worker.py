"""Optional MuScriptor 0.3.0 CPU worker. Invoked only after per-run noncommercial consent."""

import argparse
import importlib.metadata
import json
import os
from pathlib import Path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--acknowledge-noncommercial", action="store_true")
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--directory", type=Path, required=True)
    args = parser.parse_args()
    if not args.acknowledge_noncommercial:
        parser.error("MuScriptor weights are CC BY-NC 4.0: noncommercial use only")
    model_path = args.model.resolve(strict=True)
    if model_path.suffix.lower() != ".safetensors":
        parser.error("expected a local .safetensors model")
    config_path = model_path.with_name("config.json")
    if config_path.stat().st_size > 4096:
        parser.error("invalid MuScriptor Small config.json")
    config = json.loads(config_path.read_text(encoding="utf-8"))
    expected = {"model_type": "muscriptor", "variant": "small", "dim": 768,
                "num_heads": 12, "num_layers": 14, "card": 1393}
    if config != expected:
        parser.error("expected the original MuScriptor Small config.json")
    if importlib.metadata.version("muscriptor") != "0.3.0":
        parser.error("install muscriptor==0.3.0 in the optional Python environment")
    os.environ["CUDA_VISIBLE_DEVICES"] = "-1"
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["HF_HUB_DISABLE_TELEMETRY"] = "1"
    import numpy as np
    import torch
    from muscriptor.events import NoteEndEvent, ProgressEvent
    from muscriptor.transcription_model import TranscriptionModel

    torch.set_num_threads(2)
    torch.set_num_interop_threads(1)
    samples = np.fromfile(args.directory / "audio.f32", dtype="<f4")
    if not 0 < len(samples) <= 16000 * 600 or not np.isfinite(samples).all():
        parser.error("expected at most ten minutes of finite 16 kHz mono PCM")
    model = TranscriptionModel.load_model(model_path, device="cpu", dtype="float32")
    notes = []
    with torch.inference_mode():
        for event in model.transcribe(
            audio=(torch.from_numpy(samples.copy()), 16000),
            use_sampling=False, batch_size=1, beam_size=1, prelude_forcing=True,
            no_eos_is_ok=False,
        ):
            if isinstance(event, ProgressEvent):
                (args.directory / "progress.json").write_text(
                    json.dumps(event.completed / max(1, event.total)), encoding="utf-8")
            elif isinstance(event, NoteEndEvent):
                start = event.start_event
                notes.append({"pitch": start.pitch, "start": start.start_time,
                              "end": event.end_time, "instrument": start.instrument})
                if len(notes) > 200000:
                    raise ValueError("too many decoded notes")
    (args.directory / "result.json").write_text(json.dumps(notes, allow_nan=False), encoding="utf-8")


if __name__ == "__main__":
    main()
