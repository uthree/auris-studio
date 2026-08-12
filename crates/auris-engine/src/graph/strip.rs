//! The mixer strip: a fader, a pan control, a mute and a chain of effects.
//!
//! Its own file because it is the one part of the graph that is *state a user moves*. Everything
//! here exists to make a control change inaudible as a change — the gain ramps, the pan ramps, the
//! mute slides — and that machinery has nothing to say about routing, scheduling or latency. The
//! master bus is one of these too, which is why it lives here rather than beside [`RenderTrack`].
//!
//! [`RenderTrack`]: super::RenderTrack

use auris_core::param::{ParamDescriptor, db_to_gain, pan_gains};
use auris_core::plugin::{
    Effect, Parameterized, PluginCategory, PluginDescriptor, PrepareContext, ProcessContext,
};
use auris_core::project::MixerStrip;
use auris_core::registry::PluginRegistry;
use auris_core::{AudioBuffer, ParamId};

use super::MUTE_FADE_MS;

/// [`pan_gains`] returns `1/sqrt(2)` on both channels at centre, which would cost 3 dB every
/// time a signal passes a strip. Scaling by `sqrt(2)` puts a centred strip at exactly unity
/// while keeping `left^2 + right^2` constant across the sweep, so the law is still constant
/// power — it is just anchored at the centre instead of at the extremes.
const PAN_CENTRE_NORMALISE: f32 = std::f32::consts::SQRT_2;

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

/// Registry id reported by the stand-in for an effect that could not be created.
pub(crate) const MISSING_EFFECT_ID: &str = "auris.engine.missing";

/// Placeholder for an effect whose plugin id the registry does not know.
///
/// The runtime chain has to stay index-parallel with the project's
/// [`MixerStrip::effects`](auris_core::project::MixerStrip::effects), because
/// [`EngineCommand::SetEffectParam`](crate::EngineCommand::SetEffectParam) addresses effects by
/// their position *there*. Dropping the entry would silently re-aim every later command one slot
/// low and push the last one off the end, so the slot is kept and left bypassed instead — the
/// same contract the instrument path keeps with
/// [`RenderSource::Silence`](super::RenderSource::Silence).
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
    ///
    /// A slot named in `placed` takes the effect the caller already built and does not consult
    /// the registry at all — see [`PlacedEffects`](crate::graph::PlacedEffects).
    pub fn from_mixer(
        mixer: &MixerStrip,
        audible: bool,
        registry: &PluginRegistry,
        placed: &mut crate::graph::PlacedEffects,
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
            // The caller's own effect first: it is already prepared — a hosted plugin was sized
            // when it was activated and cannot be told a second time — and there is nothing for
            // the registry to be asked about.
            if let Some(effect) = placed.remove(&slot.id) {
                strip.effects.push(effect);
                strip.enabled.push(slot.enabled);
                continue;
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::RenderGraph;
    use crate::testkit;
    use auris_core::project::{AudioSourceBank, Project};

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
    fn an_effect_the_caller_placed_beats_the_registry_and_is_taken() {
        // A hosted plugin's id is not in the registry and never will be, so the placed effect is
        // the only thing standing between its slot and a bypassed placeholder.
        let mut project = Project::new("Graph", 48_000.0);
        let track = project.add_instrument_track("Lead", testkit::TONE_ID);
        let slot = project
            .add_effect(Some(track), "clap:com.example.nothing")
            .unwrap();

        let mut placed = crate::graph::PlacedEffects::new();
        placed.insert(
            slot,
            testkit::registry().create_effect(testkit::GAIN_ID).unwrap(),
        );

        let graph = RenderGraph::build_with(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &mut placed,
            &mut crate::graph::PlacedInstruments::new(),
            512,
            48_000.0,
        );

        let strip = graph.tracks()[0].strip();
        assert_eq!(strip.effects[0].descriptor().id, testkit::GAIN_ID);
        assert!(strip.enabled[0], "the slot's own switch still decides");
        assert!(
            placed.is_empty(),
            "building takes what it uses, so what is left names slots the project has dropped"
        );
    }

    #[test]
    fn an_effect_placed_for_a_slot_that_is_gone_is_left_behind() {
        let project = Project::new("Graph", 48_000.0);
        let mut placed = crate::graph::PlacedEffects::new();
        placed.insert(
            auris_core::project::EffectSlotId(999),
            testkit::registry().create_effect(testkit::GAIN_ID).unwrap(),
        );

        RenderGraph::build_with(
            &project,
            &AudioSourceBank::new(),
            &testkit::registry(),
            &mut placed,
            &mut crate::graph::PlacedInstruments::new(),
            512,
            48_000.0,
        );

        assert_eq!(placed.len(), 1, "the caller has to be told, not guessed at");
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
}
