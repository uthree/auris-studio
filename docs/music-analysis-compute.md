# Music analysis: compute and model deployment assessment

Assessment date: 2026-09-07. This accompanies the
[research and implementation plan](music-analysis-plan.md).

No cloud resources have been provisioned, no model has been trained, and no audio has been
uploaded. YAMNet and the explicitly authorized noncommercial MuScriptor Small checkpoint
have now been prepared and run locally on CPU. Their integration, hashes, measurements and
accuracy limits are in [the model guide](music-analysis-models.md).

## AMD-only hardware does not make every neural method a cloud workload

Symbolic chord recognition, onset/tempo analysis, chroma-based audio chords and monophonic
pitch tracking should first use CPU implementations. Basic Pitch and small beat/tagging models
are candidates for pretrained CPU inference; training requirements do not determine inference
requirements. Their speed in Auris is still unmeasured.

The measured environment is Windows with an AMD Ryzen 7 9800X3D CPU and an AMD GPU.
These trials use CPU only. GPU compatibility and peak RAM/VRAM requirements have not been
measured; do not infer them from the GPU vendor or checkpoint size.

The current singer already enables ONNX Runtime DirectML on Windows. DirectML supports
DirectX 12 hardware including AMD, so it is a possible later accelerator for compatible ONNX
models. The `auris-gpu` wgpu DX12 build problem is a separate dependency path and does not by
itself rule out DirectML. Test the actual model's operators, precision and CPU fallback under
the pinned runtime; today's provider documentation does not guarantee support in the older
runtime used by this repository. Microsoft now describes DirectML as sustained engineering
and directs new feature development to WinML.
[ONNX Runtime documentation](https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html).

ROCm support depends on the particular GPU, operating system and software versions. Do not
plan a native CUDA research repository as an automatic AMD port; check the applicable AMD
matrix only after the device is known.
[AMD compatibility documentation](https://rocm.docs.amd.com/en/latest/compatibility/compatibility-matrix.html).

## Separate small inference trials from expensive research experiments

| Candidate | Placement and purpose | Deployment evidence / unresolved gate |
| --- | --- | --- |
| Basic Pitch | Local CPU trial for isolated-instrument polyphony; optional DirectML after parity checks. | Official ONNX artifact; repository Apache-2.0. Verify exact downloaded artifact, preprocessing/postprocessing parity and `ort` compatibility. [Source](https://github.com/spotify/basic-pitch) |
| Beat this! small | Local CPU trial for beats and downbeats if DSP accuracy is insufficient. | Small checkpoint about 8.1 MB; code and weights MIT. File size does not equal peak RAM. Rust/ONNX export needs separate validation. [Source](https://github.com/CPJKU/beat_this) |
| YAMNet | Implemented optional CPU instrument-presence tagging. | Google weights and architecture Apache-2.0, export/reference parity checked. The local sampled fixture exposed low instrument coverage at threshold 0.2. [Weight terms](https://groups.google.com/g/audioset-users/c/Ly4guQnlv_o) |
| MT3 | External inference reference for multi-instrument notes. | T5X research stack; repository Apache-2.0. Pin checkpoint and runnable environment before estimating compute. [Source](https://github.com/magenta/mt3) |
| YourMT3+ | Priority commercial-workflow candidate; CPU trial before a RunPod proposal. | The author's actual HF source files carry Apache-2.0 headers and the checkpoint card explicitly licenses weights Apache-2.0, despite the separate GitHub landing page's GPL label. Pin the exact HF artifacts. Source acquisition failed with HTTP 429/500 during this session; no checkpoint was run. This is not evidence that CPU inference is impossible. [Code](https://huggingface.co/spaces/mimbres/YourMT3/blob/5e66c1ea173a8186e0d20432b841d3180cc015b5/amt/src/model/ymt3.py), [weights](https://huggingface.co/mimbres/YourMT3/blob/main/README.md) |
| MuScriptor Small | Implemented optional local noncommercial CPU transcription draft. | Code MIT, weights CC BY-NC 4.0; explicit per-run warning/acknowledgement. 411,888,600-byte checkpoint; twelve-second mixture completed in 22.46 s including startup. Instrument confusions remain. Medium/large are separate evaluation candidates. [Model](https://huggingface.co/MuScriptor/muscriptor-small) |
| Demucs plus transcription | Optional external diagnostic: does separation improve accepted note accuracy? | MIT repository; four broad default stems do not identify arbitrary individual parts. Compare end-to-end accuracy, runtime and separation artifacts against direct transcription. [Source](https://github.com/facebookresearch/demucs) |
| Training/fine-tuning any transcription or tagging model | Separate RunPod proposal after error analysis shows a concrete data/model gap. | Needs dataset rights, a training recipe, checkpoint choice and measured memory/throughput. No defensible fixed GPU-hour estimate yet. |

Essentia and madmom are useful research references but require artifact-level license review.
Essentia's published MTG models are CC BY-NC-SA 4.0 (with proprietary licensing offered), and
madmom distinguishes its code from model/data terms. Cloud execution does not remove these
deployment questions.
[Essentia models](https://essentia.upf.edu/models.html),
[madmom](https://github.com/CPJKU/madmom).

The dispositions above are engineering choices for this project. They are not claims that
every listed external model mathematically requires NVIDIA hardware, nor measured minimum
memory requirements. An optional model must never silently trigger cloud upload.

## A bounded RunPod pilot would answer whether multi-instrument transcription is worth integrating

Proposed initial envelope: one Linux NVIDIA GPU with 24 GB VRAM, 32–64 GB host RAM and about
100 GB working storage for pinned checkpoints and a small excerpt set. Start with batch size 1
and each model's supported chunking. These are provisional experiment allocations, not
vendor minimums or guarantees that every model fits. Full datasets such as Slakh would need
additional storage; the pilot should use a small prepared subset.

Reserve at most two billed GPU-hours for the first smoke test after the user chooses to
provision compute. The envelope includes setup/download time; stop when it is exhausted rather
than silently adding hours. Obtain the live GPU/storage/network quote at provisioning time.
If a model fails to fit, reduce supported batch/chunk settings and measure again; report
the failure and a specific 48 GB proposal before expanding resources. Parameter count alone
does not account for activations, attention caches, precision or framework overhead.

The pilot sequence should be:

1. Resolve exact code and weight licenses, pin commits/checkpoint hashes and a container image,
   and prepare authorized local evaluation excerpts and references. Do not put voice-training
   data or credentials in the container image.
2. Run one 30-second and one three-minute excerpt per chosen model; record cold/warm runtime,
   RAM/VRAM, failures and instrument-aware note F1. Include simple and dense mixtures.
3. If the smoke test fits the envelope, compare 10–20 short held-out excerpts against the
   stage-3 local baseline. Compare direct transcription to separation plus transcription only
   on a subset where masking is the diagnosed failure.
4. Return MIDI, source-time event JSON, scores, configuration, logs and hashes. Record actual
   billed runtime and storage use, then stop the compute instance. Keep source audio out of
   diagnostic logs and remove uploaded copies according to the agreed retention policy.
5. Decide on integration using correction effort, instrument-aware accuracy, latency, costs
   and deployability together. If licensed CPU/AMD inference can meet the product budget,
   prefer local deployment; otherwise propose an explicit opt-in external worker contract.

A later training request should state the observed failure, available labeled examples,
baseline/target metric, data split, expected benefit and measured steps-per-second/peak-memory
from a short trial. Only then estimate training hours and choose a larger GPU or multiple GPUs.
Renting a training machine now would not resolve the more immediate questions of task scope,
recognition accuracy and usable model artifacts.
