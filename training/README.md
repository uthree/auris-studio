# auris-singer

A singing voice synthesizer built on VITS, running at 48 kHz.

The model keeps the VITS variational framework (posterior encoder, normalizing
flow, adversarial waveform decoder) but replaces its 1D-CNN sequence modules
with modernized Transformers and its HiFi-GAN vocoder with an NSF-style vocoder
driven by an explicit source signal.

* Phonemes are given in IPA. Japanese text is converted with `jpreprocess`.
* Pitch and energy curves and per-phoneme durations are supplied explicitly at
  inference time — score-to-curve conversion lives in the DAW front-end, not
  here.

## Quick start

```bash
uv venv --python 3.11
uv pip install -e '.[dev]'
```

```bash
uv run python scripts/preprocess.py --config configs/preprocess/generic_wav_text.yml
```

```bash
uv run python scripts/train.py --config configs/train/base.yml
```

## Documentation

See [`doc/`](doc/):

| Document | Contents |
| --- | --- |
| [architecture.md](doc/architecture.md) | Model architecture and how it differs from VITS |
| [preprocessing.md](doc/preprocessing.md) | Dataset layout, feature extraction, config reference |
| [datasets.md](doc/datasets.md) | Corpus survey, license tiers, and the data policy |
| [training.md](doc/training.md) | Training procedure, losses, presets |
| [inference.md](doc/inference.md) | Inference API and input format |
| [development.md](doc/development.md) | Environment, tests, repository layout |

## License

See [LICENSE](LICENSE).
