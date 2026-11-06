# Agent audio delivery

[日本語](agent-audio-delivery.ja.md)

The saved-project MCP tools and the live rig agent use the same session command for
whole-mix normalization. A sound search result identifies a sound, but its fader value does
not predict the sound's output level. After `set_instrument` or `setup_tracks`, measure the
mix again before adjusting or exporting it.
`setup_tracks` echoes the requested sound IDs so the agent can verify its choices.

## Set a measurable goal

Choose an integrated loudness target and a true-peak ceiling for each delivery. For example,
the agent comparison experiments used -23 LUFS and -1 dBTP. Those are experiment settings,
not defaults for every song. Use `analyze` to measure the current saved project. On the live
rig path, `inspect_audio` measures a short excerpt; `normalize_mix` measures the entire
arrangement. Every live backend can export that unsaved snapshot.

`normalize_mix` applies one shared offset to source faders, preserving their differences,
then renders the full mix again. It does not change the master fader or authored notes. A
source gain automation lane must be cleared before using it. True-peak and fader limits may
prevent the requested LUFS; the tool reports the measured result rather than claiming the
target was met.

## Check the encoded file

`render` decodes each WAV it wrote and reports its measured loudness, sample peak, estimated
true peak, full-scale sample count, final half-second RMS, and SHA-256. Its earlier render
peak is measured before encoding; the encoded inspection is the result to use for delivery.
Run `verify_render` with the absolute output path and the chosen target to get a delivery
PASS or FAIL. For a fade ending, supply `ending_rms_max_db`, such as -60 dBFS. Omit that
limit for an intentional loop or hard cut. A full-scale sample count above zero indicates
possible integer saturation; inspect and adjust before delivering the file. `verify_render`
never changes the WAV.

## Export the unsaved document

The built-in Claude, Codex and native API agents share `render`, `preview`,
`export_midi` and `verify_render` with saved-project MCP. Saved calls add only
`project`; live calls need no save or project path. For a WAV mix:

```json
{"destination":{"kind":"mix","path":"/absolute/new-mix.wav"},"bit_depth":24}
```

Use destination `{"kind":"stems","folder":"/absolute/new-stems"}` for stems.
An optional `range` is `{"start_bar":1,"bars":4}`; omit it for the whole arrangement.
`include_tail` defaults to false. `preview` requires a range of at most 120 seconds
and creates a protected 24 kHz WAV under the document's preview folder.
`export_midi` takes an absolute `output` path for the current instrumental score.

Encoding, rendering and encoded-file measurement run off the UI thread. Private
stages are published only after document ownership, revision, cancellation and
asset guards are checked. Existing output files are refused. A stale or stopped
operation drops its stages and leaves existing files untouched. Export does not
save or change the score. Results identify published files and exact byte hashes;
a multi-stem publication failure reports any files already published.

`verify_render` takes `path`, optional `target_lufs`, `ceiling_db` (default -1),
`tolerance_lu` (default 1) and `ending_rms_max_db`. It measures the encoded WAV
without rendering the project again. Its PASS/FAIL verdict is separate from listening.

## Inspection and audition

`inspect_audio` returns the same measurements and PNG through saved MCP and the live
host. `preview` uses a 24 kHz protected PCM16 WAV; its reported attenuation applies
only to the audition. The mix and exported delivery settings remain unchanged.
`listen` delivers rendered bars to a model that takes audio input; a returned
`audition_id` in `compare_to` delivers before and after at a common protective gain.
See [letting the agent hear the song](agent-listening.md).
