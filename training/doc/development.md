# Development

## Environment

The project uses [uv](https://docs.astral.sh/uv/) for a reproducible
environment.

```bash
uv venv --python 3.11
uv pip install -e '.[dev]'
```

PyTorch is pinned to the **CUDA 12.8** wheels in `pyproject.toml`, because the
default PyPI wheels are built against a newer CUDA runtime than many drivers
support. If your driver differs, change the index URL:

```toml
[[tool.uv.index]]
name = "pytorch"
url = "https://download.pytorch.org/whl/cu128"   # or .../cpu, .../cu126
explicit = true
```

Then force a re-resolve:

```bash
uv pip uninstall torch torchaudio && uv pip install -e '.[dev]'
```

Check the result with:

```bash
uv run python -c "import torch; print(torch.__version__, torch.cuda.is_available())"
```

## Repository layout

```
src/auris_singer/
  model.py             # AurisSinger: the whole generator side
  lightning_module.py  # training loop, losses, logging
  losses.py            # envelope / multi-param mel / KL / GAN losses
  infer.py             # Synthesizer
  export.py            # ONNX export wrapper + verification
  modules/
    transformer.py     # RMSNorm, SwiGLU, RoPE, QK-Norm, SDPA encoder
    encoders.py        # TextEncoder, PriorEncoder, PosteriorEncoder
    flow.py            # Transformer coupling flow
    source.py          # RefineGAN-style excitation generator
    generator.py       # NSF-HiFi-GAN decoder
    discriminator.py   # MPD + multi-resolution STFT, speaker-conditional
    alignment.py       # monotonic alignment search
  data/                # dataset, collate, bucket sampler, LightningDataModule
  preprocess/          # feature extraction pipeline, FCPE wrapper
  text/                # IPA table, Japanese front-end
  utils/               # audio front-end, masks, config loading
configs/
  preprocess/          # dataset preprocessing configs
  train/               # presets.yml + one config per model size
scripts/               # preprocess.py, train.py, infer.py, export_onnx.py
tests/                 # pytest suite
doc/                   # this documentation
```

## Tests

```bash
uv run pytest
```

Skip the tests that need the FCPE model or the jpreprocess dictionary:

```bash
uv run pytest -m "not slow"
```

Everything runs on CPU. `tests/conftest.py` builds a synthetic preprocessed
dataset, so most tests need neither network access nor real audio.

Long test runs (or anything else long) should go in tmux so they survive a
disconnected session:

```bash
tmux new-session -d -s tests "uv run pytest -q"
```

## Linting

```bash
uv run ruff check .
uv run ruff format .
```

## Conventions

* Code, comments and documentation are in English.
* `README.md` stays a short overview; details belong in `doc/`.
* Tensors crossing module boundaries are channel-first `(B, C, T)`, matching
  the VITS code base. Transposes to `(B, T, C)` happen inside the Transformer.
* Masks are `(B, 1, T)` float tensors, 1 for valid frames. Every module masks
  its own output, so padded positions are exactly zero.
* Work is committed to `dev` in units, and pushed.

## Things worth knowing before changing the model

* **Frame grid.** Spectrogram, f0 and energy share one grid:
  `T = len(wav) // hop_length`. `utils/audio.py` reflection-pads by
  `(n_fft - hop_length) // 2` and uses `center=False` to guarantee it. If you
  change the STFT settings, change them in both the preprocessing and the
  training config — `tests/test_config.py` checks the shipped ones agree.
* **Upsample schedule.** `prod(upsample_rates)` must equal `hop_length`; the
  constructor raises otherwise. Padding is derived so each stage outputs
  exactly `rate ×` its input, including for odd `kernel - rate`.
* **The decoder never sees f0 or energy directly**, only the excitation signal.
  Adding a pitch embedding would defeat the design.
* **The text encoder is only reachable through the KL terms** — the decoder is
  fed by the posterior. If you write a new loss and the text encoder stops
  learning, that is why.
* **MAS is not differentiable** and runs under `no_grad` on CPU via numba. It
  is the reason `numba` is a dependency; without it, an equivalent NumPy
  fallback runs the same dynamic program more slowly.
