# Review findings: auris-sampler

Part of the [whole-repository adversarial review](README.md) of commit `52d1702`. 4 verified findings: 1 high, 3 medium.

Each entry survived an independent skeptic and an independent reproducer (and a tie-breaker when they disagreed); "executed reproduction" means the reproducer ran a test, a binary or a scratch program and observed the behaviour, "traced" means it followed the call path with concrete values.

| ID | Severity | Location | Finding |
|---|---|---|---|
| F-084 | high | `crates/auris-sampler/src/sampler.rs:676` | stop_everything(false) calls synth.note_off_all(false) immediately, starting the font's ~1ms release under the user's configured Release stage instead of […] |
| F-195 | medium | `crates/auris-sampler/src/sampler.rs:668` | stop_everything's non-immediate branch clears Slot::key before the release finishes, letting claim() steal and cut off release tails after AllNotesOff. |
| F-206 | medium | `crates/auris-sampler/src/sampler.rs:692` | step_envelopes and push call into self.synth without checking `poisoned`, contradicting the documented invariant and risking an uncaught second panic/abort on […] |
| F-226 | medium | `crates/auris-sampler/src/sampler.rs:532` | Sampler::push writes LEVEL straight into rustysynth's master_volume with no SmoothedValue ramp, unlike every other gain control in auris-dsp, causing an […] |

### F-084 · high · stop_everything(false) calls synth.note_off_all(false) immediately, starting the font's ~1ms release under the user's configured Release stage instead of deferring it like let_go does.

`crates/auris-sampler/src/sampler.rs:676` · dsp · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** On every engine seek or loop wrap (via chase_notes pushing AllNotesOff), a sampler note's configured Release tail is cut short to the SoundFont's own ~1ms release instead of the auris envelope's slower fade — audible as a click or abrupt cutoff instead of the release the user configured, on a common transport path (seeking, looping).

**Trigger.** (a) Release-defeated: turn the envelope on, set Release to e.g. 2.0s, hold a note, then deliver `NoteEvent::AllNotesOff` — which `auris_engine::renderer::chase_notes` sends automatically on every playhead seek and every loop wrap ("Drop whatever the old position left sounding"), so any project that loops with a shaped sampler track hits this every cycle. (b) Click: deliver `NoteEvent::AllSoundOff` while a shaped note is sounding (a panic/force-mute action, or a hosted CLAP plugin's note-expression all-sound-off).

**Mechanism.** `let_go` (630-662) deliberately withholds `synth.note_off` for a shaped note: "The font is not told yet: the envelope owns the fade, and telling the font now would start its own release underneath and cut the tail short. `step_envelopes` hands the note back when the level reaches silence." `stop_everything` (665-685), which `AllNotesOff`/`AllSoundOff` dispatch into (565-566), does the opposite in both branches: `slot.envelope.release()`/`.silence()` is set locally, but line 676 unconditionally also calls `synth.note_off_all(immediate)` on the same call, before the auris-side envelope has done any fading. For `immediate=false` (AllNotesOff) this is exactly the failure `let_go`'s own comment says to avoid: the font's own (often short) authored release starts concurrently with, not after, the user's configured Release stage, so the audible tail is governed by whichever is faster, not by the Release the user set. For `immediate=true` (AllSoundOff), line 670 uses `envelope.silence()` (an instant snap to level 0, per `auris_dsp::adsr::Adsr::silence`) instead of `Adsr::kill()`, which […]

**Expected.** AllNotesOff should behave like a batch of let_go() calls — set each held slot's envelope to Release and let step_envelopes hand the note back to the font only once its own level reaches silence, as let_go's own comment requires. AllSoundOff should call envelope.kill() (the documented de-click ramp) instead of .silence(), matching auris-synth::Chiptune's handling of the same NoteEvent on the same Adsr type.

**Fix direction.** In the `immediate == false` branch of `stop_everything`, do not call `synth.note_off_all(false)` at all — let `step_envelopes`/`hand_back` tell the font per-slot once each envelope's Release stage actually reaches silence, exactly as `let_go` already does. Keep `synth.note_off_all(true)` only for the `immediate` (AllSoundOff) branch, where rustysynth's `voices.clear()` is instantaneous anyway so there is nothing to defer.

**Written rule it breaks.** The font is not told yet: the envelope owns the fade, and telling the font now would start its own release underneath and cut the tail short. `step_envelopes` hands the note back when the level reaches silence.

### F-195 · medium · stop_everything's non-immediate branch clears Slot::key before the release finishes, letting claim() steal and cut off release tails after AllNotesOff.

`crates/auris-sampler/src/sampler.rs:668` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** After an All-Notes-Off (sent e.g. on a playhead jump/seek via the engine's chase_notes, or a transport stop that is not immediate), the sampler's release tails have their slot key cleared to None immediately even though the envelope has only just entered Release and is still sounding. claim() then treats that slot as free-and-least-recently-used and can hand it to the very next note that needs a channel, which retriggers over the still-decaying tail — audibly clipping the release instead of letting it ring out for the configured release time.

**Trigger.** Every transport seek/loop through a MIDI clip generates exactly this sequence at frame 0 of one block: `NoteEvent::AllNotesOff` immediately followed by `NoteEvent::NoteOn` for whatever should be sounding at the new position (see `crates/auris-engine/src/renderer.rs` lines 468-487, `resync_notes`'s comment "Drop whatever the old position left sounding before restoring what belongs to the new one"). With the sampler's envelope switch on (`ENVELOPE_KEY`) and a track that has used most/all of its 14 shaped-note channels recently — e.g. one long-sustained note (small `used`, i.e. old) held alongside several more-recently-struck notes on the other slots — a seek clears every slot's `key`, and the […]

**Mechanism.** `stop_everything` (lines 665-681) runs `slot.key = None;` unconditionally at the top of its loop, before deciding whether to `silence()` or `release()` the envelope. For the non-immediate path (`slot.envelope.release()`, line 672 — used for `NoteEvent::AllNotesOff`), this differs from every other release path in the file: `let_go` (lines 630-661) and the normal slot-finish path in `step_envelopes`/`hand_back` (lines 517-523, 691-706) keep `slot.key = Some(pitch)` for as long as the envelope is actually releasing, and only clear it once `envelope.is_finished()` (i.e. `Adsr` reaches `EnvelopeStage::Idle`, per `crates/auris-dsp/src/adsr.rs` lines 195-243). The doc comment on `key` (lines 296-299: "The key sounding here, until the envelope has taken it to silence and handed the note-off to the font") and on `claim` (lines 306-312: "A channel nobody is holding is preferred… a font's own release tail keeps sounding on a channel after the note-off has gone, and going round the houses gives it fourteen other notes of grace… When every channel is held, the quietest note is the one whose loss […]

**Expected.** `stop_everything`'s non-immediate branch should behave like `let_go` for every currently-held slot: leave `slot.key` as `Some(pitch)` while the envelope is transitioning to `Release`, and let it clear only once the envelope naturally finishes (via the existing `step_envelopes` → `hand_back` path), so `claim()`'s LRU/quietest logic keeps working exactly as documented at lines 296-299 and 306-312. Only the `immediate` branch (AllSoundOff, which really does cut everything to silence at once) […]

**Fix direction.** In stop_everything, only clear slot.key in the immediate branch (where envelope.silence() truly ends the sound); in the non-immediate branch leave slot.key as Some(pitch) and let step_envelopes/hand_back clear it once envelope.is_finished(), exactly as let_go already does for a single note-off.

**Written rule it breaks.** /// The key sounding here, until the envelope has taken it to silence and handed the note-off to the font. `None` means the channel is free, tail or no tail.

### F-206 · medium · step_envelopes and push call into self.synth without checking `poisoned`, contradicting the documented invariant and risking an uncaught second panic/abort on the audio thread.

`crates/auris-sampler/src/sampler.rs:692` · realtime · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** In the rare case a soundfont's rendering panics mid-block while the amplitude envelope is active, the sampler catches that one panic and starts filling silence for the rest of the current render_run sub-chunk, but step_envelopes (called once per INTERNAL_BLOCK before each render_run) and push (reached from set_param on every future audio-thread call until the next prepare()) keep calling into the same now-poisoned Synthesizer via process_midi_message/note_off/set_master_volume, none of which are wrapped in catch_unwind. If any of those calls panics on the corrupted synth state, the unwind is not caught and aborts the whole audio process instead of just muting the sampler — the exact failure the poisoning mechanism exists to prevent.

**Trigger.** Turn the envelope on (`ENVELOPE_KEY` >= 0.5) so the shaped path is taken, load a font whose rendering panics mid-block (the crate's own `test_support::runaway_font` reproduces this), trigger a note, and call `process()` with more than `INTERNAL_BLOCK` (32) frames — e.g. the existing test's 512-frame block. The first 32-frame sub-chunk's `render_run` panics and sets `poisoned = true`; `render_range`'s loop then runs `step_envelopes` for roughly 15 more sub-chunks in the same call, each one calling into the poisoned synth. Separately, any `set_param` call reaching `push(LEVEL)` or an envelope parameter on any subsequent block, before the next `prepare()`, does the same.

**Mechanism.** The `poisoned` field's own doc comment (lines 274-278) says: "A poisoned synthesiser is never called again — whatever state the panic left it in is not one to play", and `rendered_safely`'s doc comment (lines 800-801) repeats it to justify `AssertUnwindSafe`: "the synthesiser is never called again once poisoned: whatever invariants the panic broke are invariants nobody will read." In practice three of the four functions that touch `self.synth` on the audio thread do check the flag — `dispatch` (line 557: `if self.synth.is_none() || self.poisoned { return; }`), `render_run` (checks `*poisoned` and fills silence instead of rendering), and `reset` (line 924: `if self.poisoned { return; }` before calling `synth.reset()`). Two do not. `step_envelopes` (lines 692-707), called from `render_range`'s loop (lines 715-727) *before* the `render_run` call that would itself skip on poisoned, unconditionally calls `self.write_expression(index)` → `write_raw_expression` → `synth.process_midi_message(...)` (line 496-497), and, once an envelope finishes, `self.hand_back(index)` → […]

**Expected.** Every function that reaches `self.synth` from the audio thread should check `self.poisoned` first, the way `dispatch`, `render_run` and `reset` already do — `step_envelopes` (and by extension `render_range`'s loop) and `push` are missing that guard, contradicting the doc comments at lines 276 and 800-801 that the poisoning mechanism is built on.

**Fix direction.** Add `if self.poisoned { return; }` guards at the top of `step_envelopes` and `push` (or check it once at the top of `render_range`'s while-loop and inside `push`/`set_param`), matching the guard already present in `dispatch`, `render_run`, and `reset`, so no path reaches `self.synth` once `poisoned` is set until the next `prepare()`.

**Written rule it breaks.** "A poisoned synthesiser is never called again — whatever state the panic left it in is not one to play" (sampler.rs poisoned field doc) / "the synthesiser is never called again once poisoned: whatever invariants the panic broke are invariants nobody will read" (rendered_safely doc comment)

### F-226 · medium · Sampler::push writes LEVEL straight into rustysynth's master_volume with no SmoothedValue ramp, unlike every other gain control in auris-dsp, causing an audible step at the next internal render block.

`crates/auris-sampler/src/sampler.rs:532` · dsp · confirmed (traced through the code; reported independently 1×)

**What a user sees.** Automating the sampler's Level parameter, or dragging its fader while a note is sounding, produces an audible click or zipper-noise step in the output instead of a smooth volume change — the new master volume is applied on rustysynth's next internal 32-frame block but combined with the previous block's stale per-voice mix gain, creating a discontinuity at that boundary.

**Trigger.** Move or automate the sampler's `level` parameter while a note is sounding (e.g. drag the fader during playback, or an automation lane driving `level`).

**Mechanism.** `push(LEVEL)` calls `synth.set_master_volume(NOMINAL_VOLUME * db_to_gain(value))` synchronously with no ramp. `vendor/rustysynth/src/synthesizer.rs`'s `render_block()` (362-386) uses one `self.master_volume` field for both ends of its per-voice interpolation: `previous_gain_left = self.master_volume * voice.previous_mix_gain_left; current_gain_left = self.master_volume * voice.current_mix_gain_left;` — both read the *same, current* master_volume. If `set_master_volume` is called between two internal `render_block()` invocations, the new block's interpolation start point (`new_master_volume * old_voice_gain`) does not match the actually-rendered end of the previous block (`old_master_volume * old_voice_gain`), producing a step at the internal-block boundary. The same -60..12 dB gain control exists elsewhere in the workspace and is explicitly ramped for this reason: `crates/auris-dsp/src/gain.rs`'s `GainPan` (the `auris.fx.gain` effect) documents `SMOOTHING_SECONDS = 0.020` as the "Ramp time for gain and pan moves. Long enough to remove the step discontinuity that causes zipper […]

**Expected.** Ramp master_volume toward the target over a short window the way GainPan ramps its gain (a SmoothedValue-style per-block or per-sample interpolation), instead of writing db_to_gain(value) straight into set_master_volume on every parameter change.

**Fix direction.** Wrap the Level control in a SmoothedValue (as auris-dsp's gain.rs, delay.rs, chorus.rs, and compressor.rs already do for their gain/mix parameters) and advance it per audio block, calling synth.set_master_volume with the ramped value on each render call instead of once per parameter event in push.

**Written rule it breaks.** DSP code lives behind unit tests that assert on numbers (levels, frequencies, lengths) rather than on "it runs".
