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

### Whether the consonants are formed

Pitch and loudness reach the decoder through the excitation, and the metrics above ask
whether they arrived. The *words* reach it through the latent, and a render can track its
f0 to the cent while singing every syllable as the same vowel. The alignment the corpus run
already has — which phoneme is on which frame — lets `mel_l1` be split by manner class
(`auris_singer.intelligibility`, the classes in `text/ipa.py`):

| metric | what it is |
| --- | --- |
| `mel_l1_vowel` | the mel distance over vowel frames alone |
| `mel_l1_consonant` | the same over nasals, plosives, affricates, fricatives and approximants |
| `mel_l1_sibilant` | the same over the sibilants — `s ɕ ʃ ts tɕ tʃ` and their voiced pairs — the consonants a synthesiser fails to form first |
| `sibilant_tilt_db` | on sibilant frames, the energy above 4 kHz against the energy below it, render minus recording, in dB |

### Whether the words can be heard

```bash
uv pip install -e '.[dev,export,asr]' --torch-backend=auto
uv run python scripts/evaluate_host.py ... --asr
```

The class metrics say whether a consonant was *formed*; `--asr` asks whether the words
were *heard*. A recogniser in the voice's language transcribes each render, the
language's own front-end (`text/`) turns the transcript back into IPA, and the phoneme
error rate — edits per phoneme asked for, by Levenshtein — is the `per` row. Rests and
devoicing are taken off both sides first (`intelligibility.hearable`): a listener reports
words, not pauses, and hears a vowel rather than whether the singer voiced it.

Japanese is [ReazonSpeech](https://github.com/reazon-research/ReazonSpeech), the k2 model
on sherpa-onnx: 35 000 hours of broadcast Japanese, Apache-2.0, 200 MB into the Hugging
Face cache on first use, and a few hundred milliseconds a phrase on the CPU. It is the
`asr` extra, installed from the project's repository since it is not on PyPI, and nothing
else needs it. `--asr-precision int8` halves the download where the CPU is small.

A recogniser trained on speech is not at home in song, so in corpus mode the recording
itself is transcribed too and its rate is the **recording** column: the ceiling, and the
number to read the others against. On the mid-training JSUT voice the recording came back
near word-perfect (`春が来た春が来た` for 春が来た春が来た) and the render as `パンがパーンがたい`
— a `per` near one, which is what the ear said. There is no recording in the score run,
so its `per` stands alone, against the phonemes the frames spelled.

The rate says how many phonemes were lost; the tally under the table says **which, and
what they were heard as**. Every `(asked, heard)` pair of the edit path behind the rate is
counted across every take of every utterance (`intelligibility.align`), and the report's
`summary.confusions` holds the counts per column; the table prints the main column's worst
phonemes — asked, heard right, dropped, heard as what — and the phonemes the listener
inserted. It is the difference between "a fifth of the consonants are wrong" and "ɕ is
heard as s and h is not heard at all", which are two different fixes.

**Another language is one registration.** `asr.RECOGNISERS` maps a language code to a
class with a `language` and a `transcribe(wav, sample_rate) -> str`; the front-end that
turns its text into IPA is the one the trainer already has for that language in `text/`,
or a new one there. Nothing else in the run knows what language it is listening in.

The tilt is the consonant-width study's own measurement made permanent: at a too-short
width the model did not form the /s/ at all, and the ratio went from 6.7 to 40.6 once it
was given its width, against 36.6 in the recording. Zero is a sibilant formed as the singer
formed it; well below zero is a hiss the model never made, and a vowel distance that holds
while the consonant distance moves is a change that touched the words and not the tone. A
class the utterance lacks is NaN and left out of the mean, never counted as zero.

Corpus utterances carry no durations — training recovers them by monotonic alignment search —
so the checkpoint the voice was exported from is required: it aligns each utterance exactly
as validation does, and it is the reference render.

One take is one draw of the prior, and on a small voice a draw can put a phrase well out
of tune — the same utterance measured 19 cents of pitch error on one seed and 46 on
another. `--take-seeds 3` sings every column three times, seeds `--take-seed` onwards,
and averages each utterance's numbers over the takes; the table then reads the voice
rather than the throw. Each take's files are kept, suffixed `.s<seed>`.

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

On a multi-speaker voice the corpus run sings each utterance as the speaker the corpus says
it belongs to, and the score run as the model's first speaker; `--speaker <name>` makes
either run sing everything as that one.

`--workdir` keeps every file that crossed the boundary — each utterance's frames, the
recording, the host's render, the reference render, the song and its frames, the host's
reports — so any number in the table can be listened to. The final judge stays a pair of
ears; the numbers say where to point them.

## What the numbers have said so far

The instrument was built to answer one complaint — the consonants of the JSUT-song `small`
voice were not formed — and its first campaign (September 2026) ran every candidate fix
through both runs. The recording itself scores PER 0.09 through the same listener, which is
the floor; every voice below is 40k steps on the same corpus, measured on the labelled
alignment with three takes averaged, host column.

| voice | alignment | KL | corpus PER | consonant mel L1 | score PER |
|---|---|---|---|---|---|
| `small`, 4k steps | MAS | default | 0.635 | — | — |
| `small` | MAS | default | 0.537 | 1.39 | 0.744 |
| `small` | MAS | `kl_free_bits: 0.005` | 0.404 | 1.20 | 0.760 |
| `small` | MAS | `kl: 2.0` | 0.380 | 1.39 | 0.574 |
| `small` | labelled | default | 0.275 | 0.88 | 0.411 |
| `small` | labelled | `kl_free_bits: 0.005` | 0.328 | — | 0.744 |
| `small` | labelled | `kl: 2.0` | 0.409 | — | 0.814 |
| `base` | labelled | default | 0.203 | 0.77 | 0.310 |
| `base`, pre-trained on JSUT speech | labelled | default | **0.121** | **0.76** | **0.225** |

What it came to:

* **The alignment was the fault, not the decoder.** `scripts/compare_alignment.py` showed the
  search giving ɕ two thirds of its labelled frames and ts under three fifths, and training
  on the labels (`doc/preprocessing.md`, `duration_dir`) halved the corpus PER on its own.
  Tightening the KL looked like a fix while the alignment was wrong and *hurts* once it is
  right; those gains were compensation, not improvement. Leave the loss weights alone.
* **Capacity is the next lever.** `base` on the same labels takes another quarter off both
  runs; it costs a 164 MB voice file against 60 MB and RTF 0.11 against 0.05 on the CPU,
  still nine times realtime.
* **The score run had its own faults, all on the host's side of the line**, and none visible
  to the corpus run: the flat energy envelope `auris-vocal` drew (fixed by the per-phoneme
  level table, `doc/inference.md`), a rest of more than fifty frames inside one inference
  silencing the line after it (fixed by `MAX_REST_FRAMES`, which makes every such rest a
  chunk seam: PER 0.72 → 0.41 on the same voice), and the host spelling ざ行 and にゃ行 with
  symbols the trainer never wrote. Every one of those was found by the `song − host` and
  score columns disagreeing with the corpus column.
* **What did not matter**, measured so nobody measures it again: the sampling temperature,
  the chunk length up to a minute, the host's Irwin-Hall prior noise against PyTorch's
  Gaussian, run-length merging of repeated phonemes, and the render level.
* **The pitch between notes was the host's last step.** `auris-vocal` held each note's pitch
  to its final frame; the corpus never does (`runs/exp/glide_shape.py`: a median 60 ms of
  travel straddling the boundary), and the ablation had shown a glide worth a little. The
  host now draws one, measured from the same corpus. Through the host, ten takes each
  (`runs/exp/glide_seeds.py`), the step scored PER 0.302 and the glide 0.293, with every
  other shape tried between 0.265 and 0.277 — all inside a take-to-take spread of 0.09. The
  line is drawn because the corpus draws it; the number says only that it costs nothing.
* **What is lost now is the plosives and the flap, not the sibilants.** The tally on the
  `base` voice's corpus run: g was heard right 4 times in 24 and as n 10 times, ɾ right 13
  in 30 and as n 5, p right 0 in 12 and as t or k 7; ɕ, s and ts no longer appear in the
  worst dozen. A stop heard as a nasal is a closure the model never made and a burst it
  never released — the shortest events in the language, a few frames each, and the ones a
  21-minute corpus holds fewest examples of. That was the case for pre-training on the same
  speaker's read speech (`doc/training.md`, `--init-from`), and it held: 40k steps on JSUT
  BASIC5000 (6.4 hours, 6,293 sentences) and then the same 40k on the song took the corpus
  PER from 0.203 to 0.121 against the recording's 0.090, the score PER from 0.310 to 0.225,
  and validation mel from 0.589 to 0.547. The tally says where: g heard right 18 times in 24
  instead of 4, ɾ 20 in 30 instead of 13, p 6 in 12 instead of never. The vowels are now the
  largest remaining loss by count, at a rate of one in seven, and the score path's residue
  is k (heard as ɴ or s) and the two weak fricatives h and ɸ.
* **The tally then found the score path's own fault: the stops never burst.** With the
  speech-trained voice the corpus run sits a hair above the recording's own rate (0.218
  against 0.207 over 48 utterances, and half its remaining errors are ones the listener makes
  on the recording too), while the composed verse still lost k as ɴ or s and h and ɸ
  outright. The level table gives a consonant one number, its median, and measured from the
  labels (`runs/exp/plosive_shape.py`) a voiceless plosive sits 25 dB under its vowel through
  its closure and 8 dB under it over its last 20 ms. Reshaping the frames by hand
  (`runs/exp/burst_seeds2.py`, ten takes each through `auris sing-frames`): the flat table
  level 0.230; the same with the last two frames of every stop at the vowel's level 0.140;
  stops and fricatives 0.105; k heard right 25 times in 40 against 35, dz 2 in 10 against 8.
  A deep closure (−30 dB) helped the voiceless stops and broke the voiced ones (g 9 in 10 →
  3), so the rule `auris-vocal` now carries is the release alone: a consonant's last 20 ms
  at the vowel's level, its body at the table's. Through the whole path — `auris compose`,
  `auris sing`, ten takes — the verse went from 0.225 to **0.128**, k heard right 37 times in
  40; what is left there is ɸ (heard as k), j (as n) and s (as dz).
* **Five speakers in one voice cost the soprano little and gave the bass line less than
  expected.** JSUT-song with four VocalSet males (`configs/preprocess/jsut_song_vocalset.yml`,
  every source labelled, batches kept all-labelled), `base` from the speech pre-training
  with the speaker tensors afresh, 40k steps: as `jsut_song` the corpus PER is 0.135 against
  the single voice's 0.121 and the score's 0.194 against 0.116 (three takes, inside the
  take-to-take spread), with a better mel distance (0.600 against 0.642). The VocalSet
  speakers, who sang no consonant, hear none (score PER 0.83–0.92) and hold the verse's
  pitch to 6 cent. On a sustained A2–D3–A3 line through the host (`runs/exp/low_pitch_speakers.py`)
  every speaker of the five holds 110 Hz to 4.2–4.5 cent — and so does the single soprano
  voice, at 6.2: the pitch reaches the decoder through the excitation, so a low note is not
  where a voice trained high fails. What the male speakers change is timbre and level, which
  these numbers do not hear; a low-register *word* test would need a corpus that sings words
  low.
* **What is left on the score path** is the melody itself — the composed line against the
  corpus's — and the gap to the corpus that only more singing data closes.

## Limits

* **Linux has no GPU provider.** The host reaches the GPU through the platform's own
  provider, DirectML on Windows and Core ML on macOS, and `--acceleration gpu` on Linux is
  refused as it is in the application. `timing` says which processor sang.
* **The host column's timing includes the machine's mood.** Each corpus utterance is its
  own `auris` process, and its one chunk is that process's first inference; the same voice
  measured RTF 0.28 on a first run with the model file not yet in the page cache and 0.055
  on the next. The song line under the table — one process, several chunks — is the
  steadier number for the model's own speed. Read the host line as "what pressing *Sing*
  costs on this machine right now", and compare timings only between runs made back to
  back.
* **Timings are the dev profile's unless `--release` is given.** The workspace builds its
  dependencies at full optimisation even in debug, and onnxruntime is a prebuilt library, so
  the difference is small — but a number worth writing down wants the release build.
* **`pytest` skips the end-to-end tests without a host.** `tests/test_host_eval.py` builds a
  tiny voice and drives a real `auris` through both runs; the tests are marked `slow` and
  skip wherever `Host.find()` fails, which is CI's training job. Everything else in the file
  runs everywhere.
