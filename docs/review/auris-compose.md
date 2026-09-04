# Review findings: auris-compose

Part of the [whole-repository adversarial review](README.md) of commit `52d1702`. 18 verified findings: 4 high, 11 medium, 3 low.

Each entry survived an independent skeptic and an independent reproducer (and a tie-breaker when they disagreed); "executed reproduction" means the reproducer ran a test, a binary or a scratch program and observed the behaviour, "traced" means it followed the call path with concrete values.

| ID | Severity | Location | Finding |
|---|---|---|---|
| F-029 | high | `crates/auris-compose/src/rhythm.rs:607` | swing_offset returns a negative (early) shift for percent < 50, rushing offbeats instead of delaying them as documented and tested. |
| F-060 | high | `crates/auris-compose/src/gm.rs:248` | Drum `program` values between GM kit boundaries are silently corrupted to the nearest lower kit's number on TOML save/reparse, with no validation or error. |
| F-101 | high | `crates/auris-compose/src/parts/comp.rs:305` | Pushed-chord anticipation in auris-compose's comp() deletes (not trims) a prior close chord's onset when chords are spaced under half a beat apart. |
| F-112 | high | `crates/auris-compose/src/phrase.rs:207` | write_phrase floor-divides length into bars, so resizing a Lead/Kick/Snare/Hat/Drums clip to a non-bar-aligned length silently leaves its fractional tail with […] |
| F-053 | medium | `crates/auris-compose/src/spec/doc.rs:391` | Drum-part GM program numbers between kit boundaries (e.g. 30) are silently rounded down to the nearest kit's patch number on save/reload. |
| F-059 | medium | `crates/auris-compose/src/spec/doc.rs:731` | An empty `[section.X.part.Y]` TOML tweak table for a nonexistent part Y is silently dropped before validation, so no "part does not exist" error is ever raised. |
| F-117 | medium | `crates/auris-compose/src/frame.rs:331` | colour() gives a borrowed bVII a major seventh instead of the diatonically correct quality because its accidental!=0 skips diatonic_seventh and falls back to a […] |
| F-133 | medium | `crates/auris-compose/src/vocal.rs:158` | VIBRATO_FROM_SECONDS doc claims passing eighths "never" get vibrato, but they do below ~66.67 BPM. |
| F-140 | medium | `crates/auris-compose/src/parts/mod.rs:224` | shorten() applies gate-based note shortening to Riser parts, letting a spec-set gate cut a riser's note-off before its sample's documented peak. |
| F-141 | medium | `crates/auris-compose/src/spec/mod.rs:171` | PartSpec::range() can return bounds outside 0..127 for a legal octave, causing notes near the register extreme to silently collapse onto MIDI pitch 127 or 0. |
| F-153 | medium | `crates/auris-compose/src/spec/doc.rs:110` | SongSpec::to_toml() writes every top-level field unconditionally, contradicting its own "only what differs from a default" doc comment. |
| F-164 | medium | `crates/auris-compose/src/parts/mod.rs:219` | Setting `gate` on a riser part lets `shorten()` cut the swell's note-off before the join tick, undoing `riser()`'s exact-timing guarantee. |
| F-209 | medium | `crates/auris-compose/src/render.rs:312` | clips_of silently drops swing-delayed notes past a section boundary instead of clamping them, contradicting its own adjacent comment. |
| F-230 | medium | `crates/auris-compose/src/spec/doc.rs:838` | Riser's `note` rejection error wrongly claims its pitch "comes from the harmony," though Riser's pitch is a hardcoded constant, not harmony-derived. |
| F-231 | medium | `crates/auris-compose/src/parts/arp.rs:110` | arp()'s procedural path drops all notes for a chord event shorter than one arp step (count==0), unlike bass/comp's guaranteed onset-0 guard. |
| F-263 | low | `crates/auris-compose/src/frame.rs:597` | `is_stable` in crates/auris-compose/src/frame.rs:597 is defined but never called anywhere in the crate or workspace. |
| F-292 | low | `crates/auris-compose/src/parts/drums.rs:247` | `.max(1)` in drums.rs:247 drops the first step of a full-bar snare fill when beats*per_beat exactly equals steps (e.g. 2/4 meter, fill=1.0, intensity=1.0). |
| F-304 | low | `crates/auris-compose/src/frame.rs:169` | frame.rs:169's comment claims harmony, fill, and crash all gate joins the same way, but only harmony and crash share the intensity-comparison arrival test — […] |

### F-029 · high · swing_offset returns a negative (early) shift for percent < 50, rushing offbeats instead of delaying them as documented and tested.

`crates/auris-compose/src/rhythm.rs:607` · theory · confirmed (executed reproduction; reported independently 3×)

**What a user sees.** Any composed song whose swing setting falls in the lower half of its own accepted range (20-49, out of the validated 20..=90) has its swung offbeat notes pushed earlier than the grid instead of later. The user hears a rushed, anticipatory feel — the opposite of the laid-back "swing" the swing percentage is documented to produce — with no warning or error; only values 50-90 behave as documented.

**Trigger.** A song spec with `swing = 20` (or any value 20-49 — `crates/auris-compose/src/spec/doc.rs:499-504` explicitly validates and accepts swing in `20..=90`, so this is not a rejected input) and any groove with a hit on a swing-eligible offbeat step, e.g. `basic-rock`'s hat pattern at step 2: `swing_offset(Grid::default(), 2, 20, SwingFeel::Eighth)` computes `unit_ticks = 480`, `shift = (0.2 - 0.5) * 2 * 480 = -288`, i.e. `Ticks(-288)`.

**Mechanism.** `swing_offset`'s doc says (line 569) "How far a note on `step` is delayed by swinging at `percent`", and its result is fed straight into `note.start = (note.start + swing_offset(...)).max_zero()` in `parts::mod::swing` — a test there is literally named `swing_delays_the_offbeats_of_a_busy_part` and asserts "swing must never rush a note", and rhythm.rs's own test asserts `eighth(2, 66) > Ticks::ZERO, "swing delays, never rushes"`. But the formula at line 607, `let shift = (f64::from(percent) / 100.0 - 0.5) * 2.0 * unit_ticks as f64;`, is linear through zero at `percent == 50`: for any `percent < 50` the factor `(percent/100 - 0.5)` is negative, so `shift` — and the returned `Ticks` — is negative, i.e. the note moves EARLIER, not later. Every test that exercises this function only ever passes percent values >= 50 (50, 66, 67, 75, 90), so the sub-50 half of the domain is untested and the invariant the surrounding code repeatedly asserts is false there.

**Expected.** Per the function's own doc comment and the repeated project-level assertions that swing only ever delays, `swing_offset` should return a non-negative `Ticks` for every value in the validated `20..=90` range (e.g. by measuring the delay from 50 upward only, or by rejecting/clamping percent below 50), or the doc/tests/validated range should be revised to state that swing can push a note earlier below 50.

**Fix direction.** Change the formula in swing_offset (crates/auris-compose/src/rhythm.rs:607) so the shift is measured only as a delay from the 50 (straight) point upward, e.g. `let shift = ((f64::from(percent) - 50.0) / 50.0).max(0.0) * unit_ticks as f64;`, or clamp percent to never go below 50 before computing shift. Add a unit test exercising percent in 20..49 to assert the invariant "swing delays, never rushes" actually holds across the whole validated range.

**Written rule it breaks.** swing_offset's own doc comment: "How far a note on `step` is delayed by swinging at `percent`" and "percent says where the delayed note lands inside its pair — 50 is straight"; SongSpec::swing field doc (spec/mod.rs:415): "How much the offbeats are delayed, as a percentage where 50 is straight"; and the tests' own comments, rhythm.rs:1018 "swing delays, never rushes" and parts/mod.rs:604 "swing […]

### F-060 · high · Drum `program` values between GM kit boundaries are silently corrupted to the nearest lower kit's number on TOML save/reparse, with no validation or error.

`crates/auris-compose/src/gm.rs:248` · spec-mismatch · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** A drum part's `program` field written as anything other than one of the 8 named kit-boundary numbers (0, 8, 16, 24, 25, 32, 40, 48) is silently rewritten to the nearest lower boundary the moment the song document is saved to TOML and reparsed — e.g. `program = 5` becomes `program = "Standard Kit"` on save and reparses back as `Program(0)`, with no error, warning, or validation anywhere in the path. A user who hand-edits an `.asong` file, or an LLM composer (via auris-toolbox) that writes a non-boundary drum program, loses that value permanently and silently on the next save/reload cycle.

**Trigger.** `let s = SongSpec::parse("form = [\"verse\"]\n[[part]]\nname = \"kick\"\nrole = \"kick\"\nprogram = 5\n").unwrap();` gives `s.parts[0].program == Some(gm::Program(5))`. Calling `s.to_toml()` and reparsing (`SongSpec::parse(&s.to_toml()).unwrap()`) yields `program == Some(gm::Program(0))` — the written TOML contains `program = "Standard Kit"`, not the original number.

**Mechanism.** `Program::kit_name` (gm.rs:248-253) maps every patch 0-127 down to the nearest of only 8 named kits via `KITS.iter().rev().find(|(patch, _)| *patch <= self.0)`. `ProgramField::serialize` (doc.rs:389-392, `serializer.serialize_str(self.program.label(self.drums))`) is the *only* way a `program` field is written to a `.asong` file — there is no raw-number fallback — and for a drum-role part (`drums: part.role.is_drum()`, set at doc.rs:1040 in `PartDoc::from_spec`) `label(true)` returns `kit_name()`. So any drum part whose `program` is not exactly one of {0,8,16,24,25,32,40,48} is silently rewritten to the nearest lower boundary value the moment the document is saved, even though `Program`'s own `Deserialize` (gm.rs:364-368, `visit_u64`) explicitly accepts and stores any value 0-127 on the way in.

**Expected.** Per doc.rs:110-113 ("Round-tripping matters for the agent case too: a tool can read a specification back, change one field and send it again") and the fidelity every other round-trip test in the file enforces (e.g. `the_whole_default_specification_round_trips_unchanged`, `a_spec_round_trips_through_its_document`), a save/parse cycle should reproduce the document unchanged field for field. `ProgramField` should serialize a non-boundary drum patch as its raw number (which `Program::Deserialize` […]

**Fix direction.** Either constrain a drum part's `program` to the 8 `KITS` values at construction/deserialization time (reject or clamp non-boundary values with a `SpecError`), or make `ProgramField::serialize` preserve the exact number for drum parts (e.g. write both the raw patch and the resolved kit name, or only use the name when the number round-trips through `kit_name`/reverse lookup exactly). Add a round-trip test with a non-boundary drum program (e.g. `program = 5`) alongside the existing boundary-only test to catch regressions.

**Written rule it breaks.** Bump `Project::FORMAT_VERSION` whenever an older build could *misread* a newer file... [and more generally] the score does not change — CLAUDE.md's broader "text typed... must be preserved" principle for saved documents is violated here: a value the user/composer explicitly wrote is silently changed on save.

### F-101 · high · Pushed-chord anticipation in auris-compose's comp() deletes (not trims) a prior close chord's onset when chords are spaced under half a beat apart.

`crates/auris-compose/src/parts/comp.rs:305` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** When a chord chart changes chords faster than half a beat apart in a section with a pushed ("anticipated") comp figure, the composer's `comp()` pass silently drops one or more interior chords entirely — the exported piece has audible gaps where a chord should sound anticipated-and-held, instead of just being shortened. This is a symbolic composition bug (auris-compose), not a realtime-thread issue, but it produces wrong/incomplete generated audio on a realistic input (dense chord changes + a pushing section, which the style system can pick with up to 60% probability per section).

**Trigger.** A chord chart with changes closer than half a beat apart plus a pushing section, e.g. `chords = "| I ii iii IV V vi vii I bII |"` (9 chords in one 4/4 bar ⇒ ≈213 ticks apart, under the 240-tick `push_early`) with `syncopation = 1.0` (as already used verbatim in `bass.rs`'s `no_bass_note_outlives_the_chord_it_belongs_to` test) applied to a `chords`/`stab` part instead of `bass`. Any seed for which the per-section `pushing` roll succeeds (chance up to 0.6) reproduces it.

**Mechanism.** In `comp()`, when a section pushes (`push = pushing && !pad && !written && event.start >= push_early`), the code computes `let boundary = section.start + event.start - push_early;` (line 300) and then runs `notes.retain(|note| note.start < boundary);` (line 305) over *all* notes accumulated so far for this part, followed by a truncation pass that shortens notes overlapping `boundary`. `retain` unconditionally *removes* (not truncates) any earlier note whose `start` is `>= boundary`, regardless of how far in the past it was struck. The guard `event.start >= push_early` (comp.rs:251) only protects against the boundary going before the *section's own* start; it never checks the gap to the *previous* chord event. When two consecutive chord events are closer together than `push_early` (half a beat — 240 ticks on the default 16th grid, per the `push_early` computation at line 167), `boundary` for the second event falls at or after the *first* event's own onset-0 strike (which every non-written comp figure guarantees via the `if !written && !chosen.contains(&0) { chosen.insert(0, 0); }` […]

**Expected.** Per the comment directly above the call (comp.rs:301-304), "The old chord's own strikes inside the borrowed half-beat go entirely" — i.e. only strikes that fall *inside* the newly-borrowed half-beat window `[boundary, event.start)` should be removed; anything struck earlier (including the previous chord's own onset) should survive, truncated at most to end at `boundary`, matching the `note.length = boundary - note.start` truncation the very next block already performs for notes that merely […]

**Fix direction.** Replace the blanket `notes.retain(|note| note.start < boundary)` with logic that trims every note overlapping the boundary (as the following loop already does for `note.start + note.length > boundary`) rather than deleting notes whose `start >= boundary`; a note's onset lying inside the borrowed half-beat only means it must be cut off at `boundary`, not erased, unless it is itself the strike being replaced by the push. Concretely, drop the separate `retain` and extend the truncation loop to also shorten (or, if it truncates to zero length, then and only then remove) notes with `start >= boundary`, so a previous chord's legitimate onset that happens to fall within the borrowed window is trimmed rather than deleted.

**Written rule it breaks.** "The old chord's own strikes inside the borrowed half-beat go entirely — an offbeat figure lands one exactly where the push lands, and the two voicings struck together are the smear this block exists to prevent." (comp.rs:301-304, the doc comment directly above the offending `retain` call) — the code deletes any earlier note with start >= boundary, not just strikes belonging to the chord being […]

### F-112 · high · write_phrase floor-divides length into bars, so resizing a Lead/Kick/Snare/Hat/Drums clip to a non-bar-aligned length silently leaves its fractional tail with no notes.

`crates/auris-compose/src/phrase.rs:207` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Dragging a Lead, Kick, Snare, Hat, or Drums clip to a length that isn't a whole number of bars (the default grid is a sixteenth note, so this is easy to hit) leaves the fractional tail of the resized clip completely silent — no notes are generated there — even though the clip's stored length covers that tail. The resize doc comment on `Session::resize_clip` promises the clip "fills the bars it gained instead of trailing silence," which this breaks for exactly the instruments most commonly resized this way.

**Trigger.** Any generated clip resized to a non-bar-aligned length, which the session layer explicitly permits: `Session::resize_clip` bounds the new length only by the project's editing grid — `let length = (end - start).max(grid);` (`crates/auris-session/src/session/clips.rs:441`), and the default grid is a sixteenth note (`crates/auris-core/src/project/mod.rs:297`, `TICKS_PER_QUARTER / 4`), far finer than a bar. Dragging a 2-bar generated Drums or Lead clip out to, say, 2 bars + 1 beat and calling `session.phrase(...)`/`write_phrase(...)` again reproduces the scenario.

**Mechanism.** `let bars = (length.raw().max(0) / bar_ticks) as usize;` floor-divides the caller-supplied `length` to get `SectionPlan.bars`, but the very same `SectionPlan` (lines 244-265) also carries the *unrounded* `length` unchanged (`length,` at line 250). Every bar-scoped writer in this crate iterates `for bar in 0..section.bars` and positions each bar at `grid.bar_ticks() * bar` (confirmed directly in `crates/auris-compose/src/parts/joins.rs:104-105` for the crash/riser writer and `crates/auris-compose/src/parts/melody.rs:468` for the melody writer), so any stretch of the clip beyond `bars * bar_ticks` — up to the real `length` — receives no bar-scoped material at all. The final truncation in `write_phrase` (lines 309-319) only clamps notes that *were* written; it invents nothing for the uncovered tail.

**Expected.** A resized/regenerated clip should have material written for the whole of its `length`, not just `floor(length / bar_ticks)` worth of it — either by writing a final partial bar (clamped, as the note-level truncation already does), or by disallowing/snapping generated-clip resizes to whole bars so `bars * bar_ticks == length` always holds, consistent with the documented 'fills the bars it gained instead of trailing silence' guarantee.

**Fix direction.** In `write_phrase` (crates/auris-compose/src/phrase.rs:207), compute `bars` with a ceiling division instead of a floor division (`(length.raw().max(0) + bar_ticks - 1) / bar_ticks`) so bar-scoped writers (melody.rs:468, joins.rs:104) loop far enough to cover the fractional tail bar; the existing post-filter at phrase.rs (`draft.start >= Ticks::ZERO && draft.start < length`) already truncates any generated notes back to the true, unrounded `length`, so this alone closes the gap without touching the event-driven writers (Chords/Pad/Stab/Bass/Arp) that are unaffected.

**Written rule it breaks.** Session::resize_clip doc comment: a dragged-out generated clip "fills the bars it gained instead of trailing silence"

### F-053 · medium · Drum-part GM program numbers between kit boundaries (e.g. 30) are silently rounded down to the nearest kit's patch number on save/reload.

`crates/auris-compose/src/spec/doc.rs:391` · persistence · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** If a composed or hand-edited song document has a drum-role part whose program number falls between two kit boundaries (e.g. 30, which lies between the 25 "TR-808 Kit" and 32 boundaries), saving and reloading the project silently rewrites that number down to the nearest kit boundary (30 becomes 25). The audible drum kit selection can change on reload, and the round-trip is silent — no warning, no error — so the user only notices if they listen carefully or diff the saved file.

**Trigger.** A hand-written or agent-generated `.asong` document such as:
```
form = ["verse"]
[[part]]
name    = "kick"
role    = "kick"
program = 30
```
`SongSpec::parse` accepts it and stores `part.program == Some(gm::Program(30))`. Calling `spec.to_toml()` — which is exactly what `SongSpec::to_toml`'s own doc comment (doc.rs:114-118) says is for round-tripping through an agent ("a tool can read a specification back, change one field and send it again"), and is exactly what `auris-toolbox`'s `check_spec` tool (lib.rs:172-180) and the song-sheet's "Save as Specification…" both do — writes `program = "TR-808 Kit"`. Re-parsing that output with `SongSpec::parse` yields `Program(25)`, not `Program(30)`.

**Mechanism.** `gm::Program::parse` accepts ANY raw number 0..127 for a `program` field regardless of role (gm.rs:302-304: `if let Ok(number) = text.parse::<u16>() { return (number < 128).then_some(Program(number as u8)); }`), so a drum-role part can legally be given `program = 30` and store `Some(Program(30))`. When the spec is later serialized (`SongSpec::to_toml`), `ProgramField::serialize` (doc.rs:389-393: `serializer.serialize_str(self.program.label(self.drums))`) is fed `drums: part.role.is_drum()` from `PartDoc::from_spec` (doc.rs:1038-1041), and for a drum part `label` calls `Program::kit_name` (gm.rs:248-253: `KITS.iter().rev().find(|(patch,_)| *patch <= self.0).map_or("Standard Kit", |(_, name)| *name)`), which *rounds the patch down to the nearest of the eight `KITS` boundary values* (0, 8, 16, 24, 25, 32, 40, 48) purely for display. That rounded *name* is what actually gets written to the document. On the next parse, `Program::parse`'s kit branch (gm.rs:313-317) maps the kit name back only to its own boundary patch, so the original patch is gone: `Program(30)` -> written as `"TR-808 […]

**Expected.** CLAUDE.md: "What a saved file guarantees across builds is the text: notes typed, edited or frozen…" and this module's own tests assert `SongSpec::parse(&spec.to_toml()) == spec` (or the equivalent per-field equality) for every other field, including the closely analogous drum `note` field (doc.rs:1104, `a_drum_part_strikes_the_note_it_names_and_general_midi_otherwise`) and for `program` itself (doc.rs:1843, `a_program_survives_the_round_trip_by_name`) — but that test's own kick example uses […]

**Fix direction.** Either (a) make `ProgramField::serialize` write the exact numeric patch when `drums` is true and the value isn't exactly a kit boundary (falling back to the kit name only for boundary values, for readability), or (b) constrain `gm::Program::parse`/drum-part validation to only accept exact kit-boundary values for drum roles, rejecting non-boundary numbers with a `SpecError` at parse time instead of silently accepting and later corrupting them on round-trip. Option (a) is the minimal fix since it preserves current author-facing behavior for legitimate values and only changes serialization of the edge case.

**Written rule it breaks.** Nothing to check: `Program` refuses anything outside 0..127 as it is read, which is the one thing that could be wrong about it. (doc.rs, around line 758) — this comment asserts the round-trip is safe for drum programs, which is false for non-boundary values.

### F-059 · medium · An empty `[section.X.part.Y]` TOML tweak table for a nonexistent part Y is silently dropped before validation, so no "part does not exist" error is ever raised.

`crates/auris-compose/src/spec/doc.rs:731` · spec-mismatch · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** A composer who writes a `[section.verse.part.trombone]` header in their TOML song document — with no fields yet inside it, e.g. while drafting or after a typo of a part name — gets no error at all if `trombone` is not one of the song's actual parts. `into_spec` builds a `PartTweak::default()` for the empty table, `PartTweak::is_empty()` is true, so the (part, tweak) pair is dropped before it ever reaches the roster-validation loop that checks tweak keys against `part_names`. The document loads silently as if the header were never written; the intended validation error ("names the part `trombone`, which does not exist") never appears.

**Trigger.** SongSpec::parse("form = \"verse\"\n\n[section.verse.part.trombone]\n") — the default roster has no part called trombone (lead, chords, bass, kick, snare, hat). The [section.verse.part.trombone] header alone (with no fields) parses as a valid, entirely-empty PartTweakDoc.

**Mechanism.** SectionDoc::into_spec (doc.rs:729-734) only inserts a part-tweak into section.tweaks when !tweak.is_empty(). PartTweakDoc::into_spec returns a PartTweak::default() (all-None) whenever the sub-table declares no fields at all, or declares fields that ALL fail their own range check (e.g. octave = 40, which pushes an error but leaves tweak.octave at None). In either case is_empty() (spec/mod.rs:211-213, *self == Self::default()) is true and the entry is never inserted into section.tweaks. The only place a tweak's part name is checked against the real roster is SongDoc::into_spec's `for part in section.parts.iter().chain(section.tweaks.keys())` loop (doc.rs:634-642) — which iterates section.tweaks.keys(), so a dropped-because-empty entry's key is invisible to it. A bogus part name inside an empty (or wholly-invalid) tweak table therefore never reaches the existence check, and if the table was fully empty, no other code path emits any error at all for it.

**Expected.** Per SectionSpec::tweaks's own doc comment (spec/mod.rs:352-356): "A name that no part answers to is a mistake the format reports." And per the module's stated contract (doc.rs:41-47): every meaning-level complaint "is reported at once"; and doc.rs:74-76: "silently ignoring [a field] would mean the piece quietly ignores an instruction." The existing test a_tweak_naming_a_part_that_does_not_exist_is_a_mistake (doc.rs:1552) only exercises the case where the tweak carries an in-range field (octave […]

**Fix direction.** In `SectionDoc::into_spec` (doc.rs ~729-733), validate/insert against `self.part`'s keys before the emptiness filter — either always insert the tweak (even when default/empty) so the existing roster-check loop at doc.rs:634 sees it, or check each part name against `part_names` at the point the tweak is discarded so a bogus name is still reported even when its table is empty.

**Written rule it breaks.** // A name that does not resolve would otherwise be answered by a silent substitution: a section would play a progression nobody asked for, or fall silent for want of a part. (doc.rs, comment preceding the part_names validation loop)

### F-117 · medium · colour() gives a borrowed bVII a major seventh instead of the diatonically correct quality because its accidental!=0 skips diatonic_seventh and falls back to a bare major-triad-to-major7 mapping.

`crates/auris-compose/src/frame.rs:331` · theory · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** When the composer colours a borrowed bVII in a minor-key generated progression with a seventh, it writes a major-seventh chord (e.g. Gmaj7 in A minor) instead of the diatonically correct dominant-seventh-quality bVII7 (Gmaj-with-flat7, i.e. G7). The harmony lane and the resulting Chords clip both carry the wrong chord quality, so any borrowed-bVII-with-seventh bar in a minor-mode composed section is audibly wrong — a real, if narrow, path since colour() is applied to every colourable numeral whenever the seventh roll succeeds and the mood's seventh_rate is nonzero.

**Trigger.** Any SongSpec in a minor key whose chart is composer-generated (no `chords=` quoting a named/user progression, so ChartOrigin::Generated), with a mood whose seventh_rate is nonzero (the ordinary case - colour() is exercised by both 'neutral' and 'tense' moods in the existing tests), where the progression's weighted random walk (progression.rs MINOR_MOVES) lands on the 'bVII' state for some bar - reachable with weight 1.0-2.0 from four of the six states, so a routine occurrence rather than a rare corner - and the per-event `seventh` RNG roll succeeds.

**Mechanism.** In colour() (frame.rs 297-354), when `seventh` is rolled true the code does:
    chord.quality = Some(event.numeral)
        .filter(|numeral| numeral.accidental == 0)
        .and_then(|numeral| diatonic_seventh(source, numeral.degree))
        .unwrap_or_else(|| chord.quality.with_seventh());
only numerals with accidental==0 get the correct `diatonic_seventh(source, degree)` lookup; anything already altered falls back to `Quality::with_seventh()` (auris-core theory/chord.rs:196), which unconditionally maps `Major -> Major7`. `bVII` (progression.rs's MINOR_STATES[5], degree=7, accidental=-1, quality=None) is `is_colourable()==true` (numeral.rs:73-75, quality/extension/secondary_of all None) so colour() processes it, but its accidental is -1, so it always takes the with_seventh() fallback. Its triad is a Major triad (numeral.rs chord_in: accidental!=0 and minor_case=false -> Quality::Major), so with_seventh() gives Major7. The diatonically correct seventh on a natural-minor bVII (e.g. G-B-D in A minor) is a minor seventh -> a dominant7-shaped chord (G-B-D-F), not a major seventh […]

**Expected.** Per the function's own stated rationale (frame.rs 326-329, 'The seventh the *key* stacks on that degree... with_seventh sees a major triad and can only give it a major seventh'), any colourable dominant-functioning major triad - not only ones with accidental==0 - should receive the seventh the key/mode actually stacks there (a minor seventh for a natural-minor bVII), rather than falling back to the unconditional major-seventh with_seventh().

**Fix direction.** In colour()'s seventh branch (crates/auris-compose/src/frame.rs:326-333), drop the `.filter(|numeral| numeral.accidental == 0)` gate (or extend diatonic_seventh to accept an accidental/borrowed degree) so that a numeral with a nonzero accidental like bVII is still routed through diatonic_seventh(source, degree) using the already-computed `source` mode, rather than falling back to chord.quality.with_seventh() which only knows major-triad-to-major-seventh mapping.

**Written rule it breaks.** Composed audio is calibrated by measurement / the code's own comment: "The seventh the *key* stacks on that degree, which is the only thing that knows a dominant from a tonic." — the accidental filter defeats exactly this stated intent for any numeral with an accidental, including the common borrowed bVII.

### F-133 · medium · VIBRATO_FROM_SECONDS doc claims passing eighths "never" get vibrato, but they do below ~66.67 BPM.

`crates/auris-compose/src/vocal.rs:158` · theory · confirmed (executed reproduction; reported independently 2×)

**What a user sees.** At slow tempos (60 BPM and below, an ordinary ballad range), passing eighth-note syllables get the same vibrato ornament as phrase-final held notes — every non-final syllable in a slow phrase sways audibly, contradicting the documented design intent that only held notes should sway. A developer reading the doc comment and relying on the "never" guarantee (e.g. when reasoning about why a passing note got vibrato, or when choosing VIBRATO_FROM_SECONDS for a new tempo range) is misled about the function's actual behavior.

**Trigger.** Any composed vocal line at a slow-ballad tempo of 66 BPM or below (e.g. `TempoMap::constant(60.0)`, a common J-pop ballad tempo) — every passing (non-final) eighth-note syllable in every phrase gets `note.vibrato = Some(Vibrato { .. })` from `ornament_vocal`, not just the phrase-final held note.

**Mechanism.** The doc comment on `VIBRATO_FROM_SECONDS` (lines 113-119) claims: "With the phrase-final half note this rhythm writes, the held syllable clears the bar at any tempo under ~260 BPM and the passing eighths never do." `ornament_vocal` (line 158) applies vibrato whenever `seconds >= VIBRATO_FROM_SECONDS` (0.45), where `seconds` is the note's real duration from `TempoMap::ticks_to_seconds`. `vocal_rhythm` (lines 84-99) gives each passing syllable a fixed eighth-note slot of `TICKS_PER_QUARTER/2` = 480 ticks = 30/tempo seconds. Solving `30/tempo >= 0.45` gives `tempo <= 66.67` BPM — so at any tempo at or below roughly 66 BPM, a passing eighth note's real duration meets or exceeds the vibrato threshold, contradicting the doc's "never" claim. The threshold is a fixed wall-clock duration, not tied to whether a note is the phrase's held final syllable versus a passing one, so at slow tempos the two categories become indistinguishable to this rule.

**Expected.** Per the stated design (`ornament_vocal`'s "A held note sways" rule and the `VIBRATO_FROM_SECONDS` doc comment's explicit tempo claim), only the phrase-final held note should receive vibrato; a passing eighth note should not, regardless of tempo. The threshold should be structural (is this the phrase's last/held syllable) rather than, or in addition to, a fixed real-time duration that stops discriminating at slow tempos.

**Fix direction.** Either narrow the doc comment's claim to state the actual valid tempo range (roughly 66.67 BPM to 260 BPM) instead of an unconditional "never"/"any tempo," or make the guarantee tempo-independent by having ornament_vocal distinguish held-final notes from passing notes explicitly (e.g. via rhythm/phrase position) rather than relying solely on a wall-clock duration threshold.

**Written rule it breaks.** DSP code lives behind unit tests that assert on numbers (levels, frequencies, lengths) rather than on "it runs." / doc comment: "the held syllable clears the bar at any tempo under ~260 BPM and the passing eighths never do — which is the rule the numbers were chosen to produce."

### F-140 · medium · shorten() applies gate-based note shortening to Riser parts, letting a spec-set gate cut a riser's note-off before its sample's documented peak.

`crates/auris-compose/src/parts/mod.rs:224` · correctness · confirmed (executed reproduction; reported independently 2×)

**What a user sees.** When a spec sets a non-default gate (< 1.0) on a riser part, the exported MIDI/audio for that riser's swell note-off lands before the join tick instead of exactly on it, cutting the cymbal/riser sample before its documented peak — the swell is audibly truncated at an arrival instead of ringing through it. This only triggers on specs that explicitly set gate on a riser role (the default gate is 1.0, so most generated music is unaffected), which is why it's an edge path rather than a mainline one.

**Trigger.** A song spec containing `[[part]]\nname = "riser"\nrole = "riser"\ngate = 0.5` (or any gate below 1.0). `write_parts` calls `shorten(&played, &mut draft.notes)` unconditionally over every part's notes (mod.rs:190), including the riser's.

**Mechanism.** `shorten()` (mod.rs:219-236) only skips a note when `part.role.is_drum()` (line 224). `Role::Riser.drum_voice()` returns `None` (crates/auris-compose/src/spec/role.rs, `drum_voice` match), so `is_drum()` is `false` for a riser part and its note is not excluded. If `part.gate < 1.0` (line 227-229), the riser note's `length` is multiplied by `gate` and rounded (line 234-235), shrinking it. But `riser()` (crates/auris-compose/src/parts/joins.rs:156-187) deliberately sets `length: end - start` so the note's end lands *exactly* on the join tick, with its own doc comment stating: "Held to the join exactly. The peak is the last thing in the sample, and a note-off before it would cut the swell at the moment it exists for." `Role::default_gate()` (role.rs) returns `1.0` for every role except `Stab`, so by default `gate >= 1.0` and `shorten()` no-ops (line 228-230) — but `gate` is a fully general, role-unrestricted field (validated only as `(0.0..=1.0]` in spec/doc.rs's `into_part_fields`, around line 788-801) that a spec author can set on any part, including one with `role = "riser"`.

**Expected.** `shorten()` should also leave a `Role::Riser` part alone (the same way it explicitly does for `Role::Riser` in the sibling `swing()` pass at mod.rs:258-260: `if part.role == Role::Riser { return; }`), since the riser's note length is not an ordinary sustain but an exact, load-bearing offset to the join, per joins.rs's own documented contract.

**Fix direction.** In shorten() (crates/auris-compose/src/parts/mod.rs:224), extend the skip guard from `part.role.is_drum()` to also skip `Role::Riser` (e.g. `if part.role.is_drum() || part.role == Role::Riser { continue; }`), matching the same "note-off is load-bearing, don't shorten it" reasoning already applied to drums; alternatively restrict `gate` validation in spec/doc.rs's into_part_fields to reject a non-1.0 gate on Riser (and any other role whose writer treats note length as exact) at spec-parse time.

**Written rule it breaks.** Held to the join exactly. The peak is the last thing in the sample, and a note-off before it would cut the swell at the moment it exists for. (crates/auris-compose/src/parts/joins.rs, doc comment on riser())

### F-141 · medium · PartSpec::range() can return bounds outside 0..127 for a legal octave, causing notes near the register extreme to silently collapse onto MIDI pitch 127 or 0.

`crates/auris-compose/src/spec/mod.rs:171` · theory · confirmed (executed reproduction; reported independently 2×)

**What a user sees.** A song spec that sets a legal octave (e.g. octave = 9 on a bass part, or -1 on a treble-register role) parses with zero errors, but PartSpec::range() then returns a window like (112, 136) that partially sits above MIDI's 0..127 ceiling. Every note-generation function (bass.rs, arp.rs, coda.rs, comp.rs, melody.rs) folds pitches into that invalid window and then clamps the cast to u8 with .clamp(0, 127), so any note that folded into the 128..136 slice collapses onto the single pitch 127 (or 0 at the low extreme). The composed line loses melodic distinctness in that register - several notes silently print the same pitch - with no error or warning surfaced to the user, CLI, or MCP caller.

**Trigger.** `form = "verse"
[[part]]
name = "bass"
role = "bass"
octave = 9`  — parses cleanly (octave 9 passes the -1..=9 check) into a `SongSpec` whose bass part has `range() == (112, 136)`.

**Mechanism.** `PartSpec::range()` (spec/mod.rs:168-173) computes `let shift = (self.octave - self.role.default_octave()) * 12; (low + shift, high + shift)` with no clamp to the legal MIDI window. `octave` is only checked against the bare numeric bound `(-1..=9).contains(&octave)` in `PartDoc::into_spec` (spec/doc.rs:762-769) and again in `PartTweakDoc::into_spec` (spec/doc.rs:963-970) — the check never looks at the role's own `range()`, even though `range()` is exactly what turns the octave into real note numbers. For a role whose `default_octave()` sits away from the middle of the 0-9 span (e.g. `Role::Bass` at 2, `spec/role.rs:129`), a still-legal octave pushes the window's top or bottom past 0..127. Concretely: `Role::Bass.range() == (28, 52)` and `default_octave() == 2`; at `octave = 9` (allowed by the -1..=9 check) `shift = (9-2)*12 = 84`, giving `range() == (112, 136)` — 136 is not a MIDI note. Every writer that reads `part.range()` (bass.rs, melody.rs, comp.rs, arp.rs, coda.rs) folds a pitch into `[low, high]` with `fold_into` and then does `pitch.clamp(0, 127) as u8` before building a […]

**Expected.** doc.rs's own module contract (spec/doc.rs:44-47) says every meaning-level mistake — "a key that is not a key, a fraction outside 0 to 1" — is caught and reported once the document stops being text. An octave that, combined with the part's role, pushes `range()` outside 0..127 is exactly such a mistake and should be validated (or the resulting window clamped) the same way every other numeric field here is, rather than validating the bare octave number in isolation.

**Fix direction.** In PartSpec::range() (spec/mod.rs:168-173), clamp the shifted (low, high) to 0..=127 before returning, and/or have PartDoc::into_spec / PartTweakDoc::into_spec (doc.rs:762-769, 963-970) validate the octave against role.range() shifted by it, not just the bare -1..=9 bound, so an out-of-bounds combination is a reported SpecError instead of a silent pitch collision.

**Written rule it breaks.** /// The MIDI range this part should stay inside, moved by its octave. (crates/auris-compose/src/spec/mod.rs:168) - the range is documented as authoritative for where a part's notes stay, but it can itself fall outside 0..127.

### F-153 · medium · SongSpec::to_toml() writes every top-level field unconditionally, contradicting its own "only what differs from a default" doc comment.

`crates/auris-compose/src/spec/doc.rs:110` · spec-mismatch · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Every time a user or agent calls SongSpec::to_toml() (e.g. saving a composition dialog's state, or an agent tool reading a spec back to edit one field), the output TOML always contains all top-level scalar fields (title, key, tempo, meter, seed, groove, ending, swing, humanize, dynamics, fill, variation, brightness, energy, tension, syncopation, form) even when they equal SongSpec::default(). This contradicts the method's own doc comment and makes the file longer and noisier than "what a person would have typed," though the file still round-trips correctly since every field is individually optional in SongDoc.

**Trigger.** `SongSpec::parse("tempo = 96").unwrap().to_toml()` — a one-line input document.

**Mechanism.** The doc comment on `SongSpec::to_toml` (doc.rs:110-113) promises "Only what differs from a default is written, so what comes out of a dialog is about as short as what a person would have typed." `SectionDoc::from_spec` (doc.rs:923-943) and `PartDoc::from_spec` (doc.rs:1030-1053) both implement exactly that, each building a `plain` baseline (`SectionSpec::named(name)` / `PartSpec::of_role(...)`) and writing `(x != plain.x).then_some(...)` per field. But `SongDoc::from(spec: &SongSpec)` (doc.rs:851-921), which `to_toml()` actually calls, never compares against `SongSpec::default()` at all: `title`, `key`, `tempo`, `meter`, `seed`, `groove`, `ending`, `swing`, `humanize`, `dynamics`, `fill`, `variation`, `brightness`, `energy`, `tension`, `syncopation` and `form` are unconditionally wrapped in `Some(...)` (lines 854-888), and every entry of `spec.sections` / `spec.parts` is written (lines 913-918) even when a section is byte-for-byte identical to its own `SectionSpec::named` default.

**Expected.** Per the doc comment itself, and matching the pattern already used correctly in `SectionDoc::from_spec` / `PartDoc::from_spec`, `SongDoc::from` should compare each top-level field against `SongSpec::default()` (and each section entry against its own `SectionSpec::named` baseline before deciding to include it at all) and omit anything that matches, instead of unconditionally writing every field.

**Fix direction.** In impl From<&SongSpec> for SongDoc, compare each top-level scalar against SongSpec::default() (as SectionDoc::from_spec and PartTweakDoc already do for their fields) and use .then_some(...)/.then(|| ...) to omit fields equal to the default, instead of unconditionally wrapping every field in Some(...).

**Written rule it breaks.** "Only what differs from a default is written, so what comes out of a dialog is about as short as what a person would have typed." (doc.rs:110-113, doc comment on SongSpec::to_toml)

### F-164 · medium · Setting `gate` on a riser part lets `shorten()` cut the swell's note-off before the join tick, undoing `riser()`'s exact-timing guarantee.

`crates/auris-compose/src/parts/mod.rs:219` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** If a user sets `gate` on a riser part (default gate is 1.0, so the bug is dormant unless a value below 1.0 is explicitly configured), the riser's swell no longer holds to the join tick: `shorten()` shrinks the note's length toward `part.gate` unconditionally for every non-drum role, cutting the note off before the peak the sample was built to land on. The composed audio then has the riser's peak silenced or cut short at the exact moment `riser()`'s own doc comment says it must not be — undoing that pass's guarantee and producing an audibly wrong swell on any preset or user config that gates a riser part.

**Trigger.** Compose a SongSpec containing a part with `role = "riser"` and any `gate` below 1.0, e.g. `[[part]]\nname = "riser"\nrole = "riser"\ngate = 0.3`, over a form with at least one arrival section. At 120 BPM the riser note's documented length is 1920 ticks (one second of lead); with gate 0.3 `shorten()` computes `shortened = (1920.0 * 0.3).round() = 576`, `floor = min(30, 1920) = 30`, so `note.length` becomes `Ticks(576)` instead of `Ticks(1920)`.

**Mechanism.** `shorten()` (lines 219-237) shrinks a note's length toward `part.gate` for every role except a drum (`if part.role.is_drum() { continue; }` at line 224). `Role::Riser` is not a drum (`Role::drum_voice` in crates/auris-compose/src/spec/role.rs has no `Riser` arm, so `is_drum()` is false), so a riser note is not exempted and falls through to `let gate = part.gate.clamp(MIN_GATE, 1.0); ... note.length = Ticks(shortened.max(floor));` at lines 227-235. But `riser()` in crates/auris-compose/src/parts/joins.rs (lines 156-187) builds the note's `length` as exactly `end - start`, i.e. the note-off is deliberately placed on the join tick so the reverse-cymbal sample's calibrated peak (RISER_LEAD_SECONDS = 1.0s, documented at joins.rs:47-63) lands on the downbeat; its own doc says "Held to the join exactly... a note-off before it would cut the swell at the moment it exists for." `gate` is a fully generic, role-unrestricted field (validated only for its own 0..1 range in crates/auris-compose/src/spec/doc.rs:788-800, no role check), so `[[part]] role = "riser" gate = 0.3` is a document that […]

**Expected.** Per joins.rs's own doc comment ("Held to the join exactly") and the symmetric exemption already present in `swing()` (mod.rs:257-260), `shorten()` should also leave `Role::Riser` notes untouched, e.g. `if part.role.is_drum() || part.role == Role::Riser { continue; }`.

**Fix direction.** Add the same exemption `shorten()`'s sibling `swing()` already has: skip riser parts in `shorten()` (e.g. `if part.role.is_drum() || part.role == Role::Riser { continue; }` at crates/auris-compose/src/parts/mod.rs:224), so a gate set on a riser part cannot shrink the note off the join tick.

**Written rule it breaks.** Held to the join exactly. The peak is the last thing in the sample, and a note-off before it would cut the swell at the moment it exists for.

### F-209 · medium · clips_of silently drops swing-delayed notes past a section boundary instead of clamping them, contradicting its own adjacent comment.

`crates/auris-compose/src/render.rs:312` · spec-mismatch · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** When auris-compose writes a part with swing enabled, a note whose swing-delayed onset lands exactly at or past its section's end boundary is silently deleted from the composition instead of being clamped back inside the section as the code comment promises. The user hears a missing note (typically a downbeat-adjacent hit) at a section boundary, with no warning or log — the composed clip is simply short one note.

**Trigger.** A part (e.g. a hand-written `rhythm = "x ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ x"` on a snare/kick/crash part, or a ghost note drawn at high density on any Eighth-feel groove) that strikes step 15 of a 4/4 sixteenth grid's last bar, together with `swing = 90` (legal max): `tick_of(15) = 3600`, `unit_ticks = 480`, delay `= 0.8 * 480 = 384`, giving an absolute position `3600 + 384 = 3984` against a `bar_ticks` (and section-final-bar length) of `3840` — 144 ticks past the section's own end.

**Mechanism.** The comment at lines 308-310 states: "A note the swing delayed over a section boundary is clamped back rather than deleted — dropping one took the downbeat out of sections back when the baked wander could nudge a note either way." But the very next lines do the opposite for a note delayed past the section's end: `let offset = note.start - section.start; if offset >= section.length { return None; }` (lines 311-314) unconditionally drops the note. `swing()` in `parts/mod.rs` (called before `clips_of` sees the notes) moves `note.start` forward by up to `0.8 * unit_ticks` at the maximum validated `swing = 90` and never re-clamps it back inside its own section afterward (the `shorten`/`swing`/`untangle` pipeline in `parts/mod.rs:188-191` has nothing that re-bounds a note's start to its originating section). So a note on the last swing-eligible step of a section's final bar, once delayed, lands with `note.start > section.start + section.length` and is silently deleted here rather than "clamped back" as promised.

**Expected.** Per the comment's own stated intent, a note whose swung start lands at or past `section.length` should be clamped back into the section (as is already done for the negative-offset case via `.max_zero()`), not dropped via the early `return None`.

**Fix direction.** In crates/auris-compose/src/render.rs's clips_of, replace the early `if offset >= section.length { return None; }` with the same clamp already used for the negative-offset case — compute `start = offset.max_zero().min(section.length - Ticks(1))` unconditionally, so a swing-overrun note is pulled back to the section's last tick instead of vanishing. If dropping is actually the intended behaviour for large overruns, fix the comment at render.rs:308-310 to say so instead of promising a clamp it doesn't perform.

**Written rule it breaks.** A note the swing delayed over a section boundary is clamped back rather than deleted — dropping one took the downbeat out of sections back when the baked wander could nudge a note either way. (crates/auris-compose/src/render.rs:308-310)

### F-230 · medium · Riser's `note` rejection error wrongly claims its pitch "comes from the harmony," though Riser's pitch is a hardcoded constant, not harmony-derived.

`crates/auris-compose/src/spec/doc.rs:838` · spec-mismatch · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** A user who writes `role = "riser"` with a `note` field on a part or section tweak gets a validation error claiming the riser's pitch "comes from the harmony" — which is false and actively misleading. The riser's pitch is a fixed constant (RISER_PITCH = 60 in parts/joins.rs), never derived from the chord chart, so the message sends the user looking for a harmony/chord-progression cause that doesn't exist instead of telling them the field is simply unsupported for this role.

**Trigger.** `form = "verse"
[[part]]
name = "riser"
role = "riser"
note = 64`  — rejected with the harmony-derived-notes message.

**Mechanism.** `PartDoc::into_spec` rejects a `note` field on any role whose `drum_voice()` is `None` with the message "part `{name}` plays {role}, whose notes come from the harmony; `note` is for a drum part, which strikes one" (spec/doc.rs:834-842), and `SongDoc::into_spec` repeats the identical claim for a section tweak (spec/doc.rs:652-658). `Role::Riser::drum_voice()` returns `None` (spec/role.rs:102-110), so it falls into this branch and is told its notes "come from the harmony." That is false: `Role::Riser.range()` is documented as a single fixed point, `(60, 60)`, "because … the note is the playback rate of a recording, the writer places the recorded speed, and a range would be promising a register the part does not have" (spec/role.rs:293-297), and the actual writer (`parts/joins.rs:45,178`) always emits `pitch: RISER_PITCH` — a hardcoded `u8 = 60` constant that never reads the chord chart at all.

**Expected.** The rejection reason should distinguish a genuinely harmony-driven role (Melody/Chords/Pad/Arp/Stab/Bass) from Role::Riser, whose own doc comment (spec/role.rs:293-297) already states its pitch is fixed by the writer rather than drawn from the harmony — the error text should say that, not repeat the harmony claim for a role it does not apply to.

**Fix direction.** Give Riser (and any other future fixed-pitch, non-drum role) a distinct rejection message, or branch the message on role.range() being a single fixed pitch — e.g. "part `{name}` plays riser, whose pitch is fixed; `note` has no effect" — instead of reusing the harmony-derived wording for every role that isn't a drum voice, at both doc.rs:838-841 and doc.rs:656-658.

**Written rule it breaks.** One pitch, honestly: the note is the playback rate of a recording, the writer places the recorded speed, and a range would be promising a register the part does not have. (role.rs:293-296, doc comment on Role::Riser)

### F-231 · medium · arp()'s procedural path drops all notes for a chord event shorter than one arp step (count==0), unlike bass/comp's guaranteed onset-0 guard.

`crates/auris-compose/src/parts/arp.rs:110` · spec-mismatch · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** When composing with an arp part at low density (busy <= 0.33, giving a 4-step coarse rate) over a chord chart whose events are shorter than one arp step — e.g. a bar with many chord changes like "| I ii iii IV V vi vii I bII |" — one or more chords produce zero arp notes. The arpeggio goes silent for that chord and resumes on the next one, instead of playing at least one note per chord as its own module doc promises.

**Trigger.** An arp part over a densely-changing chord chart with density low enough to select the coarsest step rate, e.g. `chords = "| I ii iii IV V vi vii I bII |"` (≈213-tick chord events in a 4/4 bar) with `density = 0.2` (or a quiet mood/section intensity that lands `density_at`'s `busy` at or below 0.33) on an `[[part]] role = "arp"`.

**Mechanism.** In the non-written-rhythm path, `arp()` computes `let count = (event.length.raw() / step_length.raw().max(1)) as usize;` (line 110) and then `for position in 0..count` (line 111) writes the notes for that chord event. `step_length` is `grid.step_ticks() * {1,2,4}` depending on `busy` (lines 29-36), so at low density it is four grid steps (e.g. 480 ticks on the default 16th grid). Integer division truncates toward zero, so any `event.length` shorter than `step_length` yields `count == 0` and the loop body never runs — that chord event gets no arp notes at all. Both sibling writers guard against exactly this: `comp.rs` (lines 281-283) and `bass.rs` (lines 273-275) both explicitly insert onset 0 when the figure's own onsets would otherwise miss the chord's own start, with comp.rs's comment stating "A chord nobody strikes is a chord nobody hears change, so its own start always sounds." `arp.rs` has no equivalent guarantee.

**Expected.** Per arp.rs's module doc: "its density is a *rate* — how fast the figure climbs — rather than how many of its notes survive, because an arpeggio with holes punched in it is not a sparser arpeggio, it is a broken one" (arp.rs:5-6). The code should guarantee at least one note per chord event the way `comp` and `bass` do, rather than letting a coarse rate silently skip an event shorter than one step.

**Fix direction.** In the procedural (non-rhythm) branch of arp() at crates/auris-compose/src/parts/arp.rs:110, after computing count, force at least one onset when count == 0: emit a single note at event.start (clamped to event.length) using voicing[0], mirroring the `if !written && !chosen.contains(&0)` guard bass.rs:273-275 and comp.rs:281-283 already use to guarantee a chord's own start always sounds.

**Written rule it breaks.** arp.rs's own module doc: "its density is a *rate* — how fast the figure climbs — rather than how many of its notes survive, because an arpeggio with holes punched in it is not a sparser arpeggio, it is a broken one."

### F-263 · low · `is_stable` in crates/auris-compose/src/frame.rs:597 is defined but never called anywhere in the crate or workspace.

`crates/auris-compose/src/frame.rs:597` · other · confirmed (traced through the code; reported independently 1×)

**What a user sees.** No user-visible effect. `is_stable` computes nothing anyone sees: `cargo build`/`cargo run` behave identically with or without it since no cadence-planning path (`turn_around`, `plan`, or anything else in auris-compose or downstream crates) calls it — it's inert code sitting in the crate.

**Trigger.** Simply reading the crate: the function is reachable (it is `pub`) but exercised by nothing, so it neither has a unit test asserting its own five-quality list nor any production code path that depends on it.

**Mechanism.** `pub fn is_stable(quality: Quality) -> bool` is defined with a full doc comment ("Whether a chord is one a cadence would want to land on") but a workspace-wide grep for `is_stable` outside its own declaration returns zero hits — no caller in `frame.rs` (including its own `#[cfg(test)] mod tests`), no caller anywhere else in `auris-compose`, and no caller in any other crate.

**Expected.** Per CLAUDE.md's own convention ("Features that were removed, or that were deliberately not implemented, do not need to be written in the documentation"), a kept, documented, exported function should be reachable from somewhere; `is_stable` should either be wired into the turnaround/cadence code it was evidently written for, or removed.

**Fix direction.** Either wire it into the cadence logic it was clearly written for (e.g. have `turn_around` check the landing chord's quality via `is_stable` instead of comparing roots alone) or delete the function; if kept only for future/external use, add `#[allow(dead_code)]` with a comment explaining why, though outright removal is preferable per the project's "no release yet, break things freely" stance.

### F-292 · low · `.max(1)` in drums.rs:247 drops the first step of a full-bar snare fill when beats*per_beat exactly equals steps (e.g. 2/4 meter, fill=1.0, intensity=1.0).

`crates/auris-compose/src/parts/drums.rs:247` · theory · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** In an auto-composed drum part in 2/4 meter (or any meter where a full two-beat fill exactly equals the bar's step count), a maximal-intensity snare fill on the last bar of a section that leads into another section is missing its very first sixteenth-note hit — the fill starts one step late and sounds slightly shorter/off than intended. This is inaudible/unnoticeable in most meters and settings; it only occurs when beats*per_beat exactly equals steps.

**Trigger.** A song with `meter = "2/4"`, `fill = 1.0`, a section at `intensity = 1.0` (or any combination where `beats * per_beat == steps_per_bar` for the grid the fill runs on), on the last bar of a section that leads somewhere (`index + 1 < frame.sections.len() || frame.joins_on`).

**Mechanism.** `fill()` computes `let from = steps.saturating_sub(beats * per_beat).max(1);` (line 247). `beats` is at most 2 (from `wanted = settings.fill.clamp(0,1) * (0.6 + 0.4*section.intensity)`, `beats = (wanted*2.0).round()`, max value 2). In a bar whose total `steps` equals exactly `beats * per_beat` (e.g. a 2/4 bar at 4 steps/beat = 8 steps, with `fill = 1.0` and `intensity = 1.0` giving `beats = 2`), `saturating_sub` yields `0`, and the trailing `.max(1)` then forces `from` to `1` instead of `0` — discarding the very first step of what was meant to be a full two-beat fill window.

**Expected.** `from` should be `steps.saturating_sub(beats * per_beat)` without the trailing `.max(1)` (or the `max(1)` should only guard against a genuinely empty/negative window when `beats == 0`, which is already handled by the earlier `if beats == 0 { return; }` at line 244-246), so a full-window fill can start at step 0 like any other.

**Fix direction.** Replace `.max(1)` with a bounds check that only forces `from` away from 0 when `steps == 0` (which cannot happen), or simply drop the `.max(1)` guard entirely since `saturating_sub` already yields a well-defined value including 0 for a genuine full-bar fill; if step 0 needs protection for some other reason, express that as an explicit comment rather than an unconditional floor.

**Written rule it breaks.** DSP code lives behind unit tests that assert on numbers (levels, frequencies, lengths) rather than on "it runs".

### F-304 · low · frame.rs:169's comment claims harmony, fill, and crash all gate joins the same way, but only harmony and crash share the intensity-comparison arrival test — the fill fires unconditionally on every join.

`crates/auris-compose/src/frame.rs:169` · spec-mismatch · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** No audible or behavioral change for any user of the DAW or composer — this is purely a misleading source comment. A developer reading frame.rs:169's claim that "the harmony, the fill and the crash should all read one join the same way" and then modifying the fill's join gate in drums.rs could wrongly add an intensity comparison to "fix" it into matching, which would silently change which joins get a fill (breaking the deliberate every-edge behavior drums.rs's own adjacent comment documents).

**Trigger.** Any form with a drop, e.g. `form = "chorus verse"`: the verse gets a drum fill running into it (fill's gate only checks that a next section exists) while the harmony is not turned around and no crash is struck for that same join.

**Mechanism.** The comment ahead of the turnaround gate says: "only where the form actually arrives somewhere, which is the same question the cymbal asks: the harmony, the fill and the crash should all read one join the same way." The harmony's `turn_around` (gated on `next.intensity >= section.intensity`) and the crash's `marks_an_arrival` in `parts/joins.rs:79-84` (gated on `section.intensity >= before.intensity`) are indeed the same test from opposite ends. But the fill's own gate in `parts/drums.rs:231`, `let leads_somewhere = index + 1 < frame.sections.len() || frame.joins_on;`, has no intensity comparison at all — it fires on every join, including a drop (e.g. chorus into verse), which the harmony and crash explicitly do NOT mark as an arrival (`coming_down_out_of_a_chorus_is_not_turned_around` in this file, and `a_pop_form_crashes_where_it_arrives_and_not_where_it_comes_down` in joins.rs, both pin the opposite behaviour for their own writers).

**Expected.** Either the frame.rs comment should be scoped to just the harmony/crash agreement it demonstrably has (dropping the fill from the claim), or the fill should be gated by `marks_an_arrival`-equivalent logic if parity was actually intended.

**Fix direction.** Narrow the frame.rs:169 comment to state accurately that only the harmony's turnaround and the crash's marks_an_arrival share the identical intensity-comparison arrival test, and separately note that the fill (drums.rs) intentionally fires on every non-final join regardless of intensity direction, per its own doc comment.

**Written rule it breaks.** Every public item carries a doc comment ... CI builds the docs with warnings denied
