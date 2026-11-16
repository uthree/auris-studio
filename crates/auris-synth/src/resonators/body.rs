//! Any track, through the box of an instrument or a speaker.

use std::borrow::Cow;
use std::sync::Arc;

use auris_core::param::{ParamUnit, ParamValueCurve, db_to_gain};
use auris_core::resonance::{
    RESONANCE_MODES, RESONANCE_RESPONSE, ResonanceCapture, ResonanceFrame, ResonanceKind,
    ResonanceMonitor, response_hz,
};
use auris_core::{
    AudioBuffer, Effect, ParamDescriptor, ParamId, Parameterized, PluginCategory, PluginDescriptor,
    PrepareContext, ProcessContext,
};

use auris_dsp::{Biquad, BiquadCoefficients};

use super::{Level, PUBLISH_TICKS, TICK, sane_rate};
use crate::ParamBank;
use crate::physical::body::{self, BodySpec, Radiator};

/// The boxes a track can be put through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyKind {
    /// A flat-top guitar: the air in the box at about a hundred hertz, the top plate an octave up.
    Guitar,
    /// A violin's corpus, its two main modes either side of 500 Hz.
    Violin,
    /// The violin's body built two and a half times larger, as a cello's is.
    Cello,
    /// A grand piano's soundboard: no named resonance, a great many small ones.
    Soundboard,
    /// A hammered dulcimer's shallow, bright box.
    Dulcimer,
    /// An open-backed guitar speaker cabinet.
    Cabinet,
}

impl BodyKind {
    /// Every body, in the order the choice lists them.
    pub const ALL: [BodyKind; 6] = [
        BodyKind::Guitar,
        BodyKind::Violin,
        BodyKind::Cello,
        BodyKind::Soundboard,
        BodyKind::Dulcimer,
        BodyKind::Cabinet,
    ];

    fn of(value: f32) -> Self {
        Self::ALL[(value.round().max(0.0) as usize).min(Self::ALL.len() - 1)]
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|kind| *kind == self).unwrap_or(0)
    }

    /// The spec a body is laid out from, and the scale of its frequencies.
    fn spec(self) -> (&'static BodySpec, f32) {
        match self {
            BodyKind::Guitar => (&body::GUITAR, 1.0),
            BodyKind::Violin => (&body::VIOLIN, 1.0),
            // The bowed instruments scale their one body the same way.
            BodyKind::Cello => (&body::VIOLIN, 0.375),
            BodyKind::Soundboard => (&body::SOUNDBOARD, 1.0),
            BodyKind::Dulcimer => (&body::DULCIMER, 1.0),
            BodyKind::Cabinet => (&body::CABINET, 1.0),
        }
    }

    /// Where a loudspeaker's cone stops moving air, for a body that is one.
    ///
    /// A mode bank's skirts fall away at six decibels an octave however quiet its top modes
    /// are, which is right for wood and wrong for paper: above its breakup a cone simply stops
    /// radiating. Two second-order sections put that cliff in.
    fn cone_hz(self) -> Option<f32> {
        (self == BodyKind::Cabinet).then_some(5_000.0)
    }
}

const BODY_CHOICES: [Cow<'static, str>; 6] = [
    Cow::Borrowed("Guitar"),
    Cow::Borrowed("Violin"),
    Cow::Borrowed("Cello"),
    Cow::Borrowed("Piano soundboard"),
    Cow::Borrowed("Dulcimer"),
    Cow::Borrowed("Speaker cabinet"),
];

const BODY: u32 = 0;
const SIZE: u32 = 1;
const MIX: u32 = 2;
const WIDTH: u32 = 3;
const OUTPUT: u32 = 4;

/// A track put through an instrument's body or a speaker cabinet.
///
/// The body is the physical instruments' own: a bank of a hundred or more resonances laid out
/// from the instrument's few named ones and a seeded crowd above them, each heard the way the
/// bridge feels it. That is what an equaliser cannot do — every partial of every note falls on
/// its own mode, so neighbouring notes differ in colour the way they do on wood, and a vibrato
/// shimmers as it sweeps its harmonics across the peaks. Each mode leans to one side, so a mono
/// track comes out with the body's own width; the mono downmix is exactly the body without it.
///
/// *Size* moves every resonance together, a bigger box sounding lower, without laying the body
/// out again, so it can be turned while the track plays.
pub struct BodyResonance {
    params: ParamBank,
    /// One laid-out body per [`BodyKind`], so switching bodies never allocates.
    bodies: Vec<Radiator>,
    /// The body sounding, and the size it was last scaled to.
    active: Option<(BodyKind, f32)>,
    rate: f32,
    /// Frames into the current control tick.
    phase: usize,
    ticks: u32,
    mix: f32,
    gain: f32,
    input: Level,
    output: Level,
    response: [f32; RESONANCE_RESPONSE],
    response_stale: bool,
    /// A cone's top, mid and side, in two stages; identity for every body that is not a cone.
    cone: [[Biquad; 2]; 2],
    capture: ResonanceCapture,
}

impl Default for BodyResonance {
    fn default() -> Self {
        Self::new()
    }
}

impl BodyResonance {
    /// Stable plugin identifier stored in projects.
    pub const ID: &str = "auris.fx.body";

    /// A body effect on the guitar's box. Lays nothing out until `prepare`.
    pub fn new() -> Self {
        let params = ParamBank::new(vec![
            ParamDescriptor::new(BODY, "body", "Body", 0.0, 5.0, 0.0).with_choices(&BODY_CHOICES),
            ParamDescriptor::new(SIZE, "size", "Size", 0.5, 2.0, 1.0)
                .with_unit(ParamUnit::Ratio)
                .with_curve(ParamValueCurve::Logarithmic),
            ParamDescriptor::percent(MIX, "mix", "Mix", 0.8),
            ParamDescriptor::percent(WIDTH, "width", "Width", 0.6),
            ParamDescriptor::decibels(OUTPUT, "output_db", "Output", -24.0, 12.0, 0.0),
        ]);
        Self {
            params,
            bodies: Vec::new(),
            active: None,
            rate: 48_000.0,
            phase: 0,
            ticks: 0,
            mix: 0.8,
            gain: 1.0,
            input: Level::default(),
            output: Level::default(),
            response: [0.0; RESONANCE_RESPONSE],
            response_stale: true,
            cone: Default::default(),
            capture: ResonanceCapture::default(),
        }
    }

    fn cone(&self, kind: BodyKind, size: f32) -> BiquadCoefficients {
        match kind.cone_hz() {
            Some(hz) => BiquadCoefficients::lowpass(
                f64::from(self.rate),
                (hz / size.max(0.05)).min(self.rate * 0.45),
                std::f32::consts::FRAC_1_SQRT_2,
            ),
            None => BiquadCoefficients::identity(),
        }
    }

    fn wanted(&self) -> (BodyKind, f32) {
        (BodyKind::of(self.params.at(BODY)), self.params.at(SIZE))
    }

    /// Puts the chosen body in place at the chosen size. Allocates nothing.
    fn arrange(&mut self) {
        let (kind, size) = self.wanted();
        if self.active == Some((kind, size)) {
            return;
        }
        let switched = self.active.map(|(active, _)| active) != Some(kind);
        let Some(radiator) = self.bodies.get_mut(kind.index()) else {
            return;
        };
        if switched {
            radiator.reset();
        }
        // A bigger box sounds lower.
        radiator.rescale(1.0 / size.max(0.05));
        let cone = self.cone(kind, size);
        for filter in self.cone.iter_mut().flatten() {
            filter.set_coefficients(cone);
            if switched {
                filter.reset();
            }
        }
        self.active = Some((kind, size));
        self.response_stale = true;
    }

    fn publish(&mut self) {
        let Some((kind, size)) = self.active else {
            return;
        };
        let cone = self.cone(kind, size);
        let rate = f64::from(self.rate);
        let Some(radiator) = self.bodies.get(kind.index()) else {
            return;
        };
        if std::mem::take(&mut self.response_stale) {
            for (index, db) in self.response.iter_mut().enumerate() {
                let hz = response_hz(index);
                *db = radiator.response_db(hz) + 2.0 * cone.magnitude_db(hz, rate);
            }
        }
        let mut frame = ResonanceFrame {
            kind: ResonanceKind::Body,
            input: self.input.get(),
            output: self.output.get(),
            response: self.response,
            ..ResonanceFrame::default()
        };
        radiator.observe(&mut frame.modes);
        for (index, hz) in frame.mode_hz.iter_mut().enumerate() {
            *hz = if index < radiator.len() {
                radiator.mode_hz(index)
            } else {
                0.0
            };
        }
        self.capture.publish(&frame);
    }
}

impl Parameterized for BodyResonance {
    fn parameters(&self) -> &[ParamDescriptor] {
        self.params.descriptors()
    }

    fn param(&self, id: ParamId) -> f32 {
        self.params.get(id)
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        self.params.set(id, value);
    }
}

impl Effect for BodyResonance {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            Self::ID,
            "Body Resonance",
            "The box of a guitar, violin, cello, piano or speaker cabinet, as a bank of modes",
            PluginCategory::Reverb,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.rate = sane_rate(ctx.sample_rate);
        self.bodies = BodyKind::ALL
            .iter()
            .map(|kind| {
                let (spec, scale) = kind.spec();
                let mut radiator = Radiator::default();
                radiator.prepare(spec, scale, self.rate);
                radiator
            })
            .collect();
        self.active = None;
        self.reset();
    }

    fn reset(&mut self) {
        for radiator in &mut self.bodies {
            radiator.reset();
        }
        for filter in self.cone.iter_mut().flatten() {
            filter.reset();
        }
        self.phase = 0;
        self.ticks = 0;
        self.mix = self.params.at(MIX);
        self.gain = db_to_gain(self.params.at(OUTPUT));
        self.input.reset();
        self.output.reset();
    }

    fn process(&mut self, buffer: &mut AudioBuffer, ctx: &ProcessContext) {
        if self.bodies.is_empty() {
            return;
        }
        self.arrange();
        let Some((kind, _)) = self.active else {
            return;
        };
        let frames = buffer.frame_count().min(ctx.block_frames);
        let stereo = buffer.channel_count() >= 2;
        let width = self.params.at(WIDTH);
        let (mix_target, gain_target) = (self.params.at(MIX), db_to_gain(self.params.at(OUTPUT)));
        let mut start = 0;
        while start < frames {
            let length = (TICK - self.phase).min(frames - start);
            let (mut mid, mut side) = ([0.0_f32; TICK], [0.0_f32; TICK]);
            let mut heard = 0.0_f32;
            {
                let channels = buffer.channels_mut();
                for offset in 0..length {
                    let left = channels[0][start + offset];
                    let right = if stereo {
                        channels[1][start + offset]
                    } else {
                        left
                    };
                    let (left, right) = (finite(left), finite(right));
                    heard = heard.max(left.abs()).max(right.abs());
                    mid[offset] = 0.5 * (left + right);
                    side[offset] = 0.5 * (right - left);
                }
            }
            // The mix and the level move once a tick; what changes inside one is too small to
            // hear as a step at sixty-four frames.
            let (gain_from, mix_from) = (self.gain, self.mix);
            self.mix += 0.25 * (mix_target - self.mix);
            self.gain += 0.25 * (gain_target - self.gain);
            // The body alone, then the dry signal under it: the same sum the radiator makes
            // itself, with room between the two for a cone's top.
            let (dry_mid, dry_side) = (mid, side);
            if let Some(radiator) = self.bodies.get_mut(kind.index()) {
                radiator.process(&mut mid[..length], &mut side[..length], 1.0, width);
            }
            let [mid_cone, side_cone] = &mut self.cone;
            for offset in 0..length {
                let ramp = (offset + 1) as f32 / length as f32;
                let mix = mix_from + (self.mix - mix_from) * ramp;
                let wet_mid = mid_cone
                    .iter_mut()
                    .fold(mid[offset], |sample, filter| filter.process_sample(sample));
                let wet_side = side_cone
                    .iter_mut()
                    .fold(side[offset], |sample, filter| filter.process_sample(sample));
                mid[offset] = (1.0 - mix) * dry_mid[offset] + mix * wet_mid;
                side[offset] = (1.0 - mix) * dry_side[offset] + mix * wet_side;
            }
            let mut out = 0.0_f32;
            let channels = buffer.channels_mut();
            for offset in 0..length {
                let ramp = (offset + 1) as f32 / length as f32;
                let gain = gain_from + (self.gain - gain_from) * ramp;
                let (left, right) = (
                    (mid[offset] - side[offset]) * gain,
                    (mid[offset] + side[offset]) * gain,
                );
                out = out.max(left.abs()).max(right.abs());
                if stereo {
                    channels[0][start + offset] = left;
                    channels[1][start + offset] = right;
                } else {
                    channels[0][start + offset] = mid[offset] * gain;
                }
            }
            self.input.feed(heard);
            self.output.feed(out);
            self.phase += length;
            start += length;
            if self.phase == TICK {
                self.phase = 0;
                if let Some(radiator) = self.bodies.get_mut(kind.index()) {
                    radiator.tick();
                }
                self.ticks = self.ticks.wrapping_add(1);
                if self.capture.is_watched() && self.ticks.is_multiple_of(PUBLISH_TICKS) {
                    self.publish();
                }
            }
        }
    }

    fn tail_frames(&self) -> usize {
        // The longest-lived mode of any body, a guitar's air at Q 20, is down sixty decibels
        // in under half a second; a second and a half covers the largest size.
        (self.rate * 1.5) as usize
    }

    fn resonance_monitor(&self) -> Option<Arc<ResonanceMonitor>> {
        Some(self.capture.monitor())
    }
}

fn finite(sample: f32) -> f32 {
    if sample.is_finite() { sample } else { 0.0 }
}

/// Modes a frame describes, which is all [`Radiator::observe`] fills.
const _: () = assert!(RESONANCE_MODES <= 192);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::goertzel;

    const RATE: f64 = 48_000.0;

    fn prepared() -> BodyResonance {
        let mut effect = BodyResonance::new();
        effect.prepare(&PrepareContext::new(RATE, 512, 2));
        effect
    }

    /// An impulse through `effect`, mono, returned as the left channel and the right.
    fn impulse_response(effect: &mut BodyResonance, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let mut left = Vec::with_capacity(frames);
        let mut right = Vec::with_capacity(frames);
        let mut start = 0;
        while start < frames {
            let block = 512.min(frames - start);
            let mut buffer = AudioBuffer::stereo(block, RATE);
            if start == 0 {
                buffer.channel_mut(0)[0] = 1.0;
                buffer.channel_mut(1)[0] = 1.0;
            }
            effect.process(
                &mut buffer,
                &ProcessContext::realtime(RATE, block, start as u64, 120.0, true),
            );
            left.extend_from_slice(buffer.channel(0));
            right.extend_from_slice(buffer.channel(1));
            start += block;
        }
        (left, right)
    }

    #[test]
    fn the_guitar_box_rings_at_its_air_resonance_and_a_bigger_one_lower() {
        let mut effect = prepared();
        effect.set_param(ParamId(MIX), 1.0);
        let (left, right) = impulse_response(&mut effect, 24_000);
        let mid: Vec<f32> = left
            .iter()
            .zip(&right)
            .map(|(l, r)| 0.5 * (l + r))
            .collect();
        let at = |signal: &[f32], hz: f64| goertzel(signal, RATE, hz);
        assert!(at(&mid, 102.0) > at(&mid, 140.0) * 2.0);

        let mut bigger = prepared();
        bigger.set_param(ParamId(MIX), 1.0);
        bigger.set_param(ParamId(SIZE), 1.5);
        let (left, right) = impulse_response(&mut bigger, 24_000);
        let mid: Vec<f32> = left
            .iter()
            .zip(&right)
            .map(|(l, r)| 0.5 * (l + r))
            .collect();
        // The air resonance has moved from 102 Hz to 68.
        assert!(at(&mid, 68.0) > at(&mid, 102.0) * 1.5);
    }

    #[test]
    fn a_mono_source_comes_out_wide_but_folds_back_to_the_body() {
        let mut effect = prepared();
        effect.set_param(ParamId(MIX), 1.0);
        effect.set_param(ParamId(WIDTH), 1.0);
        let (left, right) = impulse_response(&mut effect, 12_000);
        let difference: f32 = left.iter().zip(&right).map(|(l, r)| (l - r).abs()).sum();
        assert!(difference > 1.0e-3, "a body with lean left the sides alike");

        let mut narrow = prepared();
        narrow.set_param(ParamId(MIX), 1.0);
        narrow.set_param(ParamId(WIDTH), 0.0);
        let (narrow_left, narrow_right) = impulse_response(&mut narrow, 12_000);
        for (l, r) in narrow_left.iter().zip(&narrow_right) {
            assert!((l - r).abs() < 1.0e-6);
        }
        // Width only adds a difference; the sum of the sides is the same body.
        for ((l, r), (nl, nr)) in left
            .iter()
            .zip(&right)
            .zip(narrow_left.iter().zip(&narrow_right))
        {
            assert!(((l + r) - (nl + nr)).abs() < 1.0e-5);
        }
    }

    #[test]
    fn a_dry_mix_passes_the_signal_untouched() {
        let mut effect = prepared();
        effect.set_param(ParamId(MIX), 0.0);
        effect.reset();
        let mut buffer = AudioBuffer::stereo(256, RATE);
        for (index, sample) in buffer.channel_mut(0).iter_mut().enumerate() {
            *sample = (index as f32 * 0.1).sin() * 0.5;
        }
        let original = buffer.clone();
        effect.process(
            &mut buffer,
            &ProcessContext::realtime(RATE, 256, 0, 120.0, true),
        );
        for channel in 0..2 {
            for (a, b) in buffer
                .channel(channel)
                .iter()
                .zip(original.channel(channel))
            {
                assert!((a - b).abs() < 1.0e-6);
            }
        }
    }

    #[test]
    fn a_watched_body_publishes_its_modes_and_its_response() {
        let mut effect = prepared();
        let monitor = effect.resonance_monitor().unwrap();
        monitor.watch(true);
        effect.set_param(ParamId(BODY), 5.0);
        let _ = impulse_response(&mut effect, 4_096);
        let frame = monitor.read().unwrap();
        assert_eq!(frame.kind, ResonanceKind::Body);
        assert!(
            (frame.mode_hz[0] - 95.0).abs() < 0.5,
            "{}",
            frame.mode_hz[0]
        );
        assert!(frame.modes.iter().any(|mode| *mode != 0.0));
        // A speaker cabinet stops radiating well before the top of the curve.
        let top = frame.response[RESONANCE_RESPONSE - 1];
        let middle = frame.response[RESONANCE_RESPONSE / 2];
        assert!(top < middle - 20.0, "top {top} middle {middle}");
    }

    #[test]
    fn switching_and_resizing_the_body_does_not_allocate_a_new_one() {
        let mut effect = prepared();
        let _ = impulse_response(&mut effect, 1_024);
        let before: Vec<*const Radiator> = effect.bodies.iter().map(|b| b as *const _).collect();
        effect.set_param(ParamId(BODY), 2.0);
        effect.set_param(ParamId(SIZE), 0.7);
        let _ = impulse_response(&mut effect, 1_024);
        let after: Vec<*const Radiator> = effect.bodies.iter().map(|b| b as *const _).collect();
        assert_eq!(before, after);
        assert_eq!(effect.active, Some((BodyKind::Cello, 0.7)));
    }
}
