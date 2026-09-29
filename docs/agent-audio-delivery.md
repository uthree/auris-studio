# Agent audio delivery

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
arrangement even though rig cannot export a WAV.

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

The live rig agent edits the open document and has no file-export command. It can use
`normalize_mix` and `inspect_audio`; use MCP `render` and `verify_render` when the request
requires a finished WAV. Both paths use the same session normalization logic.
