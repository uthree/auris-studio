# Review findings: vendor/rustysynth

Part of the [whole-repository adversarial review](README.md) of commit `52d1702`. 9 verified findings: 1 critical, 1 high, 4 medium, 3 low.

Each entry survived an independent skeptic and an independent reproducer (and a tie-breaker when they disagreed); "executed reproduction" means the reproducer ran a test, a binary or a scratch program and observed the behaviour, "traced" means it followed the call path with concrete values.

| ID | Severity | Location | Finding |
|---|---|---|---|
| ✅ F-018 | critical | `vendor/rustysynth/src/instrument.rs:33` | A crafted/corrupt SF2 file's zone-index fields cause an unchecked out-of-bounds slice panic in vendor/rustysynth's Instrument/Preset construction, crashing the […] |
| ✅ F-004 | high | `vendor/rustysynth/src/zone.rs:25` | Zone::new panics on out-of-range slice index when a SoundFont's bag/generator chunk counts disagree, crashing Auris Studio instead of rejecting the broken file. |
| F-205 | medium | `vendor/rustysynth/README.md:22` | rustysynth fork README's closed list of touched files (README.md:22-23) omits src/error.rs, which adds the InvalidModulatorList variant actually returned by […] |
| F-240 | medium | `vendor/rustysynth/src/voice.rs:235` | Unclamped modulator-sum cents can overflow to inf/NaN in voice.rs's filter-cutoff math, permanently and silently bypassing the lowpass filter for that voice. |
| F-241 | medium | `vendor/rustysynth/src/region_pair.rs:35` | Unbounded modulator summation in region_pair.rs can overflow filter cutoff to Infinity, silently disabling the low-pass filter on a crafted SoundFont. |
| F-393 | medium | `vendor/rustysynth/src/modulator.rs:104` | SOURCE_NONE (No Controller) in the vendored rustysynth modulator feeds raw=127 through the decreasing/curve/bipolar pipeline instead of returning a fixed 1.0, […] |
| F-266 | low | `vendor/rustysynth/src/preset_region.rs:151` | PresetRegion::get_initial_filter_cutoff_frequency returns a raw multiplying factor instead of Hz, but the method is dead code never called on any real […] |
| F-295 | low | `vendor/rustysynth/src/synthesizer.rs:258` | note_off_all(true)/note_off_all_channel(_, true) don't clear the pre-rendered block tail, leaving up to ~0.7ms of stale audio after an "immediate" stop […] |
| F-443 | low | `vendor/rustysynth/src/zone_info.rs:43` | A malformed SF2 bag with non-monotonic generator_index silently empties a zone instead of raising a parse error. |

### ✅ F-018 · critical · A crafted/corrupt SF2 file's zone-index fields cause an unchecked out-of-bounds slice panic in vendor/rustysynth's Instrument/Preset construction, crashing the whole app instead of returning an error.

`vendor/rustysynth/src/instrument.rs:33` · security · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Importing or loading an SF2 SoundFont file whose `inst`/`phdr` zone-index fields don't agree with the `ibag`/`pbag` zone count causes an out-of-bounds slice panic, crashing the entire Auris Studio process (or CLI/MCP frontend) instead of returning the `Result<_, SoundFontError>` the API promises — turning a single bad or malicious font file into a full application crash and loss of any unsaved session work.

**Trigger.** A `.sf2` file whose `pdta` list has an `inst` (or `phdr`) chunk with 2+ entries where the first entry's `zone_start_index` exceeds the number of zones actually present in the corresponding `ibag`/`pbag` chunk (e.g. `ibag` has only 2 entries → 1 zone, but `inst`'s first record claims `zone_start_index = 100`).

**Mechanism.** `Instrument::new` (and identically `Preset::new` in preset.rs) computes `let span_start = info.zone_start_index as usize;` (instrument.rs:31) and `let span_end = span_start + zone_count as usize;` (instrument.rs:32), then indexes `&zones[span_start..span_end]` (instrument.rs:33 / preset.rs:38) with no check that `span_end <= zones.len()`. `zone_start_index` is a raw, file-controlled `u16` (0-65535) read in instrument_info.rs:86 / preset_info.rs:131 from the `inst`/`phdr` sub-chunks, while `zones` is an entirely independently-sized array built by `Zone::create` in zone.rs from the `ibag`/`pbag` sub-chunk (its length is `ibag_size/4 - 1`, unrelated to the values inside `inst`/`phdr`). A crafted file can make `ibag`/`pbag` tiny (few zones) while `inst`/`phdr` entries name a much larger `zone_start_index`/`zone_end_index`.

**Expected.** The project's own `SoundFont::sanity_check` in vendor/rustysynth/src/soundfont.rs (lines 68-94) establishes the pattern of validating file-derived indices against real array bounds before trusting them (it does exactly this for sample start/end against `wave_data.len()`); the same discipline is missing here for `zone_start_index`/`zone_end_index` against `zones.len()`, and the separate `auris-io` chunk-size guard (crates/auris-io/src/soundfont.rs `check_chunk`) only validates byte-length […]

**Fix direction.** In `Instrument::new` (vendor/rustysynth/src/instrument.rs:26-33) and the identical pattern in `Preset::new` (preset.rs:22-38), check `span_end <= zones.len()` (and `span_start <= zones.len()`) before slicing and return `SoundFontError::InvalidInstrument`/`InvalidPreset` on failure instead of indexing directly; add a regression test with a crafted `inst`/`ibag` mismatch to `vendor/rustysynth`'s own test suite per its README convention.

**Written rule it breaks.** A file that has moved is searched for by name and confirmed by size, and what is found is written back into the document. Missing assets are reported, never fatal: the project opens with that one track silent.

### ✅ F-004 · high · Zone::new panics on out-of-range slice index when a SoundFont's bag/generator chunk counts disagree, crashing Auris Studio instead of rejecting the broken file.

`vendor/rustysynth/src/zone.rs:25` · correctness · confirmed (executed reproduction; reported independently 2×)

**What a user sees.** Loading a SoundFont whose pbag/pgen (or ibag/igen) chunks are inconsistent — a corrupted download, a hand-edited or malformed .sf2 that a user drags into Auris Studio via auris-sampler — makes `Zone::new` index the generator slice out of bounds and panic, crashing the whole application instead of failing to load that one instrument.

**Trigger.** A `.sf2` file (corrupted, truncated at the wrong chunk, or hand-crafted) whose `pbag`/`ibag` chunk declares a zone with `generator_index` (or a run of `generator_count`) that extends past the number of records the file's `pgen`/`igen` chunk actually holds — e.g. an instrument bag entry with `generator_index = 5` while `igen` contains only 2 generator records plus its terminator. Every other chunk-size field in the file can be completely honest.

**Mechanism.** `Zone::new` builds each zone's generator list with `segment.push(generators[(info.generator_index + i) as usize])` inside `for i in 0..info.generator_count` (lines 24-25) — direct slice indexing, no `.get()`. `info.generator_index`/`generator_count` come straight from the file's `pbag`/`ibag` chunk (`zone_info.rs`: `generator_count = zones[i+1].generator_index - zones[i].generator_index`, both raw u16s from the file) and are never checked against the length of the `generators` slice, which is built independently from the `pgen`/`igen` chunk. Three lines below, the *modulator* segment the fork added does the equivalent lookup safely: `if let Some(modulator) = modulators.get((info.modulator_index + i) as usize) { ... }` (lines 31-33), with a comment explaining exactly why: "A bag may name more modulators than the chunk holds, which is a broken file rather than a reason to refuse one." The pre-existing generator lookup three lines above has the identical hazard and no such guard. `crates/auris-io/src/soundfont.rs::check_chunks` (the project's own stated defense against "a *plausible* […]

**Expected.** The project's own stated threat model for this exact file (`crates/auris-io/src/soundfont.rs` doc comment: "What must not reach it is a *plausible* file whose sizes lie") and the fork's own modulator code three lines below both treat an inconsistent bag/chunk cross-reference as a broken-file condition to handle gracefully, not a reason to panic. The generator lookup should use the same `.get()` pattern as the modulator lookup (or a length check before slicing) and either skip the missing […]

**Fix direction.** Change the generator loop in `Zone::new` (vendor/rustysynth/src/zone.rs:24-26) to use `generators.get(...)` the same way the modulator loop three lines below already does, skipping/ignoring indices past the end of the slice instead of panicking on a mismatched bag/pgen file.

**Written rule it breaks.** A bag may name more modulators than the chunk holds, which is a broken file rather than a reason to refuse one: what is there is taken and the rest is left alone. (comment in vendor/rustysynth/src/zone.rs, applied to the modulator loop but not the identical-hazard generator loop above it)

### F-205 · medium · rustysynth fork README's closed list of touched files (README.md:22-23) omits src/error.rs, which adds the InvalidModulatorList variant actually returned by modulator.rs.

`vendor/rustysynth/README.md:22` · other · confirmed (traced through the code; reported independently 1×)

**What a user sees.** A developer reading vendor/rustysynth/README.md to find every line the Auris fork touched (per CLAUDE.md's own instruction that this README "is the account: what was added, what was deliberately left out") will not find src/error.rs, even though it defines a new SoundFontError variant (InvalidModulatorList) that is part of the fork's public error surface and is actually returned by Modulator::read_from_chunk when a modulator sub-chunk's size isn't a multiple of 10 bytes. Someone auditing or upstreaming the fork's changes, or diffing against upstream rustysynth, would miss this file entirely.

**Trigger.** Reading the README to find every file the fork touched (which is exactly what CLAUDE.md tells a contributor to do: "vendor/rustysynth/README.md is the account") and cross-checking it against the actual "Added by the Auris fork" markers in the tree.

**Mechanism.** The README states: "The change is `src/modulator.rs` plus the lines that carry a modulator list from the file to a voice: `zone.rs`, `soundfont_parameters.rs`, `preset_region.rs`, `instrument_region.rs`, `region_pair.rs` and `voice.rs`. Every addition is marked \"Added by the Auris fork\"." But `vendor/rustysynth/src/error.rs` also carries that exact marker (line 79: `/// Added by the Auris fork, which reads the modulator lists upstream discards.` above a new `InvalidModulatorList` enum variant) and is not named anywhere in the list.

**Expected.** Per CLAUDE.md's own rule for this file ("vendor/rustysynth/README.md is the account: what was added, what was deliberately left out"), the list of touched files should be complete — it should include `error.rs` alongside the seven files it already names.

**Fix direction.** Add src/error.rs to the list of touched files in vendor/rustysynth/README.md:22-23 (e.g. "...`region_pair.rs`, `voice.rs` and `error.rs`"), since it carries the same "Added by the Auris fork" marker and defines the InvalidModulatorList variant the modulator-reading code returns.

**Written rule it breaks.** vendor/rustysynth/README.md is the account: what was added, what was deliberately left out, and the measurement.

### F-240 · medium · Unclamped modulator-sum cents can overflow to inf/NaN in voice.rs's filter-cutoff math, permanently and silently bypassing the lowpass filter for that voice.

`vendor/rustysynth/src/voice.rs:235` · dsp · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** With a crafted or pathological SoundFont (a zone carrying roughly a dozen distinct-identity modulators driving extreme cents sums on the filter-cutoff generators), a note's lowpass filter can silently latch into NaN and permanently bypass for the rest of that voice — the sound jumps from filtered to raw/unfiltered mid-note with no error, warning, or way to trace the cause back to the offending font.

**Trigger.** An instrument zone whose `imod`/`pmod` list contains ~6 modulators (source = amount_source = SOURCE_NONE, transform = linear) targeting `initialFilterFc` with amount -32767 each (summed cents below -179,640, driving `self.cutoff` to exactly 0.0 Hz at note-on) together with ~5 modulators targeting `modEnvToFilterFc` with amount +32767 each (summed cents above +153,600, velocity-independent). At note-on the filter starts fully closed (cutoff 0 Hz); once the modulation envelope's Hold stage is reached, `factor` overflows to +inf, the product with the 0 Hz cutoff is NaN, and the filter silently switches from fully closed to fully open/unfiltered.

**Mechanism.** `region.modulated(i, key, velocity)` (region_pair.rs:35-44) sums `self.gs(i)` with the `.contribution()` of every preset- and instrument-level modulator targeting generator `i`, with no bound on the number of modulators or the resulting magnitude — unlike the base generator value it is added to, which was always capped to an `i16` before the fork. `Voice::start` feeds this straight into `self.cutoff = cents_to_hertz(region.modulated(INITIAL_FILTER_CUTOFF_FREQUENCY, ...))` (voice.rs:154-158) and `self.mod_env_to_cutoff = region.modulated(MODULATION_ENVELOPE_TO_FILTER_CUTOFF_FREQUENCY, ...) as i32` (voice.rs:166-170). If the summed cents for the initial cutoff is below roughly -179,640, `cents_to_hertz` underflows to exactly `0.0_f32`; if the summed cents fed to `mod_env_to_cutoff` exceeds roughly 153,600 (reachable with only ~5 modulators at max amount 32767, or fewer at `SOURCE_NONE`/`SOURCE_NONE` so it's velocity-independent), `SoundFontMath::cents_to_multiplying_factor` (voice.rs:234) overflows to `+inf` once the modulation envelope reaches its `Hold` stage (`get_value() == 1.0`, […]

**Expected.** The cents value fed to `cents_to_hertz`/`cents_to_multiplying_factor` should be clamped to a sane range (e.g. the ~1500-13500 cents the SF2 spec implies for a 16-bit generator, or at minimum finite-and-nonzero) before being used as a multiplier input, so that a hostile or pathological modulator combination cannot drive the filter's control path to `NaN`.

**Fix direction.** Clamp the cents value fed to `cents_to_hertz`/`cents_to_multiplying_factor` at `voice.rs` (and/or clamp `region.modulated()`'s summed result in `region_pair.rs`) to a sane generator range (e.g. roughly -12000..20000 cents per the SF2 spec's 16-bit-generator bounds) before use, so no modulator combination can drive the filter math to infinity or NaN.

**Written rule it breaks.** DSP code lives behind unit tests that assert on numbers (levels, frequencies, lengths) rather than on "it runs".

**Verifier's correction.** Mechanism, trigger, and consequence are all as claimed and reproduce exactly (verified by standalone execution of the actual formulas). One number in the mechanism text is imprecise: cents_to_hertz's f32 result underflows to exactly 0.0 at -180000 cents, not "roughly -179,640" — at -179640 the result is a nonzero subnormal (~1.1e-44), which multiplied by +inf gives +inf (then clamped, not NaN'd). The claim's own suggested trigger (~6 modulators at max negative amount, summing to -196608 or -196600-ish) already clears the real -180000 threshold, so the trigger recipe and every other part of […]

### F-241 · medium · Unbounded modulator summation in region_pair.rs can overflow filter cutoff to Infinity, silently disabling the low-pass filter on a crafted SoundFont.

`vendor/rustysynth/src/region_pair.rs:35` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** A maliciously or unusually crafted SoundFont (one with several modulators pointing at the same filter-cutoff destination whose amounts sum past ~150,012 cents) can drive a voice's low-pass cutoff to Infinity; BiQuadFilter::set_low_pass_filter then takes the false branch of its cutoff check and permanently disables filtering for that voice for the rest of the note, with no error, warning, or panic — the note just plays unfiltered. This is not reachable from any of the ordinary fonts the project ships or tests against (README reports 101/128 MuseScore General programs bit-identical, the rest moving a few dB), only from an adversarial or corrupt .sf2 file.

**Trigger.** A crafted SoundFont whose `imod`/`pmod` chunk assigns, say, 5+ modulators to one instrument zone, all with `destination = 8` (INITIAL_FILTER_CUTOFF_FREQUENCY), `source` = linear unipolar Note-On Velocity, `amount_source` = No Controller, and `amount = 32767`; struck at velocity 127 the five contributions alone sum to ~163,835 cents, well past the overflow threshold. Nothing in `Modulator::read_from_chunk`, `Zone::new`, or `modulated()` rejects or caps this.

**Mechanism.** `modulated()` (region_pair.rs:35-44) sums `Modulator::contribution()` over every modulator in a zone's list whose `destination` matches the target generator, with `.filter_map(...).sum()` — there is no cap on how many modulators may target the same destination (a zone's `modulator_count` in zone_info.rs:44 is only bounded by the file's declared bag-index delta, up to 65535) and no clamp on the resulting sum. `Voice::start` (voice.rs:154-158) feeds that unclamped f32 straight into `SoundFontMath::cents_to_hertz` (soundfont_math.rs:36-38), which is `8.176 * 2f32.powf(cents/1200.0)` with no range check. A single modulator's own `contribution()` is bounded by `|amount| <= 32767` (i16), so the pre-fork, generator-only path (raw `i16` cutoff, max 32767 cents) could never push `cents_to_hertz` past f32's finite range; but because the fork sums an attacker-controlled *count* of modulators, the total is unbounded and can exceed the ~153,600-cent threshold where `2f32.powf` saturates to `f32::INFINITY`.

**Expected.** The README's own framing is that this fork adds a specific, narrow capability (`initialFilterFc` and `modEnvToFilterFc` read through modulators) without changing anything else about how those generators behave; a font's declared modulators should shape the cutoff within a sane range, not be able to defeat range safety that the single-generator path already relied on implicitly (max i16 magnitude). The summed contribution should be bounded (e.g. clamped to a sane cents range, or the sum […]

**Fix direction.** Clamp the modulator sum (or the resulting cents value) in RegionPair::modulated / before the cents_to_hertz call in Voice::start (vendor/rustysynth/src/region_pair.rs:35, voice.rs:154-158) to a sane cents range before it reaches cents_to_hertz, so a pathological font degrades to a clamped cutoff instead of silently disabling the filter.

**Written rule it breaks.** DSP code lives behind unit tests that assert on numbers (levels, frequencies, lengths) rather than on "it runs".

### F-393 · medium · SOURCE_NONE (No Controller) in the vendored rustysynth modulator feeds raw=127 through the decreasing/curve/bipolar pipeline instead of returning a fixed 1.0, so a font's constant modulator silently zeros or inverts when the decreasing/bipolar bits are set alongside it.

`vendor/rustysynth/src/modulator.rs:104` · spec-mismatch · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** A SoundFont zone that declares a "No Controller" (constant) modulator whose source byte also sets the decreasing or bipolar bit — a plausible leftover from a font editor, or a hostile/malformed file — has its constant filter-cutoff (or other generator) offset silently zeroed or sign-flipped at every note-on, instead of applying the font author's intended constant amount. The effect is inaudible-by-omission: the cutoff or depth for that region is just wrong, with no error or warning.

**Trigger.** A modulator whose packed `source` field has controller index 0 (No Controller) but also has the decreasing bit set, e.g. `source = 0x0100` (index 0, direction=decreasing, curve=linear, unipolar) — a plausible leftover from a font editor that doesn't reset the direction flag when a modulator's source is switched to "No Controller", or a deliberately hostile file.

**Mechanism.** `source_value` (lines 104-133) computes `raw = 127` for `SOURCE_NONE` (line 114) and then unconditionally runs it through the same decreasing/curve/bipolar pipeline as every other source (lines 120-131). A search of the SF2 spec text ('No Controller ... The output of this controller module should be treated as if its value were set to "1"') and FluidSynth's reference implementation (`fluid_mod_transform_source_value`: `if (mod_src == FLUID_MOD_NONE) { return 1.0f; }`, an unconditional early return *before* any of the direction/polarity/curve flag handling) both indicate the fixed value 1.0 should bypass those transforms entirely, not merely feed `raw=127` into them.

**Expected.** `source_value` should special-case the controller index 0 (No Controller) to return `Some(1.0)` unconditionally, ignoring the decreasing/bipolar/curve bits on that source, matching the spec's 'treated as if its value were set to 1' language and FluidSynth's early return.

**Fix direction.** In `Modulator::source_value`, special-case `spec & 0x7F == SOURCE_NONE` to return `Some(1.0)` immediately, before the decreasing/curve/bipolar pipeline runs (mirroring FluidSynth's `fluid_mod_transform_source_value` early return), and add a test with `source(SOURCE_NONE, true, true, curve)` combinations to pin the fix and stop the existing `no_controller_is_a_constant` test from certifying only the coincidentally-correct default-flags case.

**Written rule it breaks.** DSP code lives behind unit tests that assert on numbers (levels, frequencies, lengths) rather than on "it runs".

### F-266 · low · PresetRegion::get_initial_filter_cutoff_frequency returns a raw multiplying factor instead of Hz, but the method is dead code never called on any real synthesis path.

`vendor/rustysynth/src/preset_region.rs:151` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** No user-observable effect today: nothing in the vendored crate or in Auris calls PresetRegion::get_initial_filter_cutoff_frequency — the real synthesis path (RegionPair::get_initial_filter_cutoff_frequency, region_pair.rs:90-94) computes cutoff itself via cents_to_hertz and never delegates to this method. It is a public method with a misleading name/units (returns a dimensionless multiplying factor, not Hz) that would silently misbehave if anyone started calling it directly (e.g. from a test or a future refactor that wires PresetRegion into the cutoff computation).

**Trigger.** Call `PresetRegion::get_initial_filter_cutoff_frequency()` on any region (it is `pub fn` on a `pub struct`, part of the crate's public surface) and compare against `InstrumentRegion::get_initial_filter_cutoff_frequency()` for the same cents value.

**Mechanism.** `PresetRegion::get_initial_filter_cutoff_frequency` (lines 150-154) computes `SoundFontMath::cents_to_multiplying_factor(self.gs[INITIAL_FILTER_CUTOFF_FREQUENCY] as f32)`, i.e. `2^(cents/1200)` with no `8.176` reference-frequency factor. The identically-named, identically-documented sibling `InstrumentRegion::get_initial_filter_cutoff_frequency` (instrument_region.rs:206-210) correctly calls `SoundFontMath::cents_to_hertz`, which does include the `8.176` factor. For a typical `initialFilterFc` of 13500 cents (the spec/instrument-region default), the correct method returns roughly 20 kHz while this one returns roughly 2477 (a dimensionless factor, off by the 8.176 Hz reference and mislabeled as a frequency).

**Expected.** Should call `SoundFontMath::cents_to_hertz`, exactly like its `InstrumentRegion` counterpart and like `RegionPair::get_initial_filter_cutoff_frequency`, so the three consistently-named methods return the same unit.

**Fix direction.** In vendor/rustysynth/src/preset_region.rs:150-154, change the call from SoundFontMath::cents_to_multiplying_factor to SoundFontMath::cents_to_hertz, matching the identically-named InstrumentRegion method and the actual RegionPair cutoff computation; add a one-line unit test asserting the value equals 8.176 * 2^(cents/1200) to prevent regression.

**Written rule it breaks.** vendor/rustysynth/README.md is the account: what was added, what was deliberately left out, and the measurement. (CLAUDE.md, "The vendored synthesiser") — this bug is present but not documented there, and unlike the modulator-list fix that motivated the fork, it has no live audio impact to measure.

**Verifier's correction.** The claim's numeric estimate of "roughly 2477" for the multiplying-factor value at 13500 cents is slightly off; the precise value is ~2435.5 (still the same order of magnitude and same qualitative bug). Everything else in the claim — mechanism, location, trigger, and consequence — is accurate as stated.

### F-295 · low · note_off_all(true)/note_off_all_channel(_, true) don't clear the pre-rendered block tail, leaving up to ~0.7ms of stale audio after an "immediate" stop (32-sample blocks at 44.1/48kHz in this codebase's only caller).

`vendor/rustysynth/src/synthesizer.rs:258` · realtime · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** When the sampler's "immediate" all-sound-off is triggered (e.g. a panic/kill-switch), up to 31 already-rendered samples (~0.7 ms at 44.1/48 kHz, since the only caller in this codebase fixes block_size at INTERNAL_BLOCK = 32) can still play from voices that were just told to stop instantly. This is well below the threshold of audibility — not the 64 ms worst case the original claim illustrated, which requires a block_size/sample_rate configuration this codebase never uses.

**Trigger.** Configure `SynthesizerSettings::block_size` to a large legal value (up to 1024, per `check_block_size`'s `8..=1024` range) at a low `sample_rate` (down to 16000, per `check_sample_rate`), then call `note_off_all(true)` (or `note_off_all_channel(ch, true)`) partway through consuming a block that is already mid-flight (block_read between 1 and block_size-1).

**Mechanism.** `render()` (lines 334-360) pre-renders a whole `block_size`-sample chunk into `self.block_left`/`self.block_right` via `render_block()` (called only when `self.block_read == self.block_size`) and then serves samples out of that already-computed buffer on subsequent calls. `note_off_all(true)` (line 258) and `note_off_all_channel(_, true)` (lines 273-278) mutate voice state (`self.voices.clear()` / `voice.kill()`) synchronously, but do nothing to `self.block_read` or the already-filled `self.block_left`/`self.block_right`. If the call happens while `self.block_read < self.block_size` (i.e. mid-way through consuming the current pre-rendered block), the remaining `self.block_size - self.block_read` samples already sitting in `block_left`/`block_right` — computed BEFORE the mute — are still copied out by the next `rem` iterations of `render()`'s loop before a fresh `render_block()` (which will see the now-empty/silenced voice list) ever runs.

**Expected.** The doc comment on `note_off_all` (lines 251-255) says notes stop immediately when `immediate` is true; correct behavior would either flush/zero the still-unread tail of `block_left`/`block_right` (from `block_read` onward) when an immediate stop is requested, or the doc should note the up-to-one-block latency inherent to the pre-rendering design.

**Fix direction.** In Synthesizer::note_off_all(true) and note_off_all_channel(_, true), also clear the unread tail of block_left/block_right from block_read onward (or set block_read = block_size to force a fresh render_block on the next render() call) so the pre-rendered buffer can't outlive the mute it's supposed to enact.

**Written rule it breaks.** "immediate" - If `true`, notes will stop immediately without the release sound.

**Verifier's correction.** The mechanism, trigger and consequence are correct as described. Two minor corrections: (1) the `self.voices.clear()` call the claim anchors to is at line 258, but the enclosing `note_off_all` function itself starts at line 256, not 258 (line 258 is accurate for the specific mutating statement, so this is not really an error). (2) In the one real caller in this codebase (`auris_sampler::Sampler::stop_everything`), `block_size` is fixed at `INTERNAL_BLOCK = 32`, not the claim's illustrative worst-case 1024 — so the actual leak in this project's current usage is bounded to at most 32 samples […]

### F-443 · low · A malformed SF2 bag with non-monotonic generator_index silently empties a zone instead of raising a parse error.

`vendor/rustysynth/src/zone_info.rs:43` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Only with a malformed or corrupted .sf2 file whose bag chunk has a non-monotonic generator_index/modulator_index does this trigger: the affected zone silently loses all its generators/modulators (indistinguishable from a legitimate empty/global zone) instead of the loader reporting a parse error. On a well-formed SoundFont, which is every font any DAW user actually ships or downloads, generator_index is monotonic by construction and this path is never taken, so no instrument mis-plays or loses volume/filtering in practice.

**Trigger.** A pbag/ibag chunk (not validated by auris-io's check_chunks, which only checks smpl parity and overall chunk-size-vs-file-length) whose generator_index for record i+1 is smaller than record i's — a spec violation but not one anything rejects before this point.

**Mechanism.** `zones[i].generator_count = zones[i + 1].generator_index - zones[i].generator_index;` (and the modulator_count line right after it) computes a signed i32 difference with no check that generator_index is non-decreasing across bag records, unlike the sibling `size % 4 != 0` check a few lines above that does reject other malformed shapes.

**Expected.** A non-monotonic bag record is malformed input; ZoneInfo::read_from_chunk already rejects other malformed shapes (`size == 0 || size % 4 != 0`) and should reject this one the same way instead of silently producing an under-specified zone.

**Fix direction.** In ZoneInfo::read_from_chunk, after computing generator_count/modulator_count for each i in 0..count-1, check that the value is >= 0 (i.e. zones[i+1].generator_index >= zones[i].generator_index, same for modulator_index) and return SoundFontError::InvalidZoneList (or a new variant) instead of letting the negative value flow into Zone::new's empty range.

**Written rule it breaks.** vendor/rustysynth/README.md documents what the fork added/left out and measured for the modulator-discarding bug it fixed; this gap is a further latent parser weakness of the same file the fork already touches, but it is not itself claimed fixed or covered by any written rule in CLAUDE.md.
