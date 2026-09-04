# Singing backends

`auris-singer` renders a common `SingerFrames` timeline through a `SingingBackend`. The session
loads a `VoiceModel` facade and does not depend on a concrete model layout. This keeps model
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
