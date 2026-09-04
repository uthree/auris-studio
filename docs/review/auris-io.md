# Review findings: auris-io

Part of the [whole-repository adversarial review](README.md) of commit `52d1702`. 5 verified findings: 2 critical, 2 medium, 1 low.

Each entry survived an independent skeptic and an independent reproducer (and a tie-breaker when they disagreed); "executed reproduction" means the reproducer ran a test, a binary or a scratch program and observed the behaviour, "traced" means it followed the call path with concrete values.

| ID | Severity | Location | Finding |
|---|---|---|---|
| ✅ F-022 | critical | `crates/auris-io/src/soundfont.rs:104` | check_chunk's LIST-descent doesn't clamp to the enclosing LIST's end, and its generic leaf handler mis-tracks ifil/iver's true byte length, letting a crafted […] |
| ✅ F-313 | critical | `crates/auris-io/src/soundfont.rs:62` | load_soundfont has no catch_unwind around SoundFont::new, so a malformed .sf2 with honest chunk sizes but bad pdta indices panics rustysynth and crashes the […] |
| ✅ F-105 | medium | `crates/auris-io/src/midi.rs:467` | MIDI export silently masks (not errors on) tick deltas over 2^28-1, corrupting event positions past ~38.8 hours with no warning, contradicting auris-io's […] |
| ✅ F-229 | medium | `crates/auris-io/src/midi.rs:272` | A TrackName meta event placed after a channel's last MIDI event is silently dropped, so the imported part falls back to "Channel N" instead of its real name. |
| ✅ F-302 | low | `crates/auris-io/src/project_file.rs:106` | folder_is_named's ASCII-only case fold misses non-ASCII cased letters, so renaming a project folder by accented-letter case alone nests a duplicate copy […] |

### ✅ F-022 · critical · check_chunk's LIST-descent doesn't clamp to the enclosing LIST's end, and its generic leaf handler mis-tracks ifil/iver's true byte length, letting a crafted .sf2 desync the pre-parse validator and reach the smpl overflow it exists to block.

`crates/auris-io/src/soundfont.rs:104` · security · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Opening a project that references a SoundFont, or importing an .sf2 through the instrument browser, can abort the whole Auris Studio process (a multi-gigabyte allocation from a lied-about size) or trigger undefined behaviour (an odd-length `smpl` chunk read as a slice of 16-bit samples one byte past the allocation) even though `load_soundfont` is specifically supposed to refuse such files before they reach the parser.

**Trigger.** A `.sf2` file: RIFF/size/"sfbk" (12 bytes), then a LIST "INFO" of declared size 16 containing one child `ifil` whose 8-byte header declares size 10000 (but is followed by only the real 4-byte version struct, so INFO's own 16-byte total stays internally honest), then padding out to at least ~10 KB, then the true LIST "sdta" containing a `smpl` chunk whose declared size is odd (e.g. 5), then a minimal valid LIST "pdta". Loading this file through `load_soundfont` (called whenever a project opens or a user/agent imports a SoundFont).

**Mechanism.** `check_chunk` bounds every chunk's declared `size` against `held = bytes.len() - body` (line 104) — distance to the END OF THE WHOLE FILE — rather than against the boundary of the LIST it is actually inside. When a LIST chunk is walked (lines 114-123), the position returned to the caller is `Ok(child)` — wherever the last child's check happened to land — not `Ok(end)`, the LIST's own validated boundary. For a chunk type where the real (forked) parser reads a size-independent FIXED number of bytes, this lets the checker's assumed cursor diverge from the real parser's actual cursor. `ifil`/`iver` are exactly such a type: `SoundFontInfo::new` (vendor/rustysynth/src/soundfont_info.rs:63,70) dispatches them to `SoundFontVersion::new`, which reads a fixed 4 bytes (vendor/rustysynth/src/soundfont_version.rs:21-26) and completely ignores the chunk's declared `size` field. So: craft an INFO LIST whose own declared size is honest and small (say 16 = 4 for "INFO" + 8 for the ifil header + 4 for the real version struct) but whose `ifil` child's own header lies about its size (say 10000). […]

**Expected.** The module's own doc comment (crates/auris-io/src/soundfont.rs:66-76) states the walk exists so that 'What must not reach it is a *plausible* file whose sizes lie' and that both the huge-allocation and the odd-`smpl` UB cases 'stop here'. A correct walk would bound each chunk's size against the boundary of the LIST that actually contains it (the `end` computed for that LIST), not against the whole file, and would use the position implied by that LIST's own validated size for the next sibling […]

**Fix direction.** In `check_chunk`'s LIST-descent branch (crates/auris-io/src/soundfont.rs:114-123), clamp each child's returned cursor to `end` (or reject with an error if a child's checked position exceeds `end`) instead of returning whatever the last child call produced; that alone still leaves the `ifil`/`iver` mismatch, so also special-case those two IDs to know their real consumed length (8 bytes: two u16s) rather than trusting the declared `size` field, mirroring `SoundFontVersion::new`'s actual read.

**Written rule it breaks.** What must not reach it is a *plausible* file whose sizes lie. / stricter, and it would refuse fonts the parser plays; looser, and a lying size gets through.

### ✅ F-313 · critical · load_soundfont has no catch_unwind around SoundFont::new, so a malformed .sf2 with honest chunk sizes but bad pdta indices panics rustysynth and crashes the whole app on project open.

`crates/auris-io/src/soundfont.rs:62` · security · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Opening a project that references a corrupted or malicious .sf2 file (or importing one directly) crashes the entire Auris Studio application with no error dialog — the user loses their whole session, not just the one track using that soundfont.

**Trigger.** Open a project (or import/relocate a SoundFont) whose referenced .sf2 has a well-formed RIFF/chunk-size structure (so check_chunks passes) but a pdta bag/instrument table with an out-of-range zone or generator index — e.g. a corrupted download, a hand-edited font, or one truncated mid-pdta.

**Mechanism.** check_chunks (soundfont.rs:77-126) validates exactly two things — a chunk's declared size against remaining file bytes, and smpl parity — then `SoundFont::new(&mut Cursor::new(bytes))` (line 62) is called with no `std::panic::catch_unwind` around it. A workspace-wide grep for `catch_unwind` finds exactly one call site (crates/auris-sampler/src/sampler.rs:803, wrapping `synth.render`, unrelated to loading), so nothing between the vendored parser and the top-level caller catches a panic raised while parsing. The vendored parser has several unchecked-index panics that check_chunks does nothing to prevent: `vendor/rustysynth/src/instrument.rs:33` (`&zones[span_start..span_end]` on an out-of-range `zone_start_index`, already reported for this unit) and `vendor/rustysynth/src/zone.rs:25` (`generators[(info.generator_index + i) as usize]` on an out-of-range bag generator_index, reported by the sibling `vendor-fork` unit) are both reachable on a file that already passes check_chunks' two checks.

**Expected.** Per soundfont.rs's own stated purpose ('The whole file is read first and its chunk tree walked before the parser is allowed to believe it') and assets.rs's documented design ('nothing here is fatal — a font that cannot be found costs one track its sound and the project still opens'), a structurally invalid SoundFont should surface as a `SessionError`, not take down the process.

**Fix direction.** Wrap the `SoundFont::new(&mut Cursor::new(bytes))` call in `crates/auris-io/src/soundfont.rs:62` with `std::panic::catch_unwind` and turn a caught panic into `IoError::Decode`, the same way the crate already turns rustysynth's `Result::Err` into that error; alternatively extend `check_chunks` to bound-check the pdta zone/generator index tables before `SoundFont::new` is ever called.

**Written rule it breaks.** The whole file is read first and its chunk tree walked before the parser is allowed to believe it — see `check_chunks` below for what the parser would otherwise do with a size field that lies.

### ✅ F-105 · medium · MIDI export silently masks (not errors on) tick deltas over 2^28-1, corrupting event positions past ~38.8 hours with no warning, contradicting auris-io's no-silent-truncation policy.

`crates/auris-io/src/midi.rs:467` · persistence · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** If a track has an event whose gap from the previous event exceeds 268,435,455 ticks (roughly 38.8 hours of song at the crate's 960-ticks-per-quarter division — an extremely long project or a huge gap between notes), the exported .mid file silently places that event, and every later event in the same track chunk, at the wrong tick position with no error, warning, or any indication the export is wrong.

**Trigger.** Call `write_midi_file`/`write_midi_bytes` on a `Project` containing an instrument track with a note (or clip) whose absolute `Ticks` position is, e.g., `Ticks(300_000_000)` — about 86.8 hours into the song at 120 BPM given `TICKS_PER_QUARTER = 960` (crates/auris-core/src/time.rs:18), a distance well past the 38.8-hour (268,435,455-tick) ceiling the u28 field can carry as a single delta. `u28::new(300_000_000)` masks to `300_000_000 & 0x0FFF_FFFF = 31_564_544`.

**Mechanism.** In `delta_encode` (crates/auris-io/src/midi.rs:452-479), `let delta = (at - previous).raw().max(0) as u32;` (line 467) computes the gap between two consecutive events in absolute ticks and hands it to `u28::new(delta)` (line 470). `midly` 0.5.3's `restricted_int!` macro implements `u28::new` as `$name(raw & Self::MASK)` (verified in `midly-0.5.3/src/primitive.rs`, the `restricted_int!` macro, `MASK = (1 << 28) - 1`), i.e. it silently masks off any bits above 28 rather than erroring or panicking. Because `previous` only advances to the true absolute `at` (line 468) while the *stored* delta is the masked value, any single event whose distance from the previous event (or from the track start, since `previous` begins at `Ticks::ZERO`) exceeds `2^28 - 1 = 268,435,455` ticks is written at the wrong file-relative position, and every later event in that track chunk is then offset from that wrong baseline in the resulting SMF.

**Expected.** The module doc at crates/auris-io/src/midi.rs:24-25 claims 'Writing happens at *our* division... So every note position and length written here reads back exactly.' crates/auris-io/src/error.rs:10-11 states the crate's own policy: 'auris-io never logs-and-swallows a failure, because a silently truncated import or export is worse than a dialog.' A delta that cannot fit in 28 bits should be rejected with an `IoError` (or split across an inserted no-op event) rather than silently masked.

**Fix direction.** In `delta_encode` (crates/auris-io/src/midi.rs:452-479), check `delta > 0x0FFF_FFFF` before calling `u28::new(delta)` and return an `IoError` (or insert a marker/no-op event to split the gap) instead of letting `midly`'s `u28::new` silently mask the high bits.

**Written rule it breaks.** "auris-io never logs-and-swallows a failure, because a silently truncated import or export is worse than a dialog." (crates/auris-io/src/error.rs:10-11); "So every note position and length written here reads back exactly." (crates/auris-io/src/midi.rs:24-25)

**Verifier's correction.** Substance and mechanism are exactly right; only a cosmetic detail is off. With the claim's own trigger value of Ticks(300_000_000), the actual round-tripped position is 31,564,544 (confirmed by execution and by the masking arithmetic 300_000_000 & 0x0FFF_FFFF), which is about 9.5x too early, not "around 8.6x too early" as stated in the claim's consequence field.

### ✅ F-229 · medium · A TrackName meta event placed after a channel's last MIDI event is silently dropped, so the imported part falls back to "Channel N" instead of its real name.

`crates/auris-io/src/midi.rs:272` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Importing a standard MIDI file where a track's `TrackName` meta event comes after that channel's last note event (a legal, unremarkable SMF layout) silently produces a part labeled "Channel N" (or "Drums") instead of the name the file actually gave it. No error or warning is shown; the user just sees the wrong track name in the imported project.

**Trigger.** A Standard MIDI File whose `TrackName` meta event follows the track's note-on/note-off events instead of preceding them (legal per the SMF spec — meta events may appear anywhere in a track — though usually placed first). If no further `Midi` event lands on that channel after the `TrackName` event, the part is never named.

**Mechanism.** `track_name` is captured from the `TrackName` meta event at line 211-214. The attempt to attach it to a part happens unconditionally after every event in the track (lines 271-275): `parts.get_mut(&(index, channel_of(&event.kind)))`. `channel_of` (lines 585-590) returns 0 for any non-`Midi` event kind, including the `TrackName` event itself and the trailing `EndOfTrack` meta event, and returns the real channel only for subsequent `Midi` events. So a part only receives the pending name when a *later* `Midi` event on that exact channel is processed after the `TrackName` event arrives; nothing retroactively applies the name to a part whose events all occurred before the name meta event.

**Expected.** The `MidiTrack::name` doc comment (line 55-56) says the name comes 'from its name meta event or from the channel it played on', implying the meta event should win whenever the file provides one, regardless of where in the track it is placed. The pending name should be applied to every part that already exists (or is later created) in that track index once `track_name` becomes `Some`, not only to whichever single channel happens to own the next event.

**Fix direction.** Attach the pending track name to every part on that track index once the track finishes (fallback for any (index, channel) key not yet named), instead of relying on happening to see another Midi event on the same channel after the TrackName meta event. E.g. after the per-event loop, iterate `order` for the current track index and call `part.name.get_or_insert_with(...)` for each part still unnamed.

### ✅ F-302 · low · folder_is_named's ASCII-only case fold misses non-ASCII cased letters, so renaming a project folder by accented-letter case alone nests a duplicate copy instead of saving in place.

`crates/auris-io/src/project_file.rs:106` · platform · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** A user whose project folder name contains a non-ASCII cased letter (e.g. "café" vs "CAFÉ", or any accented/non-Latin-1 letter whose case differs only outside A-Z/a-z) saves under a name that differs from the folder only in that letter's case. On Windows or macOS, the filesystem treats the folder and the new name as the same directory, but folder_is_named reports them as different, so document_in_folder nests a whole second copy of the project - document and Audio directory - one level down inside the first, instead of saving in place. The original project is untouched, so nothing is lost, but the user now has an unexpected duplicate project folder to notice and clean up.

**Trigger.** Saving a project whose name contains a non-ASCII letter that differs only in case from the folder it is already inside on Windows or macOS — e.g. a folder `Café` already holding `Café.auris`, and `document_in_folder` invoked with a chosen path whose stem is `CAFÉ.auris`.

**Mechanism.** `folder_is_named` treats two names as the same folder when `case_insensitive` is true using `OsStr::eq_ignore_ascii_case`, which folds only the ASCII letters A-Z/a-z. NTFS's default collation, however, is Unicode-aware and folds case for a much larger set of characters (e.g. Latin-1 accented letters). The doc comment on `CASE_INSENSITIVE_PATHS` (lines 90-94) frames this as matching "the two systems the desktop application runs on" without qualifying it to ASCII names.

**Expected.** The case-insensitive comparison should fold on the same basis the underlying filesystem does (full Unicode case folding), not `eq_ignore_ascii_case`, so any name differing only in the case of any character — not just ASCII letters — is recognised as the same folder.

**Fix direction.** Replace the ASCII-only fold in folder_is_named with a Unicode-aware comparison, e.g. lowercase both OsStrs via to_string_lossy().to_lowercase() before comparing when case_insensitive is true, so the check matches the OS's own case-insensitive collation rather than only the A-Z/a-z range.

**Written rule it breaks.** on a case-insensitive filesystem `roughmix` and `RoughMix` are one directory, so comparing them byte for byte answers "no" about a folder the save is already inside ... renaming `roughmix` to `RoughMix` writes a whole second project, audio and all, one level down inside the first. (doc comment on folder_is_named, project_file.rs:96-101)
