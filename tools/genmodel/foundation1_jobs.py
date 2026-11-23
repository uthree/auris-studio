"""Generate Foundation-1 loops with one loaded model and time each one.

Development experiment only. Run it with a stable-audio-tools environment:

    venv_rocm/Scripts/python.exe tools/genmodel/foundation1_jobs.py --jobs jobs.json --output out/

`jobs.json` is a list of `{"name", "prompt", "bpm", "bars", "seed"}` objects, plus
optional `steps`, `cfg_scale` and `checkpoint`. The loop length is computed from
`bpm` and `bars` (4/4), since the model was trained on tempo-exact loops.
"""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

REPO = "RoyalCities/Foundation-1"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--jobs", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--checkpoint", default="Foundation_1.safetensors")
    args = parser.parse_args()

    import soundfile as sf
    import torch
    from huggingface_hub import hf_hub_download
    from stable_audio_tools.inference.generation import generate_diffusion_cond
    from stable_audio_tools.models.factory import create_model_from_config
    from stable_audio_tools.models.utils import load_ckpt_state_dict

    # ROCm on Windows: the fused SDPA kernels fail with hipErrorInvalidValue on
    # these models, as they did for ACE-Step; the math kernel runs.
    torch.backends.cuda.enable_flash_sdp(False)
    torch.backends.cuda.enable_mem_efficient_sdp(False)
    torch.backends.cuda.enable_math_sdp(True)

    jobs = json.loads(args.jobs.read_text(encoding="utf-8"))
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)

    t0 = time.perf_counter()
    config = json.loads(Path(hf_hub_download(REPO, "model_config.json")).read_text())
    model = create_model_from_config(config)
    model.load_state_dict(load_ckpt_state_dict(hf_hub_download(REPO, args.checkpoint)))
    model = model.to("cuda").eval().requires_grad_(False)
    sample_rate = config["sample_rate"]
    load_seconds = time.perf_counter() - t0
    print(f"loaded {args.checkpoint} in {load_seconds:.1f} s", flush=True)

    report = {
        "model": f"{REPO}/{args.checkpoint}",
        "device": torch.cuda.get_device_name(0),
        "torch": torch.__version__,
        "load_seconds": round(load_seconds, 2),
        "jobs": [],
    }
    for job in jobs:
        # An init_audio job plays that recording again in the prompt's sound: its length is
        # the recording's, and init_noise_level (a sigma; the schedule starts at 500) says how
        # far the result may move from it.
        init = None
        if job.get("init_audio"):
            recording, rate = sf.read(job["init_audio"], dtype="float32", always_2d=True)
            init = (rate, torch.from_numpy(recording.T.copy()))
            seconds_total = len(recording) / rate
        else:
            beats = job.get("beats_per_bar", 4)
            seconds_total = job["bars"] * beats * 60.0 / job["bpm"]
        sample_size = round(seconds_total * sample_rate)
        # The DiT works in latents of 2048 samples; pad to a whole latent.
        padded = -(-sample_size // 2048) * 2048
        torch.cuda.reset_peak_memory_stats()
        t1 = time.perf_counter()
        extra = {}
        if init is not None:
            extra = {"init_audio": init, "init_noise_level": job.get("init_noise_level", 10.0)}
        audio = generate_diffusion_cond(
            model,
            steps=job.get("steps", 100),
            cfg_scale=job.get("cfg_scale", 7.0),
            conditioning=[
                {
                    "prompt": job["prompt"],
                    "seconds_start": 0,
                    "seconds_total": seconds_total,
                }
            ],
            sample_size=padded,
            sample_rate=sample_rate,
            seed=job["seed"],
            device="cuda",
            sampler_type="dpmpp-3m-sde",
            sigma_min=0.03,
            sigma_max=500,
            **extra,
        )
        seconds = time.perf_counter() - t1
        y = audio[0, :, :sample_size].float().cpu().numpy().T
        path = out / f"{job['name']}.wav"
        sf.write(path, y, sample_rate, subtype="FLOAT")
        entry = {
            **job,
            "seconds_total": round(seconds_total, 3),
            "seconds": round(seconds, 2),
            "peak_gib": round(torch.cuda.max_memory_allocated() / 2**30, 2),
            "audio": str(path),
        }
        report["jobs"].append(entry)
        print(json.dumps(entry, ensure_ascii=False), flush=True)
        (out / "report.json").write_text(
            json.dumps(report, indent=2, ensure_ascii=False)
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
