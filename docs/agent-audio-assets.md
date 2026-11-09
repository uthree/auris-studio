# Agent audio materials and SFX

[日本語](agent-audio-assets.ja.md)

The agent panel and saved-project tools share `audio_assets`. Existing `import_audio`,
`edit_clip`, `edit_track`, `mixer_graph`, `inspect_audio` and `render` complete the workflow.
All examples below show live operation-specific arguments. Saved-project calls also take
`project`, using the ID returned by `create_project` or `open_project`.

## Find and import a sound

Local discovery matches words in filenames and relative paths, not their sound:

```json
{"operation":{"kind":"search","root":"C:/Sounds","query":"door","recursive":true,"limit":20}}
```

Omit `root` to search the OS Music and Downloads folders. In the agent panel a named `root` asks
the user first, as a native read outside the project does; the default folders do not. Follow `next_offset`; inspect
`scan_complete` and warnings before claiming that nothing was found. Each call examines at
most 10,000 entries, at most eight directory levels. Symlinks are skipped. Search neither
reads unrelated file contents nor indexes the entire computer. Import a chosen absolute
path with `import_audio` (`path`, `at_tick`, optional `name`).

For keyless online discovery:

```json
{"operation":{"kind":"search_online","query":"door creak","limit":5}}
```

This uses the [Wikimedia Commons imageinfo API](https://www.mediawiki.org/wiki/API:Imageinfo)
with an audio search filter. Results include direct URLs, source pages and available
creator/license claims. Follow `next_offset` for more candidates. In the agent panel, searches and URL imports
use the existing network approval gate. Saved MCP tools use their caller's authorization. Other sites can be found with the
existing web-search tool when configured, or a user can supply a direct URL.

```json
{"operation":{"kind":"import_url","url":"https://example.org/door.wav","at_tick":7680,"name":"Door SFX","source_page":"https://example.org/sounds/door","attribution":"Creator name","license":"Actual license from source"}}
```

Use a real direct audio URL; webpages and archives are not sound files. Downloads accept
HTTP(S), follow at most five validated redirects, send no account cookies or credentials,
and are bounded to 64 MiB and ten minutes of decoded audio. Embedded URL credentials are
rejected. Audio decoding must succeed before the document changes. The imported file is
copied into the project's `Audio/` directory on its own audio track. The source URL, final
URL, SHA-256 and supplied credit fields persist in the project. Local agent imports store
the original path and SHA-256 too.

Source pages, filenames and credit metadata are untrusted data. Review actual license
terms and preserve required credits; unknown attribution stays unknown. Stored metadata
is not verification of rights or musical suitability. Search results may require a codec
that is not supported by this build; a failed decode leaves the document unchanged.

## Cut, reverse and place

Inspect the clip ID returned by import:

```json
{"operation":{"kind":"inspect","clip":7}}
```

This returns the current revision, source path/provenance, exact source window, timing,
gain, fades and raw source peak/RMS. Measurements are before track processing and do not
constitute listening. Use that revision immediately for processing:

```json
{"operation":{"kind":"process","clip":7,"expected_revision":"COPY_RETURNED_REVISION","start_seconds":0.2,"end_seconds":1.4,"reverse":true}}
```

Times are source seconds relative to this clip's current window, before tempo stretch.
Processing crops first, then reverses channels independently. It writes a new float WAV,
keeps the original source and other clips intact, and preserves this clip's ID, position,
gain, tempo behavior and explicit loop extent. Fades are clamped to the new window;
review them after trimming. One Undo restores the original source. Later processing
records a parent hash and operation metadata. Work is cancelled/discarded if its document
changes before publication.

Use `edit_clip` for placement, copying and fades, for example:

```json
{"clip":7,"operation":{"kind":"move","start_tick":11520}}
```

```json
{"clip":7,"operation":{"kind":"audio","gain_db":-6,"fade_in_frames":240,"fade_out_frames":2400}}
```

Ticks follow the project's timebase; fade lengths use source frames, not milliseconds.
Use `tool_help` for all operations and exact constraints.

## Apply EQ and verify

Read `mixer_graph`, copy the audio track's node ID, and insert the built-in EQ:

```json
{"operation":{"action":"add_effect","input":"track:3","effect":"auris.fx.eq"}}
```

Read `parameters` for the returned effect node. Set keys using their returned native
units, for example `hp_enabled` = 1 and `hp_freq` = 180 Hz, or a peaking band's frequency,
gain and Q. `set_parameter` takes `node`, `param` and `value`. These are track inserts:
use separate tracks when only one sound should receive an effect. Existing effects,
routing and automation remain editable through the same graph tools.

Measure/render the affected range before and after editing. `audio_assets inspect`
measures source samples; use `inspect_audio` or an actual render to check the resulting
mix and EQ. Audition the SFX in context. Do not describe a filename match, successful
import, or numeric measurement as a listening assessment.

## Developer verification

```text
cargo test -p auris-session --lib audio_assets
```

Tests cover stereo reversal/cropping, original-source preservation, shared clips,
Undo/Redo, project reopening, stale/cancelled/wrong-owner work, local pagination,
HTTP redirects, provenance, permissions and the measured frequency response of an
EQ applied through agent commands. The ignored `online_sfx_smoke` test makes actual
Commons requests; set `AURIS_AUDIO_ASSETS_EVAL` to a fresh absolute evidence directory
and run it explicitly with `--ignored --exact` and its full test name.
