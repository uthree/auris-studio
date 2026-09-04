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

The backend sends the lyric-bearing score to `POST /sing_frame_audio_query`, applies Auris'
pitch and energy curves to the returned frame query, and sends that query to
`POST /frame_synthesis`. The Engine must be running when rendering. Raw `SingerFrames` files do
not contain lyrics and therefore cannot be rendered through this backend; full singer tracks and
note previews can.

The library's **Set Up VOICEVOX…** row opens a connection editor for the Engine URL and the query
and frame-decode style IDs. The same screen can choose and start a local Engine executable, check
`/version` and `/singers`, and save a `*.voicevox.json` entry into Auris Studio's managed Voices
folder. The saved entry appears on the shelf with a **VOICEVOX** badge; self-contained voices are
labelled **Auris ONNX**.
