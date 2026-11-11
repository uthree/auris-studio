# Optional local music models

[日本語](music-analysis-models.ja.md)

The ordinary chord, tempo and isolated-melody commands remain CPU signal processing.
YAMNet adds learned instrument-presence tagging on the CPU without CUDA, an AMD GPU
backend, training, RunPod or an audio upload. Its model file is external to Auris
and is never downloaded by an analysis command.

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
