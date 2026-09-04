# Claims the review made and then refuted

Part of the [whole-repository adversarial review](README.md). These 40 claims were raised by a first-pass reviewer and then knocked down by the verification stage — usually because a guard, an invariant or a documented intent the reviewer missed rules the scenario out. They are kept because the next reviewer will raise them again.

### F-010 · read_wave_data heap-buffer-overflows on odd-sized `smpl` chunk

`vendor/rustysynth/src/binary_reader.rs:123`

**Why it does not hold.** Both verifiers agree on the underlying mechanism, and I confirmed it by reading the code: `read_wave_data` (vendor/rustysynth/src/binary_reader.rs:123-134) allocates a `Vec<i16>` of `size/2` elements, then unsafely reinterprets it as a `size`-byte slice and fills it with `read_exact` — for odd `size` this is a genuine one-byte heap overflow (or a write through a dangling pointer when `size == 1`). That part of the claim is correct and not in dispute.

The dispute is reachability, and the code resolves it. The only path by which an untrusted `.sf2` file's bytes reach `SoundFont::new`/`read_wave

### F-020 · Export overlay occludes root without rebinding mouse move/up, stranding drag transactions

`crates/auris-gpui/src/ui/root.rs:496`

**Why it does not hold.** Both reviewers agree on the code fact: render_export_overlay's occluding div (root.rs:496) never re-registers on_mouse_move/on_mouse_up, unlike compose_sheet and plugin_window. The dispute is whether this strands the drag transaction. It does not, because root registers a *third*, independent handler the Reproducer's own cited range (30-240) contains but didn't weigh: `.on_mouse_up_out(gpui::MouseButton::Left, cx.listener(Self::on_mouse_up))` at root.rs:244, immediately after on_mouse_move/on_mouse_up (238-239), with its own comment: "a release *outside* the window ends them too... without thi

### F-030 · One track's write error during a multi-track take discards every track's clip, not just the failed one

`crates/auris-session/src/session/record.rs:1168`

**Why it does not hold.** Both verifiers agree on the mechanics: in `write_take` (record.rs:1146-1191), a write failure on one track's `WavRecorder::write` sets `failure`, breaks only the inner per-stream loop, and the outer loop does `break Err(error);` at line 1169 without calling `finish_all(streams)` (unlike the `is_finished()` and quiet-shutdown exits at 1174/1185, which do). `stop_recording`'s `result?` (line 774) then returns that `IoError` before `begin_transaction`/`land_take` (825-845) ever runs, so no track lands a clip that take — confirmed by reading the code and by the Reproducer's isolated repro.

The di

### F-031 · Stem export aborts entirely on a Windows-reserved track name

`crates/auris-session/src/render.rs:277`

**Why it does not hold.** Both disputants agree on the Rust-side facts: `sanitised_name` (render.rs:277-291) really does pass reserved Windows device stems like "Aux", "Con", "Nul", "Com1" through unchanged, `stem_file_name` (line 261) appends ".wav" unconditionally, and `render_stems` (line 358-359) propagates any `write_wav` error with a bare `?` inside the per-track loop, which would indeed abort the whole stem export on the first failure and skip every later track. So the *code shape* of the claim is accurate and the *consequence*, if triggered, is exactly as described.

The whole claim hinges on one empirical prem

### F-051 · One non-finite sample silences the whole spectrum display, not just its own bin

`crates/auris-dsp/src/spectrum.rs:137`

**Why it does not hold.** Both reviewers agree on the mechanism (unguarded push at spectrum.rs:137, radix-2 FFT spreads one NaN to every bin, NaN > 0.0 is false so magnitudes() reports SILENCE_DB for every bin) — the Reproducer's standalone repro confirms this arithmetic. But the claim's trigger requires that a plugin's non-finite sample can reach SpectrumAnalyzer::push. It cannot in the real pipeline: push's only production caller is Session::spectrum (crates/auris-session/src/session/mod.rs:722-732), which gets its samples exclusively from `self.scope.read(&mut samples)`. Scope::publish (crates/auris-engine/src/scope

### F-074 · recipe_for clamps octave to ±2, silently mispitching a regenerated composed clip

`crates/auris-compose/src/phrase.rs:157`

**Why it does not hold.** Both verifiers agree on the mechanism and that it is reachable through real session commands (regenerate_clip/reroll_clip) — I verified the same code paths (phrase.rs:157, :293; spec/doc.rs:762-767/963-968; spec/mod.rs:219; recipe.rs:243-248). The dispute is purely whether this is a defect or documented intent, and the documentation trail is unusually thick and precise for this exact scenario:

- phrase.rs, right above the clamp (line ~154-156): "A part's octave is absolute and a recipe's is a shift from where the preset sits... Clamped to what the dial can hold: a part written four octaves of

### F-107 · SessionOptions::with_sample_rate lets a non-finite/non-positive rate reach Project unchecked

`crates/auris-session/src/session/mod.rs:485`

**Why it does not hold.** The claim asserts that `SessionOptions::with_sample_rate` / `Session::new` let a non-finite or non-positive sample rate flow unchecked into `Project`, corrupting the document on save. But the guard the reviewer says is missing already exists — one layer down, at the actual point the value becomes a stored document field.

`Project::new` (crates/auris-core/src/project/mod.rs:427-432) is:
```rust
pub fn new(name: impl Into<String>, sample_rate: f64) -> Self {
    let sample_rate = if sample_rate.is_finite() && sample_rate > 0.0 {
        sample_rate
    } else {
        48_000.0
    };
    ...
`

### F-111 · Unbounded allocation from unchecked file length fields in binary_reader.rs

`vendor/rustysynth/src/binary_reader.rs:96`

**Why it does not hold.** Both verifiers agree on the underlying library mechanism, and it checks out: `read_fixed_length_string` (binary_reader.rs:96), `discard_data` (line 119), and `read_wave_data` (line 128) all do `vec![0; n]` from a caller-supplied `usize` before `read_exact`, and that `usize` traces back to raw `u32` chunk-size fields in soundfont_info.rs / soundfont_sampledata.rs with no bound check. That part of the claim is accurate as a description of vendor/rustysynth in isolation.

The dispute is about reachability, and the claim's own "consequence" clause stakes everything on it: "a memory-exhaustion deni

### F-120 · Exclusive-class voice stealing only chokes one of several matching voices

`vendor/rustysynth/src/voice_collection.rs:37`

**Why it does not hold.** The claim requires an "old" note with two regions sharing a non-zero exclusive class to first produce two simultaneously-active sibling voices, so that voice_collection.rs's search can later choke only one of them. But that precondition is impossible under the exact code cited. In note_on (synthesizer.rs:235-247), regions of the SAME note-on are processed in the same loop that calls request_new then immediately .start() before moving to the next region. Voice::start sets `self.exclusive_class = region.get_exclusive_class()` (voice.rs:132) synchronously. So when the note's second region (same c

### F-125 · Most non-character keys bypass the IME-composing guard

`crates/auris-gpui/src/ui/prompt.rs:1207`

**Why it does not hold.** The claim's description of prompt.rs's own code is accurate — only `"escape" if !composing` and the two `"enter" if !composing` arms are guarded; `"up"`/`"down"` and the catch-all `field.apply_key(...)` (which handles backspace/delete/left/right/home/end/select-all) run unconditionally. But the claim's trigger requires that gpui actually deliver such a KeyDown to the app while `field.marked().is_some()` — and on both platforms this app ships (per CLAUDE.md: "macOS and Windows both run the desktop application"), that precondition is either impossible or already neutralized before prompt.rs's ma

### F-132 · A one-syllable phrase's only note skips the phrase-final cadence cost

`crates/auris-compose/src/vocal.rs:316`

**Why it does not hold.** Both verifiers agree on the code fact: `CADENCE_NONCHORD` (weight 5.0) is added only inside `for at in 1..count`, which is empty when `count == 1`, so a one-syllable phrase's sole note is priced solely by the `first` row — `register(*pitch) + harmony_cost(&slots[0], *pitch, None) + jitter(...)` — and `harmony_cost` with `arrived_by: None` charges at most `OPENING_NONCHORD` (1.0), never the 5.0 cadence penalty. That much is real.

The disagreement is over the claimed consequence: that this gap lets a single-mora phrase's note "land off the underlying chord... audibly wrong." I verified the Repr

### F-143 · Send fader id repeats the 64-slot collision insert_element_key was written to fix

`crates/auris-gpui/src/ui/mixer.rs:397`

**Why it does not hold.** Both reviewers agree on the raw facts, and I confirmed them independently: mixer.rs:397 reads `("mixer-send", index * 64 + position)`, `insert_element_key` (inspector.rs:83-91) was introduced specifically to retire that packing for effect slots (its doc comment and the test `every_insert_row_gets_its_own_element_key` at inspector.rs:871-889 literally cite "strip 1 slot 0 and strip 0 slot 64 both came out as 64" as the case it fixes), effect rows in the same file already use `insert_element_key` (mixer.rs:134), and `Session::add_send` (auris-session/src/session/tracks.rs:108-127) enforces no ca

### F-159 · Every automated test of the real singing voice and Japanese dictionary is always skipped in CI

`crates/auris-gpui/src/harness.rs:522`

**Why it does not hold.** Both verifiers agree, and I confirmed by reading the code, that the mechanical claim is true: harness.rs:522 (and every sibling site) does `let Some(model) = std::env::var_os("AURIS_SINGER_TEST_MODEL") else { return; };`, an early return from a #[gpui::test]/#[test] fn that Rust's test harness reports as passed, not skipped. `.github/workflows/ci.yml`'s macos/windows/linux/training jobs never set AURIS_SINGER_TEST_MODEL or AURIS_JAPANESE_DICTIONARY, so this branch is what every CI run actually takes.

Where the claim fails is its "expected" clause — that this needs to be "visible as a document

### F-161 · RegionPair keeps dead, unmodulated duplicates of the two fork-modulated getters

`vendor/rustysynth/src/region_pair.rs:90`

**Why it does not hold.** The factual mechanism is real and both verifiers agree on it: region_pair.rs:90-94 (get_initial_filter_cutoff_frequency) and :104-106 (get_modulation_envelope_to_filter_cutoff_frequency) compute their result from raw self.gs(...) only, Voice::start (voice.rs:154-158, 166-169) reads those two generator destinations exclusively via region.modulated(...), and a crate-wide grep confirms the two getters have zero call sites. The dispute is whether this is a defect worth reporting or an accepted byproduct of the project's stated vendoring policy — and the documented intent settles it.

Three indepen

### F-172 · "Keep Progression" is offered for the untouched, auto-generated default chart

`crates/auris-gpui/src/ui/compose_sheet/menus.rs:150`

**Why it does not hold.** Both verifiers agree on the mechanism; the dispute is purely about intent, and the record settles it decisively in favor of the skeptic.

1. `SongSpec::default()`'s own comment (spec/mod.rs:471-473) states the design purpose directly: "Marked generated, not quoted: a progression the user did not ask for is the composer's own, so the mood is free to colour it. A chart anyone typed or named is left alone." The default chart is deliberately built as `ChartOrigin::Generated` with `quoted_as: None` — that combination exists specifically to make the default chart eligible for mood/cadence colouring,

### F-185 · WavRecorder's sample counter desyncs from the file after a mid-block write error

`crates/auris-io/src/record.rs:96`

**Why it does not hold.** Both reviewers agree on the mechanism, and it's real: `WavRecorder::write` (record.rs:89-99) only does `self.samples += block.len() as u64` (line 98) after the whole per-sample loop succeeds, so a mid-block `write_sample` error via the `?` on line 96 leaves `self.samples` short of what hound already durably wrote. The Reproducer's standalone repro correctly demonstrates this in isolation.

The disagreement is entirely about the claim's trigger→consequence chain: "the caller stops recording and calls finish() after the error… the natural response to a write failure … a caller that trusts the re

### F-200 · empty_press's "shift always sweeps" claim is false for every create gesture but Click

`crates/auris-gpui/src/gestures.rs:174`

**Why it does not hold.** The disagreement turns on what the empty_press doc comment (gestures.rs:166-175) actually promises. Its two sentences read together: "Before it, both sites read 'create if the create gesture matches, otherwise band', which is fine while create needs a modifier and silently costs the rubber band the moment it does not" — naming the one broken configuration the function exists to fix: create = Click, the sole gesture with no modifier of its own, where an ordinary unmodified click would always satisfy create and "otherwise band" would leave no press meaning sweep. The next sentence, "PointerGestu

### F-225 · Rng::jitter panics on a negative sigma via f32::clamp

`crates/auris-core/src/rng.rs:170`

**Why it does not hold.** The panic mechanism itself is real: `f32::clamp` at rng.rs:170 panics whenever `min > max`, and for `sigma < 0.0` that's exactly what `-3.0*sigma` vs `3.0*sigma` produces — the Reproducer's standalone repro correctly demonstrates this in isolation.

But the audit standard requires the trigger be reachable from a real entry point, and on the `dev` branch under audit it is not. I verified every call site of `.jitter(` in the working tree (excluding the unrelated detached-HEAD worktree under `.claude/worktrees/`, which is not part of `dev`):

1. `crates/auris-core/src/project/transform.rs:146` —

### F-243 · OPENAI_API_KEY is auto-sent to any --url without confirming it matches

`crates/auris-agent/src/main.rs:207`

**Why it does not hold.** Both verifiers agree on the mechanism (they read the same code), so the dispute is purely whether it's a defect. It isn't, for three independent reasons visible in the source itself:

1. Documented at the point of use. The --help text (main.rs:99-100) states plainly, right next to --url: "OPENAI_API_KEY is used for openai when it is set and this is not given." A user typing --url for the openai provider is shown, in the same screen, that the ambient key will be used unless a different --api-key-env is named.

2. Pinned by a named test asserting intent. the_key_comes_from_the_environment_and_it

### F-244 · GPU min/max on a NaN sample can diverge from the CPU reference

`crates/auris-gpu/src/shaders/waveform.wgsl:43`

**Why it does not hold.** Both reviewers agree, and I independently confirmed, that the technical mechanism is real in isolation: waveform.wgsl:43-44 folds with plain WGSL `min`/`max`, which naga's SPIR-V backend (pinned at naga 30.0.0, the version this workspace's Cargo.lock resolves) lowers to `GlslStd450Op::FMin`/`FMax` (naga-30.0.0/src/back/spv/block.rs:1315-1324) — a family whose NaN-operand choice is driver-defined, unlike the NaN-avoiding `f32::min`/`max` (LLVM minnum/maxnum) used by the CPU reference `reduce()` in waveform.rs:159-172. This part of the Reproducer's trace is accurate.

The verdict turns entirely

### F-253 · GPU min/max reduction is not guaranteed NaN-safe like the CPU reference it must match

`crates/auris-gpu/src/shaders/loudness.wgsl:56`

**Why it does not hold.** Both verifiers agree on the WGSL mechanics — `min`/`max` in the reduction loop (loudness.wgsl:56-59, 64-66) are not guaranteed to ignore NaN the way Rust's `f32::max` (used in `analyze_loudness_cpu`, analysis.rs:126-148) does, and the Reproducer's naga/SPIR-V trace (FMin/FMax's undefined NaN behaviour on the Vulkan path this project actually ships on Windows) is not disputed and looks credible. The disagreement is entirely about reachability, and the Skeptic's evidence resolves it decisively.

`analysis.rs`'s own module doc (lines 8-16) states outright: "**Neither exists.** The export dialog r

### F-256 · Automation on a track's send can be misdirected because send positions aren't index-stable like effect slots

`crates/auris-engine/src/graph/automation.rs:87`

**Why it does not hold.** Both verifiers agree on the mechanism, and it's real: resolve_slot's ParamTarget::Send arm (automation.rs:87-94) resolves to `project.track(track)?.sends.iter().position(|existing| existing.id == send)?` — a *document* position — while drive_automation's Send arm (automation.rs:157-167) indexes the *render-side* `RenderTrack.sends` Vec with that number. RenderGraph::build_with builds that Vec via `.filter_map(|send| Some(RenderSend { target: bus_slot(send.target)?, ...}))` (graph/mod.rs:437-444), silently dropping any send whose target isn't a real bus track — which would compact later sends'

### F-257 · Limiter silently drops the ceiling guarantee on channels beyond prepare()'s count

`crates/auris-dsp/src/limiter.rs:139`

**Why it does not hold.** Both verifiers agree on the mechanism: `Limiter::process` (limiter.rs:139) computes `channels = buffer.channel_count().min(self.lines.len())`, and any channel beyond `self.lines.len()` (the count `prepare()` last sized `self.lines` to) is left completely untouched — not delayed, not limited. The Reproducer's standalone repro correctly demonstrates this is real, executable code behavior.

The dispute is whether this is a *defect*. It is not — it is a project-wide, deliberately designed and already-tested contract, not a limiter-specific oversight. `crates/auris-dsp/src/pack.rs` contains a test,

### F-273 · ChordScale's doc calls a 6-note collapsed scale "C mixolydian"

`crates/auris-core/src/theory/chord_scale.rs:18`

**Why it does not hold.** Line 18 ("which is C mixolydian") sits inside one continuous doc comment on `ChordScale`, and the very next paragraph (lines 21-23) — part of the same comment block, rendered as one continuous passage in `cargo doc` — immediately says: "Six notes rather than seven is not a loss: those *are* six-note scales. Two degrees standing either side of one borrowed note both give way to it, which is exactly what happens when a dominant arrives — the notes a semitone from its third are the notes nobody plays over it." A reader of the doc comment as written (not line 18 in isolation) is told in the same b

### F-280 · The single dispatch point for every menu/keyboard command carries zero tests

`crates/auris-gpui/src/ui/context_menu/command.rs:629`

**Why it does not hold.** The claim rests on two factual assertions, both of which are false on inspection.

First, "grep finds no #[test] or #[gpui::test] anywhere in this file" is true only in isolation — command.rs itself has none — but the claim's real thrust is that run_menu_command is never exercised by tests. That is false. `crates/auris-gpui/src/ui/piano_roll.rs` has a `#[gpui::test]` named `the_menu_toggles_ornaments_on_a_sung_note` (line 3165) whose body calls `this.run_menu_command(MenuCommand::SetVibrato{...}, cx)`, `run_menu_command(MenuCommand::SetScoop{...}, cx)` and `run_menu_command(MenuCommand::ResetO

### F-284 · cut_notes returns Ok(0) instead of erroring on an unknown clip

`crates/auris-session/src/session/clipboard.rs:128`

**Why it does not hold.** The mechanism both reviewers describe is factually correct and undisputed: cut_notes (clipboard.rs:127-135) returns Ok(0) for an unknown clip because it branches on copy_notes's count, and copy_notes folds "clip doesn't exist" and "selection is empty" into the same 0 return; remove_notes/remove_notes_as (notes.rs:102-118) checks clip existence first and returns Err(SessionError::UnknownClip). The dispute is whether this divergence is a defect or intended module design; the evidence favors design.

1. copy_notes's own signature (-> usize, not Result) and doc comment state the policy directly: "

### F-294 · Phoneme-boundary drag clamp can force a duration past the note's own end

`crates/auris-gpui/src/ui/root.rs:894`

**Why it does not hold.** Both verifiers agree on the mechanism and its reachability, and I confirmed both independently: root.rs:892-894's `widest` computation collapses to exactly `MIN_PHONEME_SECONDS` whenever `end_seconds - from_seconds < 2*MIN_PHONEME_SECONDS`, and `grabbed_phoneme_boundary` (piano_roll.rs:152-172, filter `*to < length`) only guarantees the grabbed boundary is strictly before the note's end, not that any particular amount of room remains -- so a sub-20ms remaining span is reachable through ordinary use (pinning several phonemes at their legal minimum inside a short, grid-floored note), as the repr

### F-298 · save_project stamps format_version/saved_by even when the save then fails

`crates/auris-io/src/project_file.rs:157`

**Why it does not hold.** Both verifiers agree on the mechanics: save_project (project_file.rs:157,161) stamps project.format_version and project.saved_by on the caller's &mut Project before any I/O, so a failed write/rename leaves those fields stamped despite nothing reaching disk — the Reproducer's repro genuinely exercises this and the numbers it printed are correct.

The dispute is whether this is a defect, and the doc comment on save_project settles it: "In memory the field means nothing at all: a `Project` this build holds has this build's shape whichever way it was built. ... It only becomes a claim when it is w

### F-303 · process() leaves stale data in out beyond ctx.block_frames instead of overwriting it

`crates/auris-sampler/src/sampler.rs:946`

**Why it does not hold.** The claim's mechanism (out.frame_count() > ctx.block_frames leaves the tail untouched) is real as isolated code behavior, but it is not a defect in Sampler::process — it is Sampler::process correctly honoring a documented type invariant that every caller in the workspace, and every other Instrument implementation, relies on.

`ProcessContext::block_frames` is documented (plugin.rs:204) as "Always equal to the buffer's frame count" — this is a stated precondition of the type itself, not an implementation detail of the sampler. `Instrument::process`'s own doc contract (plugin.rs:496-498) is writ

### F-305 · Left/Right/Backspace/Home/End are not withheld from an active IME composition

`crates/auris-gpui/src/ui/prompt.rs:1242`

**Why it does not hold.** Both verifiers agree on the mechanism: `composing = field.marked().is_some()` (prompt.rs:1193) guards only the `escape`/`enter` arms; Left/Right/Backspace/Home/End (and Up/Down) fall to the generic arm at 1242 and call `TextField::apply_key`, whose `move_left`/`move_right`/`backspace`/`replace`/`move_caret` unconditionally set `self.marked = None` (text_field.rs `replace`, ~line 104). That part of the claim is accurate.

The claim only becomes a live defect if the platform can actually deliver such a keystroke as a `KeyDownEvent` to `prompt_key` while a composition is still open, and tracing g

### F-309 · chase_notes is an unbounded linear scan of prior events on the RT thread

`crates/auris-engine/src/renderer.rs:448`

**Why it does not hold.** Both verifiers agree the mechanism is real: chase_notes (renderer.rs:439-488) runs a for loop over events[..upto] at line 448, linear in prior event count, called from render_source on the RT audio callback thread whenever the playhead jumps (seek, loop, first block). The dispute is whether this breaches the project's realtime rule. It does not: CLAUDE.md, auris_session::guide::realtime, and device.rs's own module doc all state the same four-item contract (no allocation, no locking, no blocking, no I/O) for the audio callback thread, and chase_notes violates none of the four - counts/velocity

### F-319 · Dead intra-doc link `Session::set_monitoring` breaks `cargo doc -D warnings`

`crates/auris-session/src/session/record.rs:111`

**Why it does not hold.** Both reviewers agree on the mechanism (record.rs:111 has a dead `[`Session::set_monitoring`]` link to a method renamed to `set_track_monitoring` by commit f0c836e) — that much is real. The dispute is the consequence: does it break `cargo doc --workspace --no-deps` under `RUSTDOCFLAGS=-D warnings`, which is CI's exact Document step? It does not, because the doc comment sits on `pub(super) struct Take` inside a plain (non-`pub`) `mod record;` (session/mod.rs:51), which is outside rustdoc's default-documented surface, and CI passes no `--document-private-items` flag (confirmed by grep across the

### F-321 · menu_key's "every binding out of reach" claim is false; bound shortcuts fire while a menu is open

`crates/auris-gpui/src/ui/root.rs:1397`

**Why it does not hold.** The reviewer's gpui-internals analysis (dispatch_action_on_node fires before finish_dispatch_key_event/on_key_down) is accurate as far as it goes — I confirmed it against the vendored gpui-0.2.2 source (window.rs:3730-3852). But the reviewer missed the upstream guard that makes the trigger unreachable: the root element's KeyContext is not the static "Auris" context all the commands (Undo, DeleteTrack, DeleteSelection, etc.) are bound under — it is recomputed every render by `AurisApp::window_context()` (crates/auris-gpui/src/app.rs:1603), which calls the free function `window_context(claimed,

### F-324 · A pitched-vocabulary program name on a drum part is accepted silently, playing the wrong kit

`crates/auris-compose/src/spec/doc.rs:761`

**Why it does not hold.** The mechanism the reviewer describes is real code — `Program::parse` (gm.rs:300-315) does search PROGRAMS then KITS with no role parameter, and `PartSpec::sound()` (spec/mod.rs:153-156) does pick bank/patch purely from `role.is_drum()`. But the crate states, in two places, that this cross-vocabulary reinterpretation is exactly the intended behavior, not an omitted check:

1. gm.rs module doc (top of file): "So one field on a part covers both: on a pitched role it is a program, on a drum role it is a kit. **Which one it is read as is never a guess, because the role already says.**" — this is a

### F-342 · set_param never clamps gain/send dB, letting db_to_gain overflow to Infinity/NaN

`crates/auris-session/src/session/mixer.rs:363`

**Why it does not hold.** The claim's premise is accurate: set_param/set_send_level store gain_db/level_db unclamped, unlike the automation path. But the claim's consequence is false: sane_gain() in strip.rs guards SmoothedGain::new/set_target/jump_to, so an Infinity gain from db_to_gain is replaced with 0.0 before ever reaching advance(), the only place gain touches a sample.

### F-398 · render_stems discards which stems already succeeded when a later write fails

`crates/auris-session/src/render.rs:359`

**Why it does not hold.** The mechanism is real and both reviewers agree on it: render_stems (render.rs:322-368) builds `stems: Vec<StemSummary>` across a loop, and `write_wav(&path, &out, &settings)?;` at line 359 propagates any write error immediately via `?`, dropping the accumulated `stems`. The disagreement is whether this is a *defect* — i.e. an inconsistency with the crate's own documented policy, distinct from and worse than the already-accepted cancellation behavior, and analogous to `collect_assets`. On close reading it is not:

1. The doc comment's actual promise (render.rs:317-318, "a cancellation leaves th

### F-411 · write_vocal's Viterbi DP can silently pick an infeasible predecessor as if valid

`crates/auris-compose/src/vocal.rs:329`

**Why it does not hold.** The DP mechanism the reviewer describes is real as written: at crates/auris-compose/src/vocal.rs:355-357, `best` starts at `(f64::INFINITY, 0usize)` and only updates on `cost < best.0`, so if every transition into a candidate is `f64::INFINITY` (leap_cost's tritone/`>12`-semitone case, lines 201-211), `best` stays `(INFINITY, 0)` and the backtrack (lines 366-375) would treat predecessor index 0 as chosen without ever having compared it. That part of the reviewer's mechanism is accurate.

But the claim fails on reachability, which the reviewer's own `from_suspicion` field already flags as uncon

### F-450 · SingingDataset trusts metadata's has_durations over the npz's own durations key

`training/src/auris_singer/data/dataset.py:74`

**Why it does not hold.** Both verifiers agree on the code shape: dataset.py:74-76 computes self.labelled from metadata's has_durations, and __getitem__ (89-93) independently gates the durations tensor on the npz's own durations key. collate_batch's all-check means one item lacking the key silently drops it for the whole batch. That mechanism is real and undisputed. The disagreement is whether metadata.jsonl and the npz files can actually drift apart through any path this codebase provides. They cannot: pipeline.py's run_preprocess sets features[durations] and has_durations from the same local variable in the same loop

### F-451 · ornament_vocal's closing Fall can silently land on no note at all

`crates/auris-compose/src/vocal.rs:143`

**Why it does not hold.** The mechanism the reviewer describes is real in isolation: `ornament_vocal` (vocal.rs:143-147) computes the closing-Fall target tick from `rhythm.phrases.last()` without consulting the notes actually returned, while `write_vocal` (vocal.rs:272-275) `continue`s past — silently dropping — any phrase whose `slots.iter().any(|slot| slot.candidates.is_empty())`. If that dropped phrase were the rhythm's last, `Some(note.start) == last` (line 155) would never match and the fall would land nowhere.

But the claim's load-bearing assertion is that this is "exactly the call pattern both real callers... u

### F-455 · Vendored rustysynth fork keeps upstream's exact version/repository metadata

`vendor/rustysynth/Cargo.toml:15`

**Why it does not hold.** The claim's mechanism has three problems that together sink it.

First, reachability: the reviewer's own "trigger" is hypothetical — "any tool that reads this workspace's Cargo manifests... to produce a dependency listing, SBOM, or license/advisory report (cargo-license, cargo-cyclonedx, cargo-deny, etc.)". No such tool exists anywhere in this repo. `.github/workflows/ci.yml` runs only `cargo fmt --check`, `cargo clippy`, `cargo test`, and `cargo doc` — no license scanner, no SBOM generator, no `cargo-deny`. `.github/workflows/release.yml` likewise has nothing of the sort. There is no entry po

