# Development

## Environment

The project uses [uv](https://docs.astral.sh/uv/) for a reproducible
environment. Everything below is run from `training/`, which has its own
`pyproject.toml` and its own `.venv` — the Rust workspace a directory above
knows nothing about either.

```bash
uv venv --python 3.11
uv pip install -e '.[dev]' --torch-backend=auto
```

`--torch-backend=auto` reads the installed NVIDIA driver and resolves PyTorch
from the matching index: the CUDA build where there is a card to use it, the
CPU build where there is not. That is what lets one command serve a training
machine, a laptop and a CI runner, and it is why `pyproject.toml` pins no index
of its own — it used to name the CUDA 12.8 wheels, which was correct on exactly
one of those three.

Name a backend by hand where the detection is wrong, or where you want the CPU
build on a machine that has a card:

```bash
uv pip install -e '.[dev]' --torch-backend=cpu     # or cu128, cu126, ...
```

There is deliberately no `uv.lock`. A lock file would record whichever PyTorch
index the machine that wrote it detected, and hand that machine's answer to
every other one — which is the pin this project just stopped carrying.

Check the result with:

```bash
uv run python -c "import torch; print(torch.__version__, torch.cuda.is_available())"
```

Training needs the CUDA build and a card. Everything else here — the tests, the
contract test, a preprocessing run — is content with the CPU build.

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
  test_host_contract.py  # what this and the Rust host must agree on
doc/                   # this documentation
```

Everything above is under `training/` in the Auris Studio repository. The Rust
crates a directory up are not importable from here and are not meant to be; the
one place this project reads them is `tests/test_host_contract.py`, which reads
them as *text*.

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

### The contract test

`tests/test_host_contract.py` checks this project against the code that plays
what it exports — `crates/auris-singer` and `crates/auris-vocal`. Both sides
write down the same three things independently: the `metadata_props` key, the
metadata format version, and the phoneme table down to which symbols are
voiceless. The test parses the constants out of the Rust sources and compares.

A failure means the two halves have drifted, and the fix is a decision, not an
edit to whichever side the test happened to name: either the export changes and
the host must be taught to read it, or the host is right and the export is
wrong. Bumping `FORMAT_VERSION` on both sides is how the first case is
declared.

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
