# Adversarial review, November 2026 — on a Mac

A whole-workspace review of commit `864d2ba6` (2026-11-01), and the first one actually **run on
macOS**: Apple M1, macOS 27.0.1 (26A434), rustc 1.99.1, system language `ja-JP`. Findings are
numbered `MR-nn`; cite the number from the commit that fixes one, and delete its entry once the
fix lands. Line numbers are as of `864d2ba6` and will drift.

The previous review (`AR-nn`, October 2026) said of itself: "Nothing here was checked on a Mac."
That is the gap this one closes. Its AR-09 — `Language::from_system_locale` reading
`NSLocale::preferredLanguages`, type-checked but never executed on Darwin — **works**: the desktop
application launched on a `ja-JP` Mac with no locale variables set and came up in Japanese. The
Finder double-click and release-bundle recording paths it also listed remain unverified here; see
"Not covered".

## How it was done

Read by hand, risk first — the realtime paths, project IO, asset containment, the agent
permission gate, the plugin hosts, the updater — then the whole suite, clippy and `fmt` were run,
then the product itself was driven on the machine: a project created, a song composed from a
preset, a mix rendered, the stems rendered, and every rendered file measured with `ffprobe` and
`ffmpeg astats` rather than trusted. Six findings below exist only because something was measured
or looked at on disk; they are marked **(measured)**.

The review is deliberately unbalanced in the other direction too: the realtime contract, project
IO, asset paths and the permission gate were attacked hard and did not yield. What they did and
did not cover is in "Covered without findings", because a review that reports only faults
misdescribes the codebase.

## Summary

| ID | Severity | Area | Finding |
|----|----------|------|---------|
| MR-01 | High | Tests | Two window tests fail on a non-English macOS, and CI cannot catch it |
| MR-02 | High | CI | ~250 tests guarding the vendored forks are never run; VST3 hosting has no coverage at all |
| MR-03 | High | Build | A vendored crate's tests cannot run from a git worktree |
| MR-04 | Medium | DSP | The bowed string emits DC that reaches the master unguarded |
| MR-05 | Medium | IO | Saved projects and exported audio are written `0600` |
| MR-06 | Medium | IO | Interrupted downloads leave their staging files behind for good |
| MR-07 | Low | IO | The project write lock blocks uninterruptibly |
| MR-08 | Low | Interface | A healthy start-up reports itself as an audio failure |
| MR-09 | Low | Memory | Undo keeps 64 whole documents with no bound in bytes |
| MR-10 | Low | Site | Six documentation links are `href="#"` with no fallback |
| MR-11 | Low | Docs | Two crates are in no documented layout; the CLI never localises |
| MR-12 | Low | Interface | The dictionary warning fires on commands that have no lyrics |
| MR-13 | Low | Build | `block 0.1.6` is future-incompatible and reaches the build six ways |
| MR-14 | High | Tests | The log panel tests race each other over a process-global logbook |

## Findings

### MR-01 — Three window tests fail on a non-English macOS, and CI cannot catch it

**(measured)** `cargo test --workspace` on this machine: **1226 passed, 3 failed**. The three:

```
ui::layer::tests::window::the_track_menu_adds_a_layer_and_the_inspector_shapes_it
  crates/auris-gpui/src/ui/layer.rs:474   left: "Test レイヤー"  right: "Test Layer"
ui::title_bar::tests::the_transport_joins_the_titlebar_only_where_it_fits
  crates/auris-gpui/src/ui/title_bar.rs:303   left: "無題"  right: "Untitled"
ui::log_panel::tests::a_narrow_log_keeps_severity_source_and_message_readable
  crates/auris-gpui/src/ui/log_panel.rs:374   `log-level-0` is visible
```

**Two of the three are the locale; the third is not, and finding that out took a second
experiment.** All three pass under `LANG=en_US.UTF-8`, which is what the first run showed — but
that run also happened to be a filtered one, so it changed two things at once. Run *alone* under
the Japanese locale, `ui::layer` and `ui::title_bar` still fail and `ui::log_panel` **passes**:

| Test | alone, `ja-JP` | alone, `en_US` | in the full suite, `ja-JP` |
|------|----------------|----------------|----------------------------|
| `ui::layer::…` | fail | pass | fail |
| `ui::title_bar::…` | fail | pass | fail |
| `ui::log_panel::…` | **pass** | pass | fail |

So there are two separate faults behind one red suite:

* **The locale, for two of them.** `harness::open` pins no language, the session resolves one
  from the system, and `Language::from_system_locale` answers Japanese — the very behaviour
  CLAUDE.md calls load-bearing. These two tests hold English literals where the other ~1 240
  window tests compare against `Key::…get(this.language())`. GitHub's `macos-latest` runner is
  English, so CI is green while the suite is red for the author of the project, on the platform
  development happens on; the October review's Windows run (4 253 passed) agrees, since that
  machine was English too.
* **Shared global state, for the third.** That one is MR-14, and it is a flake rather than a
  locale fault: it only fails when the rest of the suite runs beside it.

**Direction.** Pin the language in `harness::open` so the suite is about the interface rather than
about the developer's Mac, and give these two tests the language explicitly the way their
neighbours do. Note that pinning the language alone would have hidden MR-14 instead of fixing it,
which is why the second experiment was worth running.

### MR-02 — ~250 tests guarding the vendored forks are never run; VST3 hosting has no coverage at all

**(measured)** Run by hand from their own directories:

* `vendor/vst3-host`: **240 passed, 0 failed** — including `realtime.rs` (11 tests over the
  single-owner path this fork exists for), `process_isolation` (a helper that hangs is killed, a
  load that crashes is retried once, a load that kills the helper is replayed against a fresh one),
  and the transport arithmetic.
* `vendor/rustysynth`: **8 passed** (the fork's modulator curves and its generator/zone span
  validation), plus the two upstream failures CLAUDE.md documents — `test_load_reject_sf3` and
  `test_load_empty_samples` want SoundFont files the published crate does not ship.

No CI job runs any of them: `.github/workflows/ci.yml` mentions `vendor` nowhere, and the crates
are out of the workspace so `--workspace` does not reach them.

For rustysynth this matters less than it looks — the fork's two dangerous fixes are regression
tested from the workspace side, where CI does run them (`a_loop_step_larger_than_the_whole_sample_
stays_inside_the_loop` in `crates/auris-sampler/src/sampler.rs:1578` covers the oscillator fold
that used to panic inside the audio callback, and `vendor/rustysynth/doc/note-release.md` points
at `cargo test -p auris-sampler` for the note-release work).

For VST3 it is the whole story. `crates/auris-vst3` is 2 138 lines with 13 tests in `lib.rs`, and
its only end-to-end test — `tests/external_smoke.rs` — is `#[ignore]`d behind
`AURIS_VST3_SMOKE_PLUGIN`, which CI never sets. So **nothing automated exercises loading and
running a third-party VST3**, in CI or anywhere else, while the 240 tests that do cover the host
sit unrun. `auris-clap` shows the pattern that works, already in-house: it takes `clack-plugin` as
a dev-dependency, links a synthetic plugin into the test binary, and runs 88 tests over it.

**Direction.** One `cargo test` per vendored crate in the macOS job, with rustysynth's two
upstream font tests `#[ignore]`d or skipped so the command can gate. Separately, give `auris-vst3`
a synthetic plugin the way `auris-clap` has one; the vendored tests cover the host, not Auris'
use of it.

### MR-03 — A vendored crate's tests cannot run from a git worktree

**(measured)** CLAUDE.md says of the forks: "Its own tests run from its own directory." From a
worktree they do not run at all:

```
error: current package believes it's in a workspace when it's not:
current:   …/.claude/worktrees/<name>/vendor/rustysynth/Cargo.toml
workspace: /Users/uthree/Develop/auris-studio/Cargo.toml
```

The root manifest excludes `vendor/rustysynth` *relative to the root*, which does not match
`.claude/worktrees/*/vendor/rustysynth`, so cargo walks up and binds the crate to the main
checkout's workspace. None of the four vendored manifests has a `[workspace]` table. Since this
project's own workflow puts sessions in `.claude/worktrees`, the documented way to test a fork is
broken wherever the work actually happens — which is a plausible part of why MR-02 went unnoticed.

It is worse than the forks' own tests, found while verifying MR-01: `cargo fmt --all` and
`cargo clippy --workspace` resolve the workspace's *path dependencies*, and `vendor/midir` is one
of them, so **both of those fail outright in a worktree too**, with the same error. Two of the
three commands CI gates on cannot be run locally where this project's own sessions live.

**Fixed in this change**, with cargo's own suggested remedy: a `[workspace]` table in each of the
four vendored manifests (`rustysynth`, `vst3-host`, `midir`, `gpui-flow`), +4 lines each. The 248
tests reported in MR-02 were run through that fix, and so was every `fmt` and `clippy` run in this
review.

### MR-04 — The bowed string emits DC that reaches the master unguarded

**(measured)** A `pop-band` preset composed and rendered on this machine, measured with
`ffmpeg astats`:

| Stem | DC offset | Peak |
|------|-----------|------|
| `lead` | **+0.005740** | -12.20 dBFS |
| `strings` | +0.001486 | -21.95 dBFS |
| `Drums` | -0.001511 | -0.30 dBFS |
| guitar, keys, piano, pluck, lead_alt, bass, sub_bass, riser | ≤ 1.3e-4 | — |
| **master mix** | **L +0.005786 / R +0.004077** | -0.3006 dBFS |

The two stems carrying DC are exactly the two the composer assigned
`auris.physical.violin` — the bowed model. The two `electric_guitar` stems, the two `piano`
stems and the `mallet` stem all read zero, and the piano reading zero is `body.rs:40` doing its
job. On `lead` the DC is about 2.3 % of the stem's own peak.

The path is unguarded end to end:

* `crates/auris-synth/src/physical/bow.rs` contains no mean removal — no match for `mean`, `dc`,
  `highpass` or `offset` in the file. Its own test is named
  `friction_grips_at_low_speed_and_never_reverses_relative_slip`, and a friction force that never
  reverses is precisely a DC source.
* `crates/auris-synth/src/physical/body.rs:40` fits the highpass only for `Model::Piano`, and
  `body.rs:188` states the consequence as an assertion: "The piano's soundboard does not radiate
  DC at all; the others pass it unchanged."
* Nothing downstream removes it. There is no DC blocker anywhere in `auris-dsp` or `auris-engine`.
* `ExportMeasurement` (`crates/auris-session/src/export_inspection.rs:28-32`) records LUFS, peak
  and true peak but not DC, so `verify_render` cannot report it and the agent cannot see it.

The cost is headroom: the limiter judges absolute peak, so DC is subtracted from the ceiling
before any music gets there. At -45 dBFS it is inaudible on its own and still wrong to ship in a
master.

The fix already exists in this workspace for the one instrument that needed it:
`crates/auris-synth/src/clarinet.rs:291-295` subtracts a slow `flow_mean` with the reason written
out, and `clarinet.rs:509-512` asserts `mean.abs() < rms * 0.2`.

**Direction.** Do for the bow what the clarinet already does, with the same kind of numeric test;
and add a DC field to `ExportMeasurement` so a master is measured for it rather than reasoned
about. `auris.fx.eq` already has a high-pass band, so there is a user-side workaround in the
meantime.

### MR-05 — Saved projects and exported audio are written `0600`

**(measured)** With `umask 022`:

```
-rw-------  Demo/Demo.auris
-rw-r--r--  Demo/.auris-write.lock
-rw-------  Pop.wav          (38 MB, 24-bit render)
```

`new_staged_file` (`crates/auris-io/src/project_file.rs:185`) uses `NamedTempFile::new_in`, which
creates at `0600`, and `persist` renames — so the mode survives into the published file. The lock
file beside it, created with `OpenOptions`, honours the umask, which is what makes the asymmetry
visible. Every path through `publish_staged_file` is affected: documents, WAV and MIDI exports.

The fix is one call before persisting; an owner-only mode is right for a private staging file and
wrong for a deliverable the user is going to hand to somebody.

### MR-06 — Interrupted downloads leave their staging files behind for good

**(measured)** On this machine, in `~/.config/auris-studio/SoundFonts/`:

```
202 MB  .tmp28slFz     174 MB  .tmpSYyQiv
 13 MB  .tmpWNe0u9     6.8 MB  .tmpGz69HH
```

396 MB of partial SoundFont downloads, all dated 2026-09-07, beside the 215 MB font that
eventually arrived. `NamedTempFile` removes itself on drop, so these are what is left when the
process does not unwind — a force quit during a 215 MB download, which is a minutes-long window
the user has every reason to interrupt. Nothing in the codebase ever sweeps them: no match for a
stale-temp cleanup in `crates/auris-session/src/library/`.

The download itself is otherwise careful — a byte limit, a SHA-256 and a size both pinned as
compile-time constants in `library.rs:79-80`, the licence notice fetched first, a noclobber
publish, and a second instance finishing the work handled explicitly.

**Direction.** Give the staging files a recognisable prefix (`NamedTempFile`'s builder takes one)
and sweep stale ones before a download or at start-up. Without a prefix they cannot be told from
anything else a user might have put there, which is presumably why nothing deletes them today.

### MR-07 — The project write lock blocks uninterruptibly

`project_write_lock` (`crates/auris-session/src/session/files.rs:1876`) takes a blocking
`File::lock()`. `SaveJob::run` is built around cancellation — it checks `cancelled` immediately
before the call and immediately after it (`files.rs:385-398`) — but a blocking `lock()` cannot be
interrupted, so while another Auris process holds the lock the save hangs on the worker and the
cancel flag is never read. The window stays responsive and the save never finishes.

`try_lock()` is already the pattern elsewhere in this crate (`autosave.rs:287`,
`workshop.rs:445`), so the fix is a short retry loop that also polls `cancelled`.

The lock itself is sound: it is an advisory OS lock released when the process dies, so a crash
cannot leave a project unopenable — only the empty `.auris-write.lock` file stays behind.

### MR-08 — A healthy start-up reports itself as an audio failure

**(measured)** The application launched on a Mac whose default output device is present and
working (48 kHz, 2 ch) and put this in the status line:

```
オーディオ出力なし — 編集と書き出しは可能です · GPU: Apple M1 (metal)
```

Nothing is wrong. `session_options` (`crates/auris-gpui/src/app.rs:1753-1759`) opens every project
window with `audio_suspended: true`, and `acquire_audio`
(`crates/auris-gpui/src/project_windows.rs:358`) takes the device on the first explicit audio
command — "Switch only for an explicit audio command, never because focus changed." That is a good
design and it is why no `start_audio` warning appears in the log: `start_audio` was never called.

But `audio_line` (`app.rs:3850`) reads only `status.running`, and `Key::NoAudioOutput` says "No
audio output — editing and export still work", which is indistinguishable from a device that
failed to open. Every launch on a healthy machine shows what looks like a hardware fault.

The right words are already in the string table one field over:
`Key::ProjectAudioSuspended` — "Audio is now used by another project. Press Play to switch back."
A start-up variant of that sentence, used while nothing has been acquired yet, costs one `Key`.

### MR-09 — Undo keeps 64 whole documents with no bound in bytes

`History::default()` is `Self::new(64)` (`crates/auris-session/src/history.rs:327-329`), and each
entry is a whole `Project` clone. The module explains why snapshots beat command inverses, and the
reasoning holds — `AudioSource` stores a path and metadata, never samples, so the doc's "a few
kilobytes" is right about audio.

It is not right about documents in general. **(measured)** The `pop-band` preset composed here —
11 tracks, 1 841 notes, 69 bars — is a 566 KB document. A long arrangement with a sung part carries lyrics
and per-note phonemes and is plausibly several megabytes, which puts the undo stack in the
hundreds of megabytes while nothing bounds it. A limit in bytes as well as in steps would.

Separately, `push` evicts with `self.past.remove(0)` (`history.rs:374`), an O(n) move of 64
`Project` values on every edit past the limit; `VecDeque` is the idiom and costs nothing to adopt.

### MR-10 — Six documentation links are `href="#"` with no fallback

In `site/index.html:287-292` all six `[data-doc]` links ship as `href="#"` and are filled in by
`setLanguage()` in `site/site.js:26-30`. Without JavaScript they go nowhere, and — the part that
costs something — a crawler or a link preview sees `href="#"`, so the documentation is
unreachable from the site for anything that does not run scripts.

The download buttons next to them get this right and show the fix: they ship with
`href="…/releases/latest"` and are *upgraded* to the exact asset by script, with the reasoning in
a comment ("Until that answers, or if it never does (offline, rate-limited), the links lead to the
latest release's page"). The doc links want the same treatment: the English URL in the HTML,
swapped for `.ja.md` by `setLanguage`.

### MR-11 — Two crates are in no documented layout; the CLI never localises

* `auris-update-helper` (741 lines) appears in **none** of the three places that describe the
  layout — `docs/development.md`, `CLAUDE.md`, and `auris_session::guide`. It is the crate that
  replaces the application binary on disk, so it is the one that least deserves to be
  undocumented. (CI does know about it: `cargo test -p auris-update-helper --release`.)
* `auris-video` is in `CLAUDE.md` and the guide but missing from `docs/development.md`.
* `CLAUDE.md` and the guide both describe `auris-i18n` as "every word said to a person (the window
  **and the CLI**)". The CLI is hard-wired: `const LANGUAGE: Language = Language::English`
  (`crates/auris-cli/src/main.rs:30`). It was verified on this machine — `auris help` is English
  on a `ja-JP` Mac with and without `LANG`. The choice is deliberate and well argued in the doc
  comment above it (Windows console code pages, pipes into tools that assume ASCII); it is the
  layout description that is wrong, not the code. Say the CLI uses `auris-i18n` in English.

### MR-12 — The dictionary warning fires on commands that have no lyrics

`auris new`, which creates an empty project with one instrument track, prints a warning about the
missing Japanese dictionary together with the eight directories it searched. So does every
start-up. It is the first thing a new user sees from the tool, it is three lines long, and it is
about a feature they have not asked for. Warn when a kanji lyric actually needs the dictionary.

### MR-13 — `block 0.1.6` is future-incompatible and reaches the build six ways

Every build on this machine ends with:

```
warning: the following packages contain code that will be rejected by a future version of Rust:
block v0.1.6, proc-macro-error2 v2.0.1
```

`block` is the pre-`objc2` Objective-C block crate, unmaintained since 2016, and it arrives
through `gpui`, `metal`, `wgpu-hal`, `cocoa`, `coremidi`, `core-video` and `screencapturekit-sys`
— the whole macOS stack. Nothing here can be fixed locally; when rustc carries out the rejection
the macOS build stops compiling until those crates move to `objc2`. Worth a tracking note rather
than an attempt: the one lever is upgrading `gpui` and `wgpu` when they do.

### MR-14 — The log panel tests race each other over a process-global logbook

**(measured)** `ui::log_panel::tests::a_narrow_log_keeps_severity_source_and_message_readable`
fails in the full suite and passes when run alone, under either locale. It is a flake, and the
shared thing it flakes on is named: `BOOK` is a single
`OnceLock<Arc<Logbook>>` for the whole process (`crates/auris-gpui/src/logbook.rs:163`), and
`cargo test` runs this file's tests on several threads inside that one process.

Both log panel tests do `book().clear()`, then `book().push(…)`, then assert on what is in the
book — so a `clear` in one lands in the middle of the other. The second one is worse than a wrong
assertion: `the_log_can_be_copied_whole_and_line_by_line` reads `book().entries()[1].number`
(`log_panel.rs:340`), which panics on an index out of bounds if the other test cleared the book
first. The leak runs the other way too: entries one test pushes are visible to any test that
reads `book().problems()` for the status bar badge (`ui/status_bar.rs:236`) or collects log lines
for the issue report (`issue_report_window.rs:95`).

The author saw the shape of this — there is a `ClearLog` drop guard, and a comment reading "The
seconds are since the book was created, which another test may have done" — but clearing on the
way in and out cannot help: the races are *inside* the bodies, between the clear and the assert.

**Direction.** One mutex that every test touching the book holds for its whole body, so they
serialise against each other rather than against nothing. `auris-io`'s `publish_ready_staged_file`
already shows the idiom for not letting a panicking test poison it:
`lock().unwrap_or_else(PoisonError::into_inner)`.

## Covered without findings

Attacked with intent and held up.

**The realtime contract is genuinely enforced**, not merely documented. There is no `unsafe` in
`auris-core`, `auris-dsp`, `auris-synth`, `auris-sampler` or `auris-engine` outside the
allocation-counting test harnesses, and those harnesses are the mechanism: a global allocator that
counts, so "no allocation on the audio thread" is asserted rather than intended. The handover is
airtight in a way worth naming, because it is the part that is usually wrong:
`EngineCommand::SetGraph` and `SetSoloResolution` carry owned allocations onto the callback, and
`retire` sends every one back up a second channel; when that channel is full they go into a
`Vec::with_capacity(RETIRED_GRAPH_SLOTS)` stash; `poll_commands` stops accepting
ownership-carrying commands at exactly `RETIRED_GRAPH_SLOTS` (`device.rs:1193`) and defers them;
no single `apply` retires more than once, so the stash can never reallocate; a receiver that has
disconnected is explicitly *not* treated as permission to drop on the callback
(`device.rs:1225-1231`); and `retired_graphs_survive_a_full_return_queue` tests it. Instrument
voices are built in `prepare` and `note_on` only indexes them.

**Project IO.** Two-phase atomic save through a private sibling opened once and renamed, with
`flush` + `sync_all`, a noclobber variant for exports, a publish mutex, a size cap on reads, a
version probe before the full parse with a documented `READABLE_OLDER` range (and a comment
recording that version 32 forgot to join it), duplicate-id detection, and repair of the id counter,
curve order and routing. `for_each_object_id` covers all six id newtypes and every clip-bearing
`TrackKind` — `Instrument`, `Drum`, `Singer` and `Audio` — so the counter repair cannot miss an id
and hand out a collision.

**Asset containment.** `AssetPath::inside` and `resolve` keep only `Component::Normal`, so `..`,
roots and prefixes are dropped, and `resolve` splits on both separators so a Windows-saved project
opens on a Mac. `resolve_for_automatic_access` refuses UNC and device paths, so opening a document
cannot make the machine contact a server the document names.

**The agent permission gate.** `Operation::parse` fails closed — an unknown tool is refused rather
than defaulted — retired grants are dropped on load while retired denials are carried to their
successors, and the ordering in `decide` means an allow rule cannot override read-only or plan
mode. `configure_plugin_paths` is absent from `OPERATIONS`, so the live agent cannot reach the one
tool that would let a model add a folder and have native code loaded from it. The standalone
saved-file MCP server deliberately does not consult this policy (its module doc says so), which
makes it a surface to hand only to a client the user trusts as much as a shell.

**Secrets.** Settings store `api_key_env` — the name of an environment variable — never a key, so
no configuration file can leak one.

**Download and update trust.** The SoundFont download pins URL, byte count and SHA-256 as
compile-time constants. The updater pins `browser_download_url` to the exact expected release URL
for the tag and sets `https_only(true)`, with a test that rejects an asset served from another
host. The remaining gap is already documented by the project rather than hidden: releases are
unsigned and un-notarized, the install docs and the site tell users to click through Gatekeeper,
and the updater's digest comes from the same GitHub API response as the download, so it proves the
transfer rather than the publisher. Developer ID signing plus notarization is what closes both,
and nothing short of it does.

**The limiter's brickwall guarantee, confirmed by an independent tool.** The rendered master
measures a peak of **-0.3006 dBFS** against a default ceiling of -0.3 dB with a flat factor of
**0.000000** — no clipped runs anywhere in 6.5 M samples. `prepare` and `reset` both clear
`next_peak`, `settle_to` returns early on an empty history, and the modulo is guarded, so the
re-settling on a ceiling change cannot index out of range.

**Plugin delay compensation.** `plan_latency` clamps a hostile plugin latency before it can size a
delay-line allocation, with a test for it; the per-edge differences are taken from a maximum so
they cannot underflow; delay lines are always built at `RENDER_CHANNELS`, so the `zip` in
`LatencyDelay::process` cannot silently leave a channel undelayed.

**Capture.** A buffer pool over two bounded channels, a block dropped and counted rather than
blocking the device, and the count carried all the way to the transport bar
(`ui/transport_bar.rs:94`) — a take with a gap tells the user it has one. The two-clock drift is a
documented decision not to resample against a rate nobody measured.

**ffmpeg.** No shell, arguments passed individually, stderr drained on its own thread so a chatty
encoder cannot deadlock the writer.

**Platform portability rules.** Every `cmd-` in `auris-gpui` is in a comment or a test, including
one asserting the default keymap contains no `cmd-`; `Modifiers::platform` is read only inside
`actions.rs`, whose job is that translation. The 28 `#[cfg(target_os)]` sites are all
platform-only APIs, not decisions, against 93 `cfg!` uses. The Windows-only reparse-point check in
`autosave.rs` is a genuine API gap, not an asymmetry: the Unix side is covered by
`symlink_metadata` + `is_symlink` plus a `canonicalize` comparison.

**Housekeeping.** Zero `TODO`, `FIXME`, `unimplemented!` or `todo!` in 412 k lines of
`crates/`. Thirty-two `#[allow]` attributes in the whole workspace. All fourteen `#[ignore]`s are
gated on genuinely external resources — a real VST3 or CLAP bundle, a signed-in model CLI — each
with its reason written in the attribute. `auris-i18n` has two languages behind a `match` on an
enum, so a missing translation is a compile error, and `auris-gpui/src/i18n.rs:1002` asserts no
string is left untranslated.

## Not covered

* **Pressing Play.** The session running this review had no desktop-control tools, so the
  `acquire_audio` path — the one that actually opens CoreAudio — was never exercised. Everything
  up to it was: the window opened, Metal initialised on the M1, the 566 KB project loaded, the
  drum-probe worker spawned, memory held flat at 230 MB over six samples a minute apart, and the
  process exited cleanly with no crash report. This is the first thing to try by hand.
* **Recording, and double-clicking a project in Finder** — the two things the October review named
  as untested on a Mac. Both need a release bundle and a human at the keyboard.
* **Third-party plugin hosting.** No VST3 or CLAP plugin is installed on this machine, so
  `tests/external_smoke.rs` could not be run even by hand. See MR-02.
* **Release-build performance.** Idle CPU measured **~33 % of one core** with an 11-track project
  loaded and nothing playing, steady across six samples. That is a *debug* build, and
  `Cargo.toml` sets `opt-level = 0` for `auris-gpui` on purpose, so the number says nothing about
  what ships — but a window repainting continuously rather than on demand is worth measuring in
  release, on battery. `gpui_perf` is already in the graph.
* **The composer's output as music.** `compose --preset pop-band` took 5 m 47 s in a debug build
  under load; the symbolic and learned evaluators (`docs/evaluation.md`) were not run.

## Test suite

On macOS 27.0.1, Apple M1, rustc 1.99.1, at `864d2ba6`:

| Command | Result |
|---------|--------|
| `cargo test --workspace` | **1226 passed, 3 failed, 14 ignored** — MR-01 (two) and MR-14 (one) |
| same, with `LANG=en_US.UTF-8` | the three pass |
| the three, run alone, under `ja-JP` | MR-01's two fail; MR-14's passes |
| `cargo clippy --workspace --all-targets --locked` | clean; no warning from any workspace crate |
| `cargo fmt --all --check` | clean |
| `cargo test` in `vendor/vst3-host` | 240 passed, 0 failed |
| `cargo test` in `vendor/rustysynth` | 8 passed, 2 failed (both documented upstream) |

End to end on the machine, with every output measured rather than trusted: `auris new` →
`auris compose --preset pop-band` (11 tracks, 1 841 notes, 69 bars, 566 KB, mixed to -16.6 LUFS)
→ `auris render` → 2:15.379,
48 kHz, 2 ch, 24-bit, peak -0.3006 dBFS, verified with `ffprobe`; `auris render --stems` → 11
stems, each measured for DC and peak, which is where MR-04 comes from. The desktop application was
launched on the composed project; see "Not covered" for how far that went.

MR-01, MR-02, MR-03, MR-04, MR-05, MR-06, MR-08, MR-13 and MR-14 exist only because something was
run, measured or listed on disk. Nothing else in this review was caught by the existing suite, and
the suite passing on Windows in October while failing here in November is MR-01 and MR-14 rather
than a regression — one needs an English machine to pass, the other needs to be the only test
running.

MR-14 is also a note on method: the first run changed the locale *and* filtered the suite down to
three tests, and all three went green, which looked like one cause. It was two. A second run that
changed only one thing at a time is what separated them.
