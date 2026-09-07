# Optional local music models

The ordinary chord, tempo and isolated-melody commands remain CPU signal processing.
Two separate commands add learned inference: YAMNet instrument-presence tagging and
MuScriptor Small multi-instrument note drafts. Both run on the CPU; neither needs CUDA,
an AMD GPU backend, training, RunPod or an audio upload. Model files are external to Auris
and are never downloaded by an analysis command.

## Instrument presence with YAMNet

Prepare the waveform-to-scores ONNX export once, using a separate `uv` environment:

```sh
uv run tools/music-models/export_yamnet.py --output models/yamnet.onnx
auris analyze-instruments recording.wav --model models/yamnet.onnx --threshold 0.2
```

The script downloads hash-checked Google sources and weights, includes their preprocessing
in ONNX, and verifies it against TensorFlow on silence, noise, a sine and an impulse.
It refuses to replace an existing output. Preparation needs TensorFlow; inference only
needs the exported ONNX file. See the script's pinned dependencies and
[YAMNet notice](../crates/auris-analysis/NOTICE-YAMNET.md). Google's maintainer confirms
Apache 2.0 for both the architecture and weights in the
[official user group](https://groups.google.com/g/audioset-users/c/Ly4guQnlv_o).
Redistribution must preserve the applicable license and notices.

In the desktop, import audio, choose **Estimate Instruments…** from the clip menu and
select the prepared ONNX file. **View Analysis Draft** shows the clip mean and overlapping
window hypotheses. This command does not change tracks or assign instruments to notes.
The MCP/agent equivalent is `analyze_instruments` with `audio`, `model` and optional `threshold`.

YAMNet is a general AudioSet event classifier, not a specialist instrument separator.
Scores are independent, uncalibrated sigmoid values. Multiple labels may be returned;
no label above the threshold means **unknown**, not absence. JSON also retains the five
strongest event labels, including non-instrument classes. English labels follow the model
vocabulary. A clip mean can hide a briefly audible instrument; inspect the windows too.
Lowering the threshold exposes weaker hypotheses and can increase false positives.

The CPU provider uses two intra-operation threads, 16 kHz mono, 975 ms windows and
480 ms hops. The last window is padded. Limits are thirty minutes, eight source channels
and a 32 MiB ONNX file. Channel averaging can cancel opposite-phase stereo. Decoding and
resampling allocate the source before the analysis limit is checked; these are not streaming
memory limits. Cancellation is checked between inference windows.

## MuScriptor is a separate noncommercial option

MuScriptor's code is MIT, but its **weights are CC BY-NC 4.0**. The desktop presents a warning
on every invocation and starts no inference until the user explicitly chooses noncommercial
use. Cancel closes the warning without loading a model. CLI and model-facing
tools require an explicit acknowledgement on every call. Acknowledgement is not a commercial
license: do not use this model for commercial work without separate permission from its
rights holders. Review the [model's additional terms](https://huggingface.co/MuScriptor/muscriptor-small)
and [CC BY-NC 4.0](https://creativecommons.org/licenses/by-nc/4.0/legalcode.en).

Auris' own Apache 2.0 license does not change when this optional integration is available.
The application does not bundle the noncommercial weights. This is not a declaration that
every generated MIDI file automatically changes license; model use and rights to the source
music/output still need to satisfy the applicable terms. Commercial workflows can use the
ordinary commands and YAMNet without invoking MuScriptor.

### Obtain and convert the model once

The user downloads the original checkpoint directly from Hugging Face after accepting its
terms. Neither the original weights nor converted ONNX weights are redistributed with Auris.
The converter only accepts local files and does not download a checkpoint itself.

Accept access terms on the model page and authenticate locally with `hf auth login`.
Never put a token in a project or chat. Download the tested revision in PowerShell:

```powershell
hf download MuScriptor/muscriptor-small model.safetensors config.json README.md `
  --revision 8c127f603b807520fa465c838e9bfee8a91ada4e --local-dir models/muscriptor-small
```

Create a separate CPU conversion environment with `uv` (Python is needed only for conversion):

```powershell
uv venv --python 3.12 models/muscriptor-convert
uv pip install --python models/muscriptor-convert/Scripts/python.exe `
  muscriptor==0.3.0 torch==2.5.1 torchaudio==2.5.1 `
  onnx==1.17.0 onnxruntime==1.20.1 "numpy>=1.26,<3" --torch-backend cpu
models/muscriptor-convert/Scripts/python.exe tools/music-models/export_muscriptor.py `
  --checkpoint models/muscriptor-small/model.safetensors `
  --output-directory models/muscriptor-onnx --acknowledge-noncommercial
```

Keep the original `config.json` beside the checkpoint. The converter accepts only the tested
Small architecture and refuses an existing output directory. It generates:

* `audio.onnx`: five seconds of 16 kHz mono audio to the conditioning prefix, including STFT,
  magnitude mel features and the original null instrument/dataset conditioning.
* `decoder.onnx`: token embeddings, positions, Transformer, logits and explicit KV caches.
* `muscriptor.json`: format/version, original checkpoint hash, graph checksums, token vocabulary,
  instrument names, noncommercial license metadata and numerical verification results.

The converter checks audio features and consecutive cached decoding steps against official
PyTorch inference on silence, noise and a sine. A failed check does not publish the package.
Keep these three output files together. Select **decoder.onnx** in Auris; normal transcription
runs through Rust and ONNX Runtime and requires neither Python nor a Python environment variable.
Conversion does not remove the model's noncommercial restrictions. The adapted implementation's
[MIT attribution](../crates/auris-analysis/NOTICE-MUSCRIPTOR.md) is separate from weight licensing.

### Make and accept a draft

Import audio and choose **Transcribe Mixture (Noncommercial)…** from its context menu.
Accept the warning for this use, select the converted decoder.onnx, and inspect **View Analysis Draft**.
**Add Draft as Instrument Tracks** creates a track per predicted instrument group in one
Undo step. It preserves the source clip, maps trim/stretch/repeats through the tempo map,
and refuses results after source/document edits. Tracks retain `MuScriptor [NC]` in their names.
Pitched tracks use the default playback instrument until a suitable patch is chosen; drums
use the built-in drum kit when available. Group names are predictions, not recovered patches.

For file-based JSON and a new MIDI file:

```powershell
auris transcribe-mixture recording.wav --model models/muscriptor-onnx/decoder.onnx `
  --acknowledge-noncommercial --midi draft.mid
```

Omit `--midi` for read-only JSON. To add tracks to an existing project, supply
`--project Song.auris --apply [--at-beat 0]`. Existing MIDI destinations are refused.
The MCP/agent tool `transcribe_mixture` takes `audio`, `model`,
`acknowledge_noncommercial` (false by default), optional `midi_output`, and
`project`/`apply`/`at_beat`. The caller must present the restriction and obtain explicit
acknowledgement for that invocation before passing true. JSON preserves the checkpoint hash,
backend version, model license and source-second events.

Inference uses ONNX Runtime's CPU provider, float32, two compute threads, greedy decoding
and five-second chunks. Rust maintains KV caches and forces the next chunk's tie prologue
from unfinished notes, following MuScriptor 0.3.0. There are no Python subprocesses or temporary
audio files. The application never contacts Hugging Face during transcription.

Input is limited to ten minutes/eight channels, each graph to 512 MiB, the manifest to 256 KiB,
and generation to 2,000 tokens per chunk, 200,000 notes and thirty minutes. The runtime checks
model hashes, metadata, vocabulary and output shapes before using them. Hashes detect changed
files; they are not a signature authenticating the person who prepared a package. Cancellation
is checked between ONNX calls. Loading, decoding/resampling and an in-flight ONNX call finish
before cancellation is observed. These bounds are not a streaming or peak-RAM guarantee.

These are editable note drafts, not guaranteed complete scores. The model does not recover
velocity; imported notes use a fixed value. Pitch, onset, offset, instrument groups and drum
hits can be wrong or missing. Review the result against the recording before using it.

## Local measurements, 2026-09-07

Windows, AMD Ryzen 7 9800X3D, CPU inference only, optimized debug Rust build. These are single
smoke measurements on an original twelve-second piano/acoustic-bass excerpt rendered with
MuseScore General. They include process/model startup and were taken during development;
they are not stable latency targets or held-out real-recording accuracy estimates. Peak RAM
was not measured. The fixture generator is `render_analysis_fixture` in `auris-session`.

* YAMNet: piano 0.84 s, bass 1.02 s, mixture 0.35 s. None of the three clip means produced an
  instrument above 0.2. It recognized music but abstained on specific instruments; this is
  a coverage failure, not a successful identification. The export's maximum absolute error
  against TensorFlow across four fixtures was 0.0000392; Rust also matched the saved reference.
  The separate `measure_instruments` synthetic three-minute timing probe took 0.33 s for
  model loading/inference, excluding process startup and source generation/decoding; it has
  no instrument ground truth and is not directly comparable to the CLI timings above.
* MuScriptor Small ONNX: 7.33 s for twelve seconds (real-time factor 0.61), including graph
  loading and MIDI export. All 58 notes exactly matched the previous PyTorch result in pitch,
  onset, offset and instrument label across three chunks. The converter checked three
  waveforms and eight decoding steps each; maximum prefix/logit errors were 0.001971/0.001727.
  This is float32 numerical parity, not a guarantee that every future near-tied argmax agrees.
* The previous PyTorch measurement was 22.46 s for twelve seconds (real-time factor 1.87), 58 estimated notes
  against 56 references, and successful MIDI export. Instrument-agnostic onset F1 was 0.912;
  onset-and-offset F1 was 0.807. Every note was assigned to `acoustic_piano`, including bass
  pitches. Requiring the correct instrument lowered onset F1 to 0.789 and offset F1 to 0.737.
  ONNX retained these scores and instrument confusions; changing the runtime does not improve
  the model's transcription accuracy.
* A separate four-second pure-tone mixture returned no notes. Synthetic timbres alone are
  insufficient to establish real-recording performance.

Matching used 50 ms onset, 50-cent pitch and offset tolerance max(50 ms, 20% of reference
duration). The instrument-aware totals count the unrecognized eight bass reference notes
as misses; 45 onset matches and 42 offset matches remain out of 56 references/58 estimates.
The benchmark artifact records hashes and scores in
[learned-analysis-local.json](benchmarks/learned-analysis-local.json).

The local inference path is usable for evaluation and manual correction. These results do
not justify a claim of complete multi-instrument recovery. A broader authorized excerpt set
and a commercially usable transcription backend remain the next evaluation priorities;
see the [compute assessment](music-analysis-compute.md).
