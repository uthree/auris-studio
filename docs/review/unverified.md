# Claims the review could not settle

Part of the [whole-repository adversarial review](README.md). For these 34 claims the verification agents repeatedly returned unusable output, so they carry only the first-pass reviewer's argument. Treat them as leads, not findings.

### F-312 · critical (unverified) · Negative/out-of-range INITIAL_FILTER_Q drives the low-pass filter unstable

`vendor/rustysynth/src/bi_quad_filter.rs:58` · dsp

**Trigger.** A SoundFont instrument/preset region whose `initialFilterQ` generator is a small negative value (e.g. raw generator value -14, i.e. -1.4 dB — well inside the i16 range and not an extreme value) combined with an ordinary `initialFilterFc` cutoff near one quarter of the output sample rate (e.g. ~11 kHz at 44.1 kHz, itself a completely unremarkable, spec-legal cutoff). No modulators are needed at all.

**Mechanism.** `set_low_pass_filter` computes `let q = resonance - BiQuadFilter::RESONANCE_PEAK_OFFSET / (1_f32 + 6_f32 * (resonance - 1_f32));` (lines 58-59) with no domain restriction on `resonance`. `resonance` comes from `SoundFontMath::decibels_to_linear(region.get_initial_filter_q())` (voice.rs:159), and `get_initial_filter_q()` is `0.1 * gs(INITIAL_FILTER_Q)` (region_pair.rs:96-98) where `INITIAL_FILTER_Q` is a raw signed-16-bit SoundFont generator with no clamp anywhere in the crate (confirmed by grep: the value flows straight from `Generator::read_from_chunk`'s `u16 as i16` reinterpretation through `set_parameter` to this formula). For `resonance` between roughly 0.833 and 0.888 (i.e. `INITIAL_FILTER_Q` slightly negative, an out-of-spec but entirely unvalidated value), the denominator `1 + 6*(resonance-1)` crosses zero and `q` swings to a large-magnitude value; working through the formula for […]

### F-052 · high (unverified) · training/README.md Quick Start omits the `export` extra its own next step needs

`training/README.md:32` · spec-mismatch

**Trigger.** A new contributor copies the three Quick Start commands verbatim from training/README.md: `uv venv --python 3.11`, `uv pip install -e '.[dev]' --torch-backend=auto`, then the preprocess/train/export block.

**Mechanism.** Line 32 reads `uv pip install -e '.[dev]' --torch-backend=auto`, installing only the `dev` extra from `training/pyproject.toml`. Ten lines later, line 42 of the same Quick Start block runs `uv run python scripts/export_onnx.py --checkpoint runs/base/checkpoints/last.ckpt --output voice.onnx`. `scripts/export_onnx.py` imports `auris_singer.export`, whose `export_onnx()` does `import onnx` (training/src/auris_singer/export.py:407) and whose `verify_onnx()` does `import onnxruntime` (training/src/auris_singer/export.py:469) — both packages live only in the `export` optional-dependency group in `training/pyproject.toml` (`onnx`, `onnxscript`, `onnxruntime`), not in `dev`. CLAUDE.md's own Commands section confirms the intended install line is `uv pip install -e '.[dev,export]' --torch-backend=auto`, and `.github/workflows/ci.yml`'s training job likewise installs `'.[dev,export]'`.

### F-315 · high (unverified) · write_lyrics silently no-ops on empty text instead of clearing selected notes

`crates/auris-session/src/session/singer.rs:209` · correctness

**Trigger.** Select one or more notes in the piano roll, invoke 'Write Lyrics' (`open_write_lyrics_prompt`), and press Return without typing anything (the field opens empty already) — or type text, delete it all, and confirm.

**Mechanism.** prompt.rs groups `PromptTarget::Lyrics { .. }` into `empty_clears` (prompt.rs:667, with the comment 'Emptiness means something on the singing fields — take the word or the correction off the note') specifically so an empty submission is allowed through instead of being refused as 'cannot be empty'. `open_write_lyrics_prompt` (piano_roll.rs:1517-1523) even opens the field already empty. But `Session::write_lyrics` (singer.rs:171-225), which that empty text is handed to, computes `portions` from `split_kana_lyric(text.trim())` — and `split_kana_lyric("")` walks zero characters and returns `Some(vec![])` (kana.rs:46,57-77), not `None`, so the dictionary fallback is never reached either. `filled = order.len().min(portions.len())` is therefore always 0 for empty text, and `if filled == 0 { return Ok(0); }` (singer.rs:209-211) returns *before* `self.record(...)` or any note mutation. Contrast […]

### F-325 · high (unverified) · dressed()'s anti-stammer fix turns a smooth germ contour into a zigzag

`crates/auris-compose/src/parts/melody.rs:365` · correctness

**Trigger.** Any generated melody figure whose cell count relative to the piece-level germ's cell count produces a non-integer interpolation step (essentially any rhythm/germ pairing other than exact-multiple lengths landing on whole-number contour points) — reached on essentially every real seed/preset, since germ and rhythm lengths are independently drawn.

**Mechanism.** For each interpolated position, `dressed()` rounds `target` independently and then, only if the rounded value equals the *previously assigned* degree while the raw interpolated line moved by more than `f32::EPSILON` (line 361-367), force-steps the degree by `moved.signum()`. Because the comparison is against the last *assigned* (possibly already force-stepped) degree rather than the accumulated rounding residual, and because the trigger fires on any nonzero movement (not just a movement large enough to actually deserve a step), a forced step at position N changes what position N+1 is compared against, producing spurious overshoot-and-return oscillation instead of a monotone staircase. Concrete trace: for a germ with degrees [0,2,4,2] dressed onto a 7-cell rhythm the test suite happens to hit only exact-integer interpolation points (line/step ratio divides evenly), masking the bug. But […]

### F-332 · high (unverified) · Overriding `part` or `section` to an empty TOML collection silently reverts the whole roster/form to hardcoded defaults

`crates/auris-compose/src/spec/doc.rs:588` · spec-mismatch

**Trigger.** `SongSpec::parse_with_overrides(text, &[("part".to_string(), "[]".to_string())])` where `text` declares a custom `[[part]]` roster (e.g. one part named "lead"). This is directly reachable through `auris-toolbox`'s `overrides: Option<BTreeMap<String,String>>` argument (lib.rs:88-90), which is handed to a tool-calling LLM with no restriction to scalar fields ("Every name is a field of the format itself").

**Mechanism.** `apply_overrides` (doc.rs:142-154) accepts any string as a field name with no whitelist, and `toml_value` (doc.rs:161-166) reads a value 'as TOML can read it' — so an override value of `"[]"` parses as `toml::Value::Array(vec![])` and `"{}"` as an empty table. `SongDoc::into_spec` then gates the roster and section-table assignment purely on emptiness: `if !self.section.is_empty() { spec.sections = ... }` (line 578) and `if !self.part.is_empty() { ... spec.parts = parts; }` (line 588). An override that sets `part`/`section` to an *explicit* empty collection is therefore indistinguishable from the field never having been mentioned at all: the `if` is skipped, and `spec.parts`/`spec.sections` are left at whatever `SongSpec::default()` produced (the 6-part built-in roster / the intro-verse-chorus-outro sections) instead of the document's real, already-parsed roster/sections — with no error […]

### F-334 · high (unverified) · Ornament-handle grab ignores which pointer gesture was pressed

`crates/auris-gpui/src/ui/piano_roll.rs:1074` · correctness

**Trigger.** On a singer track with default gestures (create=⌘-click, delete=⌥-click), a note carries a scoop, fall or vibrato ornament whose handle (per the code's own comment) sits outside the note's rectangle in visually-empty grid space. A ⌘-click there, intended to create a new note on what looks like empty grid, instead begins Drag::Ornament and opens an Edit::SetScoop/SetFall/SetVibrato transaction on the neighbouring note; the intended note is never created. The same happens for an ⌥-click meant to […]

**Mechanism.** In begin_note_drag, `if self.editing_a_singer_clip() && let Some((index, handle)) = self.grabbed_ornament_at(...) { ... begin_drag(Drag::Ornament ...); return; }` (piano_roll.rs:1074-1090) fires whenever the press is within ORNAMENT_GRAB pixels of a note's scoop/fall/vibrato handle, with no check against `self.pointer.create` or `self.pointer.delete` (contrast the delete branch immediately above it at line ~1060, which does gate on `self.pointer.delete.matches(event)`). Because grabbed_ornament_at's own doc says handles 'float off their notes' into space that visually looks like empty grid, and because this check runs before the `None => match empty_press(...)` branch that would otherwise treat the press as Create or a deselect/Band sweep, any left-click landing on a handle — whatever modifier is held — is captured as an ornament edit instead of the gesture the modifier requested.

### F-335 · high (unverified) · Fader/pan ramp duration equals the render block size, so export can differ from playback

`crates/auris-engine/src/offline.rs:30` · correctness

**Trigger.** Any project with an automated gain/pan lane, or a manual fader/pan move made during playback, then exported/bounced with the default export options while the live session uses its default 512-frame playback block. Concretely: play a track while nudging its fader (ramp completes over ~512 frames, ~10.7ms at 48kHz), then export the same project (`OfflineOptions::whole_project()`, block_frames=1024, ramp completes over ~21.3ms) — samples from roughly frame 512 through 1024 of the ramp differ […]

**Mechanism.** `SmoothedGain::advance()` (crates/auris-engine/src/graph/strip.rs:70-74) has no notion of a fixed ramp duration — each call just returns `(current, target)` and immediately sets `current = target`. `RenderGraph::apply_automation` (crates/auris-engine/src/graph/mod.rs:703-720, doc at 691-693: 'a gain and a pan are both *targets* that the strip ramps across the block it is given, so a segment-rate write comes out as a continuous slope') and `apply_gain_and_pan` (strip.rs:503-531) both drive this once per *segment*, and `ramp()` (strip.rs:566-582) then linearly interpolates over exactly `samples.len()` — the segment's own frame count. A segment's length is bounded by `graph.max_block()` (renderer.rs `segment_frames`, line 63), which is whatever `block_frames` the graph was built with. Realtime playback builds its graph with `Settings::audio.block_frames` […]

### F-336 · high (unverified) · Re-arming a track mid-take repoints the live monitor but not the already-recording WavRecorder

`crates/auris-session/src/session/record.rs:352` · correctness

**Trigger.** Start a take with a track armed to input channel 0 (`session.start_recording(...)`), then while `session.is_recording()` is true call `session.arm_track(track, Some(InputChannels::mono(1)))` to repoint it (e.g. correcting a wrong mic assignment mid-take).

**Mechanism.** `arm_track` (record.rs:335-363) has no guard on `self.take.is_some()`. When a track is already armed, `(Some(at), Some(input)) => self.armed[at].input = input` (line 352) overwrites the stored `InputChannels` in place, then `self.point_monitor()` → `publish_monitors()` immediately re-seats the live `MonitorRing`'s `source` to the new channels (monitor.rs:160). But the `TakeStream` the running take is actually writing through was constructed once, at `start_recording` time, from the *old* `InputChannels` (record.rs:658-665) and never re-reads `self.armed`; `write_take`'s `pick_channels` call (record.rs:1161) keeps using that frozen `TakeStream.input` for the rest of the take.

### F-338 · high (unverified) · Set Key Here always inserts a new key point instead of editing the one in force

`crates/auris-gpui/src/ui/context_menu/timeline.rs:252` · correctness

**Trigger.** A song has a key change to D minor starting at bar 8 (C major before it). Right-click the harmony lane at bar 12 (well inside the D-minor stretch, no key boundary there), choose 'Key Here...' -- the prompt shows 'D minor' (the correct in-force key) -- and confirm, whether unedited or retyped as a correction (e.g. 'D melodic minor').

**Mechanism.** harmony_menu builds `MenuCommand::SetChordAt(target.chord)` where `target.chord` is resolved by `harmony_target` to the *start tick of the chord currently in force* at the click (via `harmony.chords.change_at(tick)`, timeline.rs:352-363) -- exactly so that 'Chord Here' edits 'the one you can see' rather than adding a new point wherever the pointer happened to round to (the doc comment at timeline.rs:210-214 states this explicitly). The sibling row two lines later, `.item(self.t(Key::MenuSetKeyHere), MenuCommand::SetKeyAt(placed))` (line 252), passes `placed` -- the *raw click position rounded to the harmony grid* (`self.session.snap_harmony(tick)`, line 222) -- with no equivalent lookup of the in-force key's own start tick. `AurisApp::run_menu_command`'s `SetKeyAt(tick)` handler (command.rs:1229-1233) opens a prompt pre-filled with `harmony.key_at(tick).to_text()` (the key that IS in […]

### F-344 · high (unverified) · Modulator::addresses_the_same wrongly includes transform, breaking global/local override

`vendor/rustysynth/src/modulator.rs:81` · spec-mismatch

**Trigger.** A font declares a global-zone modulator (e.g. velocity → initialFilterFc, transform=Linear, amount=X) and a local zone (instrument or preset) modulator with the same source/destination/amount-source but a different transform (e.g. transform=Absolute Value, amount=Y) — an ordinary way to author a per-region override that also changes how the value is shaped. Per spec this local modulator should entirely replace the global one.

**Mechanism.** `addresses_the_same` (lines 77-82) requires `self.source == other.source && self.destination == other.destination && self.amount_source == other.amount_source && self.transform == other.transform` before `merge_modulators` (preset_region.rs:24-31) will let a local-zone modulator replace a global-zone one, or let the last of two same-zone duplicates win. Checked against FluidSynth's reference implementation of the SF2.01 §9.5.1 identity rule (`fluid_mod_test_identity`, which compares only `dest`, `src1`/`flags1` (source) and `src2`/`flags2` (amount source) — explicitly *not* `trans`) and a search-engine summary of the spec text agreeing that a modulator's identity for override/duplicate purposes is exactly the triple (source, destination, amount-source), the extra `self.transform == other.transform` term here is not part of that rule. The doc comment above the function (lines 74-76) […]

### F-348 · high (unverified) · collect_clap_files recurses through symlinks with no cycle guard

`crates/auris-session/src/session/hosted.rs:770` · correctness

**Trigger.** `Session::installed_clap_files` (hosted.rs:786-806), called by any plugin browser, walks both the OS-standard CLAP folders and every path in its caller-supplied `extra` list — explicitly documented as covering "a build tree, a bounced copy on an external disk, a shared folder on a studio machine". Any of those containing a symlink/junction that resolves back to an ancestor directory (a stray build artifact, a cloud-sync placeholder, a mis-made shortcut) drives `collect_clap_files` into […]

**Mechanism.** `collect_clap_files` (hosted.rs:759-774) walks a directory and, for every entry that is not itself named `*.clap`, calls `path.is_dir()` (line 770) and recurses (line 771) with no set of already-visited directories and no depth limit. `Path::is_dir()` calls `fs::metadata`, which follows symlinks (unlike `symlink_metadata`), so a symlink under any searched root that points back at one of its own ancestors is walked into as an ordinary directory on every recursive call, forever.

### F-349 · high (unverified) · macOS: minimizing an embedded plugin window is read as the user closing it

`crates/auris-clap/src/window/cocoa.rs:106` · platform

**Trigger.** On macOS, open an embedded-mode hosted plugin's editor (the common case — every JUCE-built plugin, which per this crate's own docs is most of them) and click the window's minimize button, or otherwise let it become non-visible without actually closing it.

**Mechanism.** `HostWindow::was_closed` is `self.shown.get() && !self.window.isVisible()` (line 106). The container window is created with `NSWindowStyleMask::Miniaturizable` (lines 40-42), giving it a working yellow minimize button, and `NSWindow.isVisible` is well known to report `NO` while a window is miniaturized (not only when it is actually closed/ordered out) — the same ambiguity the suspicion raised for Mission Control/Spaces applies even more directly to the window's own minimize control. `ClapPlugin::take_requests` ORs this into `PendingRequests::gui_closed` (plugin.rs:160-163), and `Slot::service` in auris-session acts on it unconditionally: `if requests.gui_closed { plugin.close_gui(); closed = true; }` then `self.editor = false` (crates/auris-session/src/session/hosted.rs:471-490), and `close_gui` destroys the plugin's CLAP GUI resources and drops the container window (plugin.rs:403-431).

### F-162 · medium (unverified) · save_project leaves the scratch file behind when the write step itself fails

`crates/auris-io/src/project_file.rs:167` · correctness

**Trigger.** Any real I/O failure during `std::fs::write(&in_progress, json)` — most concretely a disk that fills up while Auris Studio writes the scratch JSON for a save (autosave or manual save). The project's own test for this function, `a_failed_save_leaves_the_previous_version_intact` (project_file.rs lines 388-406), has to call `std::fs::remove_dir(&blocker).unwrap();` itself right after asserting the save failed, because `save_project` never removes the entry on this path — that manual cleanup line […]

**Mechanism.** `save_project` writes JSON to the sibling scratch path with `std::fs::write(&in_progress, json).map_err(...)?;` (line 167) and propagates any failure immediately via `?`. Only the *second* failure point — `std::fs::rename` at lines 168-171 — calls `let _ = std::fs::remove_file(&in_progress);` before returning. If the write to `in_progress` itself fails (the realistic case this crate repeatedly calls out: 'a full disk, a dropped network share'), the function returns `Err` without ever attempting to delete the scratch file it may have just created or partially filled. `write_wav` in export.rs (lines 182-186), which shares this exact `in_progress_path` scheme, cleans up on *both* of its failure branches, so this is an asymmetry specific to `save_project`.

### F-178 · medium (unverified) · A non-finite sample makes rms/rms_db() return NaN, not the documented -120 dB floor

`crates/auris-gpu/src/analysis.rs:84` · dsp

**Trigger.** Call `analyze_loudness_cpu` (or the GPU path) on any `AudioBuffer` containing a single non-finite (NaN or infinite) sample, e.g. `AudioBuffer::from_planar(vec![vec![0.5, f32::NAN, 0.5]], 48_000.0)`.

**Mechanism.** `peak` and `true_peak` are accumulated exclusively through Rust's NaN-ignoring `f32::max` (`peak = peak.max(value.abs());` line 134, and `true_peak = true_peak.max(catmull_rom(...).abs());` line 141, then `true_peak_estimate: true_peak.max(peak)` at line 89) — a NaN operand is always discarded by `.max()`, so those two fields can never become NaN from a NaN sample. `sum_squares`, by contrast, is accumulated with plain addition — `sum_squares += (value * value) as f64;` (line 135) in the CPU reference, and identically on the GPU path via `sum_squares = sum_squares + value * value;` (loudness.wgsl:58) folded into the host total with `sum_squares += slot[2] as f64;` (analysis.rs:219) — and IEEE-754 addition propagates NaN unconditionally, so one non-finite sample anywhere in the buffer poisons `sum_squares` for every channel processed afterward (`sum_squares` is one running total across […]

### F-192 · medium (unverified) · Plugin window's height ceiling omits the title bar, capping the list short

`crates/auris-gpui/src/ui/plugin_window.rs:173` · ui

**Trigger.** Open the plugin window for the default instrument (`auris.synth.chiptune`, `auris_session::registry::DEFAULT_INSTRUMENT`), which has 16 parameters (crates/auris-synth/src/chiptune.rs) and an ADSR envelope (attack/decay/sustain/release keys, so `envelope_of` returns `Some`, adding `GRAPH_HEIGHT` = 84px to `above`). Needed height = PANEL_HEADER_HEIGHT(22) + body(16*22 + 15*4 + 2*8 = 428) + above(84) = 534px, which exceeds the ceiling `frame.height` = MAX_LIST_HEIGHT(420) + above(84) = 504px, so […]

**Mechanism.** `PluginWindow::frame` computes the window's height ceiling as `Self::MAX_LIST_HEIGHT + above` (line 173), where `above` sums every non-scrolling element drawn *between the title bar and the list* — the analyser curve, the envelope graph, the caution strip, the sidechain row (lines 315-327). The title bar itself (`Metrics::PANEL_HEADER_HEIGHT`, 22px) is never added to this ceiling, even though it is likewise a `flex_shrink_0` element drawn above the scrolling list (rendered at line ~374 with `.h(Metrics::PANEL_HEADER_HEIGHT)`) and is present on every window. The doc comment on `frame()` explains exactly why `above` content must be *added* to the ceiling rather than taken out of it ("otherwise giving a plugin a picture would have taken a third of its sliders away with it") — the identical reasoning applies to the header, which is drawn even higher up, but the header is not included. The […]

### F-218 · medium (unverified) · vocal_rhythm drops empty-syllable phrases; write_vocal zips positionally and silently misaligns everything after one

`crates/auris-compose/src/vocal.rs:91` · correctness

**Trigger.** Call `let rhythm = vocal_rhythm(&[0, 3], meter);` (rhythm.phrases has length 1: just the 3-syllable phrase's slots) and `write_vocal(&harmony, Ticks::ZERO, &rhythm, &[vec![], vec![Contour::Free, Contour::Free, Contour::Free]], VocalRange::default(), seed)` — the natural way to build the second argument from the same 2-phrase source (an empty-contour phrase followed by a 3-syllable one). The zip pairs `rhythm.phrases[0]` (the real 3-syllable slots) with `phrases[0]` (the empty Vec meant for the […]

**Mechanism.** `vocal_rhythm` (line 83) iterates `counts.iter().copied().filter(|count| *count > 0)` (line 91), so any phrase with a syllable count of 0 contributes NO entry to `VocalRhythm.phrases` — the returned `phrases` vector is shorter than `counts` whenever any count is 0, and the surviving entries no longer correspond by index to the original per-phrase list. `write_vocal`'s own doc (lines 225-227) says `phrases` (the per-phrase `Vec<Contour>` argument) 'must line up with rhythm — both come from the same lyric', and it then pairs them purely positionally: `rhythm.phrases.iter().zip(phrases).enumerate()` (line 245). If a caller builds its `contours: Vec<Vec<Contour>>` with one entry per ORIGINAL phrase (the natural, 1:1 way to build it from the same source list `counts` was derived from, and exactly the shape produced by an empty phrase yielding an empty `Vec<Contour>`), the zip pairs the wrong […]

### F-355 · medium (unverified) · Settings::save() writes settings.json in place, unlike the project file's own save

`crates/auris-session/src/settings.rs:318` · persistence

**Trigger.** The process is killed (crash, forced shutdown, power loss) while `std::fs::write` is mid-flight inside any of the many production call sites in auris-gpui (app.rs, agent_chat.rs, ui/commands.rs) that call `self.settings.save()` on ordinary actions like closing a settings panel, changing the audio device, or resizing the window.

**Mechanism.** `pub fn save(&self) -> Result<(), SessionError>` (settings.rs:309-319) serialises to JSON and then calls `std::fs::write(&path, text)` directly against `settings.json`. `std::fs::write` opens the target with truncate+create and writes in place; a crash, power loss, or full disk partway through leaves a truncated or partially-written file. The sibling document writer in `auris-io::project_file::save_project` (crates/auris-io/src/project_file.rs:156-172) deliberately avoids exactly this: it writes to an `in_progress` sibling scratch file and renames it over the target, with a doc comment spelling out why ("Writing straight to `path` would truncate it first, and a failure after that point would destroy the user's work with no backup to fall back on"). `Settings::save()` does not follow that pattern.

### F-364 · medium (unverified) · Reusing a RenderJob for two renders silently drops hosted CLAP plugins

`crates/auris-session/src/render.rs:174` · correctness

**Trigger.** On a `RenderJob` built from a project containing at least one hosted CLAP effect slot or hosted CLAP instrument track, call `job.render(&options, &mut progress)` and then call `job.render_stems(&folder, &settings, &options, &mut progress)` on the SAME job (or call either method a second time). Nothing in RenderJob's public API (both methods are plain `&mut self`) prevents this.

**Mechanism.** RenderJob::render (render.rs:174-194) calls render_project_using, which calls OfflineRender::new, which calls RenderGraph::build_with(project, bank, registry, &mut self.placed, &mut self.instruments, ...). RenderJob::render_stems (render.rs:322-373) independently calls OfflineRender::new the same way, against the same self.placed/self.instruments maps. build_with's own doc comment (auris-engine/src/graph/mod.rs:68-73) states plainly: 'Building **takes** each effect it uses, so a map that still holds entries afterwards names slots that are no longer in the project.' Concretely, graph/strip.rs:315 does `placed.remove(&slot.id)` per effect slot, and graph/mod.rs's instrument arms do `instruments.remove(&track.id)` (e.g. around line 333). A hosted CLAP plugin exists ONLY in these two maps -- per this repo's own CLAUDE.md, 'A hosted plugin cannot go through PluginRegistry' -- so once a […]

### F-369 · medium (unverified) · KILL_SECONDS doc overstates de-click attenuation above 500 Hz

`crates/auris-dsp/src/adsr.rs:33` · dsp

**Trigger.** Any force-silence of a sounding voice while its level is non-trivial: `Adsr::kill()` is reached from `NoteEvent::AllSoundOff` in every built-in instrument that shapes a voice (crates/auris-synth/src/noisedrum.rs:267, chiptune.rs:379, fm2.rs:278, vocal.rs:251) — ordinary operation, not an edge case.

**Mechanism.** The doc comment on `KILL_SECONDS` (lines 30-35) reads: "Cutting a sounding voice to zero in one sample injects a step whose spectrum covers the whole band — an audible click. Ramping over 2 ms pushes that energy below about 500 Hz and down by more than 40 dB". `Adsr::kill` (line 174) sets `kill_step = level / samples` and `EnvelopeStage::Kill` processing (line 222) does `self.level -= self.kill_step` every sample — a plain straight-line ramp from `level` to 0 over `samples = (KILL_SECONDS * sample_rate).max(1.0)` (2 ms = 96 samples at 48 kHz). The frequency response of that linear ramp relative to an instantaneous cut is a sinc: its first null does sit near 1/KILL_SECONDS ≈ 500 Hz as the comment implies, but a sinc's sidelobes beyond that null decay slowly — I computed the DTFT of the actual 96-sample ramp directly (Node.js, direct summation) and got, relative to the 0 Hz level: 500 Hz […]

### F-370 · medium (unverified) · Docs promise the lyric composer never breaks pitch accent; the code deliberately lets it

`docs/features.md:346` · spec-mismatch

**Trigger.** Call compose_from_lyrics (or the compose_lyrics MCP/agent tool) with a lyric whose accent-obeying pitch path cannot also land the phrase's final note on the underlying chord (CADENCE_NONCHORD=5.0) or requires a forbidden leap — the DP search then prefers the finite CONTOUR_BREACH=8.0 penalty over the alternative, producing a melody that goes against the word's spoken pitch shape at that syllable.

**Mechanism.** docs/features.md:344-347 says the melody search '— the Orpheus constraint — **does not contradict the lyric's spoken pitch accent**: the line rises where the word rises and falls exactly where its accent falls'. CHANGELOG.md:171-172 makes the same absolute claim ('the tune must not contradict the lyric's spoken pitch accent'). But crates/auris-compose/src/vocal.rs:169-174 documents the opposite intent: 'Breaching a syllable's contour — expensive, and deliberately not impossible. Orpheus reports its own melodies overruling the accent about six times in a hundred, nearly always where a cadence outranks a word; a hard constraint would instead refuse to end phrases.' `CONTOUR_BREACH` is a finite cost (8.0), not `f64::INFINITY` the way an actually-forbidden move (e.g. a tritone leap, `leap_cost` match arm `6 => f64::INFINITY`) is — so the Viterbi search can and by design sometimes will pick […]

### F-372 · medium (unverified) · Note-end resize check pre-empts phoneme-boundary drag near a note's end

`crates/auris-gpui/src/ui/piano_roll.rs:1113` · ui

**Trigger.** A singer-clip note whose phoneme layout places a phoneme boundary (from phoneme_layout, drawn via phoneme_divider_zones) within the last resize_grab pixels of the note's end — realistic for a short or heavily-phonemed note where consonants pack close to the note's end. Clicking at that x position always begins a note resize.

**Mechanism.** Once note_at has found a note under the pointer, begin_note_drag tests the pixel-space resize zone first (`if f32::from(end_x - (event.position.x - origin.x)).abs() <= grab { begin_drag(Drag::NoteResize ...) }`, line 1112-1118) and only falls through to `self.grabbed_boundary_at(&note, clip_start, ...)` (line 1119-1121) for Drag::PhonemeDuration in the `else if`. grab is up to RESIZE_HANDLE=5px (or width/3 for a narrow note). Both zones are painted with the identical CursorStyle::ResizeLeftRight (phoneme_divider_zones's own doc: 'both wear the left-right resize arrow, because both drag a vertical edge'), so there is nothing on screen distinguishing them, but the handler always resolves the overlap in favour of resize.

### F-394 · medium (unverified) · OfflineRender::render panics on out-of-bounds slice instead of returning EngineError for a short buffer

`crates/auris-engine/src/offline.rs:387` · correctness

**Trigger.** Call `OfflineRender::render(&mut out, &mut progress)` with an `out` buffer whose `frame_count()` is less than `render.frames()` (`self.total`) — e.g. `let mut render = OfflineRender::new(...)?; let mut out = AudioBuffer::new(RENDER_CHANNELS, 10, sample_rate); render.render(&mut out, &mut RenderProgress::default())` where `render.frames()` is much larger than 10.

**Mechanism.** The doc at line 346 states `out` "must be [`Self::frames`] long" but `render` (lines 350-400) never checks `out.frame_count()` against `self.total`. The copy at lines 386-389, `out.channel_mut(channel)[at..at + count].copy_from_slice(...)`, indexes `out` up to `at + count` (which can reach `self.total`), so any `out` shorter than `self.total` frames panics via out-of-bounds slice indexing rather than returning a `Result`. This is inconsistent with the rest of the same function, which goes out of its way to convert every other size hazard (an absurd `end_frames`, an overflowing `span`/`total`/`end`) into `EngineError` instead of letting the allocator or an index panic.

### F-403 · medium (unverified) · render_project_using silently drops hosted plugins if called twice with the same maps

`crates/auris-engine/src/offline.rs:198` · correctness

**Trigger.** Call `render_project_using` (or build two `RenderJob::render()`/`render_to_wav()` calls on the same `RenderJob`) twice with the same `PlacedEffects`/`PlacedInstruments` maps, on a project with a hosted CLAP effect or instrument in its chain. The first call drains the map entries for every hosted plugin id; the second call finds nothing in the map for those slot ids and falls through to `registry.create_effect(...)`, which returns `Err` for a hosted-only id, so the slot becomes a bypassed […]

**Mechanism.** `render_project_using` (offline.rs:189-201) hands `placed`/`instruments` straight to `OfflineRender::new` -> `RenderGraph::build_with`, which consumes each entry via `placed.remove(&slot.id)` in `RenderStrip::from_mixer` (graph/strip.rs). Nothing in `render_project_using`/`OfflineRender::new` checks whether the caller's maps were already drained by an earlier call. `RenderJob::render` (auris-session/src/render.rs:189-201) calls `render_project_using(..., &mut self.placed, &mut self.instruments, ...)` fresh on every invocation but never consumes `self` and is `pub fn render(&mut self, ...)`, so nothing stops a caller from invoking `job.render(...)` (or `render_to_wav`) twice on the same `RenderJob`.

### F-408 · medium (unverified) · Clip fade-in/fade-out can overlap and double-attenuate when source material is short

`crates/auris-engine/src/graph/schedule.rs:281` · dsp

**Trigger.** A document satisfies the session-layer invariant enforced by `Session::set_clip_fades` (`crates/auris-session/src/session/clips.rs:756-763`, itself documented as guarding exactly this: 'crossed fades would multiply into a dip no hand drawn') — e.g. clip.length_frames=1000, fade_in_frames=400, fade_out_frames=600 (sum == length). If the audio source the clip resolves to at render time actually has fewer frames available from the clip's offset than the document's length_frames assumes (e.g. […]

**Mechanism.** resolve_audio_clip first computes `let length = convert(clip.length_frames).min(available - source_offset);` (line 240), which can be strictly smaller than the clip's full converted length whenever the resolved buffer (`bank.stretched`/`bank.get`) has fewer frames available than the document's stored `offset_frames + length_frames` implies (exactly the scenario the crate's own test `audio_clips_are_clamped_to_the_source_length` exercises). It then independently clamps `fade_in: convert(clip.fade_in_frames).min(frames)` (line 277) and `fade_out: convert(clip.fade_out_frames).min(frames)` (line 281) to this shortened `frames`, without clamping fade_out to `frames - fade_in` the way the document layer does. `RenderAudioClip::fade_gain` (lines 58-75) then multiplies the fade-in and fade-out curves together for any position inside both zones, and `renderer.rs:517` applies that gain to every […]

### F-409 · medium (unverified) · Autosave's external-writer guard can miss a same-tick external write

`crates/auris-session/src/session/autosave.rs:139` · persistence

**Trigger.** Two processes with the same project open (e.g. the GUI session and an `auris-mcp`/`auris-cli` session) each write the file within the same filesystem mtime tick — most easily reproduced on a coarse-granularity filesystem (FAT32: 2s, HFS+: 1s, many SMB/NFS mounts: 1-2s), but possible on any filesystem given close enough timing.

**Mechanism.** `externally_modified` (138-146) and `mark_saved` (123-130) detect another writer purely by comparing `std::fs::Metadata::modified()` SystemTime equality. If a second writer (another auris-session process — the MCP door, auris-cli, a sync service, as the doc comment itself names) writes to the same `.auris` file and the filesystem's mtime lands on the same value this session already recorded as `disk_stamp` — which happens whenever two writes fall within one tick of the filesystem's timestamp granularity (coarser than a second on FAT32/HFS+/many network shares, and possible even on fine-grained filesystems for near-simultaneous writes) — `now != stamp` is false, so `externally_modified()` reports `false` and `should_autosave` proceeds to overwrite the file. The project's own test at autosave.rs:243-244 sidesteps this exact scenario rather than covering it: "Set explicitly rather than […]

### F-415 · medium (unverified) · A shipped font whose local byte size drifts from a saved document's recorded size is loaded twice under two ids

`crates/auris-session/src/session/files.rs:610` · persistence

**Trigger.** Point `AURIS_SOUNDFONTS` (or otherwise substitute the file at a library root) at a copy of `MuseScore_General.sf2` whose byte count differs from the one recorded in an already-saved project's `SoundFontRef::byte_size` (e.g. a different build/version of the shipped font, or a user-supplied replacement under the same file name), then open that project. The same situation recurs automatically whenever a future release ships an updated General MIDI font under the same file name with a different […]

**Mechanism.** `Session::open` calls `self.reload_assets()` before `self.install_shipped_fonts()` specifically so that, per the comment at files.rs:246-251, 'the search finds this machine's copy and writes the new path into the document, and only then does the id it already has match the file about to be installed. The other way round, the same font would arrive twice under two ids — and be held in memory twice.' `reload_assets` (assets.rs:38-97) can only rewrite the stored reference by calling `locate()` (assets.rs:261-273), which falls back to `find_named(stored.file_name()?, search, expected_size)` (auris-io/src/assets.rs:78-90) — and `find_named` rejects any candidate whose size does not exactly equal the document's recorded `byte_size` (unless that size is 0). If the shipped font actually installed on this machine differs in size from what the document recorded, `locate()` returns `None`, so […]

### F-420 · medium (unverified) · copy_into's check-then-copy can silently overwrite a concurrent writer's file

`crates/auris-io/src/assets.rs:53` · concurrency

**Trigger.** Two writers race to place a file under the same first-attempt candidate name in the same project's `Audio/` directory at nearly the same moment — plausible given this codebase already anticipates concurrent writers on one open document elsewhere (autosave.rs:134 explicitly names "the MCP door, a sync service" as another writer that can touch the same project file while a session has it open); an import triggered from `auris-mcp`/`auris-cli` racing an import in `auris-gpui` against the same […]

**Mechanism.** `copy_into`'s own doc comment (lines 37-41) promises: "A name already taken in `directory` is never overwritten." The implementation checks `if !target.exists() { std::fs::copy(file, &target)...; return Ok(candidate); }` (lines 53-56) — a classic TOCTOU: nothing prevents another writer from creating `target` between the `exists()` check and the `fs::copy` call, and `std::fs::copy` unconditionally truncates/overwrites whatever is at `target` when it runs, regardless of what put it there.

### F-279 · low (unverified) · Vendored Cargo.toml points to a Cargo.toml.orig that was never committed

`vendor/rustysynth/Cargo.toml:10` · other

**Trigger.** Anyone reading this vendored, forked crate's manifest — which CLAUDE.md explicitly flags as noteworthy ("kept in vendor/rustysynth ... see vendor/rustysynth/README.md") — and following the pointer to see what upstream's manifest actually declared.

**Mechanism.** The auto-generated header (lines 1-10) tells a reader to consult `Cargo.toml.orig` for "the original contents" of the manifest before cargo normalized it. `git log --all` for `vendor/rustysynth/Cargo.toml.orig` returns nothing — the file has never existed in this repository's history — and `ls vendor/rustysynth/` confirms it is absent today (only Cargo.lock, Cargo.toml, LICENSE.txt, README.md, src). Whoever committed this file ran it through cargo's publish/package normalization and kept only the generated output, not the original hand-written manifest the comment refers to.

### F-282 · low (unverified) · Attachment-ceiling error message rounds the reported file size down to match the ceiling

`crates/auris-agent/src/main.rs:752` · correctness

**Trigger.** `auris-agent --model m --attach clip.wav "..."` where `clip.wav` is, say, 25.5 MB (26,738,688 bytes) — an entirely ordinary audio file just over the stated limit.

**Mechanism.** `size / (1024 * 1024)` (line 752) is integer division, truncating toward zero, used to report the offending file's size in the refusal message at lines 747-755. For any file between `ATTACHMENT_CEILING` (25 MB) and just under 26 MB, `size / (1024*1024)` truncates to 25 — the same number printed for the ceiling itself (`ATTACHMENT_CEILING / (1024*1024)` = 25, line 753) — producing the self-contradicting sentence "`<path>` is 25 MB; audio over 25 MB is refused".

### F-425 · low (unverified) · rename_track records an undo step and dirties the document on a no-op rename

`crates/auris-session/src/session/tracks.rs:254` · correctness

**Trigger.** `session.rename_track(id, session.project().track(id).unwrap().name.clone())` — renaming a track to the name it already has (e.g. a text field committed on blur without the text having changed, or a redundant scripted rename).

**Mechanism.** `Session::rename_track` (tracks.rs:248-259) calls `self.require_track(id)?;` then unconditionally `self.record(Edit::RenameTrack);` (line 254) before writing `track.name = name.into();` — unlike `set_track_color`/`set_track_height` in the same file, which each compare the incoming value against the current one and return early when unchanged (tracks.rs:266-280, 484-505), `rename_track` has no such comparison.

### F-428 · low (unverified) · A disabled (file-missing) font row still records an open click though its affordance is suppressed

`crates/auris-gpui/src/ui/library.rs:1342` · ui

**Trigger.** Click a SoundFont row whose backing file has gone missing (`soundfont_is_loaded(id)` is false), which draws with no pointer cursor and no hover highlight — the row a user would reasonably assume is inert.

**Mechanism.** branch_row's own comment (1301-1303) says a branch with nothing to open "must not offer the pointer and the hover fill of a row that would answer a click" — and gates `cursor_pointer()`/`hover(...)` behind `.when(enabled, ...)` at 1304-1307. But `.on_mouse_down(gpui::MouseButton::Left, on_click)` at line 1342 is attached unconditionally, outside that gate. `soundfont_rows` calls this with `enabled: loaded` (1128), and its `on_click` (1133-1136) unconditionally runs `this.library.set_open(branch, !open)`.

### F-432 · low (unverified) · CHANGELOG misattributes per-speaker level measurement as pre-existing behavior

`CHANGELOG.md:101` · other

**Trigger.** N/A — a historical/documentation claim, falsified by diffing commit 7cf5fe1 against its parent for training/src/auris_singer/phoneme_levels.py.

**Mechanism.** CHANGELOG.md:101-102 says '`measure_phoneme_durations.py --data` measures every speaker of a labelled dataset at once, as the level script always did.' But `git show 7cf5fe1 -- training/src/auris_singer/phoneme_levels.py` (the very commit that added this changelog line) shows `measure_dataset()` in phoneme_levels.py changing its return type from `dict[str, list[float]]` (one flat pool, `return measure(utterances())`) to `dict[str, dict[str, list[float]]]` keyed by speaker (`by_speaker.setdefault(str(record["speaker"]), []).append(...)`) in that same commit. Before this commit the level script pooled every speaker's readings together undifferentiated; it did not 'always' measure per speaker.

### F-437 · low (unverified) · list_instruments hides a headless-session failure inside a fake success string

`crates/auris-toolbox/src/lib.rs:1370` · correctness

**Trigger.** Call `list_instruments` in an environment where `headless()` fails to construct a `Session` (e.g. the shipped SoundFont/dictionary the build embeds is missing or corrupted).

**Mechanism.** `list_instruments::run()` (line 1362) has signature `-> String`, not the `Result<String, String>` every other Args-based tool in this crate returns. When `headless()` fails, the error is folded into the return value as a line of text: `Err(error) => text.push_str(&format!("  (unlisted: {error})\n"))` (line 1370) — the function still returns `Ok`-shaped output (a plain `String`) to its caller.

### F-441 · low (unverified) · Routing docs claim the walk is 'exactly' Track::feeds(), but sidechains are included too

`crates/auris-core/src/project/routing.rs:7` · spec-mismatch

**Trigger.** Any project where a track's effect has a sidechain set — an ordinary, fully-supported feature reachable through Session::set_effect_sidechain — makes the routing walk depend on an edge that Track::feeds() does not enumerate.

**Mechanism.** The module doc says the three routing questions ('the order to render in, whether an edit would leave a bus waiting for itself, and which tracks a solo leaves audible') 'are walks over Track::feeds' (line 7: 'All three are walks over Track::feeds'), and Track::feeds()'s own doc (track.rs:441-443) claims the same three questions are 'a walk over exactly these' (output + sends). In fact routing_order (routing.rs:331), routing_would_cycle (routing.rs:382) and solo_resolution/audible_through (routing.rs:582, 611) all build on the private routing_edges() (routing.rs:305-322), whose own doc correctly states it adds a reversed edge for every track.mixer.sidechain_sources() on top of track.feeds() — an edge recorded on an EffectSlot, not on the Track that feeds() claims to fully describe.

