//! Playing one recorded sound across the keyboard.
//!
//! [`AudioSampler`] is the other kind of sampler: where [`Sampler`](crate::Sampler) plays a
//! SoundFont somebody else has already mapped, split and enveloped, this one takes a single audio
//! file — a hit, a vocal chop, a note of somebody's own instrument — and makes an instrument out
//! of it. The key the sound was recorded at is its *root note*; every other key plays it faster or
//! slower by the equal-tempered ratio, exactly as a tape played at a different speed would.
//!
//! # Where the audio comes from
//!
//! The samples are the project's own audio sources, which is the whole design. An imported file
//! is already copied into the project folder, found again when the folder moves and reported when
//! it cannot be, and the sampler gets all of that by naming a [`SourceId`] rather than a path. The
//! decoded buffers reach it the way fonts reach the SoundFont player: through a [`SampleBank`] the
//! registry's factory closure captured, because a factory takes no arguments and a
//! [`PluginState`] is a map of `f32`.
//!
//! # Realtime behaviour
//!
//! The buffer is looked up in [`Instrument::prepare`] and held by `Arc` until the next one, so
//! `process` never takes the bank's lock. Voices are a fixed pool built in [`AudioSampler::new`];
//! starting, stealing and releasing one is arithmetic on that pool.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use auris_core::param::{ParamDescriptor, ParamId, ParamUnit, ParamValueCurve, db_to_gain};
use auris_core::plugin::{
    Instrument, NoteEvent, Parameterized, PluginCategory, PluginDescriptor, PluginState,
    PrepareContext, ProcessContext,
};
use auris_core::registry::PluginRegistry;
use auris_core::{AudioBuffer, SourceId};
use auris_dsp::{Adsr, SmoothedValue};

/// The audio sampler's plugin id, as a project file stores it.
pub const AUDIO_SAMPLER_ID: &str = "auris.sampler.audio";

/// Where the chosen sample lives inside [`PluginState::extra`].
const SAMPLE_KEY: &str = "sample";

/// Voices one instance may sound at once.
///
/// A sample with a long tail and a quick part can stack a few dozen; past that the oldest-and-
/// quietest is the one nobody hears go.
const VOICES: usize = 32;

/// The shortest loop that is honoured, in frames.
///
/// A handful of frames looped is a buzz at the sample rate divided by the length, which is never
/// what anybody setting two handles close together meant. Below this the loop is ignored and the
/// sound plays to its end.
const MIN_LOOP_FRAMES: usize = 16;

/// Time constant used when a live level change glides to its new gain.
const LEVEL_SMOOTHING_SECONDS: f32 = 0.020;

const LEVEL: usize = 0;
const ROOT: usize = 1;
const TUNE: usize = 2;
const KEY_TRACKING: usize = 3;
const ATTACK: usize = 4;
const DECAY: usize = 5;
const SUSTAIN: usize = 6;
const RELEASE: usize = 7;
const LOOP: usize = 8;
const LOOP_START: usize = 9;
const LOOP_END: usize = 10;
const PARAM_COUNT: usize = 11;

/// The key of the root-note parameter, so a session command can set it without knowing where it
/// sits.
pub const ROOT_KEY: &str = "root";

/// The key of the loop switch.
pub const LOOP_KEY: &str = "loop";

/// A sample bank shared between the session that fills it and the samplers that read it.
pub type SharedSamples = Arc<SampleBank>;

/// Decoded audio the audio sampler may play, by the source id the project names it with.
///
/// The session mirrors its own decoded sources into this, so a sampler track and an audio clip
/// naming the same file share one buffer. **Nothing here is called from the audio thread**: a
/// sampler takes its `Arc` in `prepare`.
#[derive(Default)]
pub struct SampleBank {
    samples: RwLock<BTreeMap<SourceId, Arc<AudioBuffer>>>,
}

impl SampleBank {
    /// An empty bank.
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty bank, ready to be shared.
    pub fn shared() -> SharedSamples {
        Arc::new(Self::new())
    }

    /// Stores decoded audio under its source id, replacing whatever was there.
    pub fn insert(&self, id: SourceId, buffer: Arc<AudioBuffer>) {
        self.write().insert(id, buffer);
    }

    /// The audio for this source, if it is loaded.
    pub fn get(&self, id: SourceId) -> Option<Arc<AudioBuffer>> {
        self.read().get(&id).cloned()
    }

    /// Forgets one source. Samplers already holding it keep playing until prepared again.
    pub fn remove(&self, id: SourceId) -> Option<Arc<AudioBuffer>> {
        self.write().remove(&id)
    }

    /// Makes the bank hold exactly `sources`.
    ///
    /// One call rather than a diff at every place the session's own bank changes: the session
    /// calls this before each graph build, which is the only moment a sampler reads the bank.
    pub fn replace_all<'a>(
        &self,
        sources: impl IntoIterator<Item = (SourceId, &'a Arc<AudioBuffer>)>,
    ) {
        let next: BTreeMap<_, _> = sources
            .into_iter()
            .map(|(id, buffer)| (id, Arc::clone(buffer)))
            .collect();
        *self.write() = next;
    }

    /// How many sources are loaded.
    pub fn len(&self) -> usize {
        self.read().len()
    }

    /// `true` when nothing is loaded.
    pub fn is_empty(&self) -> bool {
        self.read().is_empty()
    }

    /// Reads through the lock, ignoring poisoning, for the reason [`crate::SoundFontBank`] gives.
    fn read(&self) -> std::sync::RwLockReadGuard<'_, BTreeMap<SourceId, Arc<AudioBuffer>>> {
        self.samples
            .read()
            .unwrap_or_else(|error| error.into_inner())
    }

    /// Writes through the lock, ignoring poisoning.
    fn write(&self) -> std::sync::RwLockWriteGuard<'_, BTreeMap<SourceId, Arc<AudioBuffer>>> {
        self.samples
            .write()
            .unwrap_or_else(|error| error.into_inner())
    }
}

impl std::fmt::Debug for SampleBank {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ids: Vec<SourceId> = self.read().keys().copied().collect();
        f.debug_struct("SampleBank").field("samples", &ids).finish()
    }
}

/// Writes the sample choice into the state a project stores for a track's instrument.
pub fn store_sample(state: &mut PluginState, source: SourceId) {
    let encoded = serde_json::Value::from(source.0);
    match state.extra.as_object_mut() {
        Some(map) => {
            map.insert(SAMPLE_KEY.to_string(), encoded);
        }
        None => {
            let mut map = serde_json::Map::new();
            map.insert(SAMPLE_KEY.to_string(), encoded);
            state.extra = serde_json::Value::Object(map);
        }
    }
}

/// Reads the sample choice back, or `None` when the state names none.
pub fn stored_sample(state: &PluginState) -> Option<SourceId> {
    state.extra.get(SAMPLE_KEY)?.as_u64().map(SourceId)
}

/// Registers the audio sampler, giving it the bank it will read samples from.
pub fn register_audio_sampler(registry: &mut PluginRegistry, samples: SharedSamples) {
    registry.register_instrument(move || Box::new(AudioSampler::new(Arc::clone(&samples))));
}

/// `true` for an instrument that is silent until a file has been chosen for it.
///
/// Both samplers are: a SoundFont player with no preset and an audio sampler with no sample are
/// registered instruments that make no sound. Anything choosing an instrument *for* somebody — a
/// fallback, a timbre search, a composer filling a track — must skip them.
pub fn needs_asset(instrument_id: &str) -> bool {
    instrument_id == crate::SAMPLER_ID || instrument_id == AUDIO_SAMPLER_ID
}

/// One sounding copy of the sample.
#[derive(Debug)]
struct Voice {
    envelope: Adsr,
    /// The key that started it, or `None` when the voice is free.
    key: Option<u8>,
    /// Whether its key is still held; a released voice is fading and does not answer note-offs.
    held: bool,
    /// Read position in source frames, fractional.
    position: f64,
    /// Source frames advanced per output frame before the pitch bend.
    step: f64,
    gain: f32,
    /// When the voice was started, so the oldest can be found.
    started: u64,
}

impl Voice {
    fn new() -> Self {
        Self {
            envelope: Adsr::new(),
            key: None,
            held: false,
            position: 0.0,
            step: 1.0,
            gain: 0.0,
            started: 0,
        }
    }

    fn free(&mut self) {
        self.key = None;
        self.held = false;
        self.envelope.silence();
    }
}

/// Plays one audio source across the keyboard, pitched from its root note.
///
/// Velocity is linear, as it is for every other instrument here: a note at half velocity is the
/// sample at half amplitude. With **Key Tracking** off every key plays the sound at the pitch it
/// was recorded at — the usual way to play a drum hit or an effect from a sampler.
///
/// The loop, when it is on, runs between two fractions of the sample's length for as long as the
/// note is held *and* through its release, so the release time is what ends a looped note. Off,
/// the sound plays to its end and the note is over there whatever the envelope says.
pub struct AudioSampler {
    samples: SharedSamples,
    params: Vec<ParamDescriptor>,
    values: [f32; PARAM_COUNT],
    level: SmoothedValue,
    /// Which source to play, from the saved state.
    source: Option<SourceId>,
    /// The audio resolved in `prepare`; `None` plays silence.
    buffer: Option<Arc<AudioBuffer>>,
    /// Output sample rate from the last `prepare`.
    rate: f64,
    /// Loop bounds in frames, `None` when looping is off or the region is too short.
    loop_region: Option<(usize, usize)>,
    voices: Vec<Voice>,
    clock: u64,
    /// Frequency ratio of the current pitch bend.
    bend: f64,
}

impl AudioSampler {
    /// A sampler reading audio from `samples`.
    pub fn new(samples: SharedSamples) -> Self {
        let params = vec![
            ParamDescriptor::decibels(LEVEL as u32, "level", "Level", -60.0, 12.0, 0.0),
            ParamDescriptor::new(ROOT as u32, ROOT_KEY, "Root Note", 0.0, 127.0, 60.0)
                .with_steps(128),
            ParamDescriptor::new(TUNE as u32, "tune", "Fine Tune", -1.0, 1.0, 0.0)
                .with_unit(ParamUnit::Semitones),
            ParamDescriptor::toggle(KEY_TRACKING as u32, "key_tracking", "Key Tracking", true),
            ParamDescriptor::new(ATTACK as u32, "attack", "Attack", 0.0, 2.0, 0.0)
                .with_unit(ParamUnit::Seconds)
                .with_curve(ParamValueCurve::Power(3.0)),
            ParamDescriptor::new(DECAY as u32, "decay", "Decay", 0.0, 4.0, 0.0)
                .with_unit(ParamUnit::Seconds)
                .with_curve(ParamValueCurve::Power(3.0)),
            ParamDescriptor::percent(SUSTAIN as u32, "sustain", "Sustain", 1.0),
            // A tenth of a second, so letting go of a key fades rather than chops.
            ParamDescriptor::new(RELEASE as u32, "release", "Release", 0.0, 4.0, 0.1)
                .with_unit(ParamUnit::Seconds)
                .with_curve(ParamValueCurve::Power(3.0)),
            ParamDescriptor::toggle(LOOP as u32, LOOP_KEY, "Loop", false),
            ParamDescriptor::percent(LOOP_START as u32, "loop_start", "Loop Start", 0.0),
            ParamDescriptor::percent(LOOP_END as u32, "loop_end", "Loop End", 1.0),
        ];
        let mut values = [0.0; PARAM_COUNT];
        for (slot, descriptor) in values.iter_mut().zip(&params) {
            *slot = descriptor.default;
        }
        let mut sampler = Self {
            samples,
            params,
            values,
            level: SmoothedValue::new(1.0, LEVEL_SMOOTHING_SECONDS, 48_000.0),
            source: None,
            buffer: None,
            rate: 48_000.0,
            loop_region: None,
            voices: (0..VOICES).map(|_| Voice::new()).collect(),
            clock: 0,
            bend: 1.0,
        };
        sampler.reshape();
        sampler
    }

    /// Which source this sampler has been told to play.
    pub fn source(&self) -> Option<SourceId> {
        self.source
    }

    /// `true` once a sample has been found for it.
    pub fn has_sample(&self) -> bool {
        self.buffer.is_some()
    }

    fn reshape(&mut self) {
        let (attack, decay, sustain, release) = (
            self.values[ATTACK],
            self.values[DECAY],
            self.values[SUSTAIN],
            self.values[RELEASE],
        );
        for voice in &mut self.voices {
            voice.envelope.set_adsr(attack, decay, sustain, release);
        }
    }

    /// Restates the loop region in frames from the current values and buffer.
    fn relocate_loop(&mut self) {
        self.loop_region = None;
        let Some(buffer) = &self.buffer else { return };
        if self.values[LOOP] < 0.5 {
            return;
        }
        let length = buffer.frame_count();
        let start = (f64::from(self.values[LOOP_START]) * length as f64).floor() as usize;
        let end = (f64::from(self.values[LOOP_END]) * length as f64).ceil() as usize;
        let end = end.min(length);
        if end > start && end - start >= MIN_LOOP_FRAMES {
            self.loop_region = Some((start, end));
        }
    }

    /// Source frames per output frame for a key, before bending.
    fn step_for(&self, key: u8) -> f64 {
        let Some(buffer) = &self.buffer else {
            return 1.0;
        };
        let rate = if self.rate > 0.0 { self.rate } else { 48_000.0 };
        let base = buffer.sample_rate() / rate;
        let tune = f64::from(self.values[TUNE]);
        let semitones = if self.values[KEY_TRACKING] >= 0.5 {
            f64::from(key) - f64::from(self.values[ROOT]) + tune
        } else {
            tune
        };
        base * (semitones / 12.0).exp2()
    }

    fn note_on(&mut self, key: u8, velocity: f32) {
        if self.buffer.is_none() {
            return;
        }
        let index = self
            .voices
            .iter()
            .enumerate()
            .filter(|(_, voice)| voice.key.is_none())
            .min_by_key(|(_, voice)| voice.started)
            .or_else(|| {
                // Every voice is busy: the released ones go first, then the quietest.
                self.voices.iter().enumerate().min_by(|(_, a), (_, b)| {
                    (a.held, a.envelope.level())
                        .partial_cmp(&(b.held, b.envelope.level()))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
            })
            .map_or(0, |(index, _)| index);
        self.clock += 1;
        let step = self.step_for(key);
        let voice = &mut self.voices[index];
        // A fresh copy starts from silence; a stolen one starts from where it was, which is what
        // keeps a steal from clicking.
        if voice.key.is_none() {
            voice.envelope.silence();
        }
        voice.key = Some(key);
        voice.held = true;
        voice.position = 0.0;
        voice.step = step;
        voice.gain = velocity.clamp(0.0, 1.0);
        voice.started = self.clock;
        voice.envelope.trigger();
    }

    fn note_off(&mut self, key: u8) {
        for voice in &mut self.voices {
            if voice.held && voice.key == Some(key) {
                voice.held = false;
                voice.envelope.release();
            }
        }
    }

    fn dispatch(&mut self, event: NoteEvent) {
        match event {
            NoteEvent::NoteOn {
                pitch, velocity, ..
            } => self.note_on(pitch, velocity),
            NoteEvent::NoteOff { pitch, .. } => self.note_off(pitch),
            NoteEvent::AllNotesOff { .. } => {
                for voice in &mut self.voices {
                    voice.held = false;
                    voice.envelope.release();
                }
            }
            NoteEvent::AllSoundOff { .. } => {
                for voice in &mut self.voices {
                    voice.held = false;
                    voice.envelope.kill();
                }
            }
            NoteEvent::PitchBend { semitones, .. } => {
                let semitones = if semitones.is_finite() {
                    semitones
                } else {
                    0.0
                };
                self.bend = (f64::from(semitones) / 12.0).exp2();
            }
            NoteEvent::Controller { .. } => {}
        }
    }

    /// Renders `[from, to)` of `out`, adding every voice into it.
    fn render_range(&mut self, out: &mut AudioBuffer, from: usize, to: usize) {
        let Some(buffer) = self.buffer.as_ref() else {
            return;
        };
        let length = buffer.frame_count();
        let left = buffer.channel(0);
        let right = buffer.channel(buffer.channel_count().min(2) - 1);
        let region = self.loop_region;
        let channels = out.channel_count();
        for frame in from..to {
            let level = self.level.next_value();
            let mut sum_left = 0.0f32;
            let mut sum_right = 0.0f32;
            for voice in &mut self.voices {
                if voice.key.is_none() {
                    continue;
                }
                let envelope = voice.envelope.process();
                if voice.envelope.is_finished() {
                    voice.free();
                    continue;
                }
                let position = voice.position;
                let index = position.floor() as isize;
                let fraction = (position - index as f64) as f32;
                let gain = envelope * voice.gain;
                sum_left += gain * hermite(left, index, fraction, length, region);
                sum_right += gain * hermite(right, index, fraction, length, region);
                let mut next = position + voice.step * self.bend;
                match region {
                    Some((start, end)) if next >= end as f64 => {
                        let span = (end - start) as f64;
                        next = start as f64 + (next - start as f64).rem_euclid(span);
                    }
                    _ => {}
                }
                voice.position = next;
                if region.is_none() && next >= length as f64 {
                    voice.free();
                }
            }
            match channels {
                0 => {}
                1 => out.channel_mut(0)[frame] = 0.5 * level * (sum_left + sum_right),
                _ => {
                    out.channel_mut(0)[frame] = level * sum_left;
                    out.channel_mut(1)[frame] = level * sum_right;
                    for channel in 2..channels {
                        out.channel_mut(channel)[frame] = 0.0;
                    }
                }
            }
        }
    }
}

/// One frame of `data` at `index`, wrapped into the loop when there is one and silent outside.
fn frame_at(data: &[f32], index: isize, length: usize, region: Option<(usize, usize)>) -> f32 {
    let index = match region {
        Some((start, end)) if index >= end as isize => start as isize + (index - end as isize),
        _ => index,
    };
    if index < 0 || index as usize >= length {
        0.0
    } else {
        data[index as usize]
    }
}

/// Four-point Hermite interpolation between `index` and `index + 1`.
///
/// Linear interpolation dulls a sample pitched down and aliases one pitched up audibly more than
/// this does, and four reads a frame are nothing next to the rest of a voice.
fn hermite(
    data: &[f32],
    index: isize,
    fraction: f32,
    length: usize,
    region: Option<(usize, usize)>,
) -> f32 {
    let y0 = frame_at(data, index - 1, length, region);
    let y1 = frame_at(data, index, length, region);
    let y2 = frame_at(data, index + 1, length, region);
    let y3 = frame_at(data, index + 2, length, region);
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * fraction + c2) * fraction + c1) * fraction + y1
}

impl Parameterized for AudioSampler {
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
        match index {
            LEVEL => self.level.set_target(db_to_gain(self.values[LEVEL])),
            ATTACK | DECAY | SUSTAIN | RELEASE => self.reshape(),
            LOOP | LOOP_START | LOOP_END => self.relocate_loop(),
            _ => {}
        }
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
        if let Some(source) = self.source {
            store_sample(&mut state, source);
        }
        state
    }

    fn load_state(&mut self, state: &PluginState) {
        for (key, value) in &state.params {
            self.set_param_by_key(key, *value);
        }
        self.source = stored_sample(state);
    }
}

impl Instrument for AudioSampler {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::instrument(
            AUDIO_SAMPLER_ID,
            "Sampler",
            "Plays an audio file across the keyboard",
            PluginCategory::Sampler,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.rate = if ctx.sample_rate.is_finite() && ctx.sample_rate > 0.0 {
            ctx.sample_rate
        } else {
            48_000.0
        };
        self.level
            .set_time(LEVEL_SMOOTHING_SECONDS, self.rate as f32);
        self.level.snap_to(db_to_gain(self.values[LEVEL]));
        for voice in &mut self.voices {
            voice.envelope.set_sample_rate(self.rate as f32);
            voice.free();
        }
        self.bend = 1.0;
        // A source with no frames is no sample: there is nothing to pitch.
        self.buffer = self
            .source
            .and_then(|source| self.samples.get(source))
            .filter(|buffer| buffer.frame_count() > 0 && buffer.channel_count() > 0);
        if self.buffer.is_none()
            && let Some(source) = self.source
        {
            log::warn!("sampler: no audio loaded for source {}", source.0);
        }
        self.relocate_loop();
    }

    fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.free();
        }
        self.bend = 1.0;
        self.level.snap_to(db_to_gain(self.values[LEVEL]));
    }

    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, ctx: &ProcessContext) {
        out.clear();
        if self.buffer.is_none() {
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
            let end = events
                .get(next_event)
                .map_or(frames, |event| (event.frame() as usize).min(frames));
            self.render_range(out, cursor, end);
            cursor = end;
        }
        while let Some(event) = events.get(next_event) {
            self.dispatch(*event);
            next_event += 1;
        }
    }

    fn active_voices(&self) -> usize {
        self.voices
            .iter()
            .filter(|voice| voice.key.is_some())
            .count()
    }
}

impl std::fmt::Debug for AudioSampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioSampler")
            .field("source", &self.source)
            .field("has_sample", &self.has_sample())
            .field("active_voices", &self.active_voices())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::count_allocations;

    const RATE: f64 = 48_000.0;

    /// A sine at `hz`, `seconds` long, at the output rate.
    fn sine(hz: f64, seconds: f64) -> Arc<AudioBuffer> {
        let frames = (seconds * RATE) as usize;
        let data: Vec<f32> = (0..frames)
            .map(|n| (std::f64::consts::TAU * hz * n as f64 / RATE).sin() as f32 * 0.5)
            .collect();
        Arc::new(AudioBuffer::from_planar(vec![data], RATE).unwrap())
    }

    fn sampler_with(buffer: Arc<AudioBuffer>) -> AudioSampler {
        let bank = SampleBank::shared();
        bank.insert(SourceId(7), buffer);
        let mut sampler = AudioSampler::new(bank);
        let mut state = sampler.save_state();
        store_sample(&mut state, SourceId(7));
        sampler.load_state(&state);
        sampler.prepare(&PrepareContext::new(RATE, 512, 2));
        sampler
    }

    /// Renders `blocks` blocks of 512 with `events` in the first.
    fn render(sampler: &mut AudioSampler, events: &[NoteEvent], blocks: usize) -> Vec<f32> {
        let mut left = Vec::new();
        let mut out = AudioBuffer::new(2, 512, RATE);
        let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        for block in 0..blocks {
            let block_events: &[NoteEvent] = if block == 0 { events } else { &[] };
            sampler.process(block_events, &mut out, &ctx);
            left.extend_from_slice(out.channel(0));
        }
        left
    }

    /// Frequency estimated from positive-going zero crossings.
    fn frequency(samples: &[f32]) -> f64 {
        let crossings: Vec<usize> = samples
            .windows(2)
            .enumerate()
            .filter(|(_, pair)| pair[0] <= 0.0 && pair[1] > 0.0)
            .map(|(index, _)| index)
            .collect();
        let span = (crossings.last().unwrap() - crossings.first().unwrap()) as f64;
        (crossings.len() - 1) as f64 * RATE / span
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs()))
    }

    fn note(pitch: u8, velocity: f32) -> NoteEvent {
        NoteEvent::NoteOn {
            frame: 0,
            pitch,
            velocity,
        }
    }

    #[test]
    fn the_root_note_plays_the_sample_at_its_own_pitch() {
        let mut sampler = sampler_with(sine(440.0, 1.0));
        let out = render(&mut sampler, &[note(60, 1.0)], 40);
        let measured = frequency(&out[4_800..19_200]);
        assert!((measured - 440.0).abs() < 1.0, "{measured} Hz");
    }

    #[test]
    fn an_octave_up_doubles_the_frequency_and_halves_the_length() {
        let mut sampler = sampler_with(sine(440.0, 1.0));
        let out = render(&mut sampler, &[note(72, 1.0)], 100);
        let measured = frequency(&out[2_400..19_200]);
        assert!((measured - 880.0).abs() < 2.0, "{measured} Hz");
        // A one-second sample played twice as fast is over after half a second.
        assert!(peak(&out[24_100..]) == 0.0);
        assert_eq!(sampler.active_voices(), 0);
    }

    #[test]
    fn a_fifth_down_is_the_equal_tempered_ratio() {
        let mut sampler = sampler_with(sine(440.0, 2.0));
        let out = render(&mut sampler, &[note(53, 1.0)], 60);
        let expected = 440.0 * (-7.0f64 / 12.0).exp2();
        let measured = frequency(&out[4_800..28_800]);
        assert!(
            (measured - expected).abs() < 1.0,
            "{measured} vs {expected}"
        );
    }

    #[test]
    fn key_tracking_off_plays_every_key_at_the_recorded_pitch() {
        let mut sampler = sampler_with(sine(440.0, 1.0));
        sampler.set_param_by_key("key_tracking", 0.0);
        let out = render(&mut sampler, &[note(84, 1.0)], 40);
        let measured = frequency(&out[4_800..19_200]);
        assert!((measured - 440.0).abs() < 1.0, "{measured} Hz");
    }

    #[test]
    fn the_root_note_parameter_moves_the_mapping() {
        let mut sampler = sampler_with(sine(440.0, 1.0));
        sampler.set_param_by_key(ROOT_KEY, 69.0);
        // A 440 Hz recording declared as A4 plays 440 on A4 and 220 on A3.
        let out = render(&mut sampler, &[note(57, 1.0)], 40);
        let measured = frequency(&out[4_800..19_200]);
        assert!((measured - 220.0).abs() < 1.0, "{measured} Hz");
    }

    #[test]
    fn velocity_is_linear() {
        let mut loud = sampler_with(sine(440.0, 1.0));
        let mut quiet = sampler_with(sine(440.0, 1.0));
        let loud = peak(&render(&mut loud, &[note(60, 1.0)], 10)[1_000..]);
        let quiet = peak(&render(&mut quiet, &[note(60, 0.5)], 10)[1_000..]);
        assert!((loud - 0.5).abs() < 0.01, "{loud}");
        assert!((quiet / loud - 0.5).abs() < 0.01, "{}", quiet / loud);
    }

    #[test]
    fn a_released_note_falls_silent_within_its_release_time() {
        let mut sampler = sampler_with(sine(440.0, 4.0));
        let mut out = AudioBuffer::new(2, 512, RATE);
        let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        sampler.process(&[note(60, 1.0)], &mut out, &ctx);
        sampler.process(
            &[NoteEvent::NoteOff {
                frame: 0,
                pitch: 60,
            }],
            &mut out,
            &ctx,
        );
        // Release defaults to 0.1 s = 4800 frames, so ten blocks later it is gone.
        for _ in 0..10 {
            sampler.process(&[], &mut out, &ctx);
        }
        assert_eq!(sampler.active_voices(), 0);
        sampler.process(&[], &mut out, &ctx);
        assert_eq!(peak(out.channel(0)), 0.0);
    }

    #[test]
    fn a_looped_note_outlasts_its_sample_while_held() {
        let mut sampler = sampler_with(sine(440.0, 0.1));
        sampler.set_param_by_key(LOOP_KEY, 1.0);
        let out = render(&mut sampler, &[note(60, 1.0)], 100);
        // A tenth of a second of audio, still sounding a second later.
        assert!(peak(&out[48_000..51_200]) > 0.3);
        assert_eq!(sampler.active_voices(), 1);
    }

    #[test]
    fn an_unlooped_note_ends_with_its_sample() {
        let mut sampler = sampler_with(sine(440.0, 0.1));
        let out = render(&mut sampler, &[note(60, 1.0)], 20);
        assert_eq!(peak(&out[4_900..]), 0.0);
        assert_eq!(sampler.active_voices(), 0);
    }

    #[test]
    fn a_pitch_bend_of_twelve_semitones_doubles_the_frequency() {
        let mut sampler = sampler_with(sine(440.0, 1.0));
        let events = [
            note(60, 1.0),
            NoteEvent::PitchBend {
                frame: 0,
                semitones: 12.0,
            },
        ];
        let out = render(&mut sampler, &events, 30);
        let measured = frequency(&out[2_400..12_000]);
        assert!((measured - 880.0).abs() < 2.0, "{measured} Hz");
    }

    #[test]
    fn a_sample_at_another_rate_still_plays_at_its_pitch() {
        // 440 Hz recorded at 24 kHz is still 440 Hz when the output runs at 48 kHz.
        let frames = 24_000;
        let data: Vec<f32> = (0..frames)
            .map(|n| (std::f64::consts::TAU * 440.0 * n as f64 / 24_000.0).sin() as f32 * 0.5)
            .collect();
        let mut sampler = sampler_with(Arc::new(
            AudioBuffer::from_planar(vec![data], 24_000.0).unwrap(),
        ));
        let out = render(&mut sampler, &[note(60, 1.0)], 40);
        let measured = frequency(&out[4_800..19_200]);
        assert!((measured - 440.0).abs() < 1.0, "{measured} Hz");
    }

    #[test]
    fn with_no_sample_it_is_silent_and_holds_no_voices() {
        let mut sampler = AudioSampler::new(SampleBank::shared());
        sampler.prepare(&PrepareContext::new(RATE, 512, 2));
        let out = render(&mut sampler, &[note(60, 1.0)], 2);
        assert_eq!(peak(&out), 0.0);
        assert_eq!(sampler.active_voices(), 0);
        assert!(!sampler.has_sample());
    }

    #[test]
    fn the_sample_choice_survives_a_save_and_load() {
        let mut sampler = sampler_with(sine(440.0, 0.1));
        sampler.set_param_by_key(ROOT_KEY, 48.0);
        let state = sampler.save_state();
        assert_eq!(stored_sample(&state), Some(SourceId(7)));
        let mut copy = AudioSampler::new(SampleBank::shared());
        copy.load_state(&state);
        assert_eq!(copy.source(), Some(SourceId(7)));
        assert_eq!(copy.param_by_key(ROOT_KEY), Some(48.0));
    }

    #[test]
    fn more_notes_than_voices_steal_rather_than_grow() {
        let mut sampler = sampler_with(sine(440.0, 1.0));
        let events: Vec<NoteEvent> = (0..(VOICES as u8 + 8))
            .map(|pitch| note(30 + pitch, 1.0))
            .collect();
        render(&mut sampler, &events, 1);
        assert_eq!(sampler.active_voices(), VOICES);
    }

    #[test]
    fn processing_never_allocates() {
        let mut sampler = sampler_with(sine(440.0, 0.5));
        sampler.set_param_by_key(LOOP_KEY, 1.0);
        let mut out = AudioBuffer::new(2, 512, RATE);
        let ctx = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        let events: Vec<NoteEvent> = (0..40).map(|pitch| note(40 + pitch, 0.8)).collect();
        let allocations = count_allocations(|| {
            sampler.process(&events, &mut out, &ctx);
            sampler.set_param_by_key("release", 0.5);
            sampler.process(&[NoteEvent::AllNotesOff { frame: 3 }], &mut out, &ctx);
            sampler.reset();
        });
        assert_eq!(allocations, 0);
    }

    #[test]
    fn only_the_samplers_need_a_file() {
        assert!(needs_asset(AUDIO_SAMPLER_ID));
        assert!(needs_asset(crate::SAMPLER_ID));
        assert!(!needs_asset("auris.synth.chiptune"));
    }
}
