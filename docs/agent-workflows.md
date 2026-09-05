# Agent composition and revision

The Agent Panel and MCP expose the same project commands through `auris-toolbox`.
Start with `capabilities` to check the General MIDI library, voice library paths and
project playback state. Registered sampler code is not a loaded SoundFont: when a
composition cannot load its requested GM preset, the session substitutes a built-in
instrument and reports the substitution. A singer without a rendered take plays a
temporary guide voice. Preview and export report missing instruments, guide vocals
and stale takes; `sing` produces or refreshes the actual vocals.

Voice discovery uses the same standard directories and additional `voice_paths` as
the desktop browser. Discovery lists candidate files without validating/loading them.
`capabilities` with a project also reports selected voice metadata and speaker names
when loaded. A playable preview is an audio artifact; a language model needs audio
input support to listen to it. Ollama's API does not accept Auris audio attachments.

## Local musical changes

Composition stores authored motif steps and rhythm patterns in each generated clip's
recipe. `write_again`, `another_take` and `edit_recipe` retain them. Older projects
without those fields retain generated defaults. `edit_recipe` accepts `motif` (relative
scale steps such as `0 2 4 2`) and `rhythm` (`x` hit, `X` accent, `o` ghost, `.` rest);
an empty string clears that authored control. Density, gate, dynamics and subdivision
remain available for articulation and phrasing.

Regeneration writes a local take in the current composer. Whole-song arrangement
context can produce different notes from a local take even with the same seed. Freeze
the clip to preserve an exact take. `edit_clip` with
`action: {kind: "copy", destination: "Flute", bar: 9, transpose: 12}` copies the stored
phrase to another note track. The source stays intact; a transposed copy is frozen so
regeneration cannot undo the transposition. Re-read clip numbers after arrangement edits.

## Effects and automation

`effects` takes a project, track (or `master`) and an `operation` object:

```json
{"action":"list"}
{"action":"add","effect":"auris.fx.compressor"}
{"action":"sidechain","slot":1,"source":"Kick"}
{"action":"enable","slot":1,"enabled":false}
{"action":"move","slot":1,"position":2}
{"action":"remove","slot":2}
```

List returns registered effect IDs and actual slot positions, enabled states and
sidechain capability/connections. Slot numbers are 1-based and change when the chain
is reordered. A null sidechain source disconnects it. Unsupported inputs and routing
cycles are rejected. Use `set_effect` for static values, in the parameter's own units.

`automation` takes a project, track, target and operation. Targets are
`{kind:"mixer"}`, `{kind:"instrument"}`, `{kind:"effect",slot:1}` or
`{kind:"send",destination:"Reverb"}`. Read with `operation:{action:"read"}` and no
`param` to discover parameter keys, units, ranges, static values and existing lanes.
Then put the selected key inside the operation, for example on the compressor:

```json
{
  "action": "set",
  "param": "threshold_db",
  "points": [{"beat":0,"value":-6},{"beat":32,"value":-24}],
  "curve": "linear",
  "replace": false
}
```

Beats are absolute quarter notes from zero, regardless of meter. Values use native
parameter units. Set merges points by position; `replace:true` replaces the entire
lane. `operation:{action:"clear",param:"threshold_db"}` removes it. Linear ramps and held values are supported;
discrete parameters require hold. A batch validates every point before changing
anything and is one session undo step. Tools checkpoint and save mutations. Removing
an effect also removes its lanes. Read and list operations do not trigger project reloads.

## Ollama configuration and failure recovery

The Agent Panel exposes context size (32K, 64K, 128K, 256K) and thinking (model default,
off, on). The CLI accepts `--context-tokens N` and `--thinking auto|off|on`; explicit
flags override shared preferences. The default request sets `options.num_ctx=32768`,
independent of Ollama's server default. Values below 16384 are refused because the
tool catalog alone needs substantial context. Model discovery reports requested
context separately from the architecture's maximum. Preflight checks tool support
and refuses requests exceeding a reported model maximum.

Before each model call, a byte-based token estimate includes the actual tool schemas,
preamble, history and 4096 tokens of output reserve. It is not a tokenizer measurement.
If the estimate exceeds the configured window, the agent stops with instructions to
increase context or start a fresh conversation. OpenAI-compatible providers keep
their own context policy and do not receive Ollama-specific parameters.

The panel's context gauge uses input tokens from the last model request, rather than
adding the inputs of every tool-loop step. JSON answers expose that count as
`input_tokens`, with the run's aggregate separately available as `total_input_tokens`.

Five minutes without provider progress ends a stalled model request. Tool work is
allowed to finish, and active multi-step composition can exceed five minutes overall.
Two failures with identical tool arguments stop a third execution; changing arguments
allows a correction. Successful calls clear that signature's failure count. Interrupted
JSON conversations retain the request and interruption summary so a following turn can
inspect saved work and resume. Completed tool edits remain on disk.
