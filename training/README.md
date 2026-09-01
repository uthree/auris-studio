# training — the singing voice models

A singing voice synthesizer built on VITS, running at 48 kHz. This directory trains the `.onnx`
voice files that `crates/auris-singer` loads and sings with, and is the only Python in the
repository that is a project rather than a script.

The model keeps the VITS variational framework (posterior encoder, normalizing flow, adversarial
waveform decoder) but replaces its 1D-CNN sequence modules with modernized Transformers and its
HiFi-GAN vocoder with an NSF-style vocoder driven by an explicit source signal.

* Phonemes are given in IPA. Japanese text is converted with `jpreprocess`.
* Pitch and energy curves and per-phoneme durations are supplied explicitly at inference time —
  score-to-curve conversion lives in the DAW, in `crates/auris-vocal`, not here.

## Why it lives in this repository

A trained model and the code that plays it share a format, and until they shared a repository
nothing checked that they agreed on it. Three things are written down twice — the `metadata_props`
key an export stores its JSON under, the metadata format version, and the phoneme table, down to
which symbols are voiceless — and each pair had to be kept in step by hand, across two checkouts,
with a comment in one repository asserting something about the other.

`tests/test_host_contract.py` is that assertion turned into a test. It reads the Rust sources
directly and fails when the two halves drift, which is the whole reason this directory is here and
not in a repository of its own.

## Quick start

```bash
cd training
uv venv --python 3.11
uv pip install -e '.[dev]' --torch-backend=auto
```

`--torch-backend=auto` reads the installed driver and picks the matching PyTorch wheels — the CUDA
build on a training machine, the CPU build on a laptop and in CI. `doc/development.md` has the
detail, including how to name a backend by hand.

```bash
uv run python scripts/preprocess.py --config configs/preprocess/generic_wav_text.yml
uv run python scripts/train.py --config configs/train/base.yml
uv run python scripts/export_onnx.py --checkpoint runs/base/checkpoints/last.ckpt --output voice.onnx
```

The `.onnx` that falls out is self-contained — phoneme table, audio parameters and voice card all
ride inside it — and pointing Auris Studio at it is the whole installation, exactly the policy a
SoundFont gets.

## Documentation

See [`doc/`](doc/):

| Document | Contents |
| --- | --- |
| [architecture.md](doc/architecture.md) | Model architecture and how it differs from VITS |
| [preprocessing.md](doc/preprocessing.md) | Dataset layout, feature extraction, config reference |
| [datasets.md](doc/datasets.md) | Corpus survey, license tiers, and the data policy |
| [training.md](doc/training.md) | Training procedure, losses, presets |
| [inference.md](doc/inference.md) | Inference API and input format |
| [evaluation.md](doc/evaluation.md) | Measuring an exported voice through the host — the application, not PyTorch |
| [development.md](doc/development.md) | Environment, tests, repository layout |
