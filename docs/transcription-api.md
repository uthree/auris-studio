# Recognizer-independent transcription API

[日本語](transcription-api.ja.md)

Live agents can analyze original attached audio with local tools and import a note
draft prepared by an external analysis tool. The same live
contracts are offered to Codex, Claude Code and native API transports. A transport
still needs access to local analysis tools; exporting a file does not make a model
hear audio or supply a recognizer.

## Export the attached source

Call `export_audio_attachment` with `attachment` set to the `attachment_id` in the
host's audio metadata. The result contains `audio_path`, `sha256` and `bytes`.
The path is absolute and contains an exact original-byte copy, with no resampling
or channel conversion. Filenames, tempo and chord summaries are not a score.

The copy is separate from the host's original snapshot. Do not modify it. There is
one current export per attachment: another export replaces the previous file.
Attachment eviction or conversation shutdown also removes the copy. Copy it to an
analysis workspace for longer retention. Originals remain subject to the existing
512 MiB per-file, 1 GiB conversation and eight-attachment limits. Retained exports
add at most another 1 GiB, plus one in-flight copy of at most 512 MiB. Copying and
hashing stream in cancellable chunks rather than loading the entire WAV into RAM.
This operation passes through `export_audio_attachment` permission rules and does
not edit or save the document.

## Import a note draft

Call `import_transcription` with exactly one of `transcription` (the external piano
object below) or `path` (an absolute local JSON
file containing that object). Optional `name` defaults to `Transcription`;
`at_tick` is a nonnegative absolute insertion tick and defaults to zero.
Use a JSON file for large drafts instead of putting thousands of events into chat.

```json
{
  "source": {
    "method": "codex.harmonic-fit",
    "model": "gpt-6.1-sol"
  },
  "seconds": 2.0,
  "notes": [
    {"pitch": 60, "start": 0.25, "end": 1.5, "strength": 0.65},
    {"pitch": 64, "start": 0.25, "end": 1.5, "strength": 0.65}
  ],
  "limitations": "Uniform dynamics; review missed notes and offsets."
}
```

`source.method` is required (1..128 bytes). Optional `model` is a nonempty name up
to 256 bytes. Optional `artifact_sha256` identifies the actual model or analysis
program used; optional `audio_sha256` identifies the analyzed source bytes. Each
hash must contain 64 hexadecimal characters. These are caller claims: the host
does not authenticate a recognizer, check the claimed audio association, or certify
musical accuracy. Recognition accuracy is not certified by acceptance.
Provenance and limitations are returned to the caller, not persisted in the score;
keep the original JSON and hashes when reproducible analysis matters.

Durations and event times must be finite, in original source seconds, with
`0 < seconds <= 600` and `0 <= start < end <= seconds`. Supply 1..200000 notes,
MIDI pitches 21..108, and an explicit finite `strength` in 0..1. Preserve overlaps.
The generic import preserves the supplied strength, including values below 0.1.
If dynamics are unavailable, declare the chosen uniform value in `limitations`
(at most 4096 bytes). A malformed draft, unknown top-level/source field or missing
strength fails before any edit. Files must be regular local files at most 32 MiB;
relative paths and UNC/device paths are rejected. Loading is cancellable.

File reads cross the `cli_file_read` permission gate first. The actual validated
draft then crosses `edit_project.import_transcription`, with approval tied to that
exact command and document revision. Read-only and Plan modes refuse the edit.
Acceptance adds one physical-piano track and clip, preserving existing music,
tempo and source performance timing at the project tick resolution. It is one Undo
step and refuses the entire draft if any interval collapses to zero ticks, so short
notes cannot silently disappear. It remains unsaved. Use `read_notes` and
`inspect_audio` to review the result.

Saved-project MCP adds required `project` to the same import contract and saves
changed results with a checkpoint. Live imports stay unsaved.

## Validation

On 2026-10-21, a frozen 129-note mixture draft from a
Codex-led transcription experiment was imported through saved
MCP `import_transcription`, checkpointed and rendered with the physical piano.
All 129 pitches survived; maximum onset/offset conversion error was 0.31 ms at
100 BPM and 960 ticks per beat. Uniform strength 0.65 was explicitly disclosed.
The 48 kHz stereo PCM24 WAV passed encoded-file checks (38.39 s, -11.0 dBFS peak,
no full-scale samples). Its SHA-256 was
`86f8d72439db88eefeeb0a2e6583ba386da66cff24c34ea1822fbbb41c5f5675`.
This reuses an existing estimate and tests API integration, not recognition quality.

A later evaluation in the real application
exercises original export, independent analysis, generic import, rendered inspection
and Undo/Redo with actual model inference, and compares two frozen DSP methods with
Basic Pitch.
