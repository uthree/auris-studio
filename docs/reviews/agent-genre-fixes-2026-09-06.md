# Genre-trial fixes and verification

Follow-up to [the genre trial](agent-genre-trial-2026-09-06.md). The implementation
addresses the four priority findings through the shared session/toolbox boundary:

| Finding | Implemented behavior |
| --- | --- |
| Server context smaller than the advertised model capacity | Explicit Ollama `num_ctx`, panel/CLI settings, model/tool preflight, estimated context budget before every request; the panel measures the last request rather than summed tool-loop inputs. |
| Missing GM samples silently rendered an empty sampler | Composition substitutes a registered instrument that does not require the missing font; availability and playback readiness are queryable and exports identify missing sounds and guide/stale vocals. |
| Local regeneration discarded authored musical intent | Motif steps and rhythm patterns persist in clip recipes and are read by local regeneration. Both can be changed or cleared through `edit_recipe`. |
| Agents could not build effect chains or parameter curves | `effects` inserts/removes/reorders/bypasses slots and connects validated sidechains. `automation` discovers parameters and reads/writes/clears lanes in native units. |

Additional fixes include shared voice-library discovery and selected voice metadata,
cross-track phrase copying with transposition, required operation-specific tool
arguments, repeated-failure detection, progress-based provider timeout, interrupted
conversation recovery, WAV destination confinement in the agent, and blocking accepted
sockets in the Windows VOICEVOX test server.

## Verification

- Workspace tests, including the GPUI binary harness; focused session/toolbox/agent/MCP
  regression tests for the final schema and context-count changes.
- `cargo clippy --workspace --all-targets` and `cargo doc --workspace --no-deps` with
  rustdoc warnings denied.
- Scripted HTTP model tests inspect the actual Ollama request: `options.num_ctx=32768`,
  `think=false`, no duplicated `options` nesting. A two-step model loop verifies that
  10 and 20 input tokens produce a context count of 20 and aggregate usage of 30.
- Direct stdio MCP calls composed the committed vocal, EDM and orchestra score fixtures,
  reopened them, regenerated their lead twice, and checked stored motifs and repeatable
  local notes. This checks local repeatability, not equality to the original whole-song take.
- The EDM trial inserted a compressor, connected the kick as its sidechain, and wrote
  and reread a threshold ramp. Final-schema calls also wrote a held instrument waveform
  lane, refused linear interpolation for that discrete parameter without changing the
  saved bytes, cleared the lane and produced another preview.
- The orchestra trial copied a phrase to another instrument one octave higher. Unit
  tests check the source remains unchanged and an out-of-range transposition saves nothing.
- All three MCP previews returned readable WAV resources with nonzero signal. The vocal
  preview explicitly reported the temporary guide voice. EDM preview: 7.5 seconds,
  -0.7 dBFS peak; orchestra preview: 9.6 seconds, -5.3 dBFS peak.
- Both composition measuring instruments ran before and after the writer changes with
  the same standard SoundFont. All 33 printed symbolic rows were unchanged. All eight
  presets had +0.00 in the reported aesthetic-score differences; mean CE/CU/PC/PQ stayed
  7.12/7.86/5.05/7.99. These measurements are not a subjective listening assessment.

## Actual model trial

`ornith-1.5:9b` ran directly through Ollama with 65536 context tokens and thinking off,
without the diagnostic proxy used in the earlier review. It composed and saved an EDM
draft, inserted a real compressor and connected its kick sidechain. It repeatedly
supplied `bar` where clip resize requires `end_bar`; the new guard stopped a third
identical failed call and preserved the saved work and interruption history. This
draft did not fulfill the original eight-bar arrangement request.

A resume trial exposed ambiguity in the new automation argument structure. The final
schema places the parameter key inside the operation and makes it required for set
and clear. With that schema and a corrective follow-up, the model wrote and verified
the threshold lane (-6 dB at quarter-note beat 0 to -24 dB at beat 16), corrected its
preview range arguments, and produced a four-bar, 7.5-second WAV at -3.2 dBFS peak.
The persisted lane was checked through MCP. Beat 16 is four 4/4 bars from the start;
the model's prose incorrectly called that sixteen bars, which is why stored readback
was used as the evidence.

The live response also exposed aggregate input tokens being used as the panel's
context gauge. That is now a separate last-request measurement, with total input
usage retained in the JSON response for accounting. Model instruction-following and
application correctness are evaluated separately: the calls, saved document and audio
were verified, rather than accepting the model's completion message as proof.
