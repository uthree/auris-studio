//! The traits that make Auris Studio extensible.
//!
//! A **sound source** implements [`Instrument`]; an **audio processor** implements [`Effect`].
//! Both are plain Rust traits with no macros or ABI concerns, so adding a new synth or effect
//! means writing one type and registering a factory with
//! [`PluginRegistry`](crate::registry::PluginRegistry) — no changes to the engine or UI.
//!
//! # Realtime contract
//!
//! [`Instrument::process`] and [`Effect::process`] run on the audio callback thread. They must
//! not allocate, lock, block, or perform I/O. Anything expensive belongs in
//! [`Instrument::prepare`] / [`Effect::prepare`], which the engine calls from a normal thread
//! before playback and whenever the sample rate or block size changes.
//!
//! The same `process` methods are also used by the offline renderer, where the realtime rules
//! do not apply but determinism does: a plugin must produce identical output for identical
//! input and state, so that an export matches what was heard.

use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::BTreeMap;

use crate::buffer::AudioBuffer;
use crate::param::{ParamDescriptor, ParamId};

/// Broad classification used to group plugins in the UI's browser.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum PluginCategory {
    /// A tone generator driven by notes.
    Synth,
    /// A pitched or unpitched sample player.
    Sampler,
    /// A drum machine.
    Drum,
    /// Filters and equalisers.
    Equalizer,
    /// Compressors, limiters, gates.
    Dynamics,
    /// Echoes.
    Delay,
    /// Reverberation.
    Reverb,
    /// Saturation, overdrive, bitcrushing.
    Distortion,
    /// Chorus, flanger, phaser, tremolo.
    Modulation,
    /// Gain, pan, width, metering helpers.
    Utility,
    /// Anything that does not fit the buckets above.
    Other,
}

impl PluginCategory {
    /// Every category, in the order a browser should list them under.
    ///
    /// Deliberately not alphabetical, which would put Delay, Distortion and Dynamics together
    /// for no reason but a shared letter. Instruments first, then effects roughly along a mixing
    /// chain — tone, then level, then colour, then space — with the two catch-alls last.
    ///
    /// A browser groups by this, so a category missing here is a plugin nobody can find.
    pub const ALL: [PluginCategory; 11] = [
        PluginCategory::Synth,
        PluginCategory::Sampler,
        PluginCategory::Drum,
        PluginCategory::Equalizer,
        PluginCategory::Dynamics,
        PluginCategory::Distortion,
        PluginCategory::Modulation,
        PluginCategory::Delay,
        PluginCategory::Reverb,
        PluginCategory::Utility,
        PluginCategory::Other,
    ];

    /// Label shown in the plugin browser.
    pub fn label(self) -> &'static str {
        match self {
            PluginCategory::Synth => "Synth",
            PluginCategory::Sampler => "Sampler",
            PluginCategory::Drum => "Drum",
            PluginCategory::Equalizer => "Equalizer",
            PluginCategory::Dynamics => "Dynamics",
            PluginCategory::Delay => "Delay",
            PluginCategory::Reverb => "Reverb",
            PluginCategory::Distortion => "Distortion",
            PluginCategory::Modulation => "Modulation",
            PluginCategory::Utility => "Utility",
            PluginCategory::Other => "Other",
        }
    }
}

/// Whether a plugin generates audio or transforms it.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PluginKind {
    /// Produces audio from note events.
    Instrument,
    /// Transforms audio in place.
    Effect,
}

/// Identity and presentation metadata for a plugin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginDescriptor {
    /// Globally unique, stable id — for example `auris.synth.pulse`. Project files store this,
    /// so it must never change once released.
    pub id: Cow<'static, str>,
    /// Name shown to the user.
    pub name: Cow<'static, str>,
    /// Who wrote it.
    pub vendor: Cow<'static, str>,
    /// One-line summary for the browser.
    pub description: Cow<'static, str>,
    /// Whether this is an instrument or an effect.
    pub kind: PluginKind,
    /// Browser grouping.
    pub category: PluginCategory,
}

impl PluginDescriptor {
    /// Builds an instrument descriptor.
    pub fn instrument(
        id: &'static str,
        name: &'static str,
        description: &'static str,
        category: PluginCategory,
    ) -> Self {
        Self {
            id: Cow::Borrowed(id),
            name: Cow::Borrowed(name),
            vendor: Cow::Borrowed("Auris Studio"),
            description: Cow::Borrowed(description),
            kind: PluginKind::Instrument,
            category,
        }
    }

    /// Builds an effect descriptor.
    pub fn effect(
        id: &'static str,
        name: &'static str,
        description: &'static str,
        category: PluginCategory,
    ) -> Self {
        Self {
            id: Cow::Borrowed(id),
            name: Cow::Borrowed(name),
            vendor: Cow::Borrowed("Auris Studio"),
            description: Cow::Borrowed(description),
            kind: PluginKind::Effect,
            category,
        }
    }
}

/// Everything a plugin needs to size its internal buffers.
///
/// Delivered by [`Instrument::prepare`] / [`Effect::prepare`] away from the audio thread.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PrepareContext {
    /// Sample rate rendering will run at.
    pub sample_rate: f64,
    /// Largest block `process` will ever be handed. Size internal delay lines and scratch
    /// buffers for at least this many frames.
    pub max_block_frames: usize,
    /// Channel count of the buffers `process` will receive.
    pub channel_count: usize,
}

impl PrepareContext {
    /// Convenience constructor.
    pub fn new(sample_rate: f64, max_block_frames: usize, channel_count: usize) -> Self {
        Self {
            sample_rate,
            max_block_frames,
            channel_count,
        }
    }
}

/// Per-block information handed to `process`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ProcessContext {
    /// Sample rate of this render pass.
    pub sample_rate: f64,
    /// Frames in this block. Always equal to the buffer's frame count.
    pub block_frames: usize,
    /// Sample position of the block's first frame on the project timeline.
    pub playhead_samples: u64,
    /// Tempo in effect at the start of the block, for tempo-synced effects.
    pub bpm: f64,
    /// `false` while the transport is stopped (plugins still process, so reverb tails ring out).
    pub is_playing: bool,
    /// `true` when rendering offline; plugins may trade latency for quality here.
    pub is_offline: bool,
}

impl ProcessContext {
    /// A context for a realtime block.
    pub fn realtime(
        sample_rate: f64,
        block_frames: usize,
        playhead_samples: u64,
        bpm: f64,
        is_playing: bool,
    ) -> Self {
        Self {
            sample_rate,
            block_frames,
            playhead_samples,
            bpm,
            is_playing,
            is_offline: false,
        }
    }

    /// Seconds elapsed at the start of this block.
    pub fn playhead_seconds(&self) -> f64 {
        self.playhead_samples as f64 / self.sample_rate
    }
}

/// A note or controller event scheduled inside a block.
///
/// `frame` is the offset from the start of the block, which is what lets notes land on exact
/// sample positions instead of being quantised to the block grid.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum NoteEvent {
    /// Start a note.
    NoteOn {
        /// Offset within the block.
        frame: u32,
        /// MIDI note number, 0..=127. 69 is A440.
        pitch: u8,
        /// Attack strength, 0..=1.
        velocity: f32,
    },
    /// Release a note.
    NoteOff {
        /// Offset within the block.
        frame: u32,
        /// MIDI note number to release.
        pitch: u8,
    },
    /// Release every sounding note, letting them decay naturally.
    AllNotesOff {
        /// Offset within the block.
        frame: u32,
    },
    /// Silence everything immediately, without a release stage.
    AllSoundOff {
        /// Offset within the block.
        frame: u32,
    },
    /// Bend every sounding note.
    PitchBend {
        /// Offset within the block.
        frame: u32,
        /// Bend amount in semitones.
        semitones: f32,
    },
    /// Move the modulation wheel.
    ///
    /// MIDI controller 1, and channel state like the bend: an instrument holds the last value it
    /// was given until it is given another. What an instrument *does* with it is the instrument's
    /// own business — the built-in synths open a vibrato, and the sampler hands it to the font,
    /// which may have been authored to do something else entirely with it.
    Modulation {
        /// Offset within the block.
        frame: u32,
        /// How far the wheel is up, 0 to 1.
        amount: f32,
    },
}

impl NoteEvent {
    /// Offset of this event within its block.
    pub fn frame(&self) -> u32 {
        match *self {
            NoteEvent::NoteOn { frame, .. }
            | NoteEvent::NoteOff { frame, .. }
            | NoteEvent::AllNotesOff { frame }
            | NoteEvent::AllSoundOff { frame }
            | NoteEvent::PitchBend { frame, .. }
            | NoteEvent::Modulation { frame, .. } => frame,
        }
    }

    /// Returns a copy of this event moved to a different offset.
    pub fn with_frame(mut self, new_frame: u32) -> Self {
        match &mut self {
            NoteEvent::NoteOn { frame, .. }
            | NoteEvent::NoteOff { frame, .. }
            | NoteEvent::AllNotesOff { frame }
            | NoteEvent::AllSoundOff { frame }
            | NoteEvent::PitchBend { frame, .. }
            | NoteEvent::Modulation { frame, .. } => *frame = new_frame,
        }
        self
    }
}

/// Converts a MIDI note number to frequency in Hz using equal temperament with A4 = 440 Hz.
pub fn pitch_to_hz(pitch: f32) -> f32 {
    440.0 * ((pitch - 69.0) / 12.0).exp2()
}

/// Saved plugin state: parameter values keyed by their stable string keys, plus an optional
/// blob for anything a plugin cannot express as a flat parameter list.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginState {
    /// Parameter values by [`ParamDescriptor::key`](crate::param::ParamDescriptor::key).
    #[serde(default)]
    pub params: BTreeMap<String, f32>,
    /// Free-form extra state; `null` when unused.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub extra: serde_json::Value,
}

impl PluginState {
    /// An empty state — the plugin keeps its defaults.
    pub fn empty() -> Self {
        Self::default()
    }
}

/// Common parameter access shared by instruments and effects.
///
/// Implementors only need the first three methods; the rest have working defaults.
pub trait Parameterized {
    /// The plugin's parameters. Position in the slice must match each descriptor's `id`.
    fn parameters(&self) -> &[ParamDescriptor];

    /// Reads a parameter's current value. Out-of-range ids return `0.0`.
    fn param(&self, id: ParamId) -> f32;

    /// Writes a parameter. Implementors should clamp with the matching descriptor and may
    /// recompute cached coefficients here — this is called from the audio thread, so keep it
    /// allocation-free.
    fn set_param(&mut self, id: ParamId, value: f32);

    /// Looks up a descriptor by its stable key.
    fn descriptor_for_key(&self, key: &str) -> Option<&ParamDescriptor> {
        self.parameters().iter().find(|p| p.key == key)
    }

    /// Reads a parameter by its stable key.
    fn param_by_key(&self, key: &str) -> Option<f32> {
        self.descriptor_for_key(key)
            .map(|p| p.id)
            .map(|id| self.param(id))
    }

    /// Writes a parameter by its stable key, returning `false` when the key is unknown.
    fn set_param_by_key(&mut self, key: &str, value: f32) -> bool {
        match self.descriptor_for_key(key).map(|p| p.id) {
            Some(id) => {
                self.set_param(id, value);
                true
            }
            None => false,
        }
    }

    /// Resets every parameter to its default.
    fn reset_params(&mut self) {
        let defaults: Vec<(ParamId, f32)> = self
            .parameters()
            .iter()
            .map(|p| (p.id, p.default))
            .collect();
        for (id, value) in defaults {
            self.set_param(id, value);
        }
    }

    /// Captures the current parameter values for saving.
    fn save_state(&self) -> PluginState {
        PluginState {
            params: self
                .parameters()
                .iter()
                .map(|p| (p.key.to_string(), self.param(p.id)))
                .collect(),
            extra: serde_json::Value::Null,
        }
    }

    /// Applies a previously saved state. Unknown keys are ignored, which keeps old project
    /// files loadable after a plugin gains or drops parameters.
    fn load_state(&mut self, state: &PluginState) {
        for (key, value) in &state.params {
            self.set_param_by_key(key, *value);
        }
    }
}

/// A sound source driven by note events.
///
/// Implement this to add a new software instrument. See `auris-synth` for worked examples.
pub trait Instrument: Parameterized + Send {
    /// Identity and presentation metadata.
    fn descriptor(&self) -> PluginDescriptor;

    /// Called before rendering starts and whenever the sample rate or block size changes.
    /// Allocate here, never in [`Self::process`].
    fn prepare(&mut self, ctx: &PrepareContext);

    /// Drops all sounding voices and clears internal state without changing parameters.
    fn reset(&mut self);

    /// Renders one block.
    ///
    /// `events` is sorted by [`NoteEvent::frame`] and every frame is `< ctx.block_frames`.
    /// The implementation **overwrites** `out` — it does not add to whatever was there.
    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, ctx: &ProcessContext);

    /// Number of voices currently sounding, for the UI's activity indicator.
    fn active_voices(&self) -> usize {
        0
    }
}

/// An audio processor placed in a track's or the master bus's effect chain.
///
/// Implement this to add a new effect.
pub trait Effect: Parameterized + Send {
    /// Identity and presentation metadata.
    fn descriptor(&self) -> PluginDescriptor;

    /// Called before rendering starts and whenever the sample rate or block size changes.
    fn prepare(&mut self, ctx: &PrepareContext);

    /// Clears delay lines and filter memory without changing parameters.
    fn reset(&mut self);

    /// Transforms `buffer` in place.
    fn process(&mut self, buffer: &mut AudioBuffer, ctx: &ProcessContext);

    /// How many frames of audio keep coming out after the input goes silent.
    ///
    /// The offline renderer keeps rendering for this long past the end of the project so that
    /// reverb and delay tails are not cut off in the exported file.
    fn tail_frames(&self) -> usize {
        0
    }

    /// Latency this effect introduces, in frames. Reported for future delay compensation;
    /// every built-in effect is currently zero-latency.
    fn latency_frames(&self) -> usize {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::param::ParamDescriptor;

    #[test]
    fn every_category_has_a_place_in_the_browser_order() {
        // The match is exhaustive and has no wildcard, so adding a category without adding it to
        // `ALL` stops compiling here — which is the point. A browser groups by `ALL`, and a
        // category left out of it would take its plugins off the list without saying so.
        for category in PluginCategory::ALL {
            let listed = match category {
                PluginCategory::Synth
                | PluginCategory::Sampler
                | PluginCategory::Drum
                | PluginCategory::Equalizer
                | PluginCategory::Dynamics
                | PluginCategory::Distortion
                | PluginCategory::Modulation
                | PluginCategory::Delay
                | PluginCategory::Reverb
                | PluginCategory::Utility
                | PluginCategory::Other => true,
            };
            assert!(listed);
        }

        let mut seen: Vec<&str> = PluginCategory::ALL.iter().map(|c| c.label()).collect();
        let listed = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(listed, seen.len(), "a category is listed twice");
    }

    struct Dummy {
        params: Vec<ParamDescriptor>,
        values: Vec<f32>,
    }

    impl Dummy {
        fn new() -> Self {
            let params = vec![
                ParamDescriptor::new(0u32, "alpha", "Alpha", 0.0, 1.0, 0.25),
                ParamDescriptor::new(1u32, "beta", "Beta", -1.0, 1.0, 0.0),
            ];
            let values = params.iter().map(|p| p.default).collect();
            Self { params, values }
        }
    }

    impl Parameterized for Dummy {
        fn parameters(&self) -> &[ParamDescriptor] {
            &self.params
        }
        fn param(&self, id: ParamId) -> f32 {
            self.values.get(id.index()).copied().unwrap_or(0.0)
        }
        fn set_param(&mut self, id: ParamId, value: f32) {
            if let Some(descriptor) = self.params.get(id.index()) {
                self.values[id.index()] = descriptor.clamp(value);
            }
        }
    }

    #[test]
    fn state_round_trips_through_keys() {
        let mut plugin = Dummy::new();
        plugin.set_param_by_key("alpha", 0.75);
        plugin.set_param_by_key("beta", -0.5);
        let state = plugin.save_state();

        let mut restored = Dummy::new();
        restored.load_state(&state);
        assert_eq!(restored.param_by_key("alpha"), Some(0.75));
        assert_eq!(restored.param_by_key("beta"), Some(-0.5));
    }

    #[test]
    fn unknown_keys_are_ignored_on_load() {
        let mut plugin = Dummy::new();
        let mut state = PluginState::empty();
        state.params.insert("gamma".into(), 1.0);
        state.params.insert("alpha".into(), 0.9);
        plugin.load_state(&state);
        assert_eq!(plugin.param_by_key("alpha"), Some(0.9));
        assert_eq!(plugin.param_by_key("gamma"), None);
    }

    #[test]
    fn set_param_clamps_to_the_descriptor_range() {
        let mut plugin = Dummy::new();
        plugin.set_param_by_key("alpha", 5.0);
        assert_eq!(plugin.param_by_key("alpha"), Some(1.0));
    }

    #[test]
    fn a440_is_midi_note_69() {
        assert!((pitch_to_hz(69.0) - 440.0).abs() < 1e-3);
        assert!((pitch_to_hz(81.0) - 880.0).abs() < 1e-3);
    }

    #[test]
    fn events_report_and_move_their_frame() {
        let event = NoteEvent::NoteOn {
            frame: 10,
            pitch: 60,
            velocity: 1.0,
        };
        assert_eq!(event.frame(), 10);
        assert_eq!(event.with_frame(3).frame(), 3);
    }
}
