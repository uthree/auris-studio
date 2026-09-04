# Review findings: auris-cli

Part of the [whole-repository adversarial review](README.md) of commit `52d1702`. 6 verified findings: 2 medium, 4 low.

Each entry survived an independent skeptic and an independent reproducer (and a tie-breaker when they disagreed); "executed reproduction" means the reproducer ran a test, a binary or a scratch program and observed the behaviour, "traced" means it followed the call path with concrete values.

| ID | Severity | Location | Finding |
|---|---|---|---|
| F-145 | medium | `crates/auris-cli/src/main.rs:538` | In auris-cli/src/main.rs, `sing`'s doc comment is fused with `collect`'s, so `sing`'s rendered docs open by describing SoundFont collection, not singing. |
| F-167 | medium | `crates/auris-cli/src/main.rs:425` | CLI `compose` silently drops --preset when a spec file is also given, unlike auris-toolbox's resolve_spec which rejects the combination outright. |
| F-181 | low | `crates/auris-cli/src/main.rs:243` | crates/auris-cli/src/main.rs:243 pads a kept progression's name with `{:<15}` instead of the file's own CJK-width-aware `pad()`, misaligning the table for […] |
| F-422 | low | `crates/auris-cli/src/main.rs:366` | `auris compose --preset --force` (flag right after --preset) is parsed as preset name "--force", producing a confusing unknown-preset error instead of enabling […] |
| F-429 | low | `crates/auris-cli/src/main.rs:1212` | auris-cli's --bpm parser lacks the finite/positive filter that the --sample-rate parser right beside it applies, so bad --bpm values are silently […] |
| F-444 | low | `crates/auris-cli/src/main.rs:867` | sing-frames prints a failure and skips the success line if the optional --report write fails, even though the WAV was already fully rendered and saved. |

### F-145 · medium · In auris-cli/src/main.rs, `sing`'s doc comment is fused with `collect`'s, so `sing`'s rendered docs open by describing SoundFont collection, not singing.

`crates/auris-cli/src/main.rs:538` · spec-mismatch · confirmed (traced through the code; reported independently 1×)

**What a user sees.** Anyone reading `cargo doc` output or the source for `auris-cli::sing` sees a doc comment that opens by describing asset/SoundFont collection ("Copies every file a project refers to into its folder... what this adds is the SoundFonts...") before abruptly switching mid-paragraph to describing singing. It misdescribes what the `sing` CLI subcommand does and, since `fn collect` immediately above presumably lost its closing lines, leaves `collect` itself under-documented or wrongly documented too.

**Trigger.** Read the doc comment immediately above `fn sing(args: &[String])` at line 547 — the merged text (rustdoc would render lines 538-546 as one comment) opens by describing `sing` as the tool that 'Copies every file a project refers to into its folder, and saves it.'

**Mechanism.** Lines 538-542 are collect()'s doc comment ('Copies every file a project refers to into its folder, and saves it. ... what this adds is the SoundFonts...'). Commit e43d5c0 ('Let the song be sung from anywhere') inserted the whole `sing` function directly after this doc block instead of after `fn collect`, appending its own two-line doc (lines 543-546, 'Renders a singer track...') onto the end of collect's comment with no blank line between them, and left the real `fn collect` (now at line 882) with no doc comment of its own at all. Verified via `git show e43d5c0^:crates/auris-cli/src/main.rs` which shows collect's doc directly above `fn collect` before the change.

**Expected.** The five-line 'Copies every file...' doc comment belongs directly above `fn collect` (line 882); `sing`'s doc comment should be only the two lines actually about singing (lines 543-546).

**Fix direction.** Split the merged block: end `collect`'s doc comment after its own description (the "Copies every file..." / SoundFonts paragraph) directly above `fn collect`, and start `sing`'s doc comment fresh with only the singer-track/voice-model content ("Renders a singer track through its voice model and keeps the take in the project." plus the save-afterwards rationale) directly above `fn sing`, separated by a blank (non-comment) line so rustdoc does not merge them.

**Written rule it breaks.** Every public item carries a doc comment (`#![warn(missing_docs)]` is on in each crate).

### F-167 · medium · CLI `compose` silently drops --preset when a spec file is also given, unlike auris-toolbox's resolve_spec which rejects the combination outright.

`crates/auris-cli/src/main.rs:425` · spec-mismatch · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Running `auris compose examples/hello.asong --preset jazz-trio --print` (or any compose invocation naming both a spec file and a preset) silently composes from the file and drops the --preset value with no error or warning — the user sees a piece written from the file, believing the preset was applied, with no message telling them it was ignored.

**Trigger.** Run `auris compose song.asong --preset lofi`: a project on disk (song.asong) and a shipped preset name are both given.

**Mechanism.** compose()'s own doc comment (lines 353-357) says the spec is 'a file, or — with `--preset` and no file — one of the styles the composer ships with', i.e. the two are meant to be mutually exclusive. The parser computes `named` by scanning the whole argument list for `--preset` regardless of whether a file was also given (lines 363-366), then at lines 425-437 the match `(&source, named)` takes the `(Some(path), _)` arm whenever a file is present — the `_` silently swallows a `Some(name)` preset with no warning or error. The identical rule is implemented a second time in auris-toolbox (used by the MCP and agent doors), where `resolve_spec` at crates/auris-toolbox/src/lib.rs:2552-2557 explicitly refuses with "pass either `spec` or `preset`, not both" when both are set.

**Expected.** Either refuse the combination the way `auris_toolbox::resolve_spec` does ('pass either spec or preset, not both'), or update the doc comment to document that a file silently wins — the current code matches neither its own doc comment nor its sibling implementation.

**Fix direction.** In compose() in crates/auris-cli/src/main.rs, add an explicit guard beside the existing `source.is_none() && named.is_none()` check that returns an error when both source and named are Some, mirroring auris-toolbox::resolve_spec's `(Some(_), Some(_)) => Err("pass either spec or preset, not both")`, so the CLI and the MCP/agent doors reject the same ambiguous input identically.

**Written rule it breaks.** auris-mcp and auris-agent both take the toolbox, which is what keeps the tool called `compose` identical at both doors (CLAUDE.md, Layout section); and the doc comment at main.rs:355-357: "The specification is a file, or — with `--preset` and no file — one of the styles the composer ships with."

### F-181 · low · crates/auris-cli/src/main.rs:243 pads a kept progression's name with `{:<15}` instead of the file's own CJK-width-aware `pad()`, misaligning the table for wide-character names.

`crates/auris-cli/src/main.rs:243` · ui · confirmed (traced through the code; reported independently 1×)

**What a user sees.** Running `auris info` in a project that has a user-kept chord progression with a wide (CJK/fullwidth) name gets a misaligned "kept progressions" table — the chart column shifts left or the row runs long — while every other name column in the same command (presets, SoundFonts, dictionaries, track names) lines up correctly. Purely a cosmetic misalignment in a terminal listing; no data is lost or misread.

**Trigger.** A user (directly, or an LLM through the `teach_progression` MCP/agent tool) keeps a progression under a wide-character name, e.g. `teach_progression(name="王道進行", chords="I-V-vi-IV")`, then runs `auris progressions` (or the MCP/agent `list_progressions` tool). `entry.name` = "王道進行" is 4 characters / 8 terminal columns; `{:<15}` treats it as width 4 and appends 11 spaces (15 characters total), landing the chart column 4 columns short of where every ASCII-named row's chart column lands.

**Mechanism.** Line 243 is `writeln!(out, "  {:<15} {}", entry.name, entry.chart)?;` — it uses Rust's built-in `{:<15}` width specifier directly on a user-supplied name. This file's own `display_width` doc comment (lines 76-84) explains exactly why that is wrong: "`{:<12}` pads by counting *characters*, which lines a table up in English and ruins it the moment a column holds anything wider" — i.e. a CJK/Hangul/fullwidth character occupies two terminal columns but counts as one toward Rust's padding width. The file built `display_width()`/`pad()` (lines 85-109) to fix exactly this, and uses `pad()` for every other piece of user-authored text in this same file: `pad(preset.name, 13)` (262), `pad(font.name, 22)` (309), `pad(dictionary.name, 22)` (346), `pad(&track.name, 18)` (979). Line 243 — the loop over `book.entries()`, i.e. progressions a user has *kept* under their own name via `teach_progression` — was left out of that treatment. `ProgressionBook::keep` (crates/auris-session/src/progressions.rs:148-163) places no ASCII restriction on the name (only trims it and rejects […]

**Expected.** Line 243 should format the name through `pad(&entry.name, 15)` (mirroring every other name column in this file) instead of the raw `{:<15}` specifier.

**Fix direction.** Change line 243 from `writeln!(out, "  {:<15} {}", entry.name, entry.chart)?;` to use the file's own `pad()` helper: `writeln!(out, "  {} {}", pad(entry.name, 15), entry.chart)?;` — matching the pattern already used for `preset.name`, `font.name`, `dictionary.name`, and `track.name` elsewhere in the same file.

**Written rule it breaks.** `display_width`'s own doc comment: "`{:<12}` pads by counting *characters*, which lines a table up in English and ruins it the moment a column holds anything wider."

### F-422 · low · `auris compose --preset --force` (flag right after --preset) is parsed as preset name "--force", producing a confusing unknown-preset error instead of enabling --force.

`crates/auris-cli/src/main.rs:366` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Running `auris compose --preset --force` (flag placed right after --preset) silently consumes "--force" as the preset name instead of enabling the force flag, so the command fails with a confusing "unknown preset: --force" error rather than either working or reporting a missing preset name.

**Trigger.** `auris compose --preset --force` (or any invocation where `--preset` is immediately followed by another flag with no preset name in between).

**Mechanism.** `named` is computed by `args.iter().position(|arg| arg == "--preset").and_then(|at| args.get(at + 1))` (lines 363-366) — it takes the literal next token with no check that it isn't itself a flag. The later parsing loop's `"--preset" => index += 1` arm (line 417) then skips exactly that one token as "already consumed", so if the token immediately following `--preset` starts with `-`, it is taken as the preset name and never reconsidered as its own flag.

**Expected.** A token starting with `-` immediately after `--preset` should be treated as a missing-value error for `--preset` (the way `--output`, `--set`, etc. already reject a missing following value elsewhere in this same function), not silently accepted as the preset name.

**Fix direction.** In the `named` computation, filter the token after `--preset` with the same `!arg.starts_with('-')` guard used for `source`, and if it's absent or starts with `-`, raise the existing "option needs value" error instead of accepting an arbitrary flag as the preset name.

### F-429 · low · auris-cli's --bpm parser lacks the finite/positive filter that the --sample-rate parser right beside it applies, so bad --bpm values are silently clamped/defaulted instead of rejected.

`crates/auris-cli/src/main.rs:1212` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Running `auris new Song --bpm nan` (or `--bpm inf`/`--bpm -50`) does not produce the CLI's usual invalid-argument error; the command silently succeeds and reports a fabricated tempo (120 BPM for non-finite input, or a value silently clamped into [10, 999] for out-of-range input) instead of telling the user their --bpm value was rejected.

**Trigger.** `auris new Song --bpm nan` (or `--bpm inf`, or `--bpm -50`).

**Mechanism.** The `--bpm` arm (lines 1208-1220) parses with `args.get(index).and_then(|value| value.parse().ok())` and no further filter, so `--bpm nan`, `--bpm inf` or `--bpm -50` all parse successfully as `f64`. The `--sample-rate` arm 13 lines below it explicitly adds `.filter(|rate: &f64| rate.is_finite() && *rate > 0.0)` with a comment explaining exactly why (`parse::<f64>` accepts `inf`/`NaN`/overflowing literals). `session.set_bpm(bpm)` eventually reaches `TempoMap::clamp_bpm` (auris-core/src/time.rs:789-795), which silently maps non-finite input to 120.0 and out-of-range input to [10, 999] with no error surfaced.

**Expected.** `--bpm` should reject non-finite or non-positive input the same way `--sample-rate` does, for the same stated reason, rather than relying entirely on the downstream clamp to silently substitute a different value.

**Fix direction.** Add the same `.filter(|bpm: &f64| bpm.is_finite() && *bpm > 0.0)` (or reuse TempoMap::MIN_BPM/MAX_BPM bounds) to the --bpm parsing arm at crates/auris-cli/src/main.rs:1208-1220 that --sample-rate already applies 13 lines below, so bad --bpm values hit the existing `.ok_or_else` error path instead of silently reaching Session::set_bpm's clamp_bpm substitution.

### F-444 · low · sing-frames prints a failure and skips the success line if the optional --report write fails, even though the WAV was already fully rendered and saved.

`crates/auris-cli/src/main.rs:867` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Running `auris sing-frames ... --report <path>` where the report path can't be written (missing parent directory, bad permissions) causes the CLI to exit with an error message and no success output, even though the WAV audio render completed and was fully written to disk beforehand. A user or script relying on the printed success line (or exit code) to know the render succeeded is misled into thinking the whole command failed, when only the optional side-artifact (the JSON report) failed.

**Trigger.** `auris sing-frames frames.json --voice v.onnx --report /nonexistent-dir/report.json` run directly at a shell (not through `training/src/auris_singer/host.py`, which always creates the report's parent directory first).

**Mechanism.** `sing_frames` calls `session.sing_frames(...)` which writes the WAV to `output` and returns `sung` (line 862-864); only afterward, if `--report` was given, does it `std::fs::write(&report, text)` and propagate any error with `?` (lines 865-868). Because that `?` is inside the same `fn sing_frames(...) -> Result<(), String>` whose `Err` makes `main` return `ExitCode::FAILURE`, a report-write failure (bad path, missing parent directory, permissions) turns an already-successful render into a reported command failure, and the success message (`frames_sung`, lines 869-878) is never printed even though the WAV on disk is complete and correct.

**Expected.** A `--report` write failure should be reported as a warning (the way missing-audio and foreign-build notices already are, via `warned`) without turning a successful render into a failed command — or the report should be written before the success message is suppressed by its own failure.

**Fix direction.** Print the success message (`messages::frames_sung`) before attempting the `--report` write, or write the report first and treat its failure as a non-fatal warning printed to stderr rather than propagating it via `?`, so a report-write failure never masks or is mistaken for an audio-render failure.
