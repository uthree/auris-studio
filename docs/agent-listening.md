# Letting the agent hear the song

`listen` renders bars of the open song and hands the WAV to the model that asked for it, as audio
input. The model hears the music itself; nothing else listens on its behalf. A model that cannot
take audio is not offered the tool, so no text ever stands in for listening.

## Which agents can hear

| Agent | `listen` |
| --- | --- |
| Desktop agent, rig, OpenAI-compatible provider | Offered. The audio goes into the request that follows the tool result. |
| Desktop agent, rig, Ollama provider | Not offered: rig's Ollama adapter cannot send audio. Choose the OpenAI-compatible provider with `http://localhost:11434/v1` instead. |
| Desktop agent, Codex, Claude Code, OpenCode, Antigravity | Not offered: none of them can put audio in front of their model. |
| `auris-mcp` (saved projects) | Offered. Answers with MCP audio content; whether the model hears it is up to the client. |

For the OpenAI-compatible provider, an Ollama server (port 11434, path `/v1`) is asked through
`/api/show` whether the model lists `audio` among its capabilities, and `listen` is offered only if
it does. `gemma4:e2b` and `gemma4:e4b` do. Any other endpoint is trusted to take audio, as audio
attachments already trust it; a model that refuses will fail the request that carries the clip.

## The call

```json
{"start_bar": 9, "bars": 4}
```

The answer is a report: `audition_id`, the document `revision`, the bar `range` and, for each
clip, the exact measurements of the WAV delivered — sample and estimated true peak, RMS, any
protective attenuation and a SHA-256. The audio itself arrives as a 48 kHz, 16-bit PCM, stereo WAV,
each clip preceded by a caption naming it.

To hear an edit, call it again on the same bars with the earlier ID:

```json
{"start_bar": 9, "bars": 4, "compare_to": "audition:…"}
```

The model then receives `before` (the retained baseline, unchanged) and then `after` (the current
render). Both share one protective gain, so their relative level is the mix's own. A tempo or meter
change needs a new baseline, as does a different range.

## Limits

* 1 to 8 bars, at most 30 seconds, inside the arrangement; rendered without effect tails.
* The four latest excerpts are retained per open document. Replacing the document expires them.
  `auris-mcp` keeps the same bounded history for two projects in the running server.
* Audio is attenuated only to keep the estimated true peak at −1 dB. Quiet audio is never raised,
  the project's faders never change, and a source overload stays visible in the measurements. The
  true-peak estimate is a 4× FIR interpolation, not a certified BS.1770 meter.
* Listening saves nothing and adds no Undo step. It is a read: it asks no permission beyond the
  ordinary policy, and an `edit_project.listen` deny rule still stops it.
* In the desktop agent the clips reach exactly one request. The model that wants to hear them again
  calls `listen` again, which keeps the conversation's context from carrying several megabytes of
  audio through every later step.

`AURIS_AUDIO_INPUT_CONTRACT` applies here as it does to audio attachments: when it is set, every
clip is checked against the agreed rates, channels, formats, durations and byte limits, and a
mismatch fails the call rather than converting the audio. A comparison needs `max_inputs` of 2.

## How it is built

The session renders and encodes (`Session::begin_listening`, `ListeningJob::run`,
`Session::continue_listening`) and never talks to a model. The desktop runs the render on a worker
and sends the whole `Audition` back to the agent worker, which stages the clips as a user message
that its request hook inserts once — rig's tool results and OpenAI-compatible tool messages can
carry text and images but not audio. `auris_session::guide` has the account.

To check the path against a real model, the ignored worker test
`local_audio_model_hears_a_rendered_bar` runs it against a local Ollama server. It writes a rising
and then a falling two-bar scale, asks the model to listen and say which, and prints the answers;
it asserts only that `listen` was called and answered with audio.

```powershell
$env:AURIS_AGENT_AUDIO_MODEL = 'gemma4:e2b'
cargo test -p auris-agent --lib local_audio_model_hears_a_rendered_bar -- --ignored --nocapture
```

On 2026-11-06 both `gemma4:e2b` and `gemma4:e4b` called `listen` as asked and received the
audio. `e2b` named both directions correctly; `e4b` called
the falling scale rising. `lfm3-audio` and `lfm3-omni`, which cannot think, answered with the
panel's medium effort dropped; both called `listen`, received the audio and called both scales
rising. `lfm3` and `lfm3-vl` list no `audio` capability and are not offered the tool. The path
delivers the sound; how well a model hears it is the model's.

Sending the same clips straight to `/v1`, three times each with the two answers offered in either
order, showed that none of these models judges a melody. `lfm3-omni` answered whichever word the
question named first, `lfm3-audio` said "rises" every time, `gemma4:e2b` said it could not tell,
and `gemma4:e4b` contradicted itself. Speech is another matter: given a Japanese sentence spoken by
VOICEVOX, `lfm3-omni`, `lfm3-audio` (asked what the speaker says) and `gemma4:e2b` transcribed it
word for word. These are speech models; the clip reaches them, and what they cannot do is hear
which way the pitch moves.
