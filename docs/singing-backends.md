# Singing backends

`auris-singer` renders a common `SingerFrames` timeline and, when needed, its parallel
`SingerScore` through a `SingingBackend`. The session loads a `VoiceModel` facade and does not
depend on a concrete model layout. This keeps model
discovery, caching, speaker selection, previewing, take rendering, and WAV output identical across
backends.

## DiffSinger voicebanks

Choose the voicebank's `dsconfig.yaml`. The backend reads the OpenUtau deployment fields
`phonemes`, `acoustic`, and `vocoder`, runs the acoustic ONNX model, then sends its mel output and
the track's F0 curve through the ONNX vocoder. A voicebank placed as a child folder of any configured
Voices directory appears on the library shelf automatically.

The vocoder must be bundled in the voicebank as `dsvocoder/vocoder.yaml` plus the ONNX file named
by its `model` field, or be in a folder relative to the voicebank named by `vocoder`. Acoustic and
vocoder sample rate, hop size, and mel-bin count must match. Base-10 and natural-log mel outputs are
converted when necessary.

Phoneme dictionaries can be line-based text or JSON token-to-ID maps. JSON IDs are preserved,
including the reserved padding ID and sparse tables. Japanese IPA is mapped to the voicebank's
Japanese tokens, including `ja/` language prefixes. With `use_lang_id`, the `languages` JSON file
supplies each token's language ID. `speakers` names little-endian float32 `.emb` files of
`hidden_size` values; their order becomes Auris' speaker selection order.

Voicebanks using energy, breathiness, voicing or tension embeddings run their linguistic encoder
and variance predictor before the acoustic model. These are named by `linguistic` and `variance`
in `dsvariance/dsconfig.yaml`, or in the main configuration when no separate variance configuration
exists. The predictor has its own phoneme dictionary and speaker embeddings. Both phoneme-duration
and word-duration linguistic encoders are supported, on the acoustic model's frame grid.
Word grouping uses `dsdict.yaml` vowel types when supplied, and Japanese vowel aliases otherwise. Auxiliary
paths such as `../shared/model.onnx` are resolved relative to their configuration; background
loading requires their canonical targets to remain inside the selected voicebank folder.

Auris supplies the written pitch and phoneme timings. Pitch is extended through silence in
log-frequency space, and the frame dynamics scale the synthesized waveform. Optional key-shift
and speed embeddings receive neutral values. Modern scalar `steps`/`depth` and legacy one-element
`speedup`/`depth` inputs are accepted; depth respects the voicebank's `max_depth`. DiffSinger graphs
use ONNX Runtime's basic optimization because the linked 1.20 runtime's extended optimizer can
crash while loading trained diffusion graphs. Automatic acceleration retries on CPU if GPU
inference refuses a graph.

DiffSinger's exported graphs generate random noise internally and do not accept Auris' render
seed. A saved audio take preserves the rendered performance.

The settings' **Set Up DiffSinger…** button opens the supported deployment fields, validates that
the phoneme table, acoustic model, and vocoder configuration exist, and writes `dsconfig.yaml`
into the chosen voicebank folder. The resulting voice appears on the shelf with a **DiffSinger**
badge. Saving the form preserves existing speaker, language, variance and other deployment fields.

### Real-model verification

The opt-in test runs a trained acoustic model, its variance predictor when required, and its
vocoder, checking the exact waveform length, finite samples and a nonzero RMS/peak:

```powershell
$env:AURIS_DIFFSINGER_TEST_CONFIG = 'C:/Voices/OpenCpop/dsconfig.yaml'
$env:AURIS_DIFFSINGER_TEST_WAV = 'C:/renders/diffsinger.wav'
cargo test -p auris-singer --test diffsinger_real -- --ignored --nocapture
```

The output folder must already exist. The optional WAV setting also writes a parallel
`.frames.json`, which can be rendered through the session and CLI:

```powershell
cargo run -p auris-cli -- sing-frames C:/renders/diffsinger.frames.json --voice C:/Voices/OpenCpop/dsconfig.yaml --acceleration cpu -o C:/renders/diffsinger-cli.wav
```

`AURIS_DIFFSINGER_TEST_ACCELERATION=auto` exercises the desktop's default processor selection;
the test defaults to CPU. CI uses tiny arithmetic ONNX contracts for dictionaries, new and old
controls, both linguistic encoders, variance curves, speakers, languages, dynamics and
cancellation. Regenerate them with
`uv run crates/auris-singer/tests/fixtures/diffsinger/generate.py`.

The real-model probe uses three pitched `a` vowels, so it verifies synthesis rather than lyric
pronunciation quality. On Windows, the public
[OpenCpop deployment](https://huggingface.co/spaces/SJTU/diffsinger-webui/tree/main/models/OpenCpop)
produced 87,040 finite samples at 44,100 Hz (1.974 seconds), through its trained linguistic,
variance and acoustic graphs and NSF-HiFiGAN vocoder. In the automatic-acceleration run,
autocorrelation estimates of the three notes were 261.65, 329.78 and 392.11 Hz, within 0.9 cents
of their written pitches; RMS was 0.01249. The acoustic configuration was used unchanged.
Deployment conventions follow the
[DiffSinger acoustic exporter](https://github.com/openvpi/DiffSinger/blob/main/deployment/exporters/acoustic_exporter.py),
[variance exporter](https://github.com/openvpi/DiffSinger/blob/main/deployment/exporters/variance_exporter.py)
and [OpenUtau renderer](https://github.com/stakira/OpenUtau/blob/master/OpenUtau.Core/DiffSinger/DiffSingerRenderer.cs).

## LeapSinger voices

Choose a `NAME.leapsinger.json` entry. Auris runs the exported LeapSinger acoustic model and
NHVSing vocoder directly through ONNX Runtime; rendering needs neither Python nor a running
server. A voice folder containing the entry appears in the configured Voices library with a
**LeapSinger** badge.

### Prepare the models

Obtain a checkpoint from the [LeapSinger model release](https://github.com/wavtechyukky/LeapSinger/releases/tag/v0.1.0)
and export it using the upstream repository. The procedure below was verified against upstream
commit `6c99e2b4194def02888ada33c20198a16bca2eb5` and its bundled NHVSing V3.2.1 models.
It exposes all three checkpoint speakers:

```sh
git clone https://github.com/wavtechyukky/LeapSinger
cd LeapSinger
git checkout 6c99e2b4194def02888ada33c20198a16bca2eb5
uv venv --python 3.11
uv pip install -e ".[export]" --torch-backend=auto
uv run python -m export.cli \
  --ckpt models/3singer_ritsu3style_uv_gan2d.pth \
  --out auris-voice --model-name acoustic \
  --variant full --speaker embed --num-steps 1
uv run python /path/to/auris-studio/tools/leapsinger/prepare.py \
  --upstream . --checkpoint models/3singer_ritsu3style_uv_gan2d.pth \
  --output auris-voice --name "LeapSinger singers" \
  --speaker-names "御丹宮くるみ" "夏目悠李" "波音リツ"
```

`models/` above is where the downloaded checkpoint was extracted. This produces
`auris-voice/acoustic.onnx`. The packaging helper copies `checkpoints/nhv_v3_2_1.onnx`,
the acoustic release's text credits and terms, and the checkpoint's exact phoneme dictionary
into `auris-voice/`. It writes `singer.leapsinger.json` with the projected speaker vectors.
Native DFT is enabled by the latest upstream exporter by default; keep it enabled for portable
ONNX Runtime inference. The dictionary contains one phoneme per line; blank
lines and text after `#` are ignored, and the first phoneme must be `pau`. Its order defines the
model's token IDs, so use the vocabulary from that checkpoint's `config["phonemes"]`, or the
dictionary distributed with that model. A newer repository dictionary may have a different
number or order of tokens.

For a baked single speaker, export with `--speaker bake --spk-id 0 --spk-name singer` and pass
`--acoustic acoustic.singer.onnx --baked` to the helper. A minimal baked entry looks like this:

```json
{
  "format_version": 1,
  "name": "LeapSinger singer",
  "acoustic": "acoustic.singer.onnx",
  "vocoder": "nhv_v3_2_1.onnx",
  "phonemes": "ja.phonemes",
  "variant": "full",
  "sample_rate": 44100,
  "hop_size": 256,
  "num_mel_bins": 128
}
```

File paths are relative to this entry. `variant`, `sample_rate`, `hop_size`, and `num_mel_bins`
default to the values shown. In **Settings → General → Voice Setup → Set Up LeapSinger…**,
open the generated entry or choose its folder. Edit the name and acoustic, vocoder and dictionary
paths; choose the full or DiffSinger layout and its matching hop. The full layout fixes the hop
to 256. Existing speaker names and projected embeddings are preserved when saving.
**Test Synthesis** loads both models on a worker and checks a short vowel on CPU.
**Register Voice** validates the ONNX input/output contracts, saves the entry atomically, adds
its folder to the voice library and refreshes cached metadata. Conflicting edits require opening
the entry again. Files must resolve inside the voice folder so automatic rendering remains portable.
The shelf uses the manifest's voice name, allowing several banks to keep the same entry filename.
The same entry can be selected with **Track → Choose Voice…**, the CLI or model tools.

The code is MIT-licensed; the distributed acoustic checkpoints and NHVSing weights have their
own terms. Keep the release's credits and accompanying terms with the voice and follow their
restrictions on output and redistribution. See the
[LeapSinger license notice](https://github.com/wavtechyukky/LeapSinger/blob/6c99e2b4194def02888ada33c20198a16bca2eb5/LICENSE)
and [NHVSing weight terms](https://github.com/wavtechyukky/NHVSing#weight-licensing).

### Export variants and speakers

The `full` variant uses the native hop-256 grid and carries an explicit voiced/unvoiced curve.
For the UV-free checkpoint `3speaker_gan2d.pth`, export with `--variant diffsinger --hop 256`
and set `"variant": "diffsinger"` in the entry. A hop-512 export uses `--hop 512`,
`"hop_size": 512`, and `nhv_v3_2_1x.onnx`. Pass `--variant diffsinger --hop 512` to the
packaging helper too. Both acoustic and vocoder must use the same grid.

A baked-speaker export, or a single-speaker export using `--speaker none`, appears as one speaker
named by `name`. To expose several speakers from a graph exported with `--speaker embed`, add a
`speakers` array. Each object has a `name` and an `embedding` array of floating-point values.
Generate each array with upstream `export.spk_embed.speaker_vector(model, speaker_id).tolist()`
after `infer.load_acoustic` loads the checkpoint. These are the projected vectors of size
`model.hidden`, rather than the checkpoint's raw speaker-bank rows. The graph requires every
entry to carry a complete vector; array order defines the speaker IDs in Auris. See the
[upstream speaker helper](https://github.com/wavtechyukky/LeapSinger/blob/6c99e2b4194def02888ada33c20198a16bca2eb5/export/spk_embed.py).

### Real-model verification

The opt-in test renders three pitched `a` vowels for every speaker, asserting exact sample count,
finite and audible audio, periodicity and pitch within 30 cents. Optional output writes each
speaker's WAV and a shared `.frames.json` timeline:

```powershell
$env:AURIS_LEAPSINGER_TEST_MODEL = 'C:/Voices/LeapSinger/singer.leapsinger.json'
$env:AURIS_LEAPSINGER_TEST_WAV = 'C:/renders/leapsinger.wav'
cargo test -p auris-singer --test leapsinger_real -- --ignored --nocapture
```

The output folder must exist. `AURIS_LEAPSINGER_TEST_ACCELERATION=auto` exercises the desktop's
default processor selection; the test defaults to CPU. The frames can also pass through the
session and CLI:

```powershell
cargo run -p auris-cli -- sing-frames C:/renders/leapsinger.frames.json --voice C:/Voices/LeapSinger/singer.leapsinger.json --acceleration cpu -o C:/renders/leapsinger-cli.wav
```

On Windows, the v0.1.0 full/embed model with NHVSing V3.2.1 generated 62,208 samples per speaker
at 44,100 Hz, on both CPU and automatic acceleration. Automatic selection also completed on CPU
on this machine. The measured pitches were within one cent
of 261.63, 329.63 and 392.00 Hz. The UV-free DiffSinger/baked export with hop 512 and V3.2.1x
also passed, generating 62,976 samples. These vowel probes verify the inference pipeline and
speaker inputs; they do not measure lyric pronunciation quality. Ordinary CI uses the small
arithmetic ONNX graphs in `auris-singer/tests/fixtures/leapsinger` for tensor-contract coverage.

### Rendering and saved takes

LeapSinger consumes Auris' manual IPA corrections and phoneme timing pins. The backend maps
Japanese IPA to the model's tokens, interpolates pitch gaps in log-frequency space, derives
voiced flags from the phoneme classes, and applies the track's energy curve to the vocoder output.
Long scores are rendered in bounded chunks and placed on the original frame timeline.

The upstream acoustic and vocoder graphs generate noise internally and do not accept a render
seed. Re-rendering the same score can therefore change the waveform. The rendered take remains
an audio file in the project, preserving the performance that was saved. The tensor layouts and
noise behavior follow the upstream
[export wrappers](https://github.com/wavtechyukky/LeapSinger/blob/6c99e2b4194def02888ada33c20198a16bca2eb5/export/wrappers.py)
and [excitation graph](https://github.com/wavtechyukky/LeapSinger/blob/6c99e2b4194def02888ada33c20198a16bca2eb5/export/excitation_onnx.py).

## Adding a curve predictor

`auris_singer::CurveGenerator` is the common interface for a backend that predicts pitch,
energy, or both. `SOURCES: CurveSources` declares each independently as `Host` or `Backend`.
Expose those sources through `VoiceCapabilities::curves`; format defaults live in
`BackendKind::capabilities`, and `SingingBackend::capabilities` can override them for a loaded
model, including a native Auris model with an optional predictor.
Such predictors can also override `CurveGenerator::curve_sources` per instance; report the
same value in the loaded model's capabilities so frame sampling and prediction agree.

Implement `generate_curves` to return a `CurvePrediction<Context>` with the base performance,
before applying the user's edits. Each predicted array includes its declared leading and
trailing context frames on the input frame clock. Leave host-owned arrays as `None`. `Context`
keeps whatever the decoder needs: phoneme timing, an HTTP query, or model tensors. The shared
code does not interpret it.

During synthesis, call `prepare_curves` with the score, parallel frames, speaker and seed.
It validates lengths and numeric values, applies relative pitch edits and energy multipliers
to predictions, and copies host-owned acoustic curves unchanged. Decode the returned
`PreparedCurves` together with its context, then trim the declared context from the waveform.
Keep progress and cancellation at the backend's existing inference boundaries.

The session samples frames with `render_frames_with_sources` using the loaded model's
capabilities. For backend-owned pitch it omits the automatic glide; for backend-owned energy
it omits the generic phoneme levels and attack/release. This avoids applying articulation
twice while retaining bends, ornaments, velocity and expression. Cold metadata reads use the
format defaults; a full render loads the model before sampling its inputs.

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

The settings' **Set Up VOICEVOX…** button opens a connection editor for the Engine URL and the query
and frame-decode style IDs. The same screen can choose and start a local Engine executable, check
`/version` and `/singers`, and save a `*.voicevox.json` entry into Auris Studio's managed Voices
folder. The saved entry appears on the shelf with a **VOICEVOX** badge.
