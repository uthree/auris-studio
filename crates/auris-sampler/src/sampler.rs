//! The instrument that plays a SoundFont.

use std::sync::Arc;

use auris_core::param::{ParamDescriptor, ParamId, db_to_gain};
use auris_core::plugin::{
    Instrument, NoteEvent, Parameterized, PluginCategory, PluginDescriptor, PluginState,
    PrepareContext, ProcessContext,
};
use auris_core::registry::PluginRegistry;
use auris_core::{AudioBuffer, PresetRef};
use rustysynth::{SoundFont, Synthesizer, SynthesizerSettings};

use crate::bank::SharedSoundFonts;

/// The sampler's plugin id, as a project file stores it.
pub const SAMPLER_ID: &str = "auris.sampler.soundfont";

/// Everything is played on one MIDI channel: a track is one sound, and the bank and patch that
/// choose it are set on the channel rather than carried by each note.
const CHANNEL: i32 = 0;

/// Frames the synthesiser computes at a time, internally.
///
/// Note events land on the next multiple of this, so it sets the timing resolution: 32 frames is
/// two thirds of a millisecond at 48 kHz, comfortably below where a listener hears a drum hit
/// arrive late. The library's own default is 64, which is twice that and audible on tight
/// percussion; going below 32 buys accuracy nobody can hear and pays per-voice overhead for it.
const INTERNAL_BLOCK: usize = 32;

/// Voices one instance may sound at once.
///
/// A voice is a few hundred bytes, and an orchestral font can spend three or four of them on a
/// single key, so the library's default of 64 runs out on a sustained chord long before anything
/// else does.
const MAX_VOICES: usize = 128;

/// Semitones of pitch bend at full deflection.
///
/// [`NoteEvent::PitchBend`] carries semitones, so the range only has to be wide enough to hold
/// what is asked for; an octave covers every bend a person writes and still leaves the 14-bit
/// wheel resolving under a thousandth of a semitone.
const BEND_RANGE: i32 = 12;

/// What the library calls unity — half of full scale, to leave a chord some headroom.
///
/// The `level` parameter is measured from here, so 0 dB is the sound the font was voiced for.
const NOMINAL_VOLUME: f32 = 0.5;

// MIDI messages, by their wire values, so the calls below read as what they are.
const CONTROL_CHANGE: i32 = 0xB0;
const PROGRAM_CHANGE: i32 = 0xC0;
const PITCH_BEND: i32 = 0xE0;
const CC_BANK_SELECT: i32 = 0x00;
const CC_DATA_ENTRY: i32 = 0x06;
const CC_RPN_LSB: i32 = 0x64;
const CC_RPN_MSB: i32 = 0x65;

/// Where a preset choice lives inside [`PluginState::extra`].
///
/// Nested under a key rather than being the whole of `extra`, so this is not the only thing the
/// sampler can ever keep there.
const PRESET_KEY: &str = "preset";

const LEVEL: usize = 0;
const PARAM_COUNT: usize = 1;

/// Writes a preset choice into the state a project stores for a track's instrument.
///
/// Public because choosing a preset is a *session* command — the document has to record the
/// choice before a graph is built — and the encoding should be written down once.
pub fn store_preset(state: &mut PluginState, preset: PresetRef) {
    let encoded = serde_json::to_value(preset).unwrap_or(serde_json::Value::Null);
    match state.extra.as_object_mut() {
        Some(map) => {
            map.insert(PRESET_KEY.to_string(), encoded);
        }
        None => {
            let mut map = serde_json::Map::new();
            map.insert(PRESET_KEY.to_string(), encoded);
            state.extra = serde_json::Value::Object(map);
        }
    }
}

/// Reads a preset choice back, or `None` when the state names none.
pub fn stored_preset(state: &PluginState) -> Option<PresetRef> {
    let stored = state.extra.get(PRESET_KEY)?;
    serde_json::from_value(stored.clone()).ok()
}

/// Registers the sampler, giving it the bank it will read fonts from.
///
/// Not a [`PluginPack`](auris_core::registry::PluginPack) like the other built-ins: a pack
/// registers itself from a static method and so has nothing to hand its plugins, whereas every
/// sampler needs the same bank. The closure captures it, which is the whole reason this function
/// exists.
pub fn register_sampler(registry: &mut PluginRegistry, fonts: SharedSoundFonts) {
    registry.register_instrument(move || Box::new(Sampler::new(Arc::clone(&fonts))));
}

/// Plays one preset of one SoundFont.
///
/// # Realtime behaviour
///
/// The synthesiser is built in [`Instrument::prepare`], which is where the font is looked up and
/// every buffer it needs is allocated. [`Instrument::process`] renders into buffers that already
/// exist and touches neither the bank nor the filesystem, and [`Instrument::reset`] — reachable
/// from the callback through every transport stop and seek — re-selects the preset `prepare`
/// resolved rather than going back to the bank, whose lock is not one to take there.
///
/// If the library panics while rendering — a degenerate font can push its oscillator off the end
/// of the sample data — the panic is caught and the instance goes silent until the next
/// `prepare` builds a fresh synthesiser. One track's sound is the most a bad font may cost.
///
/// Note events are honoured by splitting the block at each event's frame, so timing does not
/// depend on the host's buffer size — down to the synthesiser's own internal block, which is
/// what actually bounds it.
pub struct Sampler {
    fonts: SharedSoundFonts,
    params: Vec<ParamDescriptor>,
    values: [f32; PARAM_COUNT],
    /// Which sound to play, from the saved state. `None` means nothing has chosen one.
    preset: Option<PresetRef>,
    /// What [`Self::resolve`] answered when the synthesiser was built.
    ///
    /// `reset` re-selects from this rather than resolving again: it runs on the audio thread,
    /// where taking the bank's lock — let alone logging a missing font — is not allowed.
    selected: Option<PresetRef>,
    /// Built in `prepare`; `None` when there is no font to play.
    synth: Option<Synthesizer>,
    /// Set when the synthesiser panicked while rendering.
    ///
    /// A poisoned synthesiser is never called again — whatever state the panic left it in is
    /// not one to play — but it is also not dropped here, because dropping it would free its
    /// buffers on the audio thread. It waits for the next `prepare`.
    poisoned: bool,
    /// Only used to fold stereo into a host that asked for one channel.
    scratch_left: Vec<f32>,
    scratch_right: Vec<f32>,
    /// Keys held, for the activity indicator.
    held: u32,
}

impl Sampler {
    /// A sampler reading fonts from `fonts`.
    pub fn new(fonts: SharedSoundFonts) -> Self {
        // One control, because the font is the sound. Anything else a track wants doing to it —
        // ambience above all — belongs in its effect chain, where the offline renderer knows to
        // keep rendering until the tail has fallen silent. An instrument reports no tail at all,
        // so a reverb built into this one would be cut off at the end of every export.
        let params = vec![ParamDescriptor::decibels(
            LEVEL as u32,
            "level",
            "Level",
            -60.0,
            12.0,
            0.0,
        )];
        let mut values = [0.0; PARAM_COUNT];
        for (slot, descriptor) in values.iter_mut().zip(&params) {
            *slot = descriptor.default;
        }
        Self {
            fonts,
            params,
            values,
            preset: None,
            selected: None,
            synth: None,
            poisoned: false,
            scratch_left: Vec::new(),
            scratch_right: Vec::new(),
            held: 0,
        }
    }

    /// Which sound this sampler has been told to play.
    pub fn preset(&self) -> Option<PresetRef> {
        self.preset
    }

    /// `true` once a font has been found and a synthesiser built for it.
    pub fn has_voice(&self) -> bool {
        self.synth.is_some()
    }

    /// The font and preset to play, or `None` when there is nothing to play.
    ///
    /// A track that names a font the bank does not hold stays silent rather than falling back:
    /// the file has gone missing, and quietly playing some other font's piano would hide that.
    /// A track that names *nothing* is a different case — a sampler just dropped onto a track —
    /// and takes the first font loaded, so it makes a sound the moment it is used.
    fn resolve(&self) -> Option<(Arc<SoundFont>, PresetRef)> {
        match self.preset {
            Some(preset) => match self.fonts.get(preset.font) {
                Some(font) => Some((font, preset)),
                None => {
                    log::warn!(
                        "soundfont {} is not loaded, so this track stays silent",
                        preset.font.0
                    );
                    None
                }
            },
            None => self.fonts.first().map(|(id, font)| {
                // Bank 0, patch 0 is the general-MIDI piano. A font without it falls through to
                // the library's own default, which is the preset with the lowest number.
                (
                    font,
                    PresetRef {
                        font: id,
                        bank: 0,
                        patch: 0,
                    },
                )
            }),
        }
    }

    /// Points the channel at a preset and sets up the pitch-bend range.
    ///
    /// Called after every reset as well as after building: resetting the synthesiser resets its
    /// channels too, which would otherwise leave the track playing bank 0 patch 0.
    fn select(&mut self, preset: PresetRef) {
        let Some(synth) = self.synth.as_mut() else {
            return;
        };
        synth.process_midi_message(CHANNEL, CONTROL_CHANGE, CC_BANK_SELECT, preset.bank);
        synth.process_midi_message(CHANNEL, PROGRAM_CHANGE, preset.patch, 0);
        // Registered parameter 0 is the bend range; it takes both halves of the selection.
        synth.process_midi_message(CHANNEL, CONTROL_CHANGE, CC_RPN_MSB, 0);
        synth.process_midi_message(CHANNEL, CONTROL_CHANGE, CC_RPN_LSB, 0);
        synth.process_midi_message(CHANNEL, CONTROL_CHANGE, CC_DATA_ENTRY, BEND_RANGE);
    }

    /// Sends one parameter's current value to the synthesiser.
    fn push(&mut self, index: usize) {
        let Some(synth) = self.synth.as_mut() else {
            return;
        };
        let Some(value) = self.values.get(index).copied() else {
            return;
        };
        if index == LEVEL {
            synth.set_master_volume(NOMINAL_VOLUME * db_to_gain(value));
        }
    }

    /// Sends every parameter, for when a fresh synthesiser knows none of them.
    fn push_all(&mut self) {
        for index in 0..PARAM_COUNT {
            self.push(index);
        }
    }

    /// Applies one event.
    fn dispatch(&mut self, event: NoteEvent) {
        let Sampler {
            synth,
            held,
            poisoned,
            ..
        } = self;
        let Some(synth) = synth.as_mut() else {
            return;
        };
        if *poisoned {
            return;
        }
        match event {
            NoteEvent::NoteOn {
                pitch, velocity, ..
            } => {
                // Velocity 0 *is* a note-off in MIDI, so a note written very quietly has to
                // round up to 1 rather than silently releasing itself.
                let velocity = (velocity.clamp(0.0, 1.0) * 127.0).round() as i32;
                synth.note_on(CHANNEL, pitch as i32, velocity.max(1));
                *held = held.saturating_add(1);
            }
            NoteEvent::NoteOff { pitch, .. } => {
                synth.note_off(CHANNEL, pitch as i32);
                *held = held.saturating_sub(1);
            }
            NoteEvent::AllNotesOff { .. } => {
                synth.note_off_all(false);
                *held = 0;
            }
            NoteEvent::AllSoundOff { .. } => {
                synth.note_off_all(true);
                *held = 0;
            }
            NoteEvent::PitchBend { semitones, .. } => {
                let travel = (semitones / BEND_RANGE as f32).clamp(-1.0, 1.0);
                let raw = (8192.0 + travel * 8191.0).round().clamp(0.0, 16383.0) as i32;
                synth.process_midi_message(CHANNEL, PITCH_BEND, raw & 0x7F, (raw >> 7) & 0x7F);
            }
        }
    }

    /// Renders `start..end` of the block.
    fn render_range(&mut self, out: &mut AudioBuffer, start: usize, end: usize) {
        let Sampler {
            synth,
            poisoned,
            scratch_left,
            scratch_right,
            held,
            ..
        } = self;
        let Some(synth) = synth.as_mut() else {
            return;
        };
        if *poisoned {
            // Poisoned mid-block: the rest of the block is silence, not another attempt.
            for channel in out.channels_mut() {
                channel[start..end].fill(0.0);
            }
            return;
        }
        match out.channels_mut() {
            [left, right, rest @ ..] => {
                if !rendered_safely(synth, &mut left[start..end], &mut right[start..end]) {
                    *poisoned = true;
                    *held = 0;
                    left[start..end].fill(0.0);
                    right[start..end].fill(0.0);
                    for extra in rest.iter_mut() {
                        extra[start..end].fill(0.0);
                    }
                    return;
                }
                // More than two channels is not something the engine asks for, but a buffer
                // that arrives with them should not come back with silence in the extras.
                for (index, extra) in rest.iter_mut().enumerate() {
                    let source: &[f32] = if index % 2 == 0 { left } else { right };
                    extra[start..end].copy_from_slice(&source[start..end]);
                }
            }
            [mono] => {
                let available = scratch_left.len().min(scratch_right.len());
                let frames = (end - start).min(available);
                if !rendered_safely(
                    synth,
                    &mut scratch_left[..frames],
                    &mut scratch_right[..frames],
                ) {
                    *poisoned = true;
                    *held = 0;
                    mono[start..end].fill(0.0);
                    return;
                }
                for (index, sample) in mono[start..start + frames].iter_mut().enumerate() {
                    *sample = 0.5 * (scratch_left[index] + scratch_right[index]);
                }
                // Prepared for a stereo host and handed a mono buffer anyway: there is no
                // scratch to render through, and silence beats whatever was in the buffer.
                mono[start + frames..end].fill(0.0);
            }
            [] => {}
        }
    }
}

/// Lets the synthesiser render, and answers whether it survived.
///
/// The library indexes its sample data with arithmetic a degenerate font can push out of range,
/// and this is the audio callback thread: an unwind escaping into the C callback would abort the
/// whole process. Catching costs nothing until something actually panics; the price of that —
/// the hook's message, an allocation for the payload — is paid once, on the way to silence.
///
/// `AssertUnwindSafe` is honest here because the synthesiser is never called again once
/// poisoned: whatever invariants the panic broke are invariants nobody will read.
fn rendered_safely(synth: &mut Synthesizer, left: &mut [f32], right: &mut [f32]) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| synth.render(left, right))).is_ok()
}

impl Parameterized for Sampler {
    fn parameters(&self) -> &[ParamDescriptor] {
        &self.params
    }

    fn param(&self, id: ParamId) -> f32 {
        self.values.get(id.index()).copied().unwrap_or(0.0)
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        let index = id.index();
        let Some(descriptor) = self.params.get(index) else {
            return;
        };
        self.values[index] = descriptor.clamp(value);
        self.push(index);
    }

    fn save_state(&self) -> PluginState {
        let mut state = PluginState {
            params: self
                .params
                .iter()
                .map(|p| (p.key.to_string(), self.param(p.id)))
                .collect(),
            extra: serde_json::Value::Null,
        };
        if let Some(preset) = self.preset {
            store_preset(&mut state, preset);
        }
        state
    }

    fn load_state(&mut self, state: &PluginState) {
        for (key, value) in &state.params {
            self.set_param_by_key(key, *value);
        }
        // Which font to play cannot be an `f32` parameter, so it travels in `extra` and is
        // resolved by the next `prepare` — which is always the very next thing the graph does.
        self.preset = stored_preset(state);
    }
}

impl Instrument for Sampler {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::instrument(
            SAMPLER_ID,
            "SoundFont",
            "Plays an imported SoundFont",
            PluginCategory::Sampler,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        if self.poisoned {
            // Deferred from the audio thread, which had no business saying it there.
            log::warn!("the synthesiser panicked while rendering and is being rebuilt");
        }
        self.poisoned = false;
        self.synth = None;
        self.selected = None;
        self.held = 0;

        // Only a host asking for fewer than two channels needs anywhere to fold stereo down.
        let scratch = if ctx.channel_count >= 2 {
            0
        } else {
            ctx.max_block_frames
        };
        self.scratch_left.clear();
        self.scratch_left.resize(scratch, 0.0);
        self.scratch_right.clear();
        self.scratch_right.resize(scratch, 0.0);

        let Some((font, preset)) = self.resolve() else {
            return;
        };

        let rate = if ctx.sample_rate.is_finite() {
            ctx.sample_rate.round()
        } else {
            0.0
        };
        let mut settings = SynthesizerSettings::new(rate as i32);
        settings.block_size = INTERNAL_BLOCK;
        settings.maximum_polyphony = MAX_VOICES;
        // See `Sampler::new`: ambience is the effect chain's job, so the reverb and chorus
        // network is not built at all rather than being built and fed nothing.
        settings.enable_reverb_and_chorus = false;

        match Synthesizer::new(&font, &settings) {
            Ok(synth) => self.synth = Some(synth),
            Err(error) => {
                log::warn!("could not play soundfont {}: {error}", preset.font.0);
                return;
            }
        }
        self.selected = Some(preset);
        self.select(preset);
        self.push_all();
    }

    fn reset(&mut self) {
        self.held = 0;
        if self.poisoned {
            // The synthesiser died rendering; whatever state it was left in is not one to
            // call back into. `prepare` will build a fresh one.
            return;
        }
        if let Some(synth) = self.synth.as_mut() {
            synth.reset();
        }
        // A reset returns the channel to bank 0 patch 0 with the library's own controller
        // defaults, so everything the track chose has to be said again — from what `prepare`
        // resolved, because this runs on the audio thread and the bank's lock does not.
        if let Some(preset) = self.selected {
            self.select(preset);
        }
        self.push_all();
    }

    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, ctx: &ProcessContext) {
        if self.synth.is_none() || self.poisoned {
            out.clear();
            return;
        }
        let frames = out.frame_count().min(ctx.block_frames);

        let mut cursor = 0;
        let mut next_event = 0;
        while cursor < frames {
            while let Some(event) = events.get(next_event) {
                if event.frame() as usize > cursor {
                    break;
                }
                self.dispatch(*event);
                next_event += 1;
            }
            // The next event is where this run has to stop; the inner loop leaves it strictly
            // past the cursor, so the run is never empty and the outer loop always advances.
            let end = events
                .get(next_event)
                .map_or(frames, |event| (event.frame() as usize).min(frames));
            self.render_range(out, cursor, end);
            cursor = end;
        }
        // An event on the block's last frame has nothing left to render but still has to be
        // heard — a note-off there is what stops the note in the block after this one.
        while let Some(event) = events.get(next_event) {
            self.dispatch(*event);
            next_event += 1;
        }
    }

    fn active_voices(&self) -> usize {
        // Keys held, not voices sounding: the library does not publish its voice count, and one
        // key of a layered font is several voices. Close enough for an activity indicator, and
        // it is at least never wrong about whether anything is playing.
        self.held as usize
    }
}

impl std::fmt::Debug for Sampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sampler")
            .field("preset", &self.preset)
            .field("playing", &self.synth.is_some())
            .field("held", &self.held)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bank::SoundFontBank;
    use auris_core::SoundFontId;

    fn sampler() -> Sampler {
        Sampler::new(SoundFontBank::shared())
    }

    fn preset() -> PresetRef {
        PresetRef {
            font: SoundFontId(7),
            bank: 128,
            patch: 42,
        }
    }

    #[test]
    fn a_preset_survives_a_round_trip_through_the_saved_state() {
        // The document is where a preset choice lives between sessions, so this is the join
        // that decides whether a project opens playing what it was saved playing.
        let mut state = PluginState::empty();
        store_preset(&mut state, preset());
        assert_eq!(stored_preset(&state), Some(preset()));
    }

    #[test]
    fn storing_a_preset_leaves_the_rest_of_the_state_alone() {
        let mut state = PluginState::empty();
        state.extra = serde_json::json!({ "something-else": 3 });
        store_preset(&mut state, preset());
        assert_eq!(stored_preset(&state), Some(preset()));
        assert_eq!(
            state.extra.get("something-else"),
            Some(&serde_json::json!(3))
        );
    }

    #[test]
    fn a_state_naming_no_preset_reads_back_as_none() {
        assert_eq!(stored_preset(&PluginState::empty()), None);

        // And so does one holding something that is not a preset at all, which is what a file
        // written by a future version looks like from here.
        let mut state = PluginState::empty();
        state.extra = serde_json::json!({ "preset": "the loud one" });
        assert_eq!(stored_preset(&state), None);
    }

    #[test]
    fn parameters_and_preset_both_come_back_from_a_saved_state() {
        let mut before = sampler();
        before.set_param_by_key("level", -6.0);
        before.preset = Some(preset());
        let state = before.save_state();

        let mut after = sampler();
        after.load_state(&state);
        assert_eq!(after.param_by_key("level"), Some(-6.0));
        assert_eq!(after.preset(), Some(preset()));
    }

    #[test]
    fn a_sampler_with_no_font_renders_silence_rather_than_refusing_to_run() {
        // Every path here has to survive an empty bank: a project can name a font whose file
        // has moved, and that must cost one track's sound, not the session.
        let mut sampler = sampler();
        sampler.prepare(&PrepareContext::new(48_000.0, 512, 2));
        assert!(!sampler.has_voice());

        let mut out = AudioBuffer::stereo(512, 48_000.0);
        for sample in out.channel_mut(0) {
            *sample = 0.5;
        }
        let events = [
            NoteEvent::NoteOn {
                frame: 0,
                pitch: 60,
                velocity: 1.0,
            },
            NoteEvent::NoteOff {
                frame: 200,
                pitch: 60,
            },
        ];
        let ctx = ProcessContext::realtime(48_000.0, 512, 0, 120.0, true);
        sampler.process(&events, &mut out, &ctx);

        assert!(
            out.channel(0).iter().all(|s| *s == 0.0),
            "a silent sampler must still overwrite the buffer it was handed"
        );
        assert_eq!(sampler.active_voices(), 0);
        sampler.reset();
    }

    #[test]
    fn the_descriptor_says_what_a_project_file_will_store() {
        let sampler = sampler();
        let descriptor = sampler.descriptor();
        assert_eq!(descriptor.id, SAMPLER_ID);
        assert_eq!(descriptor.category, PluginCategory::Sampler);
        assert_eq!(sampler.parameters().len(), PARAM_COUNT);
        for (index, param) in sampler.parameters().iter().enumerate() {
            assert_eq!(
                param.id.index(),
                index,
                "descriptor {index} is out of place"
            );
        }
    }

    #[test]
    fn the_sampler_registers_itself_under_the_id_projects_store() {
        let mut registry = PluginRegistry::new();
        register_sampler(&mut registry, SoundFontBank::shared());
        assert!(registry.has_instrument(SAMPLER_ID));
    }

    // ------------------------------------------------------------ playing one

    const RATE: f64 = 48_000.0;
    const FONT: SoundFontId = SoundFontId(1);

    /// A bank holding the two-tone test font under [`FONT`].
    fn stocked() -> crate::bank::SharedSoundFonts {
        let bank = SoundFontBank::shared();
        bank.insert(FONT, crate::test_support::two_tone_font(RATE as i32));
        bank
    }

    /// A prepared sampler playing `patch` of the test font.
    fn playing(bank: crate::bank::SharedSoundFonts, patch: i32, frames: usize) -> Sampler {
        let mut sampler = Sampler::new(bank);
        sampler.preset = Some(PresetRef {
            font: FONT,
            bank: 0,
            patch,
        });
        sampler.prepare(&PrepareContext::new(RATE, frames, 2));
        assert!(sampler.has_voice(), "the font is in the bank");
        sampler
    }

    fn note_on(frame: u32) -> NoteEvent {
        NoteEvent::NoteOn {
            frame,
            pitch: crate::test_support::ROOT_KEY,
            velocity: 1.0,
        }
    }

    fn rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum: f32 = samples.iter().map(|s| s * s).sum();
        (sum / samples.len() as f32).sqrt()
    }

    #[test]
    fn a_note_turns_into_audio() {
        // The one test here that can fail for a real reason: everything else is arithmetic on
        // state, and this is the path that runs through the file format, the preset lookup and
        // the synthesiser.
        let mut sampler = playing(stocked(), 0, 512);
        let mut out = AudioBuffer::stereo(512, RATE);
        let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        sampler.process(&[note_on(200)], &mut out, &ctx);

        assert_eq!(
            rms(&out.channel(0)[..200]),
            0.0,
            "the sound started before the note did"
        );
        assert!(
            rms(&out.channel(0)[200..]) > 0.01,
            "the note made no sound at all"
        );
        assert_eq!(sampler.active_voices(), 1);
    }

    #[test]
    fn each_preset_plays_its_own_sound() {
        // Without this, a sampler that ignored bank and patch entirely and always played the
        // font's first preset would pass every other test in this file.
        let bank = stocked();
        let frames = 1_024;
        let ctx = ProcessContext::realtime(RATE, frames, 0, 120.0, true);

        let measure = |patch: i32| {
            let mut sampler = playing(bank.clone(), patch, frames);
            let mut out = AudioBuffer::stereo(frames, RATE);
            sampler.process(&[note_on(0)], &mut out, &ctx);
            // Past the attack, so both are measured at their sustained level.
            rms(&out.channel(0)[frames / 2..])
        };

        // The two tones are recorded four to one apart.
        let ratio = measure(0) / measure(42);
        assert!(
            (ratio - 4.0).abs() < 0.4,
            "the two presets should differ by four to one, not {ratio}"
        );
    }

    #[test]
    fn the_block_size_does_not_change_what_is_heard() {
        // A note lands on the frame it names, so splitting the same span into two calls has to
        // produce the same samples. This is what the event-splitting loop in `process` is for.
        let bank = stocked();
        let ctx_full = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        let ctx_half = ProcessContext::realtime(RATE, 256, 0, 120.0, true);

        let mut whole = playing(bank.clone(), 0, 512);
        let mut one_go = AudioBuffer::stereo(512, RATE);
        whole.process(&[note_on(300)], &mut one_go, &ctx_full);

        let mut split = playing(bank, 0, 512);
        let mut first = AudioBuffer::stereo(256, RATE);
        let mut second = AudioBuffer::stereo(256, RATE);
        split.process(&[], &mut first, &ctx_half);
        // Frame 300 of the whole block is frame 44 of the second half.
        split.process(&[note_on(44)], &mut second, &ctx_half);

        assert_eq!(&one_go.channel(0)[..256], first.channel(0));
        assert_eq!(&one_go.channel(0)[256..], second.channel(0));
    }

    #[test]
    fn a_released_note_stops() {
        let mut sampler = playing(stocked(), 0, 512);
        let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        let mut out = AudioBuffer::stereo(512, RATE);

        sampler.process(&[note_on(0)], &mut out, &ctx);
        let sounding = rms(out.channel(0));
        assert!(sounding > 0.01);

        sampler.process(
            &[NoteEvent::NoteOff {
                frame: 0,
                pitch: crate::test_support::ROOT_KEY,
            }],
            &mut out,
            &ctx,
        );
        assert_eq!(sampler.active_voices(), 0);

        // The release is short but not instant, so let a few blocks go by.
        for _ in 0..8 {
            sampler.process(&[], &mut out, &ctx);
        }
        assert!(
            rms(out.channel(0)) < sounding / 100.0,
            "the note went on sounding after it was released"
        );
    }

    #[test]
    fn an_event_on_the_last_frame_is_still_heard() {
        // There is nothing left to render at that point, but a note-off there is what stops the
        // note in the block after this one — dropping it would leave it sounding forever.
        let mut sampler = playing(stocked(), 0, 512);
        let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        let mut out = AudioBuffer::stereo(512, RATE);

        sampler.process(&[note_on(511)], &mut out, &ctx);
        assert_eq!(sampler.active_voices(), 1);
        sampler.process(&[], &mut out, &ctx);
        assert!(rms(out.channel(0)) > 0.01);
    }

    #[test]
    fn a_sampler_that_was_told_nothing_plays_the_first_font_loaded() {
        // Dropping the sampler onto a track and hearing nothing at all would read as broken, so
        // an unchosen preset takes the first font rather than silence.
        let mut sampler = Sampler::new(stocked());
        assert_eq!(sampler.preset(), None);
        sampler.prepare(&PrepareContext::new(RATE, 512, 2));
        assert!(sampler.has_voice());

        let mut out = AudioBuffer::stereo(512, RATE);
        let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        sampler.process(&[note_on(0)], &mut out, &ctx);
        assert!(rms(out.channel(0)) > 0.01);
    }

    #[test]
    fn a_font_the_bank_does_not_hold_stays_silent_rather_than_playing_another() {
        // The file has moved. Quietly playing some other font's piano would hide that, and the
        // project would come back subtly wrong instead of obviously incomplete.
        let bank = stocked();
        let mut sampler = Sampler::new(bank);
        sampler.preset = Some(PresetRef {
            font: SoundFontId(99),
            bank: 0,
            patch: 0,
        });
        sampler.prepare(&PrepareContext::new(RATE, 512, 2));
        assert!(!sampler.has_voice());

        let mut out = AudioBuffer::stereo(512, RATE);
        let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        sampler.process(&[note_on(0)], &mut out, &ctx);
        assert_eq!(rms(out.channel(0)), 0.0);
    }

    #[test]
    fn a_font_that_breaks_the_synthesiser_costs_its_sound_and_not_the_process() {
        // The library walks a looping voice by folding the position back one loop length per
        // frame; this font's one-frame loop at a preposterous claimed rate outruns the fold,
        // runs off the end of the sample data and hits an unchecked index. Uncontained, that
        // panic unwinds into the C audio callback and aborts the whole application.
        let bank = SoundFontBank::shared();
        bank.insert(FONT, crate::test_support::runaway_font(RATE as i32 * 512));
        let mut sampler = playing(bank, 0, 512);

        let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        let mut out = AudioBuffer::stereo(512, RATE);
        for sample in out.channel_mut(0) {
            *sample = 0.5;
        }
        sampler.process(&[note_on(0)], &mut out, &ctx);

        assert!(
            sampler.poisoned,
            "the runaway loop should have taken the synthesiser down"
        );
        assert!(
            out.channel(0).iter().all(|s| *s == 0.0),
            "a poisoned block must come back as silence, not as what was in the buffer"
        );
        assert_eq!(sampler.active_voices(), 0);

        // Every later block is silence rather than another attempt, and a reset must not call
        // back into whatever state the panic left behind.
        sampler.reset();
        let mut later = AudioBuffer::stereo(512, RATE);
        for sample in later.channel_mut(0) {
            *sample = 0.5;
        }
        sampler.process(&[note_on(0)], &mut later, &ctx);
        assert!(later.channel(0).iter().all(|s| *s == 0.0));

        // `prepare` is the way back to life.
        sampler.prepare(&PrepareContext::new(RATE, 512, 2));
        assert!(!sampler.poisoned);
        assert!(sampler.has_voice());
    }

    #[test]
    fn a_reset_never_goes_back_to_the_bank() {
        // `reset` runs on the audio callback thread — every transport stop and seek reaches it
        // — so it must play from what `prepare` resolved rather than take the bank's lock.
        // Observable from outside: a font that leaves the bank after `prepare` keeps sounding,
        // and keeps sounding as the sound the track chose rather than the library's default.
        let bank = stocked();
        let frames = 1_024;
        let mut sampler = playing(bank.clone(), 42, frames);
        bank.remove(FONT);

        let ctx = ProcessContext::realtime(RATE, frames, 0, 120.0, true);
        let level = |sampler: &mut Sampler| {
            let mut out = AudioBuffer::stereo(frames, RATE);
            sampler.process(&[note_on(0)], &mut out, &ctx);
            rms(&out.channel(0)[frames / 2..])
        };
        let before = level(&mut sampler);
        sampler.reset();
        let after = level(&mut sampler);
        assert!(
            (before - after).abs() < before * 0.05,
            "the chosen sound should survive a reset with the bank emptied: {before} then {after}"
        );
    }

    #[test]
    fn a_reset_keeps_playing_the_sound_the_track_chose() {
        // Resetting the synthesiser resets its channels too, which would otherwise silently
        // return the track to bank 0 patch 0 — a different instrument, from the same font.
        let bank = stocked();
        let frames = 1_024;
        let ctx = ProcessContext::realtime(RATE, frames, 0, 120.0, true);

        let level = |sampler: &mut Sampler| {
            let mut out = AudioBuffer::stereo(frames, RATE);
            sampler.process(&[note_on(0)], &mut out, &ctx);
            rms(&out.channel(0)[frames / 2..])
        };

        let mut sampler = playing(bank, 42, frames);
        let before = level(&mut sampler);
        sampler.reset();
        let after = level(&mut sampler);
        assert!(
            (before - after).abs() < before * 0.05,
            "the quiet preset came back at a different level: {before} then {after}"
        );
    }

    #[test]
    fn a_mono_host_gets_the_two_channels_folded_together() {
        let mut sampler = Sampler::new(stocked());
        sampler.preset = Some(PresetRef {
            font: FONT,
            bank: 0,
            patch: 0,
        });
        sampler.prepare(&PrepareContext::new(RATE, 512, 1));

        let mut out = AudioBuffer::new(1, 512, RATE);
        let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        sampler.process(&[note_on(0)], &mut out, &ctx);
        assert!(rms(out.channel(0)) > 0.01);
    }

    #[test]
    fn nothing_on_the_audio_path_allocates() {
        // The realtime contract, as a test rather than as a sentence in a doc comment. It covers
        // `process`, `set_param` and `reset`, which are the three things the audio thread calls
        // — `reset` is how this file once ended up taking a lock and logging on the callback.
        let mut sampler = playing(stocked(), 0, 512);
        let mut out = AudioBuffer::stereo(512, RATE);
        let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        let events = [
            note_on(0),
            NoteEvent::NoteOff {
                frame: 128,
                pitch: crate::test_support::ROOT_KEY,
            },
            NoteEvent::PitchBend {
                frame: 200,
                semitones: 2.0,
            },
            NoteEvent::AllNotesOff { frame: 400 },
        ];
        // Warm up outside the watched region, so first-touch growth is not counted.
        sampler.process(&events, &mut out, &ctx);

        let allocations = crate::test_support::count_allocations(|| {
            for _ in 0..64 {
                sampler.process(&events, &mut out, &ctx);
                sampler.set_param_by_key("level", -3.0);
                sampler.reset();
            }
        });
        assert_eq!(allocations, 0, "the sampler allocated while rendering");
    }

    #[test]
    fn the_level_parameter_moves_the_output() {
        let bank = stocked();
        let frames = 1_024;
        let ctx = ProcessContext::realtime(RATE, frames, 0, 120.0, true);

        let measure = |level: f32| {
            let mut sampler = playing(bank.clone(), 0, frames);
            sampler.set_param_by_key("level", level);
            let mut out = AudioBuffer::stereo(frames, RATE);
            sampler.process(&[note_on(0)], &mut out, &ctx);
            rms(&out.channel(0)[frames / 2..])
        };

        // Six decibels down is half the amplitude.
        let ratio = measure(0.0) / measure(-6.0206);
        assert!(
            (ratio - 2.0).abs() < 0.1,
            "-6 dB should halve the output, not scale it by {ratio}"
        );
    }
}
