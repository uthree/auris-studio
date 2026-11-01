# Adversarial review and Windows runtime tests, 2026-11-02

Reviewed `864d2ba6a484368af757fbed5f08ac753a85c207`, which matched `origin/main`
when the review began. This is a risk-based review across the workspace, with
parallel inspection of storage/session commands, audio/plugin code, and the
desktop/agent. It is not a claim that every line or every hardware configuration
was exercised. Product code was not changed.

## Findings

### WR-01 — Reject unreasonable sample rates before preparing a render graph (P2)

A valid JSON project with `sample_rate: 1e300`, no tracks, and a built-in reverb
on its master bus opens through the CLI's project reader. Rendering it then
panics instead of returning a validation error:

```text
thread 'main' (...) panicked at crates\auris-dsp\src\reverb.rs:310:13:
attempt to add with overflow
```

The command exits with **101**. No large allocation was necessary to reproduce
this: the pre-delay capacity calculation overflows first in this debug build.

The missing bound is shared across several layers:

- `crates/auris-core/src/project/mod.rs:315-323` accepts any positive finite rate.
- `crates/auris-io/src/project_file.rs:310-326` deserializes the rate without a
  supported-range check.
- `crates/auris-engine/src/offline.rs:333-353` checks only positivity/finiteness
  and builds the graph **before** its render-frame budget checks.
- `crates/auris-engine/src/graph/mod.rs:285-293` passes that rate to plugin
  preparation.
- `crates/auris-dsp/src/reverb.rs:305-322` derives delay-line capacities from it.
  The delay effect also sizes its storage from the rate.

Reproduce with a disposable project created by `auris new`, then set its
`sample_rate` to `1e300`, empty its `tracks`, and set `master.effects` to:

```json
[
  {
    "id": 20,
    "effect_id": "auris.fx.reverb",
    "enabled": true,
    "state": { "params": {} }
  }
]
```

Run `auris render hostile-rate.auris -o hostile-rate.wav --no-tail`.
The existing render-length safeguards do not protect preparation. Validate a
reasonable supported rate before preparing built-in or hosted plugins, including
rates supplied through offline render options. A malformed file should return an
error without unwinding the frontend. Release-build behavior was not measured;
the observed debug arithmetic panic is sufficient to establish this defect.

### WR-02 — Focused text fields have no Undo or Redo (P2)

Confirmed in the real Windows agent composer:

1. Open the agent panel with Ctrl+Alt+A and click its input area.
2. Type `Review draft 日本語`, then type `a` with a regular key event.
3. Press Ctrl+Z.

The final `a` remains. Ctrl+Shift+Z likewise has no text-history behavior.
The selected MIDI note remains unchanged, so the document-focus guard works;
the missing part is text editing history.

`crates/auris-gpui/src/ui/text_field.rs:318-341` handles deletion, cursor movement,
and Select All but returns `KeyEffect::Ignored` for Undo/Redo.
`apply_key_with_clipboard` at lines 354-397 adds cut/copy/paste and delegates
everything else to the same handler. The custom field stores no edit history.
Application commands are excluded while the prompt owns focus, leaving no
fallback handler.

This also affects other consumers of this field, including settings editors.
Replacing a long draft or deleting a selection cannot be undone. Implement
field-local history and route the platform Undo/Redo shortcuts to the focused
field, preserving the existing protection of the song's Undo stack. Verify
replacement, deletion, paste, and input-method composition as well as insertion.

## Existing PRs excluded

Open PRs were enumerated and their changed-file lists and descriptions read
before the review. The November review document on PR #83 was read too, so its
other recorded findings were excluded even where their fix had not yet been
published separately. A final refresh found seven additional PRs (#93-#99);
their descriptions and the review document on #93 were checked before finalizing
the findings. Its AN-01 through AN-07 are excluded as well.

| PR | Excluded work |
| --- | --- |
| [#50](https://github.com/uthree/auris-studio/pull/50) | Workshop logs, refused singing, tool-call budget notices, Windows default-output fallback |
| [#83](https://github.com/uthree/auris-studio/pull/83) | Existing MR-01 through MR-14 findings and vendor workspace discovery |
| [#84](https://github.com/uthree/auris-studio/pull/84) | Conversation transcript size ceilings |
| [#85](https://github.com/uthree/auris-studio/pull/85) | Codex npm/bun native executable discovery |
| [#86](https://github.com/uthree/auris-studio/pull/86) | Test-language isolation |
| [#87](https://github.com/uthree/auris-studio/pull/87) | Shared logbook test race |
| [#89](https://github.com/uthree/auris-studio/pull/89) | Vendored test coverage in CI |
| [#90](https://github.com/uthree/auris-studio/pull/90) | Bowed-string DC removal and its measurement |
| [#91](https://github.com/uthree/auris-studio/pull/91) | Published file permissions |
| [#92](https://github.com/uthree/auris-studio/pull/92) | Abandoned SoundFont download staging files |
| [#93](https://github.com/uthree/auris-studio/pull/93) | Existing Windows review AN-01 through AN-07 |
| [#94](https://github.com/uthree/auris-studio/pull/94) | Site documentation link fallbacks |
| [#95](https://github.com/uthree/auris-studio/pull/95) | Workspace layout documentation |
| [#96](https://github.com/uthree/auris-studio/pull/96) | Applied installer/helper cleanup |
| [#97](https://github.com/uthree/auris-studio/pull/97) | Project-lock cancellation |
| [#98](https://github.com/uthree/auris-studio/pull/98) | Unnecessary dictionary warnings |
| [#99](https://github.com/uthree/auris-studio/pull/99) | Approved-URL redirect restrictions |

## Windows execution

Windows 11 Home `10.0.26300`, Rust `1.99.0` MSVC. The rebuilt desktop used the
AMD Radeon RX 9070 XT Vulkan adapter. Test settings, autosaves, drafts, and
projects used an isolated `AURIS_CONFIG_DIR`; automatic updates were disabled.
The installed SoundFont was reused through `AURIS_SOUNDFONTS`.

The desktop and CLI were built from the reviewed worktree with an explicit
shared target directory. The workspace tests used this worktree's own target.
This distinction matters because a pre-existing executable was not used as
evidence for the current source.

| Check | Result |
| --- | --- |
| `cargo build --locked -p auris-gpui -p auris-cli -j 8` | Passed |
| `cargo fmt --all --check` | Passed |
| `cargo clippy --workspace --all-targets --locked -j 8 -- -D warnings` | Passed |
| `cargo test --workspace --locked --no-fail-fast -j 8` | 5,003 passed, 1 failed, 56 ignored, across 57 test summaries |
| Real Surge XT CLAP instrument/effect smoke tests | 2 passed |
| Hostile-rate CLI reproduction | Exit 101, arithmetic panic before rendering |

The sole workspace failure was
`ui::log_panel::tests::the_log_can_be_copied_whole_and_line_by_line`.
This is the shared-logbook race already covered by PR #87 / MR-14 and is
excluded from the new findings. The command completed all remaining targets and
doctests under `--no-fail-fast`, returning 101 because of that failure. The two
real CLAP passes above are additional executions of otherwise ignored tests;
they are not added to the workspace aggregate. Clippy reported only Cargo's
existing dependency future-incompatibility notice for `proc-macro-error2`,
not a workspace lint warning.

The real CLAP tests were run from the workspace-built `external_smoke` test
binary with `--ignored --nocapture` and the installed Surge XT instrument and
Surge XT Effects paths supplied through their test environment variables. They
verified effect state save/restore, finite non-silent processing, instrument
silence after repeated Stop/reset, and sounding again after a seek/reset.
Effect output peak was `0.106148`.

### Interactive desktop checks

- Loaded a saved project with seven piano notes and a two-bar loop.
- Pressed Play and Pause through real keyboard input. Playback opened the
  `Speakers (PRO X 2 LIGHTSPEED)` endpoint at 48 kHz / 8 channels; the playhead
  advanced and both track/master meters responded.
- Opened the piano roll and dragged a note. Saving changed that note from
  pitch 72 / tick 2880 to pitch 71 / tick 3120, retaining its 960-tick duration.
  The saved JSON was independently read to confirm the edit.
- Undo restored the original pitch/position; Redo reapplied the gesture.
- Created a second empty project tab, switched with Ctrl+Tab and Ctrl+Shift+Tab,
  and confirmed independent documents and retained selection/editor state.
- Closed the empty tab, then the last project tab. The same native window
  returned to the project chooser.
- Opened and closed the native Settings window and inspected its audio page.
- Typed Japanese and English in the local agent draft and reproduced WR-02.

### Offline audio verification

CLI project creation, info/reload, and WAV rendering succeeded. `ffprobe` and
`ffmpeg astats` independently measured the seven-note piano render:

| Property | Measured value |
| --- | --- |
| Duration | 4.000000 s |
| Rate / channels / encoding | 48,000 Hz / 2 / 24-bit PCM |
| Highest channel sample peak | -22.733520 dBFS |
| Overall RMS | -34.830239 dBFS |
| Channel DC offsets | Approximately -0.000002 |

These are signal measurements and device/UI observations, not a human listening
assessment or an end-to-end latency measurement.

## Review coverage and limits

Inspection covered project publication and asset paths, MIDI import/export,
session transactions, realtime graph preparation/retirement, built-in DSP,
plugin lifecycle, analysis-buffer handling, agent permission boundaries,
updater publication, project tabs, auxiliary-window teardown, text focus,
conversation reload, and preference mutation. Existing tests were inspected as
evidence of contracts, not treated as proof of untested edge cases.

Candidates were rejected where no meaningful boundary or production trigger
was established: the updater's same-user file replacement does not establish a
privilege gain, and a ragged `AudioBuffer` panic was not traced to a production
file-decoding path. URL redirect policy was left to #99. Singer-score omission
from MIDI was reproduced but is already recorded in the repository's frontier
evaluation, so it is not presented as a new finding.

Not exercised: macOS/Linux UI, ASIO, microphone/physical MIDI recording,
hardware loopback latency, VST3 bundles, authenticated model generation, voice
model inference, update installation, release performance, or musical quality.
Debug builds ran under compilation/test load, so no performance conclusion is
drawn. No production writer or audio constant was edited.

Local command logs, hostile fixtures, the MIDI probe, and the WAV measurements
are retained under `.local-evaluations/2026-11-02-windows-review/` and are not
included in Git.
