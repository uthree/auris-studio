"""Run a batch of ACE-Step 1.5 jobs with one loaded model and time each one.

Development experiment only: the model, its weights and its outputs stay on this
machine. Run it with the interpreter of an ACE-Step checkout's environment:

    venv_rocm/Scripts/python.exe tools/genmodel/ace_step_jobs.py \
        --ace-step <checkout> --model acestep-v15-base --jobs jobs.json --output out/

`jobs.json` is a list of objects. Each names an output `name` and carries
`GenerationParams` fields (`task_type`, `src_audio`, `instruction`, `caption`,
`repainting_start`, `repainting_end`, `bpm`, `keyscale`, `duration`, `seed`,
`inference_steps`, ...). The report records wall time and peak device memory per
job, so the first question of the experiment (does it run here, and how fast) is
answered by the same file that holds the audio paths.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
import time
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--ace-step", type=Path, required=True, help="ACE-Step checkout"
    )
    parser.add_argument("--model", default="acestep-v15-base", help="DiT config name")
    parser.add_argument("--jobs", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--offload", action="store_true", help="offload idle parts to CPU"
    )
    parser.add_argument(
        "--sdpa",
        choices=["default", "math"],
        default="default",
        help="restrict PyTorch's SDPA kernels; `math` avoids fused kernels",
    )
    parser.add_argument("--only", nargs="*", help="run only the jobs with these names")
    args = parser.parse_args()

    # ROCm on Windows: the settings ACE-Step's own ROCm launcher makes.
    os.environ.setdefault("ACESTEP_LM_BACKEND", "pt")
    os.environ.setdefault("TORCH_COMPILE_BACKEND", "eager")
    os.environ.setdefault("MIOPEN_FIND_MODE", "FAST")
    os.environ.setdefault("TOKENIZERS_PARALLELISM", "false")

    root = args.ace_step.resolve()
    sys.path.insert(0, str(root))
    os.chdir(root)

    import torch
    from acestep.handler import AceStepHandler
    from acestep.inference import GenerationConfig, GenerationParams, generate_music
    from acestep.llm_inference import LLMHandler

    if args.sdpa == "math":
        torch.backends.cuda.enable_flash_sdp(False)
        torch.backends.cuda.enable_mem_efficient_sdp(False)
        torch.backends.cuda.enable_math_sdp(True)

    jobs = json.loads(args.jobs.read_text(encoding="utf-8"))
    if args.only:
        jobs = [job for job in jobs if job["name"] in args.only]
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)

    started = time.perf_counter()
    dit = AceStepHandler()
    status, ok = dit.initialize_service(
        project_root=str(root),
        config_path=args.model,
        device="cuda",
        offload_to_cpu=args.offload,
    )
    load_seconds = time.perf_counter() - started
    if not ok:
        print(f"initialisation failed: {status}", file=sys.stderr)
        return 1
    print(f"loaded {args.model} in {load_seconds:.1f} s", flush=True)
    llm = LLMHandler()  # never initialised: DiT only, no language-model planning

    report = {
        "model": args.model,
        "device": torch.cuda.get_device_name(0),
        "torch": torch.__version__,
        "offload": args.offload,
        "sdpa": args.sdpa,
        "dtype": os.environ.get("ACESTEP_ROCM_DTYPE", "float32"),
        "load_seconds": round(load_seconds, 2),
        "jobs": [],
    }
    for job in jobs:
        job = dict(job)
        name = job.pop("name")
        job.setdefault("thinking", False)
        params = GenerationParams(**job)
        config = GenerationConfig(
            batch_size=1,
            use_random_seed=False,
            seeds=[job.get("seed", 0)],
            audio_format="wav",
        )
        torch.cuda.reset_peak_memory_stats()
        t0 = time.perf_counter()
        result = generate_music(dit, llm, params, config, save_dir=str(out / "_raw"))
        seconds = time.perf_counter() - t0
        entry = {
            "name": name,
            "params": job,
            "seconds": round(seconds, 2),
            "peak_gib": round(torch.cuda.max_memory_allocated() / 2**30, 2),
            "success": bool(result.success),
        }
        if result.success:
            src = Path(result.audios[0]["path"])
            dst = out / f"{name}{src.suffix}"
            shutil.copyfile(src, dst)
            entry["audio"] = str(dst)
        else:
            entry["error"] = str(result.error)
        report["jobs"].append(entry)
        print(json.dumps(entry, ensure_ascii=False), flush=True)
        (out / "report.json").write_text(
            json.dumps(report, indent=2, ensure_ascii=False)
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
