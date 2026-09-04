# Review findings: Manifests, CI, tooling and top-level docs

Part of the [whole-repository adversarial review](README.md) of commit `52d1702`. 8 verified findings: 3 medium, 5 low.

Each entry survived an independent skeptic and an independent reproducer (and a tie-breaker when they disagreed); "executed reproduction" means the reproducer ran a test, a binary or a scratch program and observed the behaviour, "traced" means it followed the call path with concrete values.

| ID | Severity | Location | Finding |
|---|---|---|---|
| ✅ F-146 | medium | `docs/features.md:1270` | docs/features.md:1270 says 29 tools; auris-toolbox declares 30 pub mod tool modules, confirmed by both frontends' own count-assertion tests. |
| ✅ F-176 | medium | `.github/workflows/release.yml:230` | Release notes and README list only 2 of the 4 binaries (missing auris-mcp/auris-agent) actually packaged into every Windows and macOS release archive. |
| ✅ F-236 | medium | `tools/eval/aesthetics.py:129` | aesthetics.py keys per-file scores by bare filename stem, so same-named WAVs in different subdirectories silently overwrite each other in the […] |
| ✅ F-135 | low | `.github/workflows/release.yml:18` | release.yml grants contents:write to all four jobs via workflow-root permissions, though only publish's release-creation step needs it. |
| ✅ F-272 | low | `Cargo.toml:38` | anyhow and arc-swap are declared in [workspace.dependencies] (Cargo.toml:37-38) but unused by any crate since the first commit. |
| ✅ F-275 | low | `tools/fetch-soundfonts.sh:60` | curl calls in tools/fetch-soundfonts.sh and tools/fetch-dictionary.sh lack --max-time/--connect-timeout, so a stalled peer can hang the release CI job for up […] |
| ✅ F-290 | low | `tools/fetch-soundfonts.sh:47` | tools/fetch-soundfonts.sh:47 writes the license notice straight to its final path with no temp-file staging, unlike the font download three lines later. |
| ✅ F-426 | low | `tools/eval/aesthetics.py:113` | A typo'd .wav path bypasses collect_wavs's existence check, crashing aesthetics.py with a raw soundfile traceback and losing the run's persisted --json scores […] |

### ✅ F-146 · medium · docs/features.md:1270 says 29 tools; auris-toolbox declares 30 pub mod tool modules, confirmed by both frontends' own count-assertion tests.

`docs/features.md:1270` · spec-mismatch · confirmed (traced through the code; reported independently 1×)

**What a user sees.** A reader of docs/features.md (or the auris-mcp/auris-agent tool listing docs page) is told there are 29 tools identical at both model doors; counting the actually-wired tools (or reading the toolbox's own regression tests) shows 30, since compose_lyrics was added without the doc being updated. This misleads anyone auditing tool parity between the MCP and agent frontends, or writing external documentation/integrations against the stated count.

**Trigger.** Reading docs/features.md's Frontends section after `compose_lyrics` was added (any point from commit 91af5ab onward, including the current tree).

**Mechanism.** Line 1270 states "Twenty-nine tools in all, identical at both model doors." `crates/auris-toolbox/src/lib.rs` declares 30 `pub mod` tool modules (spec_reference through sing, including compose_lyrics, confirmed by `grep -c '^pub mod ' crates/auris-toolbox/src/lib.rs` = 30), all 30 of which are wired into both `crates/auris-mcp/src/main.rs` and the `session_tool!`/`text_tool!` macro list plus `armed()` builder in `crates/auris-agent/src/main.rs`. Both frontends carry regression tests that assert the count directly: `crates/auris-mcp/src/main.rs:508` `assert_eq!(served.len(), expected.len(), "thirty tools at this door")` and `crates/auris-agent/src/main.rs:1515` `assert_eq!(unique.len(), 30, "thirty tools, no name worn twice")`. Commit 91af5ab1636b2b83d9578c63d91ff14d4a9561f2, which added the 30th tool (`compose_lyrics`), says in its own message "thirty tools now, the count both door tests keep" but updated docs/features.md only with a new prose section, not the digit on line 1270 (which had been bumped from twenty-seven to twenty-nine by the prior commit 0aa93d6 that added […]

**Expected.** The line should read "Thirty tools in all, identical at both model doors," matching the 30 `pub mod` tool modules in auris-toolbox and the assertions in both auris-mcp's and auris-agent's own test suites.

**Fix direction.** Change docs/features.md:1270 from "Twenty-nine tools in all, identical at both model doors." to "Thirty tools in all, identical at both model doors." to match crates/auris-toolbox/src/lib.rs's 30 `pub mod` tool modules and the counts already asserted in auris-mcp/src/main.rs and auris-agent/src/main.rs's tests.

**Written rule it breaks.** auris-mcp and auris-agent both take the toolbox, which is what keeps the tool called `compose` identical at both doors (CLAUDE.md, Layout section) — the docs claim of "identical at both model doors" is undermined by the stale count sitting right next to it.

### ✅ F-176 · medium · Release notes and README list only 2 of the 4 binaries (missing auris-mcp/auris-agent) actually packaged into every Windows and macOS release archive.

`.github/workflows/release.yml:230` · spec-mismatch · confirmed (traced through the code; reported independently 1×)

**What a user sees.** A user reading a published GitHub release page, or README.md's Downloads section, is told the Windows archive contains only auris-studio.exe and auris.exe, and the macOS archive contains only Auris Studio.app and the auris CLI. In fact both archives also contain auris-mcp(.exe) and auris-agent(.exe) — every release ships all four binaries on both platforms. Someone looking to use the MCP server or the LLM-agent frontend (docs/features.md even instructs `claude mcp add auris -- ./target/release/auris-mcp`) would have no reason to suspect it's already sitting in the archive they downloaded, and might re-download, rebuild from source, or conclude the feature isn't distributed at all.

**Trigger.** Any tagged release (`git push --tags v*`) runs this workflow and publishes these notes verbatim via `gh release create --notes-file notes.md`; a user reads the published release page or README.md's Downloads section to learn what a download contains.

**Mechanism.** The publish job's auto-generated release notes template (lines 225-231) says the Windows archive holds only "`auris-studio.exe` and `auris.exe`" and the macOS archive holds only "`Auris Studio.app` and the `auris` command line tool". But the Windows build step at line 126 copies all four binaries into the same archive: `cp target/release/auris-studio.exe target/release/auris.exe target/release/auris-mcp.exe target/release/auris-agent.exe "$stage/"`, and the macOS assemble step (lines ~62-88) `lipo`s all four binaries (`auris-studio auris auris-mcp auris-agent`) into the staged archive too, with only auris-studio moved inside the .app bundle — auris-mcp and auris-agent ship alongside it in every macOS archive as well. README.md's Downloads section (line 59) makes the same undercount explicit: "both `.exe`s for Windows" quantifies exactly two, when four ship.

**Expected.** The notes table and README.md's Downloads section should list all four shipped binaries per platform (auris-studio(.exe)/Auris Studio.app, auris(.exe), auris-mcp(.exe), auris-agent(.exe)), matching what the same workflow's build steps actually package.

**Fix direction.** Update the release-notes PREAMBLE heredoc in .github/workflows/release.yml (~line 227-231) and README.md's Downloads section (~line 58-59) to list all four shipped binaries per archive: `auris-studio.exe`/`Auris Studio.app`, `auris(.exe)`, `auris-mcp(.exe)`, and `auris-agent(.exe)`, matching the actual `cp`/`lipo` build steps at lines 126 and ~68-74.

### ✅ F-236 · medium · aesthetics.py keys per-file scores by bare filename stem, so same-named WAVs in different subdirectories silently overwrite each other in the aggregate/baseline/JSON output.

`tools/eval/aesthetics.py:129` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Running `uv run tools/eval/aesthetics.py takes/` (or any directory tree) over a folder containing WAVs with the same filename in different subdirectories prints a correct per-file row for each file as it's scored, but the returned `scores` dict is keyed only by `wav.stem`, so a later file overwrites an earlier same-named one. The mean, the `--baseline` diff, and the `--json` output then reflect only the last file under that name — an earlier take's score is silently dropped with no warning, so a regression in the discarded take goes undetected.

**Trigger.** `uv run tools/eval/aesthetics.py takes/` where `takes/` contains, say, `session1/mix.wav` and `session2/mix.wav` (a realistic layout for 'a folder of takes', which is exactly the use case `collect_wavs`'s directory-walk branch exists for).

**Mechanism.** `collect_wavs` (lines 108-117) walks a directory recursively with `path.rglob("*.wav")`, which can return files from different subdirectories that share the same basename. `score()` keys its result dict purely by `wav.stem` (line 129 `scores: dict[str, dict[str, float]] = {}`, populated at what was originally line ~135 `scores[wav.stem] = {...}`), so a later file with the same stem as an earlier one overwrites its entry in `scores` even though both were scored and both had their row printed individually during the loop.

**Expected.** The dict should be keyed by something unique per input file (e.g. the path relative to the collection root, or the full path) rather than the bare stem, so two differently-located files with the same filename don't collide.

**Fix direction.** Key `scores` by the file's path relative to the scan root (or by the full resolved path) instead of `wav.stem`, falling back to the stem only for display; this keeps aggregation and `--baseline`/`--json` correct even when multiple scanned WAVs share a basename.

**Written rule it breaks.** CLAUDE.md: "Before and after touching a writer or an audio constant, run the two measuring instruments ... They are dev tooling only" — implying the measuring tool's numbers are trusted for regression detection; this defect makes those numbers silently wrong on directory input with duplicate basenames.

### ✅ F-135 · low · release.yml grants contents:write to all four jobs via workflow-root permissions, though only publish's release-creation step needs it.

`.github/workflows/release.yml:18` · security · confirmed (traced through the code; reported independently 2×)

**What a user sees.** No functional bug: releases still build and publish correctly. The consequence is purely a hardening gap — if a build dependency, cargo build script, or GitHub Action used in the macos/windows/linux jobs were ever compromised (a supply-chain attack), the malicious code would run with a repo-write-capable GITHUB_TOKEN instead of a read-only one, letting it push commits, create tags/releases, or otherwise write to the repository from a job that has no legitimate need to.

**Trigger.** Any tag push matching `v*` (or a manual `workflow_dispatch`) runs `cargo build --release` on all three platforms, executing build.rs scripts from every crate in Cargo.lock (ring, ort-sys, symphonia's codec crates, windows-sys, etc.) and two curl-based fetch scripts, all under a token with repository write access they have no reason to hold.

**Mechanism.** `permissions:\n  contents: write` (lines 17-18) is declared at the workflow level, not scoped to the `publish` job. GitHub Actions applies a workflow-level `permissions:` block to every job that doesn't override it, so `macos`, `windows` and `linux` all run with a `GITHUB_TOKEN` that can push commits, tags and branches, even though the only step that needs write access is `gh release create` inside `publish` (which the comment on line 17 itself says is the reason: "`gh release create` writes to the repository").

**Expected.** Least privilege: `permissions: contents: write` should be declared only on the `publish` job (or the default workflow permissions should stay read-only and `publish` alone should escalate), so the build jobs run with no repository write access.

**Fix direction.** Move `permissions: contents: write` out of the workflow root and into a job-level `permissions:` block on the `publish` job only (the sole job whose "Create the release" step uses `GH_TOKEN`); leave the workflow-level default absent (or explicitly `contents: read`) so `macos`, `windows`, and `linux` get a read-only token.

**Written rule it breaks.** None in CLAUDE.md directly, but the workflow's own comment states the rationale narrowly: "`gh release create` writes to the repository." (release.yml:17) — the grant is scoped in intent to one step but applied workflow-wide, violating least-privilege/defense-in-depth practice implied by that comment.

### ✅ F-272 · low · anyhow and arc-swap are declared in [workspace.dependencies] (Cargo.toml:37-38) but unused by any crate since the first commit.

`Cargo.toml:38` · other · confirmed (traced through the code; reported independently 1×)

**What a user sees.** No user-visible behavior changes; this only adds two crates' worth of unnecessary compilation time and Cargo.lock entries to every build, with no runtime effect.

**Trigger.** Building any of these crates compiles and links the unused dependency (including serde_derive's proc-macro machinery for the two `serde` cases) with zero functional benefit — a plain `cargo build -p auris-dsp` or `-p auris-cli` demonstrates it.

**Mechanism.** `anyhow` (Cargo.toml:37) and `arc-swap` (Cargo.toml:38) are declared in `[workspace.dependencies]` but no crate's Cargo.toml references either one (`grep -rln '"anyhow"|"arc-swap"' crates/*/Cargo.toml` and `grep -rn '^anyhow\.workspace\|^arc-swap\.workspace' crates/*/Cargo.toml` both return nothing) — every other workspace dependency in this file carries an explanatory comment justifying its presence; these two have none. Separately, `serde` is declared in `crates/auris-dsp/Cargo.toml:12` and `crates/auris-synth/Cargo.toml:13` but neither crate's `src/` contains `serde::`, `Serialize` or `Deserialize` anywhere. And `log` is declared in `crates/auris-cli/Cargo.toml:18`, `crates/auris-mcp/Cargo.toml:22` and `crates/auris-agent/Cargo.toml:22` but none of the three call any `log::` macro — only `env_logger::Builder::...::init()` is used, which does not require the `log` facade as a direct dependency. None of this trips CI because `#![warn(missing_docs)]` is the only crate-level lint enabled (grepped across every `src/lib.rs`); `unused_crate_dependencies` is not, so `cargo clippy […]

**Expected.** A dependency listed in a crate's `[dependencies]` should be used by that crate's code, or removed; workspace-level dependencies should either be referenced by at least one crate or dropped, matching the project's own practice of commenting every dependency that stays.

**Fix direction.** Remove the `anyhow = \"1.0\"` and `arc-swap = \"1.7\"` lines from `[workspace.dependencies]` in Cargo.toml (lines 37-38), or add a short comment explaining an intended future use if one is planned; re-run `cargo build` to confirm Cargo.lock updates cleanly.

### ✅ F-275 · low · curl calls in tools/fetch-soundfonts.sh and tools/fetch-dictionary.sh lack --max-time/--connect-timeout, so a stalled peer can hang the release CI job for up to 360 minutes.

`tools/fetch-soundfonts.sh:60` · other · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** A release build (or a local run of these scripts) can hang indefinitely if a SoundFont mirror or dictionary host accepts the TCP connection but never sends data — the CI job just sits there consuming a runner for up to GitHub's 360-minute default before it is killed, rather than failing fast with a clear network error. No end user of the DAW is affected; this only touches the release-asset preparation step.

**Trigger.** The URL host stops responding mid-download (accepts the TCP connection, sends no more bytes) during any of `release.yml`'s three fetch steps (macOS, Windows, Linux jobs each call `tools/fetch-soundfonts.sh` and `tools/fetch-dictionary.sh`) or during a developer's local `tools/fetch-soundfonts.sh` after cloning.

**Mechanism.** Every `curl` invocation in both fetch scripts (`tools/fetch-soundfonts.sh:47,60` and `tools/fetch-dictionary.sh:48,60`) passes `--fail --location --show-error --silent` but no `--max-time` or `--connect-timeout`. curl's own defaults impose no overall time limit, so a stalled TCP connection (a dead mirror, a network partition between the runner and GitHub's release-asset CDN, or a MITM box that accepts the connection and never responds) blocks the script forever instead of failing fast.

**Expected.** Pass `--max-time`/`--connect-timeout` (or an equivalent `curl --retry` with a bounded total time) so a stalled download fails quickly with the same clear error the scripts already produce for a hash mismatch, rather than hanging until the runner's own timeout fires.

**Fix direction.** Add `--max-time <n> --connect-timeout <n>` (and optionally `--retry`) to the four curl invocations in tools/fetch-soundfonts.sh and tools/fetch-dictionary.sh, and/or set `timeout-minutes` on the release.yml jobs as a second line of defense.

### ✅ F-290 · low · tools/fetch-soundfonts.sh:47 writes the license notice straight to its final path with no temp-file staging, unlike the font download three lines later.

`tools/fetch-soundfonts.sh:47` · persistence · confirmed (traced through the code; reported independently 1×)

**What a user sees.** If the license-notice fetch is interrupted (network drop, Ctrl-C, disk full) partway through, a truncated or empty `_License.md` file is left at its permanent path inside `SoundFonts/`. Because the script only checks digests for the `.sfz`/font file, a rerun sees the font already present and correct and never re-fetches or re-validates the license file, so a broken license notice can silently ship in a release archive assembled from that directory.

**Trigger.** Run `tools/fetch-soundfonts.sh` (or `fetch-dictionary.sh`, same pattern at lines 48-49) with a network interruption during the license curl call — e.g. a flaky connection or a mid-download SIGINT.

**Mechanism.** The main SoundFont download is written to a `.part` file (line 59: `partial="$target.part"`), hashed, and only `mv -f`'d onto the final path once the digest matches (lines 62-69) — the exact discipline the script's own comment claims for 'an interrupted download'. The accompanying `_License.md`, three lines earlier (47-48), is `curl --output`'d straight to its final destination inside `$destination`, with no temp file and no digest of any kind. If that curl call is interrupted mid-transfer (network drop after headers, disk full, etc.) it exits non-zero and the truncated file it already wrote is left sitting at its permanent path.

**Expected.** The comment at lines 45-46 ("The notice travels with the font, always") treats the license file as something that must reliably accompany the shipped asset, the same standard applied to the asset itself three lines below — a temp-file-then-verify-then-rename pattern (or at minimum a non-empty/expected-content check) would give the license the same integrity guarantee the font already has.

**Fix direction.** Download the license notice to a `.part` temp path with `curl --output "$destination/${file%.*}_License.md.part" "$license_url"`, then `mv -f` it into place only after the curl exits 0 (no hash is available for license text, so success-based atomic rename is enough — no need to replicate the font's SHA check).

**Written rule it breaks.** A half-finished download left where the application looks would be found, loaded and refused by the parser, and the error would name a corrupt file rather than an interrupted one. (comment at tools/fetch-soundfonts.sh:56-58, describing the discipline applied to the font but not the license file)

### ✅ F-426 · low · A typo'd .wav path bypasses collect_wavs's existence check, crashing aesthetics.py with a raw soundfile traceback and losing the run's persisted --json scores instead of a clean error.

`tools/eval/aesthetics.py:113` · correctness · confirmed (traced through the code; reported independently 1×)

**What a user sees.** Running `uv run tools/eval/aesthetics.py --preset all` (or any invocation) with a typo'd or missing .wav path crashes with a raw `soundfile.LibsndfileError` traceback instead of the tool's own clean `not a wav or a folder of them` message. Because the `--json` file is only written after `score()` returns in full, scores for every already-scored WAV in that run (and, on a `--preset all` run, any rendered presets) are lost from the persisted output and the run must be repeated — though per-file scores already printed to stdout as they were computed are not lost.

**Trigger.** Run `aesthetics.py real1.wav real2.wav typo.wav` (or a `--preset all` run whose renders feed into the same list) where `typo.wav` does not exist. `score()` (called from `main()` at line 189) reaches `soundfile.read(wav, ...)` for `typo.wav` and raises an unhandled exception with a raw soundfile traceback instead of the module's own clean-error convention.

**Mechanism.** `collect_wavs` only branches on `path.is_dir()` (line 111) and `path.suffix.lower() == ".wav"` (line 113) before appending the path (line 114); it never calls `.exists()`/`.is_file()`. The `else: sys.exit(...)` clean-error branch (lines 115-116) therefore never fires for a nonexistent path that merely ends in `.wav`.

**Expected.** collect_wavs should treat a `.wav`-suffixed path that does not exist the same as any other bad input and route it through the existing `sys.exit(f"not a wav or a folder of them: {path}")` clean-error path.

**Fix direction.** In `collect_wavs` (tools/eval/aesthetics.py:107-117), add an existence check alongside the suffix check — e.g. `elif path.suffix.lower() == ".wav" and path.is_file():` — so a nonexistent `.wav`-suffixed path falls through to the existing `sys.exit(f"not a wav or a folder of them: {path}")` clean-error branch instead of reaching `soundfile.read` unchecked.
