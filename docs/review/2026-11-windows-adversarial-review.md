# Adversarial review on Windows, November 2026

A review of `main` at `864d2ba6` (2026-11-01), run on Windows 11 with the machine's real
devices, files and installed tools. Findings are numbered `AN-nn`; cite the number from the commit
that fixes one, and delete its entry once the fix lands. Line numbers drift; the descriptions name
functions instead.

A review on a Mac ran at the same time (`MR-nn`, `2026-11-adversarial-review.md`). Two of its
findings were also found here and are not repeated: MR-08 is AN-03 below, whose fix cites both,
and the log-panel test race (MR-14) failed here too, on every parallel run of
`cargo test -p auris-gpui --bins log_panel`, and is left to its own fix. The October review
(`AR-nn`) still has AR-08 open.

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

Each finding's heading says whether it is open; a fix's pull request changes only its own
heading, so the fixes can land in any order.

| ID | Severity | Area | Finding |
|----|----------|------|---------|
| AN-01 | Medium | Updates | Applied installers and helper copies are never deleted |
| AN-02 | Low | Security | An approved audio URL can redirect to loopback, the LAN or plain HTTP |
| AN-03 | Low | Desktop | Every window opens saying there is no audio output |
| AN-04 | Low | Privacy | Issue-report redaction misses other spellings of a Windows path |
| AN-05 | Low | Security | The audio-file search lists any folder without asking |
| AN-06 | Low | Windows | The installer does not associate `.auris` files with the application |
| AN-07 | Low | CLI | `auris render` replaces an existing WAV without `--force` |

## Findings

### AN-01 — Applied installers and helper copies are never deleted (open)

`download_update` keeps the installer in `<config>/updates`, and `spawn_update_helper` copies the
helper there as `auris-update-helper-<pid>.exe`. Nothing ever removes either. On the review
machine the folder held **540 MB**: eight installers from v0.11.0 to v0.17.0, about 70 MB each, and
nine helper copies. It grows by one installer per release.

**Direction.** Once per process, delete installers for versions no newer than the running one and
helper copies belonging to other processes. Keep a newer, not yet applied installer, because
`download_update` reuses one that still verifies.

### AN-02 — An approved audio URL can redirect to loopback, the LAN or plain HTTP (open)

`audio_assets` `import_url` asks for approval of one URL, then `download` follows up to five
redirects with no check beyond `audio_url`'s scheme and credential rules. A public HTTPS address
the person approved could therefore send the request on to `http://192.168.1.1/…`,
`169.254.169.254` or a service on `127.0.0.1`, none of which they were asked about. The response
only lands in the project if it decodes as audio, so this is a blind request rather than a read,
but the VOICEVOX client and the audio reviewer already refuse to reach non-loopback services
automatically, and this was the one path that let a remote server pick the destination.

**Direction.** Follow a redirect within the approved origin; to another origin, only over HTTPS
and only to a host that is not `localhost` or a loopback, private, link-local, CGNAT or
unspecified address.

### AN-03 — Every window opens saying there is no audio output (open)

The same finding as MR-08. A desktop window starts with its audio suspended and claims the device
on the first explicit audio command (`acquire_audio`), so that a second project window does not
take the output from the first. `audio_line` only looks at whether a stream is running, so every
launch reports "No audio output — editing and export still work" on a machine with working
speakers. It was in the log of the run above, which is how it was found.

**Direction.** Pass `audio_line` whether the session is suspended, and say that output opens on
Play in that case.

### AN-04 — Issue-report redaction misses other spellings of a Windows path (open)

`issue_report::redact` replaces `USERPROFILE`, `HOME`, the project folder and the configuration
folder as typed, with `/` and doubled `\\` variants. Windows paths have more spellings than that.
A user name longer than eight characters, or one with a space, makes `TEMP` an 8.3 short path
(`C:\Users\ALEXAN~1\AppData\Local\Temp`), and `std::env::temp_dir` and every `tempfile` error
message then carry a spelling that does not match. Paths are also case-insensitive, and
`c:\users\alice` survives redaction. The review machine's user name is short, so this was
reasoned rather than reproduced.

**Direction.** On Windows, compare case-insensitively and add each private path's
`GetShortPathNameW` spelling to the list.

### AN-05 — The audio-file search lists any folder without asking (fixed)

`audio_assets` `search` takes any absolute `root` and is not marked `requires_confirmation`, so
the agent can list the names and sizes of audio files anywhere the user can read, eight levels
deep, without a prompt. A native CLI read outside the project does ask (`cli_file_read` with
`within_project` false). Only audio file names are returned, which keeps this low.

**Fix.** `Operation::requires_confirmation` is true for a `search` with a `root`, so the agent panel asks
before listing a folder the model names. The default search of the Music and Downloads folders
still runs without asking. The project folder is not exempted: the rule has no project context,
and the project's own audio is already in the project.

### AN-06 — The installer does not associate `.auris` files with the application (open)

`main` opens a project passed as the first argument (`project_argument`), and the macOS bundle
declares the document type, but `tools/installer/auris-studio.iss` has no `[Registry]` section.
On Windows, double-clicking `MySong.auris` asks which program to use.

**Direction.** Register `.auris` under `HKCU\Software\Classes` (the installer is per-user, with
`PrivilegesRequired=lowest`), with `"{app}\auris-studio.exe" "%1"` as the open command and the
`uninsdeletekey` flag.

### AN-07 — `auris render` replaces an existing WAV without `--force` (open)

Rendering to a mix path that already exists replaces it silently. `compose` and `new` refuse an
existing destination unless given `--force`, stem export refuses existing stems, and the desktop
asks before replacing. Re-running the same command line after editing the output name by hand is
the way a delivered master gets lost.

**Direction.** Refuse an existing output unless `--force`, as `compose` does.

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

`cargo test --workspace --no-fail-fast` on Windows 11 at `864d2ba6`: 5 004 passed, 0 failed,
56 ignored, across 57 test binaries and the doctests. The ffmpeg test ran against the installed
build and passed. A second run failed the two log-panel tests of MR-14, which is a race and not a
platform fault. None of the findings above was caught by the suite.
