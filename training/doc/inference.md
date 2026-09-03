# Inference

The model is not given a score. It is given a phoneme sequence, an integer
duration per phoneme, and frame-level f0 and energy curves. Converting a score
into those curves is the DAW front-end's job and is out of scope for this
repository.

## Command line

```bash
uv run python scripts/infer.py \
    --checkpoint runs/base/checkpoints/last.ckpt \
    --input score.json \
    --output out.wav \
    --device cuda
```

`score.json`:

```json
{
  "speaker": "my_singer",
  "phonemes": ["<sil>", "k", "o", "ɴ", "ɲ", "i", "tɕ", "i", "w", "a", "<sil>"],
  "durations": [20, 6, 10, 8, 6, 12, 6, 10, 6, 25, 20],
  "f0":     [0.0, 0.0, 220.0, 220.4, "..."],
  "energy": [0.0, 0.01, 0.08, 0.09, "..."]
}
```

Rules:

* `durations` counts **frames** per phoneme — 100 frames per second at 48 kHz
  with the default hop of 480 samples. It must be the same length as
  `phonemes`.
* `f0` and `energy` must each have exactly `sum(durations)` entries.
* `f0` is in Hz; write it as a *contour* — consonant frames carry the pitch of
  the vowel they lead into, `0` only where nothing is sung. `voiced` may be
  supplied explicitly as an optional array of the same length; otherwise a
  frame is voiced iff its phoneme is not voiceless (`text.VOICELESS`) and its
  f0 is nonzero. It is never derived from f0 alone — that would voice every
  /k/ and /s/ under the contour and swallow the consonants.
* `energy` is linear RMS, on the same scale the preprocessing pipeline
  produced (roughly `0.0`–`0.5` for peak-normalized audio).
* `speaker` may be a name from the training set or an integer id.

## Python API

```python
from auris_singer.infer import Synthesizer

synth = Synthesizer.from_checkpoint("runs/base/checkpoints/last.ckpt", device="cuda")

wav = synth.synthesize(
    phonemes=["<sil>", "a", "i", "<sil>"],
    durations=[20, 50, 50, 20],
    f0=[0.0] * 20 + [440.0] * 50 + [493.9] * 50 + [0.0] * 20,
    energy=[0.0] * 20 + [0.15] * 100 + [0.0] * 20,
    speaker="my_singer",
    noise_scale=0.667,
)   # -> float32 numpy array at synth.sample_rate
```

The checkpoint carries the phoneme table and the speaker map, so nothing else
needs to be loaded. `synth.speaker_to_id` lists the available speakers.

`noise_scale` is the sampling temperature of the prior. Lower values give a
flatter, more deterministic delivery; `0.0` makes synthesis deterministic.

## Getting phonemes from Japanese text

```python
from auris_singer.text import JapaneseFrontend

phonemes = JapaneseFrontend().g2p("こんにちは")
# ['<sil>', 'k', 'o', 'ɴ', 'ɲ', 'i', 'tɕ', 'i', 'w', 'a', '<sil>']
```

Durations still have to come from somewhere — the front-end only produces the
symbol sequence.

## Control notes

Pitch and loudness reach the decoder **only** through the source signal (see
[architecture.md](architecture.md#source-signal-refinegan-style)), so they are
directly controllable:

* transposing `f0` transposes the output without touching timbre;
* scaling `energy` scales loudness and, because the excitation amplitude
  changes with it, the accompanying change in vocal effort;
* setting `f0` to 0 over a span makes that span unvoiced (breath, whisper-like
  consonants).

Very large deviations from the training distribution — an octave above anything
in the data, say — will degrade quality; the prior is still conditioned on f0
and energy and has only seen the training range.

## Exporting to ONNX

```bash
uv pip install -e '.[export]'
uv run python scripts/export_onnx.py \
    --checkpoint runs/base/checkpoints/last.ckpt --output runs/base/model.onnx
```

This writes `model.onnx` plus a `model.json` sidecar and, unless `--no-verify`
is given, checks the graph against PyTorch with onnxruntime at input sizes the
trace never saw. Weight norm is folded into the weights as part of the export
(`remove_weight_norm()` — a one-way operation, which is why the script loads
its own copy of the checkpoint).

The graph is the same computation `Synthesizer.synthesize` runs, made a pure
function: the random draws are inputs, so a caller that seeds its own
generator gets bit-identical renders. All inputs are required.

| input | shape | dtype | |
| --- | --- | --- | --- |
| `phonemes` | `(B, S)` | int64 | ids into the phoneme table in the metadata |
| `phoneme_lengths` | `(B,)` | int64 | valid entries per row |
| `durations` | `(B, S)` | int64 | frames per phoneme; each row sums to `T` |
| `f0` | `(B, T)` | float32 | Hz; 0 on unvoiced and silent frames |
| `energy` | `(B, T)` | float32 | linear RMS, as in the score input above |
| `voiced` | `(B, T)` | float32 | 1.0 on voiced frames |
| `speaker_ids` | `(B,)` | int64 | |
| `noise_scale` | scalar | float32 | prior sampling temperature |
| `z_noise` | `(B, inter_channels, T)` | float32 | standard normal draws |
| `source_noise` | `(B, 1, T * hop_length)` | float32 | uniform on [-1, 1] |

Outputs: `wav` `(B, 1, T * hop_length)` float32, and `source`, the excitation
signal at the same shape — a diagnostics output that a runtime asked only for
`wav` never computes.

Two contract points that differ from the Python API:

* **`voiced` is required, not derived from `f0`.** A DAW front-end that
  writes pitch as a contour puts real f0 values on unvoiced consonant
  frames; deriving voicing from `f0 > 0` would silently voice them. Decide
  voicing from the phoneme class and say so explicitly — the classification
  the Python API uses is `auris_singer.text.VOICELESS` /
  `infer.frame_voicing`, and a port of it belongs in the caller.
* **`sum(durations)` must equal `T` exactly** — the graph does not trim the
  curves the way `Synthesizer.synthesize` does.

The phoneme table, the speaker map and the audio parameters ride along as
JSON, both under the `auris_singer` key of the ONNX `metadata_props` and in
the `.json` sidecar:

```json
{
  "format_version": 1,
  "sample_rate": 48000,
  "hop_length": 480,
  "inter_channels": 192,
  "n_speakers": 2,
  "f0_min": 40.0,
  "symbols": ["<pad>", "<unk>", "<sil>", "..."],
  "speaker_to_id": {"my_singer": 0}
}
```

`inter_channels` is there so the caller can shape `z_noise` without knowing
the model config; `symbols` maps IPA strings to the ids `phonemes` wants
(index in the list = id).

### Execution providers

The graph runs on onnxruntime's CPU and DirectML providers at any `B`, `S` and
`T` — the sequence lengths are dynamic dimensions, not the sizes the trace
happened to see. On the same inputs the two agree to about 1e-7.

Getting DirectML (which is what an AMD GPU uses on Windows) to accept it took
two things, because that provider rejects with a bare "the parameter is
incorrect" two spellings the CPU provider is happy with:

* **`Reshape` with `allowzero=1` and a `-1` in its shape tensor.** That is
  what a traced `view(b, t, -1)` becomes, and the attention blocks had one.
  They now write the head dimension out (`n_heads * head_dim`), which is the
  same reshape with nothing to infer.
* **`ConvTranspose` carrying an `output_padding` attribute — even `[0]`.**
  The generator's upsampling stages need a nonzero one whenever
  `kernel - rate` is odd, so it cannot simply be dropped at the source.
  `export_onnx` folds it into `pads` instead: the output length is
  `stride * (in - 1) + output_padding + (kernel - 1) * dilation + 1 -
  pads_begin - pads_end`, so `output_padding` and a smaller `pads_end` are the
  same crop of the same transposed convolution, element for element.

Both are properties of the exported graph, so a model file exported by this
repository needs no provider-specific handling in the consumer. A test guards
them (`tests/test_export.py`), since CI has no GPU to catch a regression with.

### Consonant widths

`durations` is an input, so something upstream has to decide how many frames
each phoneme gets. Syllabic phonemes are easy — they stretch to fill the note.
The consonants leading into them are not, and a front-end with no better
information has to guess.

A single flat guess is measurably wrong, and wrong in a way that costs
intelligibility. Consonant length in sung Japanese spans a factor of three by
phoneme class, so any one constant is far too short for the sibilants and too
long for the liquids. Because the numbers are a property of the corpus a voice
was trained on rather than of the architecture, they travel **with the model**,
under the optional `phoneme_durations` key of the same metadata JSON:

```json
{
  "phoneme_durations": {
    "unit": "seconds",
    "default": 0.060,
    "seconds": {"ts": 0.119, "tɕ": 0.113, "ɕ": 0.110, "s": 0.104, "k": 0.091},
    "counts": {"ts": 679, "tɕ": 357, "ɕ": 1231, "s": 1907, "k": 4859},
    "measured_from": "Namine Ritsu singing DB Ver2.0.2, mono labels, 110 songs"
  }
}
```

| field | meaning |
| --- | --- |
| `unit` | always `"seconds"`; frames are `round(seconds * sample_rate / hop_length)` — 100 per second at the shipped 48 kHz / hop 480 |
| `default` | the width to use for any phoneme not named in `seconds` |
| `seconds` | IPA symbol → width, longest first |
| `counts` | how many occurrences each median came from, so a consumer can apply a stricter threshold without re-measuring |
| `measured_from` | free text naming the corpus and label set |

#### The rule for a consumer

```
width(phoneme) = seconds[phoneme] if present else default
```

That is the whole contract. Three things follow from it that are worth stating
outright, because each one is a mistake that looks reasonable:

* **Only apply it to phonemes that take a fixed slot.** The table never names a
  vowel, the moraic nasal `ɴ`, or the glottal stop `ʔ`, because their length
  belongs to the note rather than to the phoneme. A consumer that looks them up
  correctly gets `default` — but it should not be giving them a fixed width at
  all. The devoiced vowels (`i̥`, `ɯ̥`, …) are the opposite case: they *are*
  slot-taking and may appear in the table.
* **Every entry is longer than `default`.** This is deliberate, not an
  accident of the corpus. Measured widths shorter than the default were tested
  and made the output worse: `ɾ`'s true median is 36 ms, and giving it that
  instead of 60 ms cost 20 % on the spectral distance below, because the
  model's own preference bottoms out near 50 ms and is flat above it. Only
  lengthening is supported by evidence, so only lengthening is shipped. A
  consumer therefore never needs to shorten anything.
* **The block is optional.** A model exported without one has no
  `phoneme_durations` key, and the consumer falls back to its own default. Do
  not fail to load a voice over a missing table.

#### Why it matters

Sweeping consonant duration through the exported Ritsu model and comparing the
output against the real recordings — same phrase, same pitch, 24 renders per
point, `a C a` context — the distance roughly halves once a sibilant is given
its own width:

| | at 60 ms | at its table width | improvement |
| --- | --- | --- | --- |
| `ɕ` | 0.97 | 0.46 | −53 % |
| `ts` | 0.86 | 0.41 | −52 % |
| `s` | 0.85 | 0.42 | −50 % |
| `tɕ` | 0.78 | 0.41 | −48 % |
| `k` | 0.88 | 0.68 | −23 % |
| `t` | 0.67 | 0.52 | −22 % |

The high-frequency content that carries sibilant identity follows: the ratio of
energy above 4 kHz to below it, for `s`, goes from 6.7 at 60 ms to 40.6 at
104 ms, against 36.6 measured in the real recordings. At 60 ms the model does
not form the sibilant at all; at its trained width it matches the singer. The
curves flatten above roughly 90–150 ms, so this is a floor to clear rather than
a target to hit precisely.

#### One interaction to be aware of

A front-end that takes the consonant's time from *inside* the note delays the
vowel by the consonant's width. Going from 60 ms to 110 ms on `ɕ` therefore
moves that syllable's vowel onset 50 ms later, and only for the syllables that
start with a sibilant — an uneven lateness that is easier to hear than a
uniform one. Singers place a consonant ahead of the beat so the vowel lands on
it. Whether to do the same is the front-end's decision, but it should be made
deliberately alongside adopting the table, not discovered afterwards.

#### Producing the block

Needs a corpus that ships phoneme alignments — mono labels as in the Namine
Ritsu database, or HTS full-context labels as in JSUT-song. Training itself
does not use them; this is the one thing they are read for.

```bash
uv run python scripts/measure_phoneme_durations.py \
    --label-dir 'data/raw/namine_ritsu_v2/「波音リツ」歌声データベースVer2.0.2/DATABASE' \
    --measured-from 'Namine Ritsu singing DB Ver2.0.2, mono labels, 110 songs' \
    --output data/raw/namine_ritsu_durations.json
```

```bash
uv run python scripts/export_onnx.py --checkpoint last.ckpt --output ritsu.onnx \
    --phoneme-durations data/raw/namine_ritsu_durations.json
```

Durations are counted in **medial** position — consonants whose predecessor is
a sound rather than a boundary. The distinction is not cosmetic for plosives:
the label span of an intervocalic `k` includes its closure and runs 91 ms,
while phrase-initially there is no closure to include and the same label covers
24 ms. A sung phrase is nearly all medial. Continuants barely move between the
two contexts (`ɕ` is 112 ms against 110 ms), so the choice really only decides
the plosives.

A phoneme needs at least 90 occurrences before its median is shipped; below
that the default is the better estimate. That bar was set on the Ritsu corpus
(4.4 hours); on JSUT-song (21 minutes) it leaves six phonemes in the table, and
`--min-samples 10` puts twenty in. Measured on the labelled corpus, giving every
consonant its median width instead of a flat 60 ms took the phoneme error rate
from 0.40 back to 0.30 (0.26 with the labels themselves), so on a small corpus
lower the bar rather than ship the default. The exporter refuses a table naming
symbols outside the checkpoint's phoneme table, which catches a table measured
against a phoneme set that has since moved on.

### Consonant levels

The widths say how *long* a consonant is given; this table says how *loud*. A
score front-end writes one energy per frame — the note's velocity, shaped by a
short attack and release — and a plateau is wrong for the consonants: measured
on JSUT-song, a voiceless plosive or fricative sits twenty-odd decibels below
the vowel after it, a voiced one six to nine, an approximant three. A model
asked for a /k/ at the vowel's level has never heard one. On the labelled
corpus, putting the vowel's level on every consonant cost the phoneme error
rate 0.25 → 0.56; putting these medians back recovered 0.35, and one number
per class did as well as one per phoneme.

The numbers are a property of the corpus, so they travel with the model under
the optional `phoneme_levels` key, beside the widths:

```json
{
  "phoneme_levels": {
    "unit": "db",
    "default": -11.5,
    "db": {"p": -26.5, "k": -22.6, "s": -20.6, "ɕ": -11.0, "n": -5.9, "ɾ": -3.7},
    "counts": {"p": 20, "k": 282, "s": 142, "ɕ": 71, "n": 261, "ɾ": 214},
    "measured_from": "JSUT-song, HTS full-context labels, 27 songs"
  }
}
```

| field | meaning |
| --- | --- |
| `unit` | always `"db"`: decibels against the first vowel after the phoneme |
| `default` | the level for a consonant not named in `db` — the pooled median, a consonant's level, never 0 dB |
| `db` | IPA symbol → level, quietest first |
| `counts` | how many occurrences each median came from |
| `measured_from` | free text naming the corpus and label set |

The rule for a consumer:

```
gain(phoneme) = 10 ** (db[phoneme] / 20)   if phoneme in db
              = 10 ** (default / 20)       if phoneme is a consonant
              = 1                          if phoneme is a syllabic (a vowel, ɴ, ʔ)
```

A syllabic keeps the note's level unless the table measured it — `ɴ` is, at
−8 dB — and every energy the front-end writes for a consonant is multiplied by
its gain, **except the consonant's last 20 ms**, which the consumer sings at the
vowel's level (gain 1). The table's number is the consonant's body — the
closure of a stop, the noise of a fricative — and the corpus does not hold it
to the end: a voiceless plosive sits 25 dB under its vowel through its closure
and 8 dB under it over its last 20 ms, and every class rises the same way. Held
at the closure's level to the last frame, a /k/ never bursts, and through the
host it was heard as /k/ 25 times in 40 against 35 with the release; the
composed verse's phoneme error rate went 0.23 → 0.14 over ten takes. The block
is optional; a model exported without one has no `phoneme_levels` key, and the
consumer gives every phoneme the note's level as it always did.

```bash
uv run python scripts/measure_phoneme_levels.py --data data/processed/jsut_song_lab \
    --measured-from 'JSUT-song, HTS full-context labels, 27 songs' \
    --output data/raw/jsut_song_levels.json
uv run python scripts/export_onnx.py --checkpoint last.ckpt --output voice.onnx \
    --phoneme-durations data/raw/jsut_durations.json \
    --phoneme-levels data/raw/jsut_song_levels.json
```

It is measured from a *preprocessed* dataset that stored labelled durations
([preprocessing.md](preprocessing.md#input-layout)), since it needs the frame
energies the preprocessor computed and the durations that say whose they are.
A phoneme needs twenty occurrences before its median ships.

### Voice card

Presentational metadata — what a host application shows to a person browsing
voices, as opposed to what it feeds the model — travels in the same JSON under
the `voice` key:

```bash
uv run python scripts/export_onnx.py --checkpoint last.ckpt --output ritsu.onnx \
    --voice-card card.json --portrait ritsu.png
```

`card.json` is a free-form JSON object; these field names are the convention a
UI can rely on:

```json
{
  "name": "波音リツ",
  "description": "Strong low-range female voice. 107 songs, 4.4 h.",
  "author": "...",
  "version": "1.0",
  "license": "Namine Ritsu singing DB terms; fine-tuning to other voices prohibited",
  "credits": ["波音リツ", "カノン"],
  "url": "https://..."
}
```

`--portrait` embeds a character image (png/jpeg/webp, at most 8 MB) as
`voice.portrait = {"mime": ..., "base64": ...}` — decode the base64 to get the
image bytes back. Everything, artwork included, lives inside the one `.onnx`
file (and its `.json` sidecar), so a published model file carries its own
name, description, credit line and 立ち絵 with no companion archive to lose.

From Rust, the [ort](https://ort.pyke.io) crate runs the file as-is on CPU;
request only the `wav` output. Renders are reproducible: same inputs, same
noise, same waveform. (Across *runtimes* the match is exact except for the
excitation's impulse timing, where float32 rounding can shift an impulse by
one sample — inaudible, and training's random phase offset makes the model
indifferent to it by construction.)
