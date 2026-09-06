# Local audio reviewer

This optional service connects Auris `listen` to
[Qwen2-Audio-7B-Instruct](https://huggingface.co/Qwen/Qwen2-Audio-7B-Instruct),
a local model with published sound and music question-answering evaluations.
It receives embedded WAV audio and returns the model's text through an OpenAI-compatible
endpoint. Reviews are model judgments; validate musical claims with listening and blind controls.

## Install and run

Use a separate `uv` environment. Install the PyTorch build appropriate for your GPU first;
AMD Windows needs its supported ROCm build, while NVIDIA CUDA can use `--torch-backend=auto`.
The inference extra does not choose a GPU wheel or install bitsandbytes.

```powershell
cd tools/audio-review
uv venv --python 3.12
uv pip install -e ".[inference,dev]" --torch-backend=auto
uv run --no-sync python server.py --device auto --quantization none
```

The following separate environment was tested on Windows with a Radeon RX 9070 XT:

```powershell
cd tools/audio-review
uv venv --python 3.12
uv pip install --index-url https://stable.repo.amd.com/rocm/whl-next/ "torch[device-gfx1201]==2.13.0+rocm10.0.0"
uv pip install -e ".[inference,quantization]"
$env:HIP_VISIBLE_DEVICES = "0"
uv run --no-sync python server.py --device cuda --quantization nf4 --revision 0a095220c30b7b31434169c3086508ef3ea5bf0a
```

`HIP_VISIBLE_DEVICES=0` selected the discrete GPU on the tested host; check device ordering on
another machine. This configuration completed local GPU inference trials. Music critique and reliable
A/B judgments were not validated by that fact; the live blind controls exposed incorrect answers.
Several MusicFlamingo requests completed, but a later cold start crashed in the native
`torch_cpu.dll` while loading weights, before returning any review. This tested AMD runtime
therefore remains experimental; successful earlier runs do not establish restart reliability.
The [interface trial](../../docs/reviews/local-model-interface-2026-09-07.md) preserves both
the completed runs and the failed startup.

The official BF16 checkpoint download is about 16.8 GB. CPU execution uses float32 and needs
more memory. GPU execution uses BF16 when supported, otherwise FP16. With limited VRAM,
`--max-memory-gib 12` caps GPU 0 allocations for model placement and allows CPU offload;
generation still needs additional working memory. Unload other local models before testing.
`--cache-dir PATH` selects a local Hugging Face cache. The first request loads the weights.
Use `--revision COMMIT` to pin both the processor and model to the same Hugging Face commit.
The loaded revision and runtime versions are logged and included in `/healthz`.

Optional `uv pip install -e ".[quantization]"` enables `--quantization nf4` or `int8` on a
GPU with a compatible bitsandbytes build. Compatibility depends on the PyTorch and GPU runtime;
the audio tower and multimodal projector are excluded from quantization. Use
`--device cpu --quantization none` for CPU fallback. No remote model Python code is enabled.

Configure the Auris process before invoking `listen`:

```powershell
$env:AURIS_AUDIO_URL = "http://127.0.0.1:11435/v1"
$env:AURIS_AUDIO_MODEL = "Qwen/Qwen2-Audio-7B-Instruct"
```

Keep the default loopback binding: the service has no authentication. `/healthz` reports
whether the model has loaded; `/v1/models` lists its configured name. These endpoints do not
load weights. Stop the process to release its model memory.
After each successful or failed inference, unused GPU allocator cache is released while model
weights stay loaded. `/healthz` includes current allocated/reserved bytes and lifetime peaks for
each loaded GPU. These are Torch allocator measurements, not total GPU memory used by all processes.

## Request limits

`POST /v1/chat/completions` accepts system and user text plus one or two user content parts of
the form `{"type":"input_audio","input_audio":{"data":"BASE64","format":"wav"}}`.
Text, labels, and audio order are preserved. URLs and filesystem paths are not audio inputs.
The combined WAV limit is 25 MiB and each excerpt must be no longer than 30 seconds. Audio is
decoded as float32, downmixed to mono, and resampled to 16 kHz without loudness normalization.

Generation defaults to 256 total tokens. A single model call is capped at 512 tokens; the
MusicFlamingo two-excerpt path allows up to 1024 total tokens across its two independent calls.
Explicit smaller token budgets are preserved. Actual truncation
returns `finish_reason: "length"`; empty responses are errors. The model's refusals and uncertain
answers are preserved after a short input-format note: the model receives mono 16 kHz audio,
so stereo placement cannot be assessed. This note is added only to the response; it does not
prime inference or remove contradictory model claims. Streaming, tool execution, and concurrent inference are not
supported; a second active inference receives a retryable HTTP 503 response. Validation errors
use the usual JSON `error.message` field. Requests exceeding the model context are rejected.

## Optional MusicFlamingo research backend

[MusicFlamingo](https://huggingface.co/nvidia/music-flamingo-2601-hf) is a separate, explicit
backend using the native `MusicFlamingoForConditionalGeneration` implementation in Transformers
5.15.1. Qwen2-Audio remains the default. MusicFlamingo weights are licensed under the NVIDIA
OneWay Noncommercial License for noncommercial research only. This repository does not bundle
the weights; review the model's terms before downloading or using them.

```powershell
uv run --no-sync python server.py --backend music-flamingo --model nvidia/music-flamingo-2601-hf --device cuda --quantization nf4 --revision 6b5be086d52f65a1e204cb0faf70bf54e2741ecd
$env:AURIS_AUDIO_MODEL = "nvidia/music-flamingo-2601-hf"
```

The same HTTP request format and bounds apply. MusicFlamingo supports one audio per text input,
so two excerpts produce two independent model calls using the same focus instructions. Each
call receives only its own audio, without prior answers or the other excerpt's audio. The bridge
labels the returned observations `Excerpt 1` and `Excerpt 2` and explicitly identifies them as
independent observations, not a direct A/B judgment. The total generation budget is divided
between the excerpts, with at most 512 tokens per call. With no requested budget, the default
remains 256 total tokens (128 per excerpt). A single excerpt keeps the original request. Model refusals and truncated
answers retain their usual handling.

The adapter uses SDPA and enables the native language-model generation cache. Audio features are
consumed during prefill and excluded from cached decoding steps by Transformers. NF4/int8 exclude
`model.audio_tower`, `model.multi_modal_projector`, and `lm_head` from quantization. These choices
still require live validation on the target hardware and music; passing protocol tests establishes
neither perceptual accuracy nor a reliable improvement judgment.
Completed requests log audio count, requested and effective total output budgets, actual
completion tokens when available, and finish reason. The log entry contains no audio or prompt text.

## Verify without a model

```powershell
uv pip install -e ".[dev]"
uv run --no-sync pytest
uv run --no-sync ruff check .
uv run --no-sync ruff format --check .
```

Tests decode real in-memory WAVs, preserve A/B amplitude differences, exercise the HTTP server,
and verify the Transformers adapter with a fake backend. They do not download weights or
establish model listening quality. Live blinded controls are in `../eval/audio_input.ps1`.
