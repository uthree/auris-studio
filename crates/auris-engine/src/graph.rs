//! The flattened, render-ready form of a [`Project`].
//!
//! A [`RenderGraph`] is built on the UI thread — where allocating, instantiating plugins and
//! walking the tempo map are all free — and then *moved* onto the audio thread. Nothing in it is
//! shared: the audio thread owns the graph outright while it renders, and the previous graph is
//! handed back so its destructor runs on the UI thread.
//!
//! Building resolves the three indirections the document model deliberately keeps:
//!
//! * plugin ids become live [`Instrument`] and [`Effect`] objects from the registry,
//! * musical tick positions become absolute timeline sample positions through the [`TempoMap`],
//! * [`SourceId`](auris_core::project::SourceId)s become `Arc<AudioBuffer>` handles from the
//!   [`AudioSourceBank`].
//!
//! Everything a block needs is pre-sized here, including the per-block event scratch, so
//! rendering never touches the allocator.

use std::sync::Arc;

use auris_core::automation::AutomationLane;
use auris_core::param::{ParamDescriptor, ParamTarget, db_to_gain, pan_gains};
use auris_core::plugin::{
    Effect, Instrument, NoteEvent, Parameterized, PluginCategory, PluginDescriptor, PrepareContext,
    ProcessContext,
};
use auris_core::project::{
    AudioClip, AudioSourceBank, EffectSlotId, MidiClip, MixerStrip, Project, TrackId, TrackKind,
};
use auris_core::registry::PluginRegistry;
use auris_core::time::{Samples, TempoMap, Ticks};
use auris_core::{AudioBuffer, ParamId};

/// Channel count of the internal mix bus.
///
/// The graph always renders stereo because the pan law is a stereo law; the renderer maps the
/// bus onto whatever channel count the output buffer has.
pub const RENDER_CHANNELS: usize = 2;

/// Rate used when neither the caller nor the project offers a usable one.
const DEFAULT_SAMPLE_RATE: f64 = 48_000.0;

/// Room left in each track's per-block event buffer for notes played from the UI.
const AUDITION_HEADROOM: usize = 16;

/// Room the note-chase needs on top of whatever it is re-issuing: the all-notes-off that clears
/// the old position before the notes belonging to the new one go in.
pub(crate) const CHASE_HEADROOM: usize = 1;

/// Number of MIDI pitches, and therefore the size of the chase table.
pub(crate) const PITCH_COUNT: usize = 128;

/// Number of graphs the audio thread can hold before it stops accepting new ones.
pub(crate) const RETIRED_GRAPH_SLOTS: usize = 8;

/// [`pan_gains`] returns `1/sqrt(2)` on both channels at centre, which would cost 3 dB every
/// time a signal passes a strip. Scaling by `sqrt(2)` puts a centred strip at exactly unity
/// while keeping `left^2 + right^2` constant across the sweep, so the law is still constant
/// power — it is just anchored at the centre instead of at the extremes.
const PAN_CENTRE_NORMALISE: f32 = std::f32::consts::SQRT_2;

/// A note event pinned to an absolute position on the timeline.
///
/// The `frame` inside `event` is meaningless here; the renderer rewrites it to a block-relative
/// offset with [`NoteEvent::with_frame`] as it hands the event to the instrument.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ScheduledEvent {
    /// Absolute timeline position, in frames.
    pub frame: u64,
    /// What happens at that position.
    pub event: NoteEvent,
}

/// One audio clip, resolved against the sample bank and the tempo map.
#[derive(Clone, Debug)]
pub struct RenderAudioClip {
    /// Decoded samples, shared with the bank rather than copied.
    pub buffer: Arc<AudioBuffer>,
    /// Where the clip starts on the timeline, in frames.
    pub start_frame: u64,
    /// First frame of `buffer` the clip plays.
    pub source_offset: u64,
    /// How many frames the clip plays. Always within the source's bounds.
    pub length: u64,
    /// Clip trim as a linear multiplier.
    pub gain: f32,
    /// Fade-in length in frames.
    pub fade_in: u64,
    /// Fade-out length in frames.
    pub fade_out: u64,
}

impl RenderAudioClip {
    /// Fade multiplier `position` frames into the clip.
    ///
    /// Mirrors [`AudioClip::fade_gain_at`] so what the arrangement draws is what plays.
    pub fn fade_gain(&self, position: u64) -> f32 {
        let mut gain = 1.0f32;
        if self.fade_in > 0 && position < self.fade_in {
            gain *= position as f32 / self.fade_in as f32;
        }
        if self.fade_out > 0 {
            let fade_start = self.length.saturating_sub(self.fade_out);
            if position >= fade_start {
                let into_fade = position - fade_start;
                gain *= 1.0 - (into_fade as f32 / self.fade_out as f32).min(1.0);
            }
        }
        gain
    }
}

/// Where a track's audio comes from.
pub enum RenderSource {
    /// A software instrument driven by a pre-flattened, frame-sorted event list.
    Instrument {
        /// The live plugin instance.
        instrument: Box<dyn Instrument>,
        /// Every note on the track, in absolute timeline frames, sorted by frame.
        events: Vec<ScheduledEvent>,
    },
    /// Audio clips read straight out of the sample bank.
    Audio {
        /// Clips in timeline order.
        clips: Vec<RenderAudioClip>,
    },
    /// Whatever the rest of the graph has routed here.
    ///
    /// A bus plays nothing of its own; its material is the sum sitting in the graph's bus input
    /// buffer at this slot, which every track feeding it has already written into by the time the
    /// routing order reaches the bus.
    Bus {
        /// Which of the graph's bus input buffers this reads.
        input: usize,
    },
    /// Nothing to play.
    ///
    /// Used when a track's instrument id is missing from the registry: the track keeps its slot
    /// so that command indices still line up with the project's track order, but it is silent.
    Silence,
}

/// A gain that ramps to its target across one block instead of jumping.
///
/// A jump between blocks is audible as a click, so `advance` hands the renderer the
/// start and end of a linear ramp. When nothing has changed the two are equal and the ramp
/// degenerates into a plain multiply, which is what keeps rendering independent of block size.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SmoothedGain {
    current: f32,
    target: f32,
}

impl SmoothedGain {
    /// A gain already settled at `gain`.
    pub fn new(gain: f32) -> Self {
        let gain = sane_gain(gain);
        Self {
            current: gain,
            target: gain,
        }
    }

    /// The value the next block ramps towards.
    pub fn target(&self) -> f32 {
        self.target
    }

    /// The value the next block ramps from.
    pub fn current(&self) -> f32 {
        self.current
    }

    /// Aims at a new gain, reached by the end of the next block.
    pub fn set_target(&mut self, gain: f32) {
        self.target = sane_gain(gain);
    }

    /// Jumps to `gain` with no ramp at all.
    pub fn jump_to(&mut self, gain: f32) {
        let gain = sane_gain(gain);
        self.current = gain;
        self.target = gain;
    }

    /// Consumes one block, returning the `(start, end)` of its ramp.
    pub(crate) fn advance(&mut self) -> (f32, f32) {
        let start = self.current;
        self.current = self.target;
        (start, self.target)
    }
}

fn sane_gain(gain: f32) -> f32 {
    if gain.is_finite() { gain } else { 0.0 }
}

/// A pan a mix can survive: `clamp` passes NaN through, and a NaN pan does not silence one
/// track the way a NaN gain does — its gains poison the track's scratch, `mix_from` spreads
/// them across the master, and the sanitiser zeroes the whole mix. Centre is the only answer
/// that costs nothing.
fn sane_pan(pan: f32) -> f32 {
    if pan.is_finite() {
        pan.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

/// How long a mute takes to close or open, in milliseconds.
///
/// Short enough that a mute still feels instant under the finger, long enough that the step it
/// replaces is inaudible. At 48 kHz it is 240 frames — under half a typical block, so in practice
/// a mute is silent by the end of the block it was pressed in.
pub(crate) const MUTE_FADE_MS: f64 = 5.0;

/// The mute switch as a gain sliding between silence and unity.
///
/// A mute that took effect within one sample would step the waveform to zero, and a step is a
/// click. Sliding over a few milliseconds instead costs nothing audible and removes it.
///
/// The slide is counted in *frames*, not in blocks, which is what keeps it — like every other
/// ramp in the renderer — independent of the block size: splitting a block in two and rendering
/// the halves separately lands on the same samples, because a linear ramp cut anywhere is still
/// the same linear ramp.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MuteFade {
    /// Frames the slide takes end to end.
    length: usize,
    /// How far up the slide the gain currently sits: `0` is silence, `length` is unity.
    ///
    /// Counted in frames rather than held as a gain because the slide advances by exactly one
    /// frame per frame. That is what makes it block-size independent by construction: there is
    /// no accumulated float position to drift depending on where the blocks happen to fall.
    position: usize,
    open: bool,
}

/// How far the fade slid across one block, and over how many frames of it.
///
/// `moving` is usually the whole block, but a fade that reaches its end partway through reports
/// only the frames it actually covered; the rest of the block sits at the settled value.
struct FadeStep {
    /// Position the fade sat at on the block's first frame.
    first: usize,
    /// Whether the position climbs or falls across the block.
    opening: bool,
    /// Frames the fade slides over before settling.
    moving: usize,
}

impl MuteFade {
    /// A fade already settled: at unity when `open`, at silence otherwise.
    ///
    /// Settled rather than mid-slide, so a project that opens with a track already muted starts
    /// silent instead of fading in over its first block.
    fn new(open: bool, sample_rate: f64) -> Self {
        let length = (sample_rate * MUTE_FADE_MS / 1_000.0).round().max(1.0) as usize;
        Self {
            length,
            position: if open { length } else { 0 },
            open,
        }
    }

    /// Aims the fade at unity or at silence. Nothing moves until [`Self::advance`] runs.
    fn set_open(&mut self, open: bool) {
        self.open = open;
    }

    /// `true` once the fade has reached silence, so the strip can be skipped outright rather
    /// than rendered into samples that are about to be multiplied by zero.
    fn is_closed(&self) -> bool {
        !self.open && self.position == 0
    }

    /// `true` once the fade has reached unity, so the pass over the buffer can be skipped.
    fn is_open(&self) -> bool {
        self.open && self.position == self.length
    }

    /// Gain of the frame `offset` frames into a slide that began at `first`.
    ///
    /// Derived from the position rather than accumulated frame by frame, so the value for a given
    /// frame is the same however the block containing it was cut up. An accumulating `gain +=
    /// step` drifts by a rounding error per frame and the drift depends on where the block
    /// boundaries fell, which is exactly the dependence this is here to avoid.
    fn gain_at(&self, first: usize, opening: bool, offset: usize) -> f32 {
        let position = if opening {
            first + offset
        } else {
            first - offset
        };
        position as f32 / self.length as f32
    }

    /// Consumes up to `frames` frames of the slide.
    fn advance(&mut self, frames: usize) -> FadeStep {
        let first = self.position;
        let remaining = if self.open {
            self.length - self.position
        } else {
            self.position
        };
        let moving = remaining.min(frames);
        self.position = if self.open {
            self.position + moving
        } else {
            self.position - moving
        };
        FadeStep {
            first,
            opening: self.open,
            moving,
        }
    }
}

/// A fixed whole-frame delay, used to line the tracks up with each other.
///
/// An effect that looks ahead — the limiter is the one that ships — can only do so by handing
/// back audio it has already held on to, so its output arrives late by however many frames it
/// declares in [`Effect::latency_frames`]. That is not a fault to be removed; it is what makes
/// the effect possible. What has to be removed is the *difference*: a track carrying such an
/// effect would otherwise play behind the tracks that do not, and a mix where one part drags is
/// wrong in a way no fader can fix.
///
/// So every track is delayed up to the longest chain in the graph. The whole mix then sits that
/// far behind the playhead, which for playback is latency the engine cannot avoid and for an
/// export is a lead-in that [`render_project`](crate::render_project) drops.
///
/// The buffer is a ring per channel, sized once here; [`Self::process`] only swaps samples in and
/// out of it, so it allocates nothing and is safe on the audio thread.
pub(crate) struct LatencyDelay {
    lines: Vec<Vec<f32>>,
    frames: usize,
    write: usize,
}

impl LatencyDelay {
    /// A delay of `frames` frames on `channels` channels. Zero frames is free and does nothing.
    fn new(frames: usize, channels: usize) -> Self {
        Self {
            lines: if frames == 0 {
                Vec::new()
            } else {
                vec![vec![0.0; frames]; channels]
            },
            frames,
            write: 0,
        }
    }

    /// Frames this delay holds back.
    pub(crate) fn frames(&self) -> usize {
        self.frames
    }

    /// Delays `buffer` in place.
    ///
    /// Every channel starts from the same ring position, so they stay in phase with each other;
    /// the shared write cursor is only moved on once they have all been walked.
    pub(crate) fn process(&mut self, buffer: &mut AudioBuffer) {
        if self.frames == 0 {
            return;
        }
        let start = self.write;
        let mut finished = start;
        for (samples, line) in buffer.channels_mut().iter_mut().zip(&mut self.lines) {
            let mut position = start;
            for sample in samples.iter_mut() {
                std::mem::swap(sample, &mut line[position]);
                position += 1;
                if position == self.frames {
                    position = 0;
                }
            }
            finished = position;
        }
        self.write = finished;
    }

    /// Empties the delay, so nothing held from before a panic comes back afterwards.
    fn reset(&mut self) {
        for line in &mut self.lines {
            line.fill(0.0);
        }
        self.write = 0;
    }
}

/// Registry id reported by the stand-in for an effect that could not be created.
pub(crate) const MISSING_EFFECT_ID: &str = "auris.engine.missing";

/// Placeholder for an effect whose plugin id the registry does not know.
///
/// The runtime chain has to stay index-parallel with the project's
/// [`MixerStrip::effects`](auris_core::project::MixerStrip::effects), because
/// [`EngineCommand::SetEffectParam`](crate::EngineCommand::SetEffectParam) addresses effects by
/// their position *there*. Dropping the entry would silently re-aim every later command one slot
/// low and push the last one off the end, so the slot is kept and left bypassed instead — the
/// same contract the instrument path keeps with [`RenderSource::Silence`].
struct MissingEffect;

impl Parameterized for MissingEffect {
    fn parameters(&self) -> &[ParamDescriptor] {
        &[]
    }

    fn param(&self, _id: ParamId) -> f32 {
        0.0
    }

    fn set_param(&mut self, _id: ParamId, _value: f32) {}
}

impl Effect for MissingEffect {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            MISSING_EFFECT_ID,
            "Missing Effect",
            "Placeholder keeping the slot of an effect the registry does not know",
            PluginCategory::Utility,
        )
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, _buffer: &mut AudioBuffer, _ctx: &ProcessContext) {}
}

/// Volume, pan and effects for a track or the master bus.
pub struct RenderStrip {
    pub(crate) gain: SmoothedGain,
    pub(crate) pan: f32,
    pan_current: f32,
    pub(crate) mute: bool,
    pub(crate) audible: bool,
    /// Mute and solo as a gain, so switching either one slides instead of stepping.
    pub(crate) fade: MuteFade,
    pub(crate) effects: Vec<Box<dyn Effect>>,
    /// Bypass flags, parallel to `effects`, so toggling a bypass keeps slot indices stable.
    pub(crate) enabled: Vec<bool>,
}

impl RenderStrip {
    /// A strip with no effects.
    ///
    /// `sample_rate` sizes the mute fade, which is measured in milliseconds and therefore has to
    /// know how many frames one of those is.
    pub fn new(gain_db: f32, pan: f32, mute: bool, audible: bool, sample_rate: f64) -> Self {
        let pan = sane_pan(pan);
        Self {
            gain: SmoothedGain::new(db_to_gain(gain_db)),
            pan,
            pan_current: pan,
            mute,
            audible,
            fade: MuteFade::new(audible && !mute, sample_rate),
            effects: Vec::new(),
            enabled: Vec::new(),
        }
    }

    /// Instantiates a mixer strip's effect chain from the registry.
    ///
    /// `audible` carries the project-wide solo resolution; the strip's own mute stays separate
    /// so that toggling it later needs a command rather than a rebuild.
    ///
    /// The resulting chain is always as long as `mixer.effects`: an id the registry does not know
    /// becomes a bypassed placeholder rather than a hole, so slot indices keep their meaning.
    pub fn from_mixer(
        mixer: &MixerStrip,
        audible: bool,
        registry: &PluginRegistry,
        prepare: &PrepareContext,
    ) -> Self {
        let mut strip = Self::new(
            mixer.gain_db,
            mixer.pan,
            mixer.mute,
            audible,
            prepare.sample_rate,
        );
        strip.effects.reserve(mixer.effects.len());
        strip.enabled.reserve(mixer.effects.len());
        for slot in &mixer.effects {
            match registry.create_effect(&slot.effect_id) {
                Ok(mut effect) => {
                    effect.load_state(&slot.state);
                    effect.prepare(prepare);
                    strip.effects.push(effect);
                    strip.enabled.push(slot.enabled);
                }
                Err(error) => {
                    // A missing plugin must not stop a project from opening, and it must not
                    // shift the slots after it either: command indices are positions in the
                    // project's chain, so the slot is kept and bypassed.
                    log::warn!(
                        "effect `{}` keeps its slot but stays bypassed: {error}",
                        slot.effect_id
                    );
                    strip.effects.push(Box::new(MissingEffect));
                    strip.enabled.push(false);
                }
            }
        }
        strip
    }

    /// `true` when this strip should contribute to the mix.
    ///
    /// This is the switch position, not what is coming out: a strip muted a moment ago is already
    /// inactive while its fade is still sliding down. Use [`Self::is_silent`] to decide whether
    /// there is anything left to render.
    pub fn is_active(&self) -> bool {
        self.audible && !self.mute
    }

    /// `true` when the strip is muted *and* has finished fading out.
    ///
    /// Until then a muted strip still has to be rendered, because the fade needs samples to slide
    /// down; afterwards it produces nothing but zeroes and can be skipped outright.
    pub fn is_silent(&self) -> bool {
        self.fade.is_closed()
    }

    /// Current fader position as a linear gain.
    pub fn gain(&self) -> f32 {
        self.gain.target()
    }

    /// Moves the fader; the change is ramped across the next block.
    pub fn set_gain_db(&mut self, gain_db: f32) {
        self.gain.set_target(db_to_gain(gain_db));
    }

    /// Current stereo position.
    pub fn pan(&self) -> f32 {
        self.pan
    }

    /// Moves the pan control; the change is ramped across the next block.
    pub fn set_pan(&mut self, pan: f32) {
        self.pan = sane_pan(pan);
    }

    /// Puts the fader at `gain_db` with no ramp at all.
    ///
    /// For arriving somewhere rather than moving: the playhead has jumped, and the value the lane
    /// holds *there* is the value that stretch of the song sounds like. Ramping to it would slide
    /// up from wherever the fader happened to be left, which is a swell nobody wrote.
    pub fn jump_gain_db(&mut self, gain_db: f32) {
        self.gain.jump_to(db_to_gain(gain_db));
    }

    /// Puts the pan control at `pan` with no ramp, for the same reason.
    pub fn jump_pan(&mut self, pan: f32) {
        self.pan = sane_pan(pan);
        self.pan_current = self.pan;
    }

    /// Sets the strip's own mute switch. The change is faded in or out, not stepped.
    pub fn set_mute(&mut self, mute: bool) {
        self.mute = mute;
        self.fade.set_open(self.is_active());
    }

    /// Sets the solo-resolved audibility flag. Faded like a mute, for the same reason.
    pub fn set_audible(&mut self, audible: bool) {
        self.audible = audible;
        self.fade.set_open(self.is_active());
    }

    /// Number of slots in the chain, which always matches the project's chain length: a plugin
    /// the registry could not create keeps a bypassed placeholder so slot indices stay stable.
    pub fn effect_count(&self) -> usize {
        self.effects.len()
    }

    /// How long the chain keeps producing sound after its input stops, in frames.
    ///
    /// The slots are in series, so their tails add up rather than overlap: a delay ringing for
    /// two seconds into a reverb that rings for three keeps the chain sounding for five, because
    /// the reverb is still being fed for the whole of the delay's decay.
    ///
    /// A bypassed slot never has `process` called on it, so it cannot ring out and must not
    /// lengthen an export. That is also what makes the placeholder for a missing plugin free:
    /// it is always bypassed, so it can never claim a tail it could not produce.
    ///
    /// Saturating rather than wrapping, because the figures come from plugins: one that reports
    /// a nonsense tail should pad an export, not overflow the length it is added to.
    pub fn tail_frames(&self) -> usize {
        self.effects
            .iter()
            .zip(&self.enabled)
            .filter(|(_, enabled)| **enabled)
            .map(|(effect, _)| effect.tail_frames())
            .fold(0usize, usize::saturating_add)
    }

    /// How late this chain's output is, in frames.
    ///
    /// Summed rather than maximised for the same reason as [`Self::tail_frames`]: the slots run
    /// in series, so each one holds the audio back on top of whatever the one before it did. A
    /// bypassed slot never sees a sample and so delays nothing.
    pub fn latency_frames(&self) -> usize {
        self.effects
            .iter()
            .zip(&self.enabled)
            .filter(|(_, enabled)| **enabled)
            .map(|(effect, _)| effect.latency_frames())
            .fold(0usize, usize::saturating_add)
    }

    /// Writes a parameter on one effect. Out-of-range slots are ignored.
    pub fn set_effect_param(&mut self, slot: usize, param: ParamId, value: f32) {
        if let Some(effect) = self.effects.get_mut(slot) {
            effect.set_param(param, value);
        }
    }

    /// Clears every effect's delay lines and filter memory.
    pub fn reset(&mut self) {
        for effect in &mut self.effects {
            effect.reset();
        }
    }

    /// Applies the fader and the pan law to a stereo block, ramping both across it.
    pub(crate) fn apply_gain_and_pan(&mut self, buffer: &mut AudioBuffer) {
        let (gain_from, gain_to) = self.gain.advance();
        let pan_from = self.pan_current;
        let pan_to = self.pan;
        self.pan_current = pan_to;

        let (left_from, right_from) = pan_gains(pan_from);
        let (left_to, right_to) = pan_gains(pan_to);
        let channels = buffer.channels_mut();
        match channels {
            [] => {}
            [mono] => ramp(mono, gain_from, gain_to),
            [left, right, rest @ ..] => {
                ramp(
                    left,
                    gain_from * left_from * PAN_CENTRE_NORMALISE,
                    gain_to * left_to * PAN_CENTRE_NORMALISE,
                );
                ramp(
                    right,
                    gain_from * right_from * PAN_CENTRE_NORMALISE,
                    gain_to * right_to * PAN_CENTRE_NORMALISE,
                );
                for extra in rest {
                    ramp(extra, gain_from, gain_to);
                }
            }
        }
    }

    /// Applies the mute fade, the last stage of the strip.
    ///
    /// Kept out of [`Self::apply_gain_and_pan`] rather than folded into its ramp because the two
    /// are independent ramps: multiplying them together within a block would be a curve, not a
    /// line, and the fade has to stay a straight line for splitting a block to be free. It costs
    /// a pass over the buffer only while a mute is actually moving — a strip settled open returns
    /// immediately, which is every strip almost all of the time.
    pub(crate) fn apply_mute(&mut self, buffer: &mut AudioBuffer) {
        if self.fade.is_open() {
            return;
        }
        if self.fade.is_closed() {
            buffer.clear();
            return;
        }
        let step = self.fade.advance(buffer.frame_count());
        // A fade that reached its end partway through the block leaves a settled remainder: held
        // at silence when it closed, and left untouched when it opened, because that is unity.
        let fill_rest = self.fade.is_closed();
        for channel in buffer.channels_mut() {
            let split = step.moving.min(channel.len());
            let (sliding, settled) = channel.split_at_mut(split);
            for (offset, sample) in sliding.iter_mut().enumerate() {
                *sample *= self.fade.gain_at(step.first, step.opening, offset);
            }
            if fill_rest {
                settled.fill(0.0);
            }
        }
    }
}

/// Multiplies `samples` by a gain sweeping linearly from `start` to `end`.
fn ramp(samples: &mut [f32], start: f32, end: f32) {
    if samples.is_empty() {
        return;
    }
    if (start - end).abs() <= f32::EPSILON {
        for sample in samples.iter_mut() {
            *sample *= start;
        }
        return;
    }
    let step = (end - start) / samples.len() as f32;
    let mut gain = start;
    for sample in samples.iter_mut() {
        *sample *= gain;
        gain += step;
    }
}

/// A copy of a track's signal, on its way to a bus.
///
/// The gain is smoothed for the same reason a fader is: a send level moved during playback would
/// otherwise step, and a step is a click.
pub(crate) struct RenderSend {
    /// Which of the graph's bus input buffers this feeds.
    pub(crate) target: usize,
    /// Send level as a linear gain, ramped across the block it changes in.
    pub(crate) gain: SmoothedGain,
    /// Whether the copy is taken before the fader rather than after it.
    pub(crate) pre_fader: bool,
    /// Holds this copy back so it reaches the bus in step with everything else feeding it.
    pub(crate) delay: LatencyDelay,
    /// Somewhere to hold the delayed copy. Empty, and never touched, when the delay is zero —
    /// which it is for every send in a graph where nothing looks ahead.
    pub(crate) scratch: AudioBuffer,
}

/// One track, ready to render.
pub struct RenderTrack {
    /// Project id of the track this came from.
    pub id: TrackId,
    /// Track name, kept for logging and for the meter tooltips.
    pub name: String,
    pub(crate) source: RenderSource,
    pub(crate) strip: RenderStrip,
    /// Which bus input this track's output is mixed into; `None` for the master.
    pub(crate) output: Option<usize>,
    /// Copies of this track's signal, on their way to buses.
    pub(crate) sends: Vec<RenderSend>,
    /// Holds this track back to the longest path through the graph, so the sources line up.
    pub(crate) delay: LatencyDelay,
    /// Holds this track's *output* back so it reaches its destination in step with everything
    /// else arriving there.
    ///
    /// Distinct from [`Self::delay`], and applied after the fader rather than before it, because
    /// every one of a track's outgoing edges needs a delay of its own: a track feeding the master
    /// dry while sending to a bus that looks ahead has two paths of different lengths, and only a
    /// delay per edge can make both arrive together. Zero unless a send makes it otherwise, which
    /// is why the fader still acts immediately in every ordinary graph.
    pub(crate) output_delay: LatencyDelay,
    pub(crate) scratch: AudioBuffer,
    /// Events handed to the instrument for the current block, block-relative and sorted.
    pub(crate) block_events: Vec<NoteEvent>,
    /// Notes triggered from the UI, consumed at the start of the next block.
    pub(crate) audition: Vec<NoteEvent>,
    /// Frame the previous rendered block ended on. A mismatch with the next block's start means
    /// the playhead jumped — a seek, a loop wrap or a fresh start — and the notes that should be
    /// sounding there have to be chased.
    pub(crate) continued_from: Option<u64>,
    /// How many times each pitch is on at the chase position.
    pub(crate) chase_counts: [u8; PITCH_COUNT],
    /// Velocity each pitch was last struck with, so a chased note comes back at its own level.
    pub(crate) chase_velocity: [f32; PITCH_COUNT],
    /// Post-fader peak of the last block, published to the meters by the engine.
    pub(crate) peak: f32,
}

impl RenderTrack {
    /// The track's mixer strip.
    pub fn strip(&self) -> &RenderStrip {
        &self.strip
    }

    /// Frames this track is held back by to match the longest chain in the graph.
    pub fn compensation_frames(&self) -> usize {
        self.delay.frames()
    }

    /// The track's mixer strip, mutably.
    pub fn strip_mut(&mut self) -> &mut RenderStrip {
        &mut self.strip
    }

    /// What the track plays.
    pub fn source(&self) -> &RenderSource {
        &self.source
    }

    /// Post-fader peak of the most recently rendered block.
    pub fn peak(&self) -> f32 {
        self.peak
    }

    /// Number of scheduled note events, or 0 for a track with no instrument.
    pub fn event_count(&self) -> usize {
        match &self.source {
            RenderSource::Instrument { events, .. } => events.len(),
            _ => 0,
        }
    }

    /// Queues a note to sound at the start of the next block, for piano-roll auditioning.
    ///
    /// Dropped when the queue is full, because growing it would allocate on the audio thread.
    pub fn note_on(&mut self, pitch: u8, velocity: f32) {
        self.push_audition(NoteEvent::NoteOn {
            frame: 0,
            pitch,
            velocity: velocity.clamp(0.0, 1.0),
        });
    }

    /// Queues a note release for the start of the next block.
    pub fn note_off(&mut self, pitch: u8) {
        self.push_audition(NoteEvent::NoteOff { frame: 0, pitch });
    }

    fn push_audition(&mut self, event: NoteEvent) {
        if self.audition.len() < self.audition.capacity() {
            self.audition.push(event);
        }
    }

    /// Writes a parameter on the track's instrument.
    pub fn set_instrument_param(&mut self, param: ParamId, value: f32) {
        if let RenderSource::Instrument { instrument, .. } = &mut self.source {
            instrument.set_param(param, value);
        }
    }

    /// Silences the instrument and clears the pending audition queue.
    ///
    /// Playback continuity is left alone, so nothing is chased back in: this is what a panic
    /// wants, where the point is that everything shuts up until the next note begins.
    pub fn silence_voices(&mut self) {
        self.audition.clear();
        if let RenderSource::Instrument { instrument, .. } = &mut self.source {
            instrument.reset();
        }
    }

    /// Silences the instrument and marks the next block as a jump.
    ///
    /// This is what a stop or a seek wants: any note spanning the new position is chased back in
    /// rather than staying silent until its successor.
    pub fn reset_voices(&mut self) {
        self.silence_voices();
        self.continued_from = None;
    }
}

/// Every track plus the master bus, sized and prepared for one particular block size.
pub struct RenderGraph {
    pub(crate) tracks: Vec<RenderTrack>,
    pub(crate) master: RenderStrip,
    sample_rate: f64,
    max_block: usize,
    /// Where every track's signal is summed on the way to the master, one buffer per bus.
    ///
    /// Cleared at the start of every segment and filled as the routing order is walked, so a bus
    /// finds its whole input waiting by the time its own turn comes.
    pub(crate) bus_inputs: Vec<AudioBuffer>,
    /// Which track each bus input belongs to, so the latency plan can be written in track indices
    /// while the render loop addresses buffers directly.
    bus_tracks: Vec<usize>,
    /// Track indices ordered so that everything feeding a bus comes before it.
    pub(crate) order: Vec<usize>,
    /// Total latency the delay lines were laid out for: playhead to output, in frames.
    latency: usize,
    /// Every strip's own latency as it was when the delays were laid out — each track's, then the
    /// master's last.
    ///
    /// Kept rather than recomputed so it can be compared with what the strips report now: a
    /// parameter that moves a plugin's latency after the graph was built leaves the two
    /// disagreeing, and that disagreement is the signal to rebuild. Compared entry by entry rather
    /// than as a total, because two plugins can trade latency between them and leave the total
    /// alone while the tracks fall out of step with each other.
    built_latencies: Vec<usize>,
    tempo_map: TempoMap,
    /// The document's automation, resolved to positions in this graph.
    ///
    /// Empty for a project nobody has automated, which is what makes the whole feature free when
    /// it is not in use: the renderer's per-segment call returns on the first line.
    automation: Vec<RenderAutomation>,
    /// Frame the next segment has to begin on for the automation to be *continuing* rather than
    /// arriving.
    ///
    /// The same idea as [`RenderTrack::continued_from`] and for the same reason: a playhead that
    /// jumped is not a parameter that moved. A seek into the middle of a fade should sound like
    /// that part of the fade at once, and ramping there from wherever the fader was left is a
    /// swell nobody wrote. `None` on a fresh graph, so the first segment always arrives.
    automation_from: Option<u64>,
    pub(crate) master_scratch: AudioBuffer,
    pub(crate) master_peak: [f32; 2],
    /// Where a spectrum display, if one is open, gets its samples.
    ///
    /// Shared with the UI rather than owned by it, because only the render path ever sees a
    /// strip's signal — the document holds parameter values, not audio.
    pub(crate) scope: Arc<crate::scope::Scope>,
}

impl RenderGraph {
    /// Builds a render graph for `project` at the project's own sample rate.
    ///
    /// Never fails: a plugin id the registry does not know is logged and replaced by a silent
    /// stand-in, so a project still opens when a plugin has been removed — and every track and
    /// effect slot keeps its position, which is what command indices are addressed by.
    pub fn build(
        project: &Project,
        bank: &AudioSourceBank,
        registry: &PluginRegistry,
        max_block: usize,
    ) -> RenderGraph {
        Self::build_at(project, bank, registry, max_block, project.sample_rate)
    }

    /// Builds a render graph at an explicit sample rate.
    ///
    /// The audio device decides the rate the engine actually runs at, which is not necessarily
    /// the rate stored in the project, so the caller passes it in.
    pub fn build_at(
        project: &Project,
        bank: &AudioSourceBank,
        registry: &PluginRegistry,
        max_block: usize,
        sample_rate: f64,
    ) -> RenderGraph {
        let max_block = max_block.max(1);
        // A zero, negative or NaN rate would make every derived position meaningless and would
        // poison the meters, so fall back to the project's rate and then to a sane default: a
        // project deserialised from a corrupt file can carry a nonsense rate too.
        let sample_rate = [sample_rate, project.sample_rate, DEFAULT_SAMPLE_RATE]
            .into_iter()
            .find(|rate| rate.is_finite() && *rate > 0.0)
            .unwrap_or(DEFAULT_SAMPLE_RATE);
        let prepare = PrepareContext::new(sample_rate, max_block, RENDER_CHANNELS);
        // The solo resolution travels along the routing, so it is worked out for the whole project
        // at once rather than per track: soloing a drum track has to leave the drum bus open.
        let solo = project.solo_resolution();
        // One input buffer per bus, and the slot each bus track owns. Resolved before the tracks
        // are built so that an output or a send can name a bus that appears later in the list.
        let bus_tracks: Vec<usize> = (0..project.tracks.len())
            .filter(|index| project.tracks[*index].kind.is_bus())
            .collect();
        let bus_slot = |id: TrackId| {
            let index = project.track_index(id)?;
            bus_tracks.iter().position(|track| *track == index)
        };

        let mut tracks = Vec::with_capacity(project.tracks.len());
        for (index, track) in project.tracks.iter().enumerate() {
            // `audible` carries only the solo resolution; the strip keeps its own mute so a
            // mute toggle is a command rather than a rebuild.
            let audible = solo.get(index).copied().unwrap_or(true);
            let strip = RenderStrip::from_mixer(&track.mixer, audible, registry, &prepare);
            let source = match &track.kind {
                TrackKind::Instrument(instrument_track) => {
                    match registry.create_instrument(&instrument_track.instrument_id) {
                        Ok(mut instrument) => {
                            instrument.load_state(&instrument_track.instrument_state);
                            instrument.prepare(&prepare);
                            let mut events = Vec::new();
                            for clip in &instrument_track.clips {
                                schedule_clip(clip, &project.tempo_map, sample_rate, &mut events);
                            }
                            sort_events(&mut events);
                            RenderSource::Instrument { instrument, events }
                        }
                        Err(error) => {
                            log::warn!(
                                "track `{}` keeps its slot but stays silent: {error}",
                                track.name
                            );
                            RenderSource::Silence
                        }
                    }
                }
                TrackKind::Audio(audio_track) => {
                    let mut clips = Vec::with_capacity(audio_track.clips.len());
                    for clip in &audio_track.clips {
                        // A clip's trim is counted in the frames of the file it came from, which
                        // is not necessarily the rate this graph renders at.
                        let source_rate = project
                            .audio_sources
                            .get(&clip.source)
                            .map_or(project.sample_rate, |source| source.sample_rate);
                        if let Some(resolved) = resolve_audio_clip(
                            clip,
                            bank,
                            &project.tempo_map,
                            sample_rate,
                            source_rate,
                        ) {
                            clips.push(resolved);
                        }
                    }
                    clips.sort_by_key(|clip| clip.start_frame);
                    RenderSource::Audio { clips }
                }
                // A bus that somehow has no slot cannot happen — the slots are made from exactly
                // the tracks that are buses — but silence is the right answer if it ever did.
                TrackKind::Bus => match bus_slot(track.id) {
                    Some(input) => RenderSource::Bus { input },
                    None => RenderSource::Silence,
                },
            };

            // Two independent bounds on the per-block event buffer: how many scheduled events
            // can fall inside one block, and how many notes the chase can re-issue after a jump.
            let (event_headroom, chase_headroom) = match &source {
                RenderSource::Instrument { events, .. } => (
                    max_events_in_window(events, max_block as u64),
                    max_sounding_notes(events) + CHASE_HEADROOM,
                ),
                _ => (0, 0),
            };
            let mut scratch = AudioBuffer::new(RENDER_CHANNELS, max_block, sample_rate);
            scratch.reserve_frames(max_block);

            // A route to a bus that is not there was already repaired when the document was
            // loaded; dropping it to the master here is the second lock on that door, and it is
            // what keeps the render loop free of a check on every block.
            let output = match track.output.bus() {
                Some(id) => match bus_slot(id) {
                    Some(slot) => Some(slot),
                    None => {
                        log::warn!("track `{}` names a bus that is not there", track.name);
                        None
                    }
                },
                None => None,
            };
            let sends = track
                .sends
                .iter()
                .filter_map(|send| {
                    Some(RenderSend {
                        target: bus_slot(send.target)?,
                        gain: SmoothedGain::new(db_to_gain(send.level_db)),
                        pre_fader: send.pre_fader,
                        // Both sized below, once every chain's latency is known.
                        delay: LatencyDelay::new(0, RENDER_CHANNELS),
                        scratch: AudioBuffer::new(RENDER_CHANNELS, 0, sample_rate),
                    })
                })
                .collect();

            tracks.push(RenderTrack {
                id: track.id,
                name: track.name.clone(),
                source,
                strip,
                output,
                sends,
                // Sized below, once every chain's latency is known.
                delay: LatencyDelay::new(0, RENDER_CHANNELS),
                output_delay: LatencyDelay::new(0, RENDER_CHANNELS),
                scratch,
                block_events: Vec::with_capacity(
                    event_headroom + AUDITION_HEADROOM + chase_headroom,
                ),
                audition: Vec::with_capacity(AUDITION_HEADROOM),
                continued_from: None,
                chase_counts: [0; PITCH_COUNT],
                chase_velocity: [0.0; PITCH_COUNT],
                peak: 0.0,
            });
        }

        let master = RenderStrip::from_mixer(&project.master, true, registry, &prepare);
        let mut master_scratch = AudioBuffer::new(RENDER_CHANNELS, max_block, sample_rate);
        master_scratch.reserve_frames(max_block);

        // Plugin delay compensation, laid out over the whole routing rather than over one row of
        // tracks: the sources run in parallel into a graph of buses, and they can only stay in
        // step if each is held back to the longest path through it. A graph where nothing looks
        // ahead allocates nothing here, because every delay comes out zero.
        let order = project.routing_order();
        let plan = plan_latency(&tracks, &bus_tracks, &order, master.latency_frames());
        for (index, track) in tracks.iter_mut().enumerate() {
            track.delay = LatencyDelay::new(plan.node[index], RENDER_CHANNELS);
            let mut edges = plan.edges[index].iter().copied();
            track.output_delay = LatencyDelay::new(edges.next().unwrap_or(0), RENDER_CHANNELS);
            for send in &mut track.sends {
                let frames = edges.next().unwrap_or(0);
                send.delay = LatencyDelay::new(frames, RENDER_CHANNELS);
                if frames > 0 {
                    // Only a send that is actually held back needs somewhere to be held, which is
                    // why an ordinary send costs one extra buffer of nothing at all.
                    send.scratch = AudioBuffer::new(RENDER_CHANNELS, max_block, sample_rate);
                    send.scratch.reserve_frames(max_block);
                }
            }
        }

        let mut bus_inputs = Vec::with_capacity(bus_tracks.len());
        for _ in &bus_tracks {
            let mut buffer = AudioBuffer::new(RENDER_CHANNELS, max_block, sample_rate);
            buffer.reserve_frames(max_block);
            bus_inputs.push(buffer);
        }

        let built_latencies = tracks
            .iter()
            .map(|track| track.strip.latency_frames())
            .chain(std::iter::once(master.latency_frames()))
            .collect();

        RenderGraph {
            tracks,
            master,
            sample_rate,
            max_block,
            bus_inputs,
            bus_tracks,
            order,
            latency: plan.total,
            built_latencies,
            tempo_map: project.tempo_map.clone(),
            automation: resolve_automation(project),
            automation_from: None,
            master_scratch,
            master_peak: [0.0, 0.0],
            scope: Arc::new(crate::scope::Scope::new()),
        }
    }

    /// Points this graph at the scope the UI is reading.
    ///
    /// Handed in after building rather than created here, so a rebuild — which happens on every
    /// structural edit — does not leave an open spectrum display reading a scope nothing writes
    /// to any more.
    pub fn set_scope(&mut self, scope: Arc<crate::scope::Scope>) {
        self.scope = scope;
    }

    /// Rate this graph was prepared for.
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Largest block the graph's plugins and scratch buffers are sized for.
    ///
    /// The renderer splits anything longer into chunks of at most this many frames.
    pub fn max_block(&self) -> usize {
        self.max_block
    }

    /// Channel count of the internal mix bus.
    pub fn channel_count(&self) -> usize {
        RENDER_CHANNELS
    }

    /// Number of tracks, which always matches the project's track count.
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// The tracks, in project order.
    pub fn tracks(&self) -> &[RenderTrack] {
        &self.tracks
    }

    /// The tracks, mutably.
    pub fn tracks_mut(&mut self) -> &mut [RenderTrack] {
        &mut self.tracks
    }

    /// One track by index.
    pub fn track(&self, index: usize) -> Option<&RenderTrack> {
        self.tracks.get(index)
    }

    /// One track by index, mutably.
    pub fn track_mut(&mut self, index: usize) -> Option<&mut RenderTrack> {
        self.tracks.get_mut(index)
    }

    /// The master bus strip.
    pub fn master(&self) -> &RenderStrip {
        &self.master
    }

    /// The master bus strip, mutably.
    pub fn master_mut(&mut self) -> &mut RenderStrip {
        &mut self.master
    }

    /// The tempo map positions were flattened against.
    pub fn tempo_map(&self) -> &TempoMap {
        &self.tempo_map
    }

    /// Tempo in effect at an absolute frame position.
    pub fn bpm_at_frame(&self, frame: u64) -> f64 {
        let tick = self
            .tempo_map
            .samples_to_ticks(Samples(frame), self.sample_rate);
        self.tempo_map.bpm_at(tick)
    }

    /// How many parameters this graph is driving from a lane.
    pub fn automated_count(&self) -> usize {
        self.automation.len()
    }

    /// Drives every automated parameter to the value its lane holds at `frame`.
    ///
    /// Called once per rendered segment rather than once per sample. That is not a compromise for
    /// the faders — a gain and a pan are both *targets* that the strip ramps across the block it
    /// is given, so a segment-rate write comes out as a continuous slope. It is the honest rate
    /// for a plugin parameter, which has nowhere finer to put one.
    ///
    /// `frames` is how long the segment about to be rendered is, which is how the next call knows
    /// whether the playhead carried on or jumped. Arriving somewhere puts the values there
    /// outright; carrying on ramps to them.
    ///
    /// Allocation-free and lock-free: the lanes were resolved to positions when the graph was
    /// built, and what happens here is a binary search per lane and a store.
    pub(crate) fn apply_automation(&mut self, frame: u64, frames: usize) {
        if self.automation.is_empty() {
            return;
        }
        let continuing = self.automation_from == Some(frame);
        self.automation_from = Some(frame + frames as u64);
        let tick = self
            .tempo_map
            .samples_to_ticks(Samples(frame), self.sample_rate);
        // Destructured rather than walked through `self`, so the lanes can be read while the
        // tracks they point at are written.
        let Self {
            tracks,
            master,
            automation,
            ..
        } = self;
        drive_automation(automation, tracks, master, tick, continuing);
    }

    /// Post-fader peak of a track's last rendered block.
    pub fn track_peak(&self, index: usize) -> f32 {
        self.tracks.get(index).map_or(0.0, |track| track.peak)
    }

    /// Peak of one master channel over the last rendered block.
    pub fn master_channel_peak(&self, channel: usize) -> f32 {
        self.master_peak.get(channel).copied().unwrap_or(0.0)
    }

    /// Loudest master channel over the last rendered block.
    pub fn master_peak(&self) -> f32 {
        self.master_peak[0].max(self.master_peak[1])
    }

    /// How long the graph keeps producing sound after its last event, in frames.
    ///
    /// The longest path through the routing decides it. Tracks run in parallel, so the longest of
    /// them wins; a chain runs in series with whatever it feeds, so a track's tail, its bus's tail
    /// and the master's tail add up rather than overlapping — the bus is still being fed for the
    /// whole of the track's decay, and the master for the whole of the bus's.
    ///
    /// The offline renderer keeps going for this long past the end of the arrangement so that
    /// reverbs and delays are not chopped off in the exported file.
    pub fn tail_frames(&self) -> usize {
        let master = self.master.tail_frames();
        let through = longest_paths(
            &self.tracks,
            &self.bus_tracks,
            &self.order,
            RenderStrip::tail_frames,
            master,
        );
        self.tracks
            .iter()
            .enumerate()
            .filter(|(_, track)| !matches!(track.source, RenderSource::Bus { .. }))
            .map(|(index, _)| through[index])
            .max()
            .unwrap_or(master)
    }

    /// How far behind the playhead this graph's output runs, in frames.
    ///
    /// Every source is held back to the longest path through the routing, and that path's own
    /// length is this. Playback simply arrives this late; an export renders the extra frames and
    /// drops them, so the file still lines up with the timeline.
    pub fn latency_frames(&self) -> usize {
        self.latency
    }

    /// `true` when the delay lines no longer match what the chains need.
    ///
    /// Compared strip by strip rather than as one total, because two plugins can trade latency
    /// between them and leave the total where it was while the tracks fall out of step with each
    /// other. Nothing but a parameter moving a plugin's latency — the limiter's lookahead is the
    /// only one that ships — can cause this, since every other way of changing a chain rebuilds
    /// the graph outright.
    ///
    /// Allocation-free, because the audio thread asks it once per callback.
    pub fn latency_is_stale(&self) -> bool {
        let now = self
            .tracks
            .iter()
            .map(|track| track.strip.latency_frames())
            .chain(std::iter::once(self.master.latency_frames()));
        self.built_latencies.iter().copied().ne(now)
    }

    /// Last frame any scheduled event or audio clip touches.
    pub fn end_frame(&self) -> u64 {
        self.tracks
            .iter()
            .map(|track| match &track.source {
                RenderSource::Instrument { events, .. } => {
                    events.last().map_or(0, |event| event.frame)
                }
                RenderSource::Audio { clips } => clips
                    .iter()
                    .map(|clip| clip.start_frame + clip.length)
                    .max()
                    .unwrap_or(0),
                // A bus has nothing of its own, so it can never be what makes a project longer.
                RenderSource::Bus { .. } | RenderSource::Silence => 0,
            })
            .max()
            .unwrap_or(0)
    }

    /// Moves a track's fader.
    pub fn set_track_gain_db(&mut self, index: usize, gain_db: f32) {
        if let Some(track) = self.tracks.get_mut(index) {
            track.strip.set_gain_db(gain_db);
        }
    }

    /// Moves a track's pan control.
    pub fn set_track_pan(&mut self, index: usize, pan: f32) {
        if let Some(track) = self.tracks.get_mut(index) {
            track.strip.set_pan(pan);
        }
    }

    /// Toggles a track's mute.
    pub fn set_track_mute(&mut self, index: usize, mute: bool) {
        if let Some(track) = self.tracks.get_mut(index) {
            track.strip.set_mute(mute);
        }
    }

    /// Moves one of a track's send levels. Out-of-range indices are ignored.
    ///
    /// Addressed by position in the track's send list, the same way an effect is addressed by
    /// position in its chain: a send the graph could not resolve keeps no slot, so this is the one
    /// place where a document position and a graph position can disagree — which is why adding or
    /// removing a send rebuilds the graph rather than sending a command.
    pub fn set_send_level_db(&mut self, track: usize, send: usize, level_db: f32) {
        if let Some(send) = self
            .tracks
            .get_mut(track)
            .and_then(|track| track.sends.get_mut(send))
        {
            send.gain.set_target(db_to_gain(level_db));
        }
    }

    /// Moves the master fader. `gain_db` is in decibels.
    pub fn set_master_gain_db(&mut self, gain_db: f32) {
        self.master.set_gain_db(gain_db);
    }

    /// Sets the master bus pan.
    pub fn set_master_pan(&mut self, pan: f32) {
        self.master.set_pan(pan);
    }

    /// Writes an effect parameter on a track, or on the master bus when `track` is `None`.
    pub fn set_effect_param(
        &mut self,
        track: Option<usize>,
        slot: usize,
        param: ParamId,
        value: f32,
    ) {
        match track {
            Some(index) => {
                if let Some(track) = self.tracks.get_mut(index) {
                    track.strip.set_effect_param(slot, param, value);
                }
            }
            None => self.master.set_effect_param(slot, param, value),
        }
    }

    /// Writes a parameter on a track's instrument.
    pub fn set_instrument_param(&mut self, track: usize, param: ParamId, value: f32) {
        if let Some(track) = self.tracks.get_mut(track) {
            track.set_instrument_param(param, value);
        }
    }

    /// Queues an audition note on a track.
    pub fn note_on(&mut self, track: usize, pitch: u8, velocity: f32) {
        if let Some(track) = self.tracks.get_mut(track) {
            track.note_on(pitch, velocity);
        }
    }

    /// Queues an audition note release on a track.
    pub fn note_off(&mut self, track: usize, pitch: u8) {
        if let Some(track) = self.tracks.get_mut(track) {
            track.note_off(pitch);
        }
    }

    /// Drops every sounding voice without touching effect tails.
    ///
    /// This is what a stop or a seek does: notes must not hang, but a reverb should keep ringing.
    pub fn reset_voices(&mut self) {
        for track in &mut self.tracks {
            track.reset_voices();
        }
    }

    /// Silences everything: voices, delay lines and filter memory.
    ///
    /// Notes stay off until the next note-on rather than being chased back, because a panic that
    /// immediately restored what it had just killed would be useless.
    pub fn panic(&mut self) {
        for track in &mut self.tracks {
            track.silence_voices();
            track.strip.reset();
            track.delay.reset();
            track.output_delay.reset();
            for send in &mut track.sends {
                send.delay.reset();
            }
            track.peak = 0.0;
        }
        for input in &mut self.bus_inputs {
            input.clear();
        }
        self.master.reset();
        self.master_peak = [0.0, 0.0];
    }
}

/// Where every delay line in the graph goes, in frames.
struct LatencyPlan {
    /// Total latency from the playhead to the output.
    total: usize,
    /// What each track is held back by so that every source lines up, by track index.
    node: Vec<usize>,
    /// What each of a track's outgoing edges is held back by so that everything arriving at a bus
    /// lines up: the output edge first, then one per send, by track index.
    edges: Vec<Vec<usize>>,
}

/// What each of a track's outgoing edges costs downstream of the track itself, output edge first.
///
/// Written into `out` rather than returned so the caller can reuse one allocation across a whole
/// graph — this runs once per node per pass and none of it happens on the audio thread, but a
/// `Vec` per node for a number that is almost always zero is still waste.
fn edge_costs(
    track: &RenderTrack,
    bus_tracks: &[usize],
    through: &[usize],
    master_cost: usize,
    out: &mut Vec<usize>,
) {
    let resolve = |slot: Option<usize>| match slot {
        None => master_cost,
        Some(slot) => bus_tracks
            .get(slot)
            .and_then(|index| through.get(*index))
            .copied()
            .unwrap_or(master_cost),
    };
    out.clear();
    out.push(resolve(track.output));
    out.extend(track.sends.iter().map(|send| resolve(Some(send.target))));
}

/// The longest path from every track to the output, adding each strip's own `cost` along the way.
///
/// One walk in reverse routing order is enough: the order puts a bus after everything feeding it,
/// so walking it backwards reaches every destination before the tracks that name it.
fn longest_paths(
    tracks: &[RenderTrack],
    bus_tracks: &[usize],
    order: &[usize],
    cost: impl Fn(&RenderStrip) -> usize,
    master_cost: usize,
) -> Vec<usize> {
    let mut through = vec![0usize; tracks.len()];
    let mut edges = Vec::new();
    for &index in order.iter().rev() {
        let Some(track) = tracks.get(index) else {
            continue;
        };
        edge_costs(track, bus_tracks, &through, master_cost, &mut edges);
        let downstream = edges.iter().copied().max().unwrap_or(master_cost);
        through[index] = cost(&track.strip).saturating_add(downstream);
    }
    through
}

/// Works out every delay the graph needs so that nothing arrives anywhere out of step.
///
/// The rule is one sentence long: **a signal's whole journey to the output must take the same
/// time however it goes.** A track's chain is only part of that journey — the bus it feeds has a
/// chain too, and so does the master — so what has to be equalised is the *path*, not the chain.
///
/// So each track is held back by the difference between the longest path in the graph and its own,
/// which is what puts the sources in step; and each outgoing *edge* is held back by the difference
/// between the track's longest onward path and that edge's, which is what puts the copies of one
/// track in step with each other. A track feeding the master dry while sending to a bus that looks
/// ahead is exactly the case the second one exists for: without it the dry and the wet arrive at
/// different times and comb-filter each other.
///
/// A bus is never held back itself: everything reaching it was lined up on the way in.
fn plan_latency(
    tracks: &[RenderTrack],
    bus_tracks: &[usize],
    order: &[usize],
    master_latency: usize,
) -> LatencyPlan {
    let through = longest_paths(
        tracks,
        bus_tracks,
        order,
        RenderStrip::latency_frames,
        master_latency,
    );

    let mut edges = Vec::with_capacity(tracks.len());
    let mut costs = Vec::new();
    for track in tracks {
        edge_costs(track, bus_tracks, &through, master_latency, &mut costs);
        let longest = costs.iter().copied().max().unwrap_or(0);
        edges.push(costs.iter().map(|cost| longest - cost).collect());
    }

    // Only a track with material of its own is a source to be lined up. A bus has none, and is
    // already in step with whatever reached it.
    let is_source = |track: &RenderTrack| !matches!(track.source, RenderSource::Bus { .. });
    let total = tracks
        .iter()
        .enumerate()
        .filter(|(_, track)| is_source(track))
        .map(|(index, _)| through[index])
        .max()
        .unwrap_or(master_latency);
    let node = tracks
        .iter()
        .enumerate()
        .map(|(index, track)| match is_source(track) {
            true => total.saturating_sub(through[index]),
            false => 0,
        })
        .collect();

    LatencyPlan { total, node, edges }
}

/// Flattens one MIDI clip's notes into absolute timeline events.
///
/// Notes starting past the clip's end are dropped and notes running past it are cut short, which
/// is what the arrangement shows: the clip's length is the gate.
fn schedule_clip(
    clip: &MidiClip,
    tempo_map: &TempoMap,
    sample_rate: f64,
    out: &mut Vec<ScheduledEvent>,
) {
    if clip.muted || clip.length <= Ticks::ZERO {
        return;
    }
    // Which notes a clip actually plays is `MidiClip`'s own rule, asked rather than repeated: the
    // MIDI writer asks the same question, and an export that answered it differently from the
    // renderer would write a file that is not the piece you can hear.
    for note in clip.playable_notes() {
        let start_tick = clip.start + note.start;
        let end_tick = clip.start + note.end();
        let start = tempo_map.ticks_to_samples(start_tick, sample_rate).raw();
        // A note must occupy at least one frame or the instrument would see the release before
        // it ever produced a sample.
        let end = tempo_map
            .ticks_to_samples(end_tick, sample_rate)
            .raw()
            .max(start + 1);
        out.push(ScheduledEvent {
            frame: start,
            event: NoteEvent::NoteOn {
                frame: 0,
                pitch: note.pitch,
                velocity: note.velocity.clamp(0.0, 1.0),
            },
        });
        out.push(ScheduledEvent {
            frame: end,
            event: NoteEvent::NoteOff {
                frame: 0,
                pitch: note.pitch,
            },
        });
    }
    // The bend, sampled the same way and by the same rule — asked of the clip rather than worked
    // out here, so the roll drawing the curve and the renderer playing it read one answer.
    for (at, semitones) in clip.bend_events(auris_core::project::BEND_STEP) {
        out.push(ScheduledEvent {
            frame: tempo_map
                .ticks_to_samples(clip.start + at, sample_rate)
                .raw(),
            event: NoteEvent::PitchBend {
                frame: 0,
                semitones,
            },
        });
    }
}

/// Rank used to break ties between events landing on the same frame.
///
/// Releases go first so that a note repeated at the same pitch retriggers instead of the new
/// note being cut short by the old note's release.
fn event_rank(event: &NoteEvent) -> u8 {
    match event {
        NoteEvent::NoteOff { .. }
        | NoteEvent::AllNotesOff { .. }
        | NoteEvent::AllSoundOff { .. } => 0,
        NoteEvent::PitchBend { .. } => 1,
        NoteEvent::NoteOn { .. } => 2,
    }
}

fn sort_events(events: &mut [ScheduledEvent]) {
    events.sort_by_key(|scheduled| (scheduled.frame, event_rank(&scheduled.event)));
}

/// Resolves a project audio clip against the sample bank.
///
/// `source_rate` is the rate the clip's frame counts — its trim, its length, its fades — are
/// expressed in, which is the rate the file was decoded at. `sample_rate` is the rate this graph
/// renders at, and the two are not always the same: an output device is free to run at 44.1 kHz
/// under a 48 kHz project, and an export can be asked for any rate at all. Every frame count is
/// therefore converted on the way in, and the caller is expected to have handed the bank buffers
/// at the render rate to match.
fn resolve_audio_clip(
    clip: &AudioClip,
    bank: &AudioSourceBank,
    tempo_map: &TempoMap,
    sample_rate: f64,
    source_rate: f64,
) -> Option<RenderAudioClip> {
    if clip.muted {
        return None;
    }
    let Some(buffer) = bank.get(clip.source) else {
        log::warn!(
            "clip `{}` references source {} which is not loaded",
            clip.name,
            clip.source.0
        );
        return None;
    };
    // A nonsense rate in the document would otherwise scale every position to zero or to NaN.
    let ratio = if source_rate.is_finite() && source_rate > 0.0 {
        sample_rate / source_rate
    } else {
        1.0
    };
    // Casting a float to an integer saturates in Rust, so an absurd figure clamps rather than
    // wrapping round to a small one.
    let convert = |frames: u64| (frames as f64 * ratio).round() as u64;

    let available = buffer.frame_count() as u64;
    let source_offset = convert(clip.offset_frames).min(available);
    let length = convert(clip.length_frames).min(available - source_offset);
    if length == 0 {
        return None;
    }
    Some(RenderAudioClip {
        buffer: Arc::clone(buffer),
        start_frame: tempo_map
            .ticks_to_samples(clip.start.max_zero(), sample_rate)
            .raw(),
        source_offset,
        length,
        gain: db_to_gain(clip.gain_db),
        fade_in: convert(clip.fade_in_frames),
        fade_out: convert(clip.fade_out_frames),
    })
}

/// Largest number of events that can fall inside any window of `window` frames.
///
/// This is the exact bound the per-block event buffer needs: rendering never sees more than one
/// window's worth at a time, so sizing to this makes the buffer allocation-free without ever
/// dropping an event.
/// Largest number of notes this track ever has sounding at once.
///
/// That is exactly what the chase after a seek has to be able to re-issue: it re-triggers every
/// note spanning the new position, including several on one pitch when they overlap. Sizing the
/// event buffer to this makes the chase allocation-free without ever dropping a note.
///
/// The events arrive sorted by frame with releases ranked ahead of strikes on the same frame, so
/// a single pass over them is enough — a note ending exactly where the next begins never counts
/// as two.
fn max_sounding_notes(events: &[ScheduledEvent]) -> usize {
    let mut sounding = 0usize;
    let mut most = 0usize;
    for scheduled in events {
        match scheduled.event {
            NoteEvent::NoteOn { .. } => {
                sounding += 1;
                most = most.max(sounding);
            }
            NoteEvent::NoteOff { .. } => sounding = sounding.saturating_sub(1),
            NoteEvent::AllNotesOff { .. } | NoteEvent::AllSoundOff { .. } => sounding = 0,
            NoteEvent::PitchBend { .. } => {}
        }
    }
    most
}

fn max_events_in_window(events: &[ScheduledEvent], window: u64) -> usize {
    let mut best = 0;
    let mut low = 0;
    for (high, event) in events.iter().enumerate() {
        while event.frame - events[low].frame >= window {
            low += 1;
        }
        best = best.max(high - low + 1);
    }
    best
}

/// Where an automated value goes, in this graph's own coordinates.
///
/// The document addresses a parameter by id; the graph addresses it by position, exactly as every
/// [`EngineCommand`](crate::EngineCommand) does. Translating once when the graph is built rather
/// than once per block is what keeps the audio thread free of lookups — and it turns a lane
/// pointing at something this graph does not have into an absence rather than a miss on every
/// block for the life of the project.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum AutomationSlot {
    /// A track's fader, by track index.
    TrackGain(usize),
    /// A track's pan, by track index.
    TrackPan(usize),
    /// The master fader.
    MasterGain,
    /// The master pan.
    MasterPan,
    /// One of a track's send levels, by track index and position in its send list.
    Send { track: usize, send: usize },
    /// A parameter on a track's instrument.
    Instrument { track: usize, param: ParamId },
    /// A parameter on an effect, on a track or on the master bus.
    Effect {
        track: Option<usize>,
        slot: usize,
        param: ParamId,
    },
}

/// One document lane, resolved to where it lands.
struct RenderAutomation {
    lane: AutomationLane,
    slot: AutomationSlot,
}

/// Translates every lane in `project` into a graph position, dropping the ones that resolve to
/// nothing.
///
/// A lane can fail to resolve when the thing it names is gone — and it should not be possible,
/// because the document removes a lane along with the track or effect slot it points at. Dropping
/// it here is the second lock on that door: what a lane resolved wrongly would do is drive a
/// parameter on whatever now occupies that position, which is worse than silence.
fn resolve_automation(project: &Project) -> Vec<RenderAutomation> {
    project
        .automation
        .lanes()
        .iter()
        .filter_map(|lane| {
            Some(RenderAutomation {
                lane: lane.clone(),
                slot: resolve_slot(project, lane.target)?,
            })
        })
        .collect()
}

/// Where one target lands in a graph built from `project`, if it lands anywhere.
fn resolve_slot(project: &Project, target: ParamTarget) -> Option<AutomationSlot> {
    let chain_position = |track: Option<TrackId>, slot: EffectSlotId| {
        let effects = match track {
            Some(id) => &project.track(id)?.mixer.effects,
            None => &project.master.effects,
        };
        effects.iter().position(|effect| effect.id == slot)
    };
    match target {
        ParamTarget::TrackGain(id) => project.track_index(id).map(AutomationSlot::TrackGain),
        ParamTarget::TrackPan(id) => project.track_index(id).map(AutomationSlot::TrackPan),
        ParamTarget::MasterGain => Some(AutomationSlot::MasterGain),
        ParamTarget::MasterPan => Some(AutomationSlot::MasterPan),
        ParamTarget::Send { track, send } => Some(AutomationSlot::Send {
            track: project.track_index(track)?,
            send: project
                .track(track)?
                .sends
                .iter()
                .position(|existing| existing.id == send)?,
        }),
        ParamTarget::Instrument { track, param } => Some(AutomationSlot::Instrument {
            track: project.track_index(track)?,
            param,
        }),
        ParamTarget::Effect { track, slot, param } => Some(AutomationSlot::Effect {
            track: match track {
                Some(id) => Some(project.track_index(id)?),
                None => None,
            },
            slot: chain_position(track, slot)?,
            param,
        }),
    }
}

/// Writes every lane's value at `tick` into the strip or plugin it drives.
///
/// Free rather than a method so it can be given the tracks and the lanes as separate borrows, and
/// so the rule can be asserted on a handful of structs instead of a whole graph.
///
/// Every value goes in through the same setter a moved fader or a turned knob uses. That is the
/// point: there is no second path into a parameter that could clamp differently, skip the ramp, or
/// forget to tell the pan law.
///
/// `continuing` is false when the playhead arrived rather than advanced — the first segment of a
/// graph, a seek, a loop wrap — and the faders are then put where the lane says instead of sliding
/// there. Only the mixer's own controls have a ramp to skip; a plugin parameter is written the
/// same way either way, because there is nothing between two values of it.
fn drive_automation(
    automation: &[RenderAutomation],
    tracks: &mut [RenderTrack],
    master: &mut RenderStrip,
    tick: Ticks,
    continuing: bool,
) {
    for entry in automation {
        let value = entry.lane.value_at(tick);
        match entry.slot {
            AutomationSlot::TrackGain(index) => {
                if let Some(track) = tracks.get_mut(index) {
                    match continuing {
                        true => track.strip.set_gain_db(value),
                        false => track.strip.jump_gain_db(value),
                    }
                }
            }
            AutomationSlot::TrackPan(index) => {
                if let Some(track) = tracks.get_mut(index) {
                    match continuing {
                        true => track.strip.set_pan(value),
                        false => track.strip.jump_pan(value),
                    }
                }
            }
            AutomationSlot::MasterGain => match continuing {
                true => master.set_gain_db(value),
                false => master.jump_gain_db(value),
            },
            AutomationSlot::MasterPan => match continuing {
                true => master.set_pan(value),
                false => master.jump_pan(value),
            },
            AutomationSlot::Send { track, send } => {
                if let Some(send) = tracks
                    .get_mut(track)
                    .and_then(|track| track.sends.get_mut(send))
                {
                    match continuing {
                        true => send.gain.set_target(db_to_gain(value)),
                        false => send.gain.jump_to(db_to_gain(value)),
                    }
                }
            }
            AutomationSlot::Instrument { track, param } => {
                if let Some(track) = tracks.get_mut(track) {
                    track.set_instrument_param(param, value);
                }
            }
            AutomationSlot::Effect { track, slot, param } => match track {
                Some(index) => {
                    if let Some(track) = tracks.get_mut(index) {
                        track.strip.set_effect_param(slot, param, value);
                    }
                }
                None => master.set_effect_param(slot, param, value),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit;
    use auris_core::project::Note;
    use auris_core::time::TICKS_PER_QUARTER;

    fn quarter_note_project() -> Project {
        let mut project = Project::new("Graph", 48_000.0);
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        let clip = project
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        let midi = project.midi_clip_mut(clip).unwrap();
        midi.notes
            .push(Note::new(60, Ticks::QUARTER, Ticks::QUARTER));
        project
    }

    #[test]
    fn notes_land_on_absolute_sample_positions() {
        let project = quarter_note_project();
        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        let RenderSource::Instrument { events, .. } = &graph.tracks()[0].source else {
            panic!("expected an instrument source");
        };
        // 120 BPM at 48 kHz: one quarter note is 0.5 s, so 24 000 frames.
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].frame, 24_000);
        assert_eq!(events[1].frame, 48_000);
        assert!(matches!(
            events[0].event,
            NoteEvent::NoteOn { pitch: 60, .. }
        ));
        assert!(matches!(
            events[1].event,
            NoteEvent::NoteOff { pitch: 60, .. }
        ));
    }

    #[test]
    fn notes_are_clipped_to_their_clips_length() {
        let mut project = Project::new("Graph", 48_000.0);
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        let clip = project
            .add_midi_clip(track, "Short", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        let midi = project.midi_clip_mut(clip).unwrap();
        // Four beats long inside a one-beat clip, plus a note that starts after the clip ends.
        midi.notes
            .push(Note::new(60, Ticks::ZERO, Ticks::from_beats(4.0)));
        midi.notes
            .push(Note::new(62, Ticks::from_beats(2.0), Ticks::QUARTER));

        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        let RenderSource::Instrument { events, .. } = &graph.tracks()[0].source else {
            panic!("expected an instrument source");
        };
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].frame, 0);
        assert_eq!(events[1].frame, 24_000);
    }

    #[test]
    fn a_muted_clip_schedules_nothing() {
        let mut project = quarter_note_project();
        project.tracks[0].kind.as_instrument_mut().unwrap().clips[0].muted = true;
        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        assert_eq!(graph.tracks()[0].event_count(), 0);
    }

    #[test]
    fn an_unknown_instrument_keeps_the_track_slot() {
        let mut project = Project::new("Graph", 48_000.0);
        project.add_instrument_track("Ghost", "does.not.exist");
        project.add_instrument_track("Real", testkit::TONE_ID);
        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        assert_eq!(graph.track_count(), 2);
        assert!(matches!(graph.tracks()[0].source, RenderSource::Silence));
        assert!(matches!(
            graph.tracks()[1].source,
            RenderSource::Instrument { .. }
        ));
    }

    #[test]
    fn an_unknown_effect_keeps_its_slot_bypassed() {
        let mut project = Project::new("Graph", 48_000.0);
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        project.add_effect(Some(track), "does.not.exist");
        project.add_effect(Some(track), testkit::GAIN_ID);
        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        let strip = graph.tracks()[0].strip();
        assert_eq!(strip.effect_count(), 2);
        assert_eq!(strip.effects[0].descriptor().id, MISSING_EFFECT_ID);
        assert!(!strip.enabled[0], "a placeholder must never process audio");
        assert!(strip.enabled[1]);
        // The placeholder declares no tail, so it cannot lengthen an export either.
        assert_eq!(strip.tail_frames(), 0);
    }

    #[test]
    fn tails_add_up_along_a_chain() {
        // Two ringing effects in series: the second is still being fed while the first decays,
        // so the chain sounds for both tails end to end rather than for the longer of the two.
        let mut project = Project::new("Tails", 48_000.0);
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        project.add_effect(Some(track), testkit::TAIL_ID);
        project.add_effect(Some(track), testkit::TAIL_ID);

        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        assert_eq!(
            graph.tracks()[0].strip().tail_frames(),
            2 * testkit::TAIL_FRAMES
        );
    }

    #[test]
    fn a_bypassed_slot_contributes_nothing_to_the_sum() {
        let mut project = Project::new("Tails", 48_000.0);
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        project.add_effect(Some(track), testkit::TAIL_ID);
        project.add_effect(Some(track), testkit::TAIL_ID);
        project.tracks[0].mixer.effects[0].enabled = false;

        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        assert_eq!(
            graph.tracks()[0].strip().tail_frames(),
            testkit::TAIL_FRAMES
        );
    }

    #[test]
    fn the_master_tail_follows_the_longest_track_tail_rather_than_overlapping_it() {
        // Tracks are parallel, so the two of them together still only reach one tail's worth —
        // but the master runs after both, so its tail starts where theirs ends.
        let mut project = Project::new("Tails", 48_000.0);
        let a = project.add_instrument_track("A", testkit::TONE_ID);
        let b = project.add_instrument_track("B", testkit::TONE_ID);
        project.add_effect(Some(a), testkit::TAIL_ID);
        project.add_effect(Some(b), testkit::TAIL_ID);
        project.add_effect(None, testkit::TAIL_ID);

        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        assert_eq!(graph.tail_frames(), 2 * testkit::TAIL_FRAMES);
    }

    #[test]
    fn a_nonsense_tail_saturates_instead_of_overflowing() {
        // `tail_frames` is a figure a plugin chooses, so two greedy ones must not wrap the sum
        // round to a short export.
        let mut project = Project::new("Tails", 48_000.0);
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        project.add_effect(Some(track), testkit::HUGE_TAIL_ID);
        project.add_effect(Some(track), testkit::HUGE_TAIL_ID);
        project.add_effect(None, testkit::HUGE_TAIL_ID);

        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        assert_eq!(graph.tracks()[0].strip().tail_frames(), usize::MAX);
        assert_eq!(graph.tail_frames(), usize::MAX);
    }

    #[test]
    fn every_track_is_held_back_to_the_longest_chain() {
        let mut project = Project::new("PDC", 48_000.0);
        let late = project.add_instrument_track("Late", testkit::TONE_ID);
        project.add_instrument_track("Plain", testkit::TONE_ID);
        project.add_effect(Some(late), testkit::LOOKAHEAD_ID);

        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        // The track that runs late needs no delay; the one that does not has to wait for it.
        assert_eq!(graph.tracks()[0].compensation_frames(), 0);
        assert_eq!(
            graph.tracks()[1].compensation_frames(),
            testkit::LOOKAHEAD_FRAMES
        );
        assert_eq!(graph.latency_frames(), testkit::LOOKAHEAD_FRAMES);
        assert!(!graph.latency_is_stale());
    }

    #[test]
    fn latencies_add_up_along_a_chain_and_the_master_adds_on_top() {
        let mut project = Project::new("PDC", 48_000.0);
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        project.add_effect(Some(track), testkit::LOOKAHEAD_ID);
        project.add_effect(Some(track), testkit::LOOKAHEAD_ID);
        project.add_effect(None, testkit::LOOKAHEAD_ID);

        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        assert_eq!(
            graph.tracks()[0].strip().latency_frames(),
            2 * testkit::LOOKAHEAD_FRAMES
        );
        assert_eq!(graph.latency_frames(), 3 * testkit::LOOKAHEAD_FRAMES);
    }

    #[test]
    fn a_bypassed_effect_delays_nothing_and_needs_no_compensation() {
        let mut project = Project::new("PDC", 48_000.0);
        let late = project.add_instrument_track("Late", testkit::TONE_ID);
        project.add_instrument_track("Plain", testkit::TONE_ID);
        project.add_effect(Some(late), testkit::LOOKAHEAD_ID);
        project.tracks[0].mixer.effects[0].enabled = false;

        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        assert_eq!(graph.latency_frames(), 0);
        assert_eq!(graph.tracks()[1].compensation_frames(), 0);
    }

    #[test]
    fn a_graph_with_nothing_to_compensate_allocates_no_delay_lines() {
        let project = quarter_note_project();
        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        assert_eq!(graph.latency_frames(), 0);
        assert_eq!(graph.tracks()[0].compensation_frames(), 0);
        assert!(graph.tracks()[0].delay.lines.is_empty());
    }

    #[test]
    fn a_delay_line_hands_samples_back_a_fixed_number_of_frames_later() {
        let mut delay = LatencyDelay::new(3, 2);
        let mut buffer =
            AudioBuffer::from_planar(vec![vec![1.0, 2.0, 3.0, 4.0]; 2], 48_000.0).expect("planar");
        delay.process(&mut buffer);
        // Three frames of the empty line come out first, then the input begins.
        assert_eq!(buffer.channel(0), &[0.0, 0.0, 0.0, 1.0]);
        assert_eq!(buffer.channel(1), &[0.0, 0.0, 0.0, 1.0]);

        let mut next =
            AudioBuffer::from_planar(vec![vec![5.0, 6.0, 7.0, 8.0]; 2], 48_000.0).expect("planar");
        delay.process(&mut next);
        assert_eq!(next.channel(0), &[2.0, 3.0, 4.0, 5.0]);
    }

    #[test]
    fn a_delay_line_reads_the_same_however_the_blocks_are_cut() {
        let feed: Vec<f32> = (0..64).map(|index| index as f32).collect();
        let run = |block: usize| {
            let mut delay = LatencyDelay::new(7, 2);
            let mut out = Vec::new();
            for chunk in feed.chunks(block) {
                let mut buffer =
                    AudioBuffer::from_planar(vec![chunk.to_vec(); 2], 48_000.0).expect("planar");
                delay.process(&mut buffer);
                out.extend_from_slice(buffer.channel(0));
            }
            out
        };
        assert_eq!(run(64), run(5));
        assert_eq!(run(64), run(1));
    }

    #[test]
    fn a_missing_effect_does_not_shift_the_slots_after_it() {
        // The UI addresses effects by their position in the *project's* chain, so slot 2 must
        // still be the second real effect even though slot 0 could not be instantiated.
        let mut project = Project::new("Graph", 48_000.0);
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        project.add_effect(Some(track), "does.not.exist");
        project.add_effect(Some(track), testkit::GAIN_ID);
        project.add_effect(Some(track), testkit::GAIN_ID);

        let mut graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        graph.set_effect_param(Some(0), 1, ParamId(0), 2.0);
        graph.set_effect_param(Some(0), 2, ParamId(0), 3.0);

        let strip = graph.tracks()[0].strip();
        assert_eq!(strip.effects[1].param(ParamId(0)), 2.0);
        assert_eq!(
            strip.effects[2].param(ParamId(0)),
            3.0,
            "the last slot must not fall off the end of the runtime chain"
        );
    }

    #[test]
    fn saved_plugin_state_reaches_the_instance() {
        let mut project = quarter_note_project();
        let track_id = project.tracks[0].id;
        project.tracks[0]
            .kind
            .as_instrument_mut()
            .unwrap()
            .instrument_state
            .params
            .insert("amplitude".into(), 0.25);
        let slot = project
            .add_effect(Some(track_id), testkit::GAIN_ID)
            .unwrap();
        let effect = project.tracks[0]
            .mixer
            .effects
            .iter_mut()
            .find(|e| e.id == slot)
            .unwrap();
        effect.state.params.insert("gain".into(), 3.0);

        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        let RenderSource::Instrument { instrument, .. } = &graph.tracks()[0].source else {
            panic!("expected an instrument source");
        };
        assert_eq!(instrument.param(ParamId(0)), 0.25);
        assert_eq!(
            graph.tracks()[0].strip().effects[0].param(ParamId(0)),
            3.0,
            "a saved effect parameter must survive the rebuild"
        );
    }

    #[test]
    fn a_bypassed_effect_is_instantiated_but_keeps_the_next_slot_index_stable() {
        let mut project = Project::new("Bypass", 48_000.0);
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        project.add_effect(Some(track), testkit::GAIN_ID);
        project.add_effect(Some(track), testkit::GAIN_ID);
        {
            let strip = &mut project.tracks[0].mixer;
            strip.effects[0].enabled = false;
            strip.effects[0].state.params.insert("gain".into(), 4.0);
        }

        let mut graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        // Both slots exist, so `SetEffectParam { slot: 1 }` still means the second effect.
        assert_eq!(graph.tracks()[0].strip().effect_count(), 2);
        graph.set_effect_param(Some(0), 1, ParamId(0), 2.0);
        assert_eq!(graph.tracks()[0].strip().effects[0].param(ParamId(0)), 4.0);
        assert_eq!(graph.tracks()[0].strip().effects[1].param(ParamId(0)), 2.0);
    }

    #[test]
    fn a_nonsense_sample_rate_falls_back_instead_of_poisoning_the_graph() {
        let mut project = quarter_note_project();
        project.sample_rate = 0.0;
        let graph = RenderGraph::build_at(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            512,
            f64::NAN,
        );
        assert_eq!(graph.sample_rate(), DEFAULT_SAMPLE_RATE);
        assert!(graph.sample_rate().is_finite());
    }

    #[test]
    fn solo_clears_the_audible_flag_on_other_tracks() {
        let mut project = Project::new("Graph", 48_000.0);
        project.add_instrument_track("A", testkit::TONE_ID);
        let b = project.add_instrument_track("B", testkit::TONE_ID);
        project.track_mut(b).unwrap().mixer.solo = true;
        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        assert!(!graph.tracks()[0].strip().is_active());
        assert!(graph.tracks()[1].strip().is_active());
    }

    /// A track whose notes all start at tick 0 and last `length`.
    fn stacked_note_project(pitches: std::ops::Range<u8>, length: Ticks) -> Project {
        let mut project = Project::new("Graph", 48_000.0);
        let track = project.add_instrument_track("Chords", testkit::TONE_ID);
        let clip = project
            .add_midi_clip(track, "Stack", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        let midi = project.midi_clip_mut(clip).unwrap();
        for pitch in pitches {
            midi.notes.push(Note::new(pitch, Ticks::ZERO, length));
        }
        project
    }

    #[test]
    fn the_event_scratch_is_sized_for_the_densest_block() {
        // Six note-ons on frame 0; their releases are a quarter note away, so no 512-frame
        // window ever holds more than six events — and six is also the most that can be sounding
        // at once, so the chase needs room for six and the all-notes-off before them.
        let sparse = stacked_note_project(60..66, Ticks::QUARTER);
        let graph = RenderGraph::build(&sparse, &AudioSourceBank::new(), &testkit::registry(), 512);
        assert_eq!(
            graph.tracks()[0].block_events.capacity(),
            6 + AUDITION_HEADROOM + 6 + CHASE_HEADROOM
        );

        // Every pitch struck and released inside one block: 256 events in a single window, twice
        // the 128 that are ever sounding together, so the density term is what sizes the buffer.
        let dense = stacked_note_project(0..128, Ticks(1));
        let graph = RenderGraph::build(&dense, &AudioSourceBank::new(), &testkit::registry(), 512);
        let expected = 256 + AUDITION_HEADROOM + 128 + CHASE_HEADROOM;
        assert_eq!(graph.tracks()[0].block_events.capacity(), expected);
    }

    #[test]
    fn the_chase_headroom_counts_overlapping_notes_on_one_pitch() {
        // Four notes on the same pitch, each starting before the last has ended: all four are
        // sounding together at the third strike, so a seek there has to re-issue four voices.
        let mut project = Project::new("Overlap", 48_000.0);
        let track = project.add_instrument_track("Pedal", testkit::TONE_ID);
        let clip = project
            .add_midi_clip(track, "c", Ticks::ZERO, Ticks::from_beats(16.0))
            .unwrap();
        let midi = project.midi_clip_mut(clip).unwrap();
        for index in 0..4 {
            midi.notes.push(Note::new(
                60,
                Ticks::from_beats(index as f64),
                Ticks::from_beats(8.0),
            ));
        }

        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        // One note-on per block at most, so density contributes 1.
        assert_eq!(
            graph.tracks()[0].block_events.capacity(),
            1 + AUDITION_HEADROOM + 4 + CHASE_HEADROOM
        );
    }

    #[test]
    fn the_sounding_bound_ignores_notes_that_never_overlap() {
        let events: Vec<ScheduledEvent> = [
            (0u64, 60u8, true),
            (10, 60, false),
            (20, 62, true),
            (30, 62, false),
        ]
        .into_iter()
        .map(|(frame, pitch, on)| ScheduledEvent {
            frame,
            event: if on {
                NoteEvent::NoteOn {
                    frame: 0,
                    pitch,
                    velocity: 1.0,
                }
            } else {
                NoteEvent::NoteOff { frame: 0, pitch }
            },
        })
        .collect();
        assert_eq!(max_sounding_notes(&events), 1);
        assert_eq!(max_sounding_notes(&[]), 0);
    }

    #[test]
    fn a_tempo_change_moves_later_notes() {
        let mut project = quarter_note_project();
        project.tempo_map.set_point(Ticks(TICKS_PER_QUARTER), 240.0);
        let graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        let RenderSource::Instrument { events, .. } = &graph.tracks()[0].source else {
            panic!("expected an instrument source");
        };
        // The note still starts one quarter in at 120 BPM (24 000 frames) but now lasts only
        // 0.25 s because the tempo doubles exactly where it begins.
        assert_eq!(events[0].frame, 24_000);
        assert_eq!(events[1].frame, 36_000);
    }

    #[test]
    fn window_bound_counts_the_densest_run() {
        let events: Vec<ScheduledEvent> = [0u64, 10, 20, 30, 500, 505]
            .into_iter()
            .map(|frame| ScheduledEvent {
                frame,
                event: NoteEvent::AllNotesOff { frame: 0 },
            })
            .collect();
        assert_eq!(max_events_in_window(&events, 100), 4);
        assert_eq!(max_events_in_window(&events, 11), 2);
        assert_eq!(max_events_in_window(&[], 512), 0);
    }

    #[test]
    fn audio_clips_are_clamped_to_the_source_length() {
        let mut project = Project::new("Graph", 48_000.0);
        let track = project.add_audio_track("Drums");
        let source = project.add_audio_source(
            "loop",
            auris_core::AssetPath::inside("Audio/loop.wav"),
            1_000,
            48_000.0,
            2,
        );
        let clip = project.add_audio_clip(track, source, Ticks::ZERO).unwrap();
        {
            let clip = project.audio_clip_mut(clip).unwrap();
            clip.offset_frames = 900;
            clip.length_frames = 500;
        }
        let mut bank = AudioSourceBank::new();
        bank.insert(source, Arc::new(AudioBuffer::stereo(1_000, 48_000.0)));

        let graph = RenderGraph::build(&project, &bank, &testkit::registry(), 512);
        let RenderSource::Audio { clips } = &graph.tracks()[0].source else {
            panic!("expected an audio source");
        };
        assert_eq!(clips[0].source_offset, 900);
        assert_eq!(clips[0].length, 100);
    }

    #[test]
    fn a_clips_frame_counts_are_converted_to_the_render_rate() {
        // The document counts a clip's trim in the frames of the file it came from. Rendering at
        // another rate — an output device that disagrees with the project, or an export asked for
        // a different rate — means those counts no longer measure the timeline, so they are
        // converted rather than used as they stand.
        let mut project = Project::new("Rate", 48_000.0);
        let track = project.add_audio_track("Sample");
        let source = project.add_audio_source(
            "s",
            auris_core::AssetPath::inside("Audio/s.wav"),
            48_000,
            48_000.0,
            2,
        );
        let clip = project.add_audio_clip(track, source, Ticks::ZERO).unwrap();
        {
            let clip = project.audio_clip_mut(clip).unwrap();
            clip.offset_frames = 4_800;
            clip.length_frames = 24_000;
            clip.fade_in_frames = 480;
            clip.fade_out_frames = 960;
        }
        // The bank holds the source converted to the rate the graph will render at.
        let mut bank = AudioSourceBank::new();
        bank.insert(source, Arc::new(AudioBuffer::stereo(96_000, 96_000.0)));

        let graph = RenderGraph::build_at(&project, &bank, &testkit::registry(), 512, 96_000.0);
        let RenderSource::Audio { clips } = &graph.tracks()[0].source else {
            panic!("expected an audio source");
        };
        assert_eq!(clips[0].source_offset, 9_600);
        assert_eq!(clips[0].length, 48_000);
        assert_eq!(clips[0].fade_in, 960);
        assert_eq!(clips[0].fade_out, 1_920);
    }

    #[test]
    fn a_matching_rate_leaves_a_clips_frame_counts_alone() {
        let mut project = Project::new("Rate", 48_000.0);
        let track = project.add_audio_track("Sample");
        let source = project.add_audio_source(
            "s",
            auris_core::AssetPath::inside("Audio/s.wav"),
            1_000,
            48_000.0,
            2,
        );
        let clip = project.add_audio_clip(track, source, Ticks::ZERO).unwrap();
        project.audio_clip_mut(clip).unwrap().length_frames = 800;
        let mut bank = AudioSourceBank::new();
        bank.insert(source, Arc::new(AudioBuffer::stereo(1_000, 48_000.0)));

        let graph = RenderGraph::build(&project, &bank, &testkit::registry(), 512);
        let RenderSource::Audio { clips } = &graph.tracks()[0].source else {
            panic!("expected an audio source");
        };
        assert_eq!(clips[0].length, 800);
    }

    #[test]
    fn a_source_recorded_with_a_nonsense_rate_is_played_as_it_stands() {
        // A corrupt document should not scale every position to zero or to NaN.
        let mut project = Project::new("Rate", 48_000.0);
        let track = project.add_audio_track("Sample");
        let source = project.add_audio_source(
            "s",
            auris_core::AssetPath::inside("Audio/s.wav"),
            1_000,
            0.0,
            2,
        );
        project.add_audio_clip(track, source, Ticks::ZERO).unwrap();
        let mut bank = AudioSourceBank::new();
        bank.insert(source, Arc::new(AudioBuffer::stereo(1_000, 48_000.0)));

        let graph = RenderGraph::build(&project, &bank, &testkit::registry(), 512);
        let RenderSource::Audio { clips } = &graph.tracks()[0].source else {
            panic!("expected an audio source");
        };
        assert_eq!(clips[0].length, 1_000);
    }

    #[test]
    fn a_smoothed_gain_ramps_once_then_settles() {
        let mut gain = SmoothedGain::new(1.0);
        assert_eq!(gain.advance(), (1.0, 1.0));
        gain.set_target(0.5);
        assert_eq!(gain.advance(), (1.0, 0.5));
        assert_eq!(gain.advance(), (0.5, 0.5));
    }

    #[test]
    fn a_centred_strip_at_unity_passes_audio_untouched() {
        let mut strip = RenderStrip::new(0.0, 0.0, false, true, 48_000.0);
        let mut buffer = AudioBuffer::from_planar(vec![vec![1.0; 4], vec![1.0; 4]], 48_000.0)
            .expect("planar buffer");
        strip.apply_gain_and_pan(&mut buffer);
        for channel in 0..2 {
            for sample in buffer.channel(channel) {
                assert!((sample - 1.0).abs() < 1e-6, "got {sample}");
            }
        }
    }

    #[test]
    fn hard_panning_moves_all_the_energy_to_one_side() {
        let mut strip = RenderStrip::new(0.0, 1.0, false, true, 48_000.0);
        strip.pan_current = 1.0;
        let mut buffer = AudioBuffer::from_planar(vec![vec![1.0; 4], vec![1.0; 4]], 48_000.0)
            .expect("planar buffer");
        strip.apply_gain_and_pan(&mut buffer);
        assert!(buffer.channel_peak(0) < 1e-6);
        // Constant power anchored at the centre puts the extremes 3 dB up: sqrt(2).
        assert!((buffer.channel_peak(1) - std::f32::consts::SQRT_2).abs() < 1e-5);
    }

    #[test]
    fn the_graph_can_be_moved_to_the_audio_thread() {
        fn assert_send<T: Send>() {}
        assert_send::<RenderGraph>();
        assert_send::<crate::EngineCommand>();
    }
}
