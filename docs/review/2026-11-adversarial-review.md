# Adversarial review, November 2026

A review of `main` at `864d2ba6` (2026-11-01), run on Windows 11 with the machine's real
devices, files and installed tools. Findings are numbered `AN-nn`; cite the number from the commit
that fixes one, and delete its entry once the fix lands. Line numbers drift; the descriptions name
functions instead.

The October review (`AR-nn`) still has one open entry, AR-08, in
`2026-10-adversarial-review.md`.

## How it was done

Read by hand, risk first, over the code written since the October review (about 500 commits):
the automatic updater and its helper, the agent backends added since (OpenCode, Antigravity), the
agent's file and audio-asset jobs, the issue reporter, video export, the workshop library, and
the Windows-only paths (`cfg(windows)`, `USERPROFILE`/`HOME`, `.cmd` launchers, reparse
points). Then the build was exercised on the machine:

* `cargo test --workspace --no-fail-fast`;
* the CLI end to end in a folder named `実機 テスト`: `new`, `compose --preset`, `info`,
  `render` (mix, and 16-bit dithered stems), `video` through the installed ffmpeg, `collect`;
* `auris-mcp` over stdio with backslash paths: `initialize`, `tools/list`, `inspect_project`,
  `render`;
* the desktop application opened on that project from the command line with an isolated
  configuration folder, and closed with an ordinary close request (no synthetic input).

All of those worked. The findings are what did not hold up.

## Summary

| ID | Severity | Area | Status | Finding |
|----|----------|------|--------|---------|
| AN-01 | Medium | Updates | Fixed | Applied installers and helper copies are never deleted |
| AN-02 | Low | Security | Fixed | An approved audio URL can redirect to loopback, the LAN or plain HTTP |
| AN-03 | Low | Desktop | Fixed | Every window opens saying there is no audio output |
| AN-04 | Low | Privacy | Open | Issue-report redaction misses other spellings of a Windows path |
| AN-05 | Low | Security | Open | The audio-file search lists any folder without asking |
| AN-06 | Low | Windows | Open | The installer does not associate `.auris` files with the application |
| AN-07 | Low | CLI | Open | `auris render` replaces an existing WAV without `--force` |
| AN-08 | Low | Tests | Fixed | Two log-panel tests race on the shared log book |

## Findings

### AN-01 — Applied installers and helper copies are never deleted (fixed)

`download_update` keeps the installer in `<config>/updates`, and `spawn_update_helper` copies the
helper there as `auris-update-helper-<pid>.exe`. Nothing ever removed either. On the review
machine the folder held **540 MB**: eight installers from v0.11.0 to v0.17.0, about 70 MB each, and
nine helper copies. It grows by one installer per release.

**Fix.** `prune_update_folder`, run once per process when a window starts its update checks,
deletes installers for versions no newer than the running one and helper copies belonging to
other processes. A newer, not yet applied installer is kept, because `download_update` reuses one
that still verifies. A helper that is still running cannot be deleted on Windows; the error is
ignored and the file goes on the next launch.

### AN-02 — An approved audio URL can redirect to loopback, the LAN or plain HTTP (fixed)

`audio_assets` `import_url` asks for approval of one URL, then `download` follows up to five
redirects with no check beyond `audio_url`'s scheme and credential rules. A public HTTPS address
the person approved could therefore send the request on to `http://192.168.1.1/…`,
`169.254.169.254` or a service on `127.0.0.1`, none of which they were asked about. The response
only lands in the project if it decodes as audio, so this is a blind request rather than a read,
but the VOICEVOX client and the audio reviewer already refuse to reach non-loopback services
automatically, and this was the one path that let a remote server pick the destination.

**Fix.** `redirect_allowed`: a redirect within the approved origin is followed as before; one to
another origin must be HTTPS and must not name `localhost`, a loopback, private, link-local,
CGNAT or unspecified address. The address the person typed is still trusted, so a local server
they chose works. A hostname that *resolves* to a private address is not caught; that would need
the resolver's answer, and is left.

### AN-03 — Every window opens saying there is no audio output (fixed)

A desktop window starts with its audio suspended and claims the device on the first explicit
audio command (`acquire_audio`), so that a second project window does not take the output from the
first. `audio_line` only looked at whether a stream was running, so every launch reported
"No audio output — editing and export still work" on a machine with working speakers. It was in
the log of the run above, which is how it was found.

**Fix.** `audio_line` takes whether the session is suspended, and says "Audio output opens when
you press Play" (「再生すると音声出力を開きます」) in that case.

### AN-04 — Issue-report redaction misses other spellings of a Windows path

`issue_report::redact` replaces `USERPROFILE`, `HOME`, the project folder and the configuration
folder as typed, with `/` and doubled `\\` variants. Windows paths have more spellings than that.
A user name longer than eight characters, or one with a space, makes `TEMP` an 8.3 short path
(`C:\Users\ALEXAN~1\AppData\Local\Temp`), and `std::env::temp_dir` and every `tempfile` error
message then carry a spelling that does not match. Paths are also case-insensitive, and
`c:\users\alice` survives redaction. The review machine's user name is short, so this was
reasoned rather than reproduced.

**Direction.** On Windows, compare case-insensitively and add each private path's
`GetShortPathNameW` spelling to the list.

### AN-05 — The audio-file search lists any folder without asking

`audio_assets` `search` takes any absolute `root` and is not marked `requires_confirmation`, so
the agent can list the names and sizes of audio files anywhere the user can read, eight levels
deep, without a prompt. A native CLI read outside the project does ask (`cli_file_read` with
`within_project` false). Only audio file names are returned, which keeps this low.

**Direction.** Ask when `root` is given and lies outside the project folder and the OS Music and
Downloads folders that the default search uses.

### AN-06 — The installer does not associate `.auris` files with the application

`main` opens a project passed as the first argument (`project_argument`), and the macOS bundle
declares the document type, but `tools/installer/auris-studio.iss` has no `[Registry]` section.
On Windows, double-clicking `MySong.auris` asks which program to use.

**Direction.** Register `.auris` under `HKCU\Software\Classes` (the installer is per-user, with
`PrivilegesRequired=lowest`), with `"{app}\auris-studio.exe" "%1"` as the open command and the
`uninsdeletekey` flag.

### AN-07 — `auris render` replaces an existing WAV without `--force`

Rendering to a mix path that already exists replaces it silently. `compose` and `new` refuse an
existing destination unless given `--force`, stem export refuses existing stems, and the desktop
asks before replacing. Re-running the same command line after editing the output name by hand is
the way a delivered master gets lost.

**Direction.** Refuse an existing output unless `--force`, as `compose` does.

### AN-08 — Two log-panel tests race on the shared log book (fixed)

`the_log_can_be_copied_whole_and_line_by_line` and
`a_narrow_log_keeps_severity_source_and_message_readable` both clear `logbook::book()`, which is
one per process, push entries and read them back. Run in parallel, one clears the other's
entries in between. The first full run passed by scheduling luck; the second failed both, and
`cargo test -p auris-gpui --bins log_panel` failed every one of five tries, while
`--test-threads=1` passed.

**Fix.** Both hold one `ExclusiveLog` guard for their whole length, which clears the book on
taking and on dropping. Six parallel runs in a row then passed.

## Covered without findings

These held up: the update check and download (HTTPS only, no redirects for metadata, a pinned
asset URL and name, size and SHA-256 verified on download and again by the helper, a per-user
installer); the OpenCode and Antigravity permission configuration (deny by default, native tools
ask, nothing persistent is granted); the Windows lookup of native CLI executables, including
Codex's npm payload behind `codex.cmd`; Claude Code's read confinement on Windows (the root is
canonicalised, so `\\?\` paths compare); the agent's export guard and stem checks; the file jobs'
UNC refusal; the workshop library's copy and lock handling; junction and reparse-point handling in
autosave and plugin discovery; video staging and `CREATE_NO_WINDOW` for ffmpeg; the output clock's
sequence lock; the audio opener thread's timeout.

## Test suite

`cargo test --workspace --no-fail-fast` on Windows 11 at `864d2ba6`, before any change: 5 004
passed, 0 failed, 56 ignored, across 57 test binaries and the doctests. The ffmpeg test ran
against the installed build and passed. None of the findings above was caught by it.

With the fixes: a second workspace run passed everything except the two AN-08 tests (5 005
passed, 2 failed, 56 ignored); after AN-08's fix, the desktop crate's binary passed in full
(1 230 passed, 0 failed, 14 ignored), for 5 007 passing in all. `cargo clippy --workspace
--all-targets` is clean, and `cargo doc` with warnings denied builds the three changed crates.
