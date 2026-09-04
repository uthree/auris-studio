# Review findings: auris-cli

Part of the [whole-repository adversarial review](README.md) of commit `52d1702`. 3 verified findings: 2 medium, 1 low.

Each entry survived an independent skeptic and an independent reproducer (and a tie-breaker when they disagreed); "executed reproduction" means the reproducer ran a test, a binary or a scratch program and observed the behaviour, "traced" means it followed the call path with concrete values.

| ID | Severity | Location | Finding |
|---|---|---|---|
| F-145 | medium | `crates/auris-cli/src/main.rs:538` | In auris-cli/src/main.rs, `sing`'s doc comment is fused with `collect`'s, so `sing`'s rendered docs open by describing SoundFont collection, not singing. |
| F-167 | medium | `crates/auris-cli/src/main.rs:425` | CLI `compose` silently drops --preset when a spec file is also given, unlike auris-toolbox's resolve_spec which rejects the combination outright. |
| F-181 | low | `crates/auris-cli/src/main.rs:243` | crates/auris-cli/src/main.rs:243 pads a kept progression's name with `{:<15}` instead of the file's own CJK-width-aware `pad()`, misaligning the table for […] |

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
