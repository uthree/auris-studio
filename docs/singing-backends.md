# Singing backends

`auris-singer` renders a common `SingerFrames` timeline and, when needed, its parallel
`SingerScore` through a `SingingBackend`. The session loads a `VoiceModel` facade and does not
depend on a concrete model layout. This keeps model
discovery, caching, speaker selection, previewing, take rendering, and WAV output identical across
backends.

## Auris ONNX voices

Choose the exported `.onnx` file. It contains the inference graph and `auris_singer` metadata,
including the phoneme vocabulary, sample rate, hop size, speakers, voice card, and measured
consonant timing and levels.

## DiffSinger voicebanks

Choose the voicebank's `dsconfig.yaml`. The backend reads the OpenUtau deployment fields
`phonemes`, `acoustic`, and `vocoder`, runs the acoustic ONNX model, then sends its mel output and
the track's F0 curve through the ONNX vocoder. A voicebank placed as a child folder of any configured
Voices directory appears on the library shelf automatically.

The vocoder must be bundled in the voicebank as `dsvocoder/vocoder.yaml` plus the ONNX file named
by its `model` field, or be in a folder relative to the voicebank named by `vocoder`. Acoustic and
vocoder sample rate, hop size, and mel-bin count must match. Base-10 and natural-log mel outputs are
converted when necessary.

This first backend covers acoustic voicebanks whose optional key-shift and speed inputs can use
neutral values. Voicebanks requiring language IDs, speaker embeddings, or energy, breathiness,
voicing, or tension predictors are rejected while loading. Those packages need their auxiliary
models and embedding-file semantics implemented before they can be rendered faithfully.

The library's **Set Up DiffSinger…** row opens the supported deployment fields, validates that
the phoneme table, acoustic model, and vocoder configuration exist, and writes `dsconfig.yaml`
into the chosen voicebank folder. The resulting voice appears on the shelf with a **DiffSinger**
badge.

## LeapSinger voices

Choose a `NAME.leapsinger.json` entry. Auris runs the exported LeapSinger acoustic model and
NHVSing vocoder directly through ONNX Runtime; rendering needs neither Python nor a running
server. A voice folder containing the entry appears in the configured Voices library with a
**LeapSinger** badge.

### Prepare the models

Obtain a checkpoint from the [LeapSinger model release](https://github.com/wavtechyukky/LeapSinger/releases/tag/v0.1.0)
and export it using the upstream repository. The following example uses the full model and bakes
speaker 0 into one acoustic graph:

```sh
git clone https://github.com/wavtechyukky/LeapSinger
cd LeapSinger
uv venv --python 3.11
uv pip install -e ".[export]" --torch-backend=auto
uv run python -m export.cli \
  --ckpt models/3singer_ritsu3style_uv_gan2d.pth \
  --out auris-voice --model-name voice \
  --variant full --speaker bake --spk-id 0 --spk-name singer --num-steps 1
```

`models/` above is where the downloaded checkpoint was extracted. This produces
`auris-voice/voice.singer.onnx`. Copy `checkpoints/nhv_v3_1.onnx` and the checkpoint's matching
phoneme dictionary into `auris-voice/`. The dictionary contains one phoneme per line; blank
lines and text after `#` are ignored, and the first phoneme must be `pau`. Its order defines the
model's token IDs, so use the vocabulary from that checkpoint's `config["phonemes"]`, or the
dictionary distributed with that model. A newer repository dictionary may have a different
number or order of tokens.

Save `auris-voice/singer.leapsinger.json` beside those files:

```json
{
  "format_version": 1,
  "name": "LeapSinger singer",
  "acoustic": "voice.singer.onnx",
  "vocoder": "nhv_v3_1.onnx",
  "phonemes": "ja.phonemes",
  "variant": "full",
  "sample_rate": 44100,
  "hop_size": 256,
  "num_mel_bins": 128
}
```

File paths are relative to this entry. `variant`, `sample_rate`, `hop_size`, and `num_mel_bins`
default to the values shown. Add `auris-voice/` to the Voices library, or select the JSON entry
with **Track → Choose Voice…**. The same entry path works with the CLI and model tools.

The code is MIT-licensed; the distributed acoustic checkpoints and NHVSing weights have their
own terms. Keep the release's credits and accompanying terms with the voice and follow their
restrictions on output and redistribution. See the
[LeapSinger license notice](https://github.com/wavtechyukky/LeapSinger/blob/003b32f1fb552e112ac6facb5b36d567520bf521/LICENSE)
and [NHVSing weight terms](https://github.com/wavtechyukky/NHVSing#weight-licensing).

### Export variants and speakers

The `full` variant uses the native hop-256 grid and carries an explicit voiced/unvoiced curve.
For the UV-free checkpoint `3speaker_gan2d.pth`, export with `--variant diffsinger --hop 256`
and set `"variant": "diffsinger"` in the entry. A hop-512 export uses `--hop 512`,
`"hop_size": 512`, and `nhv_v3_1x.onnx`. Both acoustic and vocoder must use the same grid.

A baked-speaker export, or a single-speaker export using `--speaker none`, appears as one speaker
named by `name`. To expose several speakers from a graph exported with `--speaker embed`, add a
`speakers` array. Each object has a `name` and an `embedding` array of floating-point values.
Generate each array with upstream `export.spk_embed.speaker_vector(model, speaker_id).tolist()`
after `infer.load_acoustic` loads the checkpoint. These are the projected vectors of size
`model.hidden`, rather than the checkpoint's raw speaker-bank rows. The graph requires every
entry to carry a complete vector; array order defines the speaker IDs in Auris. See the
[upstream speaker helper](https://github.com/wavtechyukky/LeapSinger/blob/003b32f1fb552e112ac6facb5b36d567520bf521/export/spk_embed.py).

### Rendering and saved takes

LeapSinger consumes Auris' manual IPA corrections and phoneme timing pins. The backend maps
Japanese IPA to the model's tokens, interpolates pitch gaps in log-frequency space, derives
voiced flags from the phoneme classes, and applies the track's energy curve to the vocoder output.
Long scores are rendered in bounded chunks and placed on the original frame timeline.

The upstream acoustic and vocoder graphs generate noise internally and do not accept a render
seed. Re-rendering the same score can therefore change the waveform. The rendered take remains
an audio file in the project, preserving the performance that was saved. The tensor layouts and
noise behavior follow the upstream
[export wrappers](https://github.com/wavtechyukky/LeapSinger/blob/003b32f1fb552e112ac6facb5b36d567520bf521/export/wrappers.py)
and [excitation graph](https://github.com/wavtechyukky/LeapSinger/blob/003b32f1fb552e112ac6facb5b36d567520bf521/export/excitation_onnx.py).

## VOICEVOX Engine

Start a [VOICEVOX Engine](https://github.com/VOICEVOX/voicevox_engine), then place a file named
`NAME.voicevox.json` in a configured Voices
directory (or choose it from the voice picker). The file is connection metadata, not a voicebank:

```json
{
  "format_version": 1,
  "name": "VOICEVOX singer",
  "url": "http://127.0.0.1:50021",
  "sample_rate": 24000,
  "frame_rate": 93.75,
  "styles": [
    {
      "name": "Singer / normal",
      "query_style_id": 6000,
      "decode_style_id": 3001
    }
  ]
}
```

`url`, `sample_rate`, and `frame_rate` default to the standard local Engine values shown above.
Find the two style IDs in `GET /singers`: `query_style_id` must name a `sing` or
`singing_teacher` style, while `decode_style_id` must name a `frame_decode` style. Multiple entries
become speaker choices in Auris Studio.

The backend sends the lyric-bearing score to `POST /sing_frame_audio_query`, preserves the
Engine's pitch contour, unvoiced frames and phoneme volume balance, and sends the query to
`POST /frame_synthesis`. The Engine must be running when rendering. Raw `SingerFrames` files do
not contain lyrics and therefore cannot be rendered through this backend; full singer tracks and
note previews can.

Written bends and ornaments shift the predicted pitch relative to each note's key; velocity
and expression scale the predicted volume. Auris does not add its generic glide or note
attack/release envelope to this backend. Controls extend into rests so the Engine can sound
anticipatory consonants without being muted by the host's note boundaries. Existing audio
takes stay as recorded; render the singer track again to hear this behavior.

A standalone prolonged-sound mark (`ー`) is expanded to the preceding vowel only in the
outgoing score: `こ・ー・ひ・ー` is sent as `こ・オ・ひ・イ`. Notes, rests, pitches,
durations and the saved lyrics remain unchanged. A mark without a preceding vowel is reported
with its note number. HTTP failures include the Engine's explanation, such as the lyric it
refused, instead of only a status code.

Half-width kana, decomposed voiced marks, and surrounding whitespace are normalized in the
outgoing lyrics too (` ｶﾞ ` becomes `ガ`). Each pitched note still needs one kana mora;
entering several morae on a single note, such as `ララ`, is rejected by the Engine.

VOICEVOX Engine 0.25.2 fails its query when an internal note or rest is only one frame long
and the following note starts with a consonant. Auris reports the score event before sending
the query: lengthen it to at least two frames (about 21.3 ms), or remove the tiny rest.
Boundary rests receive padding automatically. Vowels, `ン`, and `ッ` do not need that preceding
consonant space. Zero-length events, invalid MIDI keys, and rests with lyrics are also reported
before transmission.

The connection's output sample rate must divide into whole samples per Engine frame. Use
24000 or 48000 Hz for the standard 93.75 fps Engine; 44100 Hz cannot represent this grid exactly
and is rejected when loading the connection. This is the voice connection's rate, independent
of the project's audio output rate.

The library's **Set Up VOICEVOX…** row opens a connection editor for the Engine URL and the query
and frame-decode style IDs. The same screen can choose and start a local Engine executable, check
`/version` and `/singers`, and save a `*.voicevox.json` entry into Auris Studio's managed Voices
folder. The saved entry appears on the shelf with a **VOICEVOX** badge; self-contained voices are
labelled **Auris ONNX**.
