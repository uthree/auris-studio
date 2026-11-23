# Music generation models (experiment)

Scripts for trying local music generation models on parts of an Auris song: a new stem
over the other tracks, a bar range regenerated, a loop layered on, or an Auris score
performed. They are development tooling. Nothing here is shipped, and the models, their
weights and their outputs stay on the machine that ran them.

| Script | Model | Environment |
| --- | --- | --- |
| `ace_step_jobs.py` | ACE-Step 1.5 (`lego`, `repaint`, …) | the ACE-Step checkout's own venv |
| `foundation1_jobs.py` | Foundation-1 loops | a stable-audio-tools venv |
| `auris_to_abc.py` | none: writes the ABC score `yue-synth` (yue2.cpp) performs | any Python 3.11+ |

## Running on an RX 9070 XT under Windows

* PyTorch comes from AMD's index:
  `uv pip install --index-url https://repo.amd.com/rocm/whl-multi-arch/ "torch[device-gfx1201]==2.12.0+rocm7.14.1" "torchaudio==2.11.0+rocm7.14.1"`.
  Install each project's own requirements around it, never its CUDA pins.
* PyTorch's fused SDPA kernels fail on both models with `hipErrorInvalidValue`. Both
  scripts can use the math kernel instead (`--sdpa math` for ACE-Step; Foundation-1
  always does).
* ACE-Step loads in float32 on ROCm. A 64-second `lego` then goes past 16 GB, so set
  `ACESTEP_ROCM_DTYPE=bfloat16`.
* yue2.cpp is built with its Vulkan backend (Vulkan SDK + MSVC) and runs with
  `--device Vulkan0`.
