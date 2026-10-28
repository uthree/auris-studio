# Native migration candidates

`auris_io::project_to_native` converts an already decoded legacy `Project` into a
standalone `NativeDocument`. `load_native_candidate` captures a supported project
file and returns that document with the exact input bytes and a repair report.
These are pre-activation preparation APIs governed by the [M2 activation
gate](render-pipeline-plan.md#activation-gate).

## Ownership and conversion

The native envelope has three parts: shared project context and asset registries,
ordered track presentation, and `PipelineContent`. It does not embed a legacy
`Project`. Notes, lyrics, recipes, performance stages, audio placements and plugin
state each have one owner. Context includes tempo/meter/harmony/sections, loop/punch,
metronome/count-in/grid, composition metadata, transcription provenance and mixer
layout. Drum presentation stays explicit.

Conversion retains track, clip, effect, send and asset IDs. New source, stage,
processor, group, curve and edge IDs are deterministic for the same decoded input.
A read-only allocation-start query repairs a stale counter in the candidate while
preserving the original high-water mark after deletion. Count budgets and the full
required ID reservation are checked before cloning authored payloads. Conflicting
IDs and invalid routing are rejected without changing an in-memory input; file
loading retains the existing loader's explicit repairs and reports them.

Every processing group has an explicit sum before its ordered insert effects and
strip. Outputs and sends enter the destination sum. Empty output lists remain empty;
parallel routes and their order are preserved. Pre-fader sends use the strip's
post-insert `pre_fader` output. Sidechains use the source strip's post-fader audio,
including master-insert sidechains. The master strip feeds one output node.

Singer clips feed the offline singer's event input and the independent guide
instrument. Guide audio enters `guide_in`; singer audio enters the group's sum.
The cached take, voice settings and provenance are copied exactly. A declared take
remains selected even when stale; a missing asset means silence, while absence of a
take selects the guide. Conversion does not synthesize or refresh a take.

Automation curves preserve points and interpolation. Every lane becomes a curve
source and an enabled binding to a stable processor/key. Built-in control domains
come from the same descriptors as session commands. Plugin domains remain unresolved
until authoritative metadata is available. A plugin lane without a stable key fails
explicitly; its numeric runtime parameter index is never guessed into a key.

Asset references remain metadata. Missing files and missing asset records are
retained for preparation diagnostics, and conversion loads no plugins, SoundFonts,
audio files or voice models. Ownership validation is distinct from capability and
runtime readiness checks.

## Source capture and backup

The file path is read once with the existing 64 MiB project-file bound. The existing
version dispatcher, ID checks, loop-expansion checks, curve repair and routing repair
decode those captured bytes. `PipelineMigrationReport` records the source version
before normalization, ID-counter repair, clip pitch/controller sorting and routing
repair. The original bytes remain unchanged, including whitespace and unknown fields.

`source_matches` compares a path with that capture using another bounded read when
the caller needs a source-change check. `preserve_backup` writes the captured bytes,
using a private synchronized sibling and atomic no-clobber publication. An identical
existing regular file is an idempotent success; different bytes, directories, symlinks
and special files are rejected. Oversized input is rejected before staging.

The explicit native IO path now groups source checks, backup and atomic replacement
under the same `.auris-write.lock` convention as Session saves. Default Session Save,
recovery and checkpoints now write format 33 through a separate lossless conventional
codec. The [storage transition decision](render-pipeline-plan.md#storage-transition-decision-2026-10-28)
separates that migration from activation of the native runtime command owner.

## Guarded native persistence

The native envelope uses `NATIVE_FORMAT_VERSION = 33` beside the legacy
`Project::FORMAT_VERSION = 32`. `load_native` probes the envelope version before
decoding its payload; the explicit legacy decoder rejects native files before interpreting
them as a legacy score. Native ownership, numeric playback fields and existing
clip loop/event budgets are validated without opening assets or plugins.

`stage_native` serializes through the bounded 64 MiB writer, validates a lossless
decode of the staged payload, synchronizes a private sibling, and returns an owned
staging value. Dropping it removes that sibling. Its replacement method is private;
public callers use one of three guarded publication operations:

- `save_native` claims an absent destination and refuses an existing file.
- `publish_native_update` requires the exact destination bytes observed by its caller,
  validates that they describe a native document, and rejects a changed destination.
- `publish_native_upgrade` checks the captured legacy bytes, preserves a distinct
  exact-byte backup, stages the native document, rechecks the source, and publishes.
  An in-place upgrade replaces the source; another destination must be absent.

Each operation holds the project write lock during its checks and publication. An
upgrade across folders holds both the source and destination folder locks. A busy
lock reports failure for the caller to retry. The lock coordinates Auris writers;
an unrelated program that ignores it can still race the final check and rename.
Backup/source aliases, conflicting backups and stale sources fail without replacing
the source. Asset collection and Save As remain Session policies, not implicit IO
side effects: these functions receive an exact file path and do not copy assets.

## Verification

Historical format 27–32 fixtures are loaded through the production decoder and
converted without external plugins or assets. Tests compare context, authored clips,
opaque states, singer payloads, routes, insert order, sidechains and automation targets.
Native JSON round trips preserve generated identities. Synthetic fixture capabilities
validate the entire converted DAG without claiming that external plugins are installed.

Additional tests cover exact budgets, stale/deleted/exhausted identity history,
malformed routing, missing keys, missing asset references, unchanged inputs on failure,
captured version/repair reporting, source changes, raw-byte backups, existing-file
collisions and competing backup publishers. Existing production load/save baseline
tests remain the regression reference.

Validation on 2026-10-28 covered 4,972 passing tests and 51 ignored tests across
71 suites. The full workspace run passed all 1,241 GPUI tests; a new legacy-repair
fixture needed its historical `output` field corrected, after which the remaining
workspace suites and doctests passed in a run excluding GPUI. The test failure was
in the fixture schema, not the decoder. Warning-denied workspace Clippy and
formatting checks passed. `cargo doc --workspace --no-deps` passed with
`RUSTDOCFLAGS=-D warnings`.
