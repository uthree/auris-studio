# Evaluating a voice through the host

Training's validation answers one question: does the model follow the curves it is given?
It sings the validation set through `AurisSinger.infer` in PyTorch, re-analyses the output
with FCPE and logs `val/mel`, `val/f0_rmse_cent` and the rest ([training.md](training.md)).
Export's verification answers a second: is the exported graph the same function? It compares
onnxruntime against PyTorch on one machine at sizes the trace never saw
([inference.md](inference.md#exporting-to-onnx)).

Between those two and the person who presses **Sing** there is a third thing, and it is the
one the person actually hears: the host. `crates/auris-singer` reads the file, cuts a long
timeline into chunks and stitches the answers, run-length-encodes frames into tokens, puts
the energy on the model's scale, draws its own noise from its own generator and runs its own
copy of onnxruntime — and `crates/auris-vocal` before it decided what every frame's phoneme,
pitch and energy would be. None of that is in the training log. A voice that validates well
and verifies exactly can still sing differently in the application, and until now the only
way to know was to listen.

`scripts/evaluate_host.py` holds the host to the same numbers the other two are held to.

## How it reaches the host

It runs `auris`, the command line frontend, which drives the same session the window does.
Two subcommands exist for this:

* `auris sing-frames <frames.json> --voice <model.onnx> -o <out.wav> --report <facts.json>`
  sings a frames file — one phoneme, one pitch, one energy per hop, the format
  `auris frames` writes — through a voice into a WAV, by exactly the inference a take goes
  through, and writes the session's own account of it: sample rate, chunk count, load and
  render time, whether the GPU sang.
* `auris frames <project.auris> -o <frames.json>` writes what a project's singer track will
  be sung as.

Everything crosses as files. The two languages are kept apart on purpose — `uv run pytest`
must not need a Rust toolchain and `cargo test` must not need a Python one — so the host is
*found*, never imported: `AURIS_CLI` names a built binary, and otherwise the command is
`cargo run -p auris-cli` from the repository root. The two constants this side has to know
about the host, the frames' silence token and the energy scale, are read out of the Rust
sources as text and pinned by `tests/test_host_contract.py`, the way every shared constant is.

## What it measures

```bash
uv run python scripts/evaluate_host.py \
    --voice runs/small/model.onnx \
    --checkpoint runs/small/checkpoints/last.ckpt \
    --data data/processed/jsut_song \
    --json before.json --workdir eval/before
```

The corpus run takes the validation utterances — the same split, seed and cap the data
module draws, so these are the utterances the training log's `val/…` were measured on — and
sings each one three ways:

| column | what sings | what it tells |
| --- | --- | --- |
| **host** | the utterance's own curves, laid out with the checkpoint's alignment, through `auris sing-frames` and the `.onnx` | the numbers the application would get, after export and after the host |
| **reference** | the same curves through `Synthesizer.synthesize` from the checkpoint | what export and host cost together, as `host − ref` |
| **song** | every utterance end to end on one timeline with silence between, sung as one file, sliced back apart | the only column where the host's chunking and stitching run — a corpus utterance is shorter than one chunk — so a seam that costs something shows as `song − host` |

Each column carries the trainer's own metrics from `auris_singer.metrics`, so a number here
means what the same name means in the training log: `mel_l1` against the recording;
`f0_rmse_cent`, `f0_accuracy`, `f0_corr`, `vuv_error`, `voiced_ratio_error` from FCPE over
the render; `energy_rmse_db`, `energy_bias_db`, `energy_corr` from its frame RMS; and `peak`,
because the host's recorder does not clip and a peak near 1.0 is a voice about to. Under the
table: how long the host took to sing how much audio, as a real-time factor, with the model's
load time separated out.

Corpus utterances carry no durations — training recovers them by monotonic alignment search —
so the checkpoint the voice was exported from is required: it aligns each utterance exactly
as validation does, and it is the reference render.

The noise differs between host and reference. Each draws its own — the host from
`auris_core::rng` streams named by the seed, PyTorch from its generator — so the two columns
are compared metric to metric, never sample to sample; a bit-exact comparison is what
`verify_onnx` is for.

### The score run

```bash
uv run python scripts/evaluate_host.py --voice runs/small/model.onnx --score
uv run python scripts/evaluate_host.py --voice runs/small/model.onnx --score --spec my.asong
```

Notes and words through the whole path a person walks: `auris compose` writes a project from
a specification (a built-in verse of kana when none is given, so no dictionary is needed),
`auris frames` writes what its singer track will be sung as, `auris sing` sings it into the
project. The take is then measured against those frames — the pitch and energy the session
itself asked for, with the frames' energy put back on the model's scale the way the host puts
it. There is no recording, so there is no `mel_l1`; the run reports control fidelity and the
wall time of `auris sing`, session and model included, as a wall-clock real-time factor.

This is the run that has `auris-vocal` in it: the consonant widths, the attack and release,
the placeholder vowel, the frame hop the document carries. A regression there is invisible to
the corpus run by construction.

## Using the numbers

The intended workflow is the baseline diff, exactly as `docs/evaluation.md` at the
repository root describes for the composer:

```bash
uv run python scripts/evaluate_host.py ... --json before.json
# change something on either side — the trainer, the export, the host — and re-export
uv run python scripts/evaluate_host.py ... --baseline before.json
```

The table then carries a `Δ baseline` column against the previous run's host column. Read
the columns together, never one alone:

* `host − ref` far from zero, with `song − host` near it, is the export or the host's
  arrangement — the run-length encoding, the energy scale, the voicing rule.
* `song − host` far from zero is the chunking: a cut landing somewhere it should not, a seam
  the stitching leaves audible. `timing` says how many chunks the song took.
* Both near zero and the numbers still bad is the model, and the training log already said so.

`--workdir` keeps every file that crossed the boundary — each utterance's frames, the
recording, the host's render, the reference render, the song and its frames, the host's
reports — so any number in the table can be listened to. The final judge stays a pair of
ears; the numbers say where to point them.

## Limits

* **Linux has no GPU provider.** The host reaches the GPU through the platform's own
  provider, DirectML on Windows and Core ML on macOS, and `--acceleration gpu` on Linux is
  refused as it is in the application. `timing` says which processor sang.
* **The host column's timing is a cold one.** Each corpus utterance is its own `auris`
  process, and its one chunk is that process's first inference, which onnxruntime spends
  warming its arenas on. The song line under the table is the warm number — one process,
  several chunks — and on the first run it was five times faster per second of audio. Read
  the host line as "what pressing *Sing* on a fresh document costs" and the song line as
  the model's speed.
* **Timings are the dev profile's unless `--release` is given.** The workspace builds its
  dependencies at full optimisation even in debug, and onnxruntime is a prebuilt library, so
  the difference is small — but a number worth writing down wants the release build.
* **`pytest` skips the end-to-end tests without a host.** `tests/test_host_eval.py` builds a
  tiny voice and drives a real `auris` through both runs; the tests are marked `slow` and
  skip wherever `Host.find()` fails, which is CI's training job. Everything else in the file
  runs everywhere.
