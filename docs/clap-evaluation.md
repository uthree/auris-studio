# CLAP audio and prompt evaluation

The audio adjustment search can rank actual rendered candidates using learned CLAP embeddings.
Choose a reference recording to search toward its semantic sound, or write a prompt describing
the sound you want. The search explores the same mix, performance, generated clip seeds,
instrument choices, and non-destructive arrangement controls documented in
[reference audio matching](reference-audio.md), retains the exact winning render, and applies the
retained settings in one undo step.

CLAP inference runs locally on the CPU through ONNX Runtime. Python and network access are used
only by the explicit model preparation command below; they are not used by a desktop search.

## Prepare the model once

Install [uv](https://docs.astral.sh/uv/getting-started/installation/), then run from the repository:

```sh
uv run --python 3.12 tools/music-models/export_clap.py --output models/clap-htsat-unfused
```

Choose an output directory that does not already exist. The script downloads the official
[`laion/clap-htsat-unfused`](https://huggingface.co/laion/clap-htsat-unfused) checkpoint, pinned to
commit `8fa0f1c6d0433df6e97c127f64b2a1d6c0dcda8a`, and exports both encoders. Allow several GB of
free space for the Python environment, checkpoint cache, and approximately 620 MB exported
package. Download and conversion time depend on the network and CPU.

To choose CPU PyTorch explicitly, prepare an isolated environment instead:

```sh
uv venv --python 3.12 models/clap-export-env
uv pip install --python models/clap-export-env --index-url https://download.pytorch.org/whl/cpu torch==2.6.0
uv pip install --python models/clap-export-env transformers==4.57.3 onnx==1.17.0 onnxruntime==1.20.1 "numpy>=1.26,<3"
uv run --python models/clap-export-env --no-project --no-managed-python python tools/music-models/export_clap.py --output models/clap-htsat-unfused
```

The exporter checks PyTorch/ONNX parity with several audio and text inputs before publishing
`manifest.json`. If preparation fails, the partial directory has no complete load contract;
choose a fresh output directory after fixing the reported error. A completed package contains:

| File | Contents |
| --- | --- |
| `manifest.json` | Format, model ID, immutable source revision, license, and file SHA-256 hashes |
| `audio.onnx` | Normalized audio embedding encoder |
| `text.onnx` | Normalized text embedding encoder |
| `tokenizer.json` | Official RoBERTa tokenizer |
| `preprocess.bin` | Exact Hann window and Slaney mel coefficients from the pinned processor |
| `verification.json` | Measured PyTorch/ONNX parity and preparation package versions |
| `parity/` | Deterministic waveform, features, tokens, and embeddings for native validation |

Keep the files together. Auris verifies the runtime input hashes, model metadata, tensor shapes,
and tokenizer contract when loading the directory. Exported model files are local data and are
not checked into the repository.

## Use the desktop search

1. Open **Compose → Adjust by Audio Evaluation…**.
2. Select **Reference · CLAP** or **Text prompt · CLAP**, then use **Choose CLAP Model Folder…**
   to choose the prepared model directory containing `manifest.json`.
3. For a reference target, choose a recording and its excerpt start. For a prompt target,
   choose **Describe the target sound**, enter a description such as “A warm, relaxed
   instrumental track with soft percussion.”, and choose **Use Description**.
   Prompts are limited to 77 tokenizer tokens including start/end tokens. Overlong prompts are
   rejected with an error rather than silently shortened. English descriptions are recommended
   for this model.
4. Set the project excerpt, attempt limit, search seed, and enabled search scopes, then start the
   render search. The default budget is 32 candidates; select up to 512 for a broader search.
   Generation seeds affect generated clips only, instrument choices use built-in sounds and
   loaded SoundFonts, and arrangement adjustments preserve written notes. Applying a seed
   change adopts the evaluated generated notes. Model loading and inference run on the
   background worker.
5. Compare the baseline and best cosine similarities, listen to the retained renders, and use
   **Apply Best** to adopt the result.

The target embedding and loaded model remain fixed throughout one search. Changing the target,
model directory, or search settings invalidates the old comparison. Cancellation keeps an
already evaluated partial winner; a cancelled preparation has no completed baseline to apply.

## What the score means

CLAP compares normalized 512-dimensional embeddings with cosine similarity. The diagnostic
`clap_cosine_similarity` lies in `[-1, 1]`; **higher means closer to the chosen target in this
model's embedding space**. It is not a probability, percentage of prompt compliance, or a
general measure of musical quality. Ranking rounds cosine similarity to the nearest 0.000001;
the diagnostic keeps the unrounded value. The unchanged baseline participates in the search,
and an earlier candidate wins a tie.

Both candidate and reference audio use the same deterministic preprocessing: mono conversion,
48 kHz resampling, 10-second chunks, and the official unfused model's Slaney log-mel features.
Short chunks repeat the available waveform as many whole times as fit, then zero-pad the
remainder to ten seconds. There is no random crop during evaluation. Normalized chunk embeddings
are averaged with weights proportional to their original durations, then normalized into one
excerpt embedding before comparison. Waveform amplitudes are preserved for model inference;
audition volume normalization affects only the preview copy.

The learned objective emphasizes semantic sound and can miss subtle balance or performance
differences. Mono conversion also discards stereo width as a distinct feature. The existing
acoustic-feature reference evaluator remains useful for frequency balance, dynamics, stereo
image, and transient texture. Listen to the exact retained baseline and best audio when deciding
whether an improved score gives the result you want.

## Native model contract and tests

The package format is `auris-clap-htsat-unfused-v1`. Graph metadata includes
`auris.clap=htsat-unfused-v1`, `role=audio` or `role=text`, and source model provenance.
`audio.onnx` takes `input_features: float32[1,1,1001,64]`; `text.onnx` takes
`input_ids: int64[1,77]` and `attention_mask: int64[1,77]`. Both return
`embedding: float32[1,512]`. The text sequence uses right padding with ID 1, BOS 0, and EOS 2.

`preprocess.bin` contains little-endian float64 values: a 1024-point periodic Hann window,
followed by the 513 × 64 Slaney mel matrix in row-major order. The STFT uses a 1024-point FFT,
480-sample hop, reflection padding, power spectra, and a -100 dB floor. Coefficients are exported
from the pinned processor so native analysis uses the exact model's preprocessing.

Python contract tests do not download a checkpoint:

```sh
uv run --with pytest --with ruff pytest tools/music-models/test_export_clap.py
uv run --with ruff ruff check tools/music-models/export_clap.py tools/music-models/test_export_clap.py
```

The headless session example uses the same evaluator and render search as the window:

```sh
cargo run -p auris-session --example match_clap -- Project.auris models/clap-htsat-unfused text "A soft instrumental melody with gentle percussion." target/clap-search 8 10
cargo run -p auris-session --example match_clap -- Project.auris models/clap-htsat-unfused audio reference.wav target/clap-reference-search 8 10
```

Each command requires a new output directory. It writes the baseline and winning WAVs, metric
report, and adopted project. Audio mode also retains the exact reference excerpt as
`reference.wav`. The optional arguments specify attempts and excerpt duration.

The Rust parity tests are explicit opt-ins after preparation. Set `AURIS_CLAP_TEST_MODEL` to the
exported model directory, then run:

```sh
cargo test -p auris-analysis real_export_matches_python_embeddings_and_tokens -- --ignored --nocapture
cargo test -p auris-session real_frontend_and_audio_text_objectives_match_the_exported_model -- --ignored --nocapture
```

The checkpoint is provided under Apache-2.0 by its publisher. Upstream implementation and model
details are available in the [official CLAP repository](https://github.com/LAION-AI/CLAP) and
[Transformers CLAP documentation](https://huggingface.co/docs/transformers/model_doc/clap).
