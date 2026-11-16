//! Strings nobody plays, tuned to the song, ringing with whatever is played in tune.

use std::borrow::Cow;
use std::f32::consts::PI;
use std::sync::Arc;

use auris_core::param::{ParamUnit, ParamValueCurve, pan_gains};
use auris_core::resonance::{
    RESONANCE_POINTS, RESONANCE_STRINGS, ResonanceCapture, ResonanceFrame, ResonanceKind,
    ResonanceMonitor, SharedHarmony,
};
use auris_core::{
    AudioBuffer, Effect, ParamDescriptor, ParamId, Parameterized, PluginCategory, PluginDescriptor,
    PrepareContext, ProcessContext,
};

use super::{Level, PUBLISH_TICKS, TICK, sane_rate};
use crate::ParamBank;
use crate::physical::rotors::{LANES, decay_rate, pole};
use crate::physical::strings::partial_ratio;

/// Partials of each string, at most; fewer for a string whose upper ones would pass the top of
/// the band.
const PARTIALS: usize = 24;
const GROUPS: usize = PARTIALS / LANES;
const _: () = assert!(PARTIALS.is_multiple_of(LANES));

/// Where along the string the bridge drives it and listens to it, as a fraction of its length.
/// A string driven at its very end would answer every partial alike; a little way in, each
/// partial is weighed by how much it moves there, which is the colour of a bridge.
const BRIDGE: f32 = 0.11;
/// Stiffness of a thin steel string: what pushes its upper partials a little sharp.
const INHARMONICITY: f32 = 1.5e-4;
/// How long a string being retuned takes to be damped before it is: a finger on it for a
/// moment, so a chord change never sounds as a string sliding to its new note.
const DAMP_TICKS: u32 = 16;
/// Sixty-decibel decay time of a string while it is being damped.
const DAMP_SECONDS: f32 = 0.03;
/// Overall level of what the strings give back. The bridge weighs each partial twice, by the
/// drive and by the listening, and a string answers a sine only at its own harmonics; this
/// brings a held note on a string's own pitch back at about its own level.
const MAKEUP: f32 = 8.0;
/// Silent control ticks after which the strings are checked for having died away.
const IDLE_TICKS: u32 = 16;

/// What the strings are tuned to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tuning {
    /// The chord sounding at the playhead. Where none is written the last one rings on, and
    /// before the first, the key.
    SongChords,
    /// The scale of the key at the playhead.
    SongKey,
    /// The root and scale chosen on the effect itself.
    Fixed,
}

impl Tuning {
    fn of(value: f32) -> Self {
        match value.round() as i32 {
            1 => Tuning::SongKey,
            2 => Tuning::Fixed,
            _ => Tuning::SongChords,
        }
    }
}

const TUNING_CHOICES: [Cow<'static, str>; 3] = [
    Cow::Borrowed("Song chords"),
    Cow::Borrowed("Song key"),
    Cow::Borrowed("Fixed"),
];

const ROOT_CHOICES: [Cow<'static, str>; 12] = [
    Cow::Borrowed("C"),
    Cow::Borrowed("C#"),
    Cow::Borrowed("D"),
    Cow::Borrowed("D#"),
    Cow::Borrowed("E"),
    Cow::Borrowed("F"),
    Cow::Borrowed("F#"),
    Cow::Borrowed("G"),
    Cow::Borrowed("G#"),
    Cow::Borrowed("A"),
    Cow::Borrowed("A#"),
    Cow::Borrowed("B"),
];

const SCALE_CHOICES: [Cow<'static, str>; 4] = [
    Cow::Borrowed("Drone"),
    Cow::Borrowed("Major"),
    Cow::Borrowed("Minor"),
    Cow::Borrowed("Pentatonic"),
];

/// Pitch classes of each fixed scale above its root, bit 0 being the root.
const SCALE_MASKS: [u16; 4] = [
    // The root and its fifth, as a tanpura or a sitar's chikari strings are.
    0b0000_1000_0001,
    0b1010_1011_0101,
    0b0101_1010_1101,
    0b0010_1001_0101,
];

const TUNING: u32 = 0;
const ROOT: u32 = 1;
const SCALE: u32 = 2;
const STRINGS: u32 = 3;
const OCTAVE: u32 = 4;
const DECAY: u32 = 5;
const BRIGHTNESS: u32 = 6;
const MIX: u32 = 7;
const WIDTH: u32 = 8;

type Lane = [f32; LANES];

/// What every string is set to, so a change of controls can be noticed.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Voicing {
    decay: f32,
    brightness: f32,
    width: f32,
    count: usize,
}

/// A bank of sympathetic strings beside a track.
///
/// Up to sixteen strings, none of them played, each a row of damped phasors at its harmonics —
/// slightly stretched, as a steel string's are — driven through a bridge by the track. A string
/// answers only what falls on its own partials, and builds up for as long as that keeps
/// sounding: a melody that dwells on the notes they are tuned to leaves a halo behind it, and
/// one that does not leaves them nearly still. That is the whole difference from a reverb, which
/// answers everything alike.
///
/// The strings are tuned from the lowest one up to the notes of the chord sounding at the
/// playhead, or the key's scale, or a root and scale of their own. When the harmony moves on,
/// each string whose note changes is damped for a moment and retuned, so the halo follows the
/// song without sliding.
pub struct SympatheticStrings {
    params: ParamBank,
    rate: f32,
    re: Vec<Lane>,
    im: Vec<Lane>,
    cosine: Vec<Lane>,
    sine: Vec<Lane>,
    drive: Vec<Lane>,
    left: Vec<Lane>,
    right: Vec<Lane>,
    /// The note each string is tuned to now, and the one it is being damped to be retuned to.
    pitch: [f32; RESONANCE_STRINGS],
    pending: [Option<f32>; RESONANCE_STRINGS],
    /// Control ticks left of each string's damping.
    damping: [u32; RESONANCE_STRINGS],
    voicing: Option<Voicing>,
    root: i32,
    classes: u16,
    harmony: Option<SharedHarmony>,
    phase: usize,
    ticks: u32,
    idle: u32,
    heard: bool,
    asleep: bool,
    mix: f32,
    input: Level,
    output: Level,
    capture: ResonanceCapture,
}

impl Default for SympatheticStrings {
    fn default() -> Self {
        Self::new()
    }
}

impl SympatheticStrings {
    /// Stable plugin identifier stored in projects.
    pub const ID: &str = "auris.fx.sympathetic";

    /// Eleven strings from the D below middle C, tuned to the song's chords.
    pub fn new() -> Self {
        let params = ParamBank::new(vec![
            ParamDescriptor::new(TUNING, "tuning", "Tuning", 0.0, 2.0, 0.0)
                .with_choices(&TUNING_CHOICES),
            ParamDescriptor::new(ROOT, "root", "Root", 0.0, 11.0, 2.0).with_choices(&ROOT_CHOICES),
            ParamDescriptor::new(SCALE, "scale", "Scale", 0.0, 3.0, 0.0)
                .with_choices(&SCALE_CHOICES),
            ParamDescriptor::new(
                STRINGS,
                "strings",
                "Strings",
                4.0,
                RESONANCE_STRINGS as f32,
                11.0,
            )
            .with_steps(RESONANCE_STRINGS as u32 - 3),
            ParamDescriptor::new(OCTAVE, "octave", "Lowest Octave", 1.0, 5.0, 3.0).with_steps(5),
            ParamDescriptor::new(DECAY, "decay", "String Decay", 0.5, 12.0, 5.0)
                .with_unit(ParamUnit::Seconds)
                .with_curve(ParamValueCurve::Logarithmic),
            ParamDescriptor::percent(BRIGHTNESS, "brightness", "Brightness", 0.5),
            ParamDescriptor::percent(MIX, "mix", "Mix", 0.5),
            ParamDescriptor::percent(WIDTH, "width", "Width", 0.7),
        ]);
        Self {
            params,
            rate: 48_000.0,
            re: Vec::new(),
            im: Vec::new(),
            cosine: Vec::new(),
            sine: Vec::new(),
            drive: Vec::new(),
            left: Vec::new(),
            right: Vec::new(),
            pitch: [0.0; RESONANCE_STRINGS],
            pending: [None; RESONANCE_STRINGS],
            damping: [0; RESONANCE_STRINGS],
            voicing: None,
            root: -1,
            classes: 0,
            harmony: None,
            phase: 0,
            ticks: 0,
            idle: 0,
            heard: false,
            asleep: true,
            mix: 0.5,
            input: Level::default(),
            output: Level::default(),
            capture: ResonanceCapture::default(),
        }
    }

    fn count(&self) -> usize {
        (self.params.at(STRINGS).round() as usize).clamp(1, RESONANCE_STRINGS)
    }

    /// The pitch classes and root the strings should be tuned to at `frame`.
    fn harmony_at(&self, frame: u64) -> (i32, u16) {
        let fixed = || {
            let root = self.params.at(ROOT).round() as i32;
            let scale = (self.params.at(SCALE).round() as usize).min(SCALE_MASKS.len() - 1);
            (root, rotate(SCALE_MASKS[scale], root))
        };
        let tuning = Tuning::of(self.params.at(TUNING));
        let Some(span) = self
            .harmony
            .as_deref()
            .filter(|_| tuning != Tuning::Fixed)
            .and_then(|timeline| timeline.at(frame))
        else {
            return fixed();
        };
        match (tuning, span.chord_root) {
            (Tuning::SongChords, Some(root)) if span.chord_classes != 0 => {
                (i32::from(root), span.chord_classes)
            }
            _ => (i32::from(span.key_root), span.key_classes),
        }
    }

    /// Configures string `index` at `pitch`, keeping where its phasors are.
    fn configure(&mut self, index: usize, pitch: f32, voicing: Voicing) {
        let hz = 440.0 * 2.0_f32.powf((pitch - 69.0) / 12.0);
        let limit = self.rate * 0.45;
        // Brighter strings lose their upper partials more slowly and give them back louder.
        let loss = 0.02 + 0.2 * (1.0 - voicing.brightness);
        let tilt = 1.1 - 0.8 * voicing.brightness;
        let place = if voicing.count > 1 {
            index as f32 / (voicing.count - 1) as f32 * 2.0 - 1.0
        } else {
            0.0
        };
        let (pan_left, pan_right) = pan_gains(place * voicing.width);
        let sounding = index < voicing.count;
        for partial in 0..PARTIALS {
            let (group, lane) = (index * GROUPS + partial / LANES, partial % LANES);
            let number = (partial + 1) as f32;
            let at = hz * partial_ratio(number, INHARMONICITY);
            let (cosine, sine, drive, listen) = if sounding && at < limit {
                let seconds = voicing.decay / (1.0 + loss * (number - 1.0));
                let (cosine, sine) = pole(at, decay_rate(seconds), self.rate);
                let radius = cosine.hypot(sine);
                let shape = (PI * number * BRIDGE).sin();
                (
                    cosine,
                    sine,
                    2.0 * (1.0 - radius) * shape,
                    MAKEUP * shape * number.powf(-tilt),
                )
            } else {
                (0.0, 0.0, 0.0, 0.0)
            };
            let slots = [
                (&mut self.cosine, cosine),
                (&mut self.sine, sine),
                (&mut self.drive, drive),
                (&mut self.left, listen * pan_left),
                (&mut self.right, listen * pan_right),
            ];
            for (lanes, value) in slots {
                if let Some(slot) = lanes.get_mut(group) {
                    slot[lane] = value;
                }
            }
        }
        if let Some(slot) = self.pitch.get_mut(index) {
            *slot = pitch;
        }
    }

    /// Lays a finger on string `index`: its phasors shrink towards rest from here on.
    fn damp(&mut self, index: usize) {
        let radius = (-decay_rate(DAMP_SECONDS) / self.rate).exp();
        for group in index * GROUPS..(index + 1) * GROUPS {
            let (Some(cosine), Some(sine)) = (self.cosine.get_mut(group), self.sine.get_mut(group))
            else {
                continue;
            };
            for lane in 0..LANES {
                let now = cosine[lane].hypot(sine[lane]);
                if now > radius {
                    let factor = radius / now;
                    cosine[lane] *= factor;
                    sine[lane] *= factor;
                }
            }
        }
    }

    fn energy(&self, index: usize) -> f32 {
        let groups = index * GROUPS..(index + 1) * GROUPS;
        self.re
            .get(groups.clone())
            .into_iter()
            .flatten()
            .chain(self.im.get(groups).into_iter().flatten())
            .flat_map(|lane| lane.iter())
            .map(|value| value * value)
            .sum()
    }

    /// Once a tick: follows the harmony and the controls, and finishes any retuning that is due.
    fn control(&mut self, frame: u64) {
        let voicing = Voicing {
            decay: self.params.at(DECAY),
            brightness: self.params.at(BRIGHTNESS),
            width: self.params.at(WIDTH),
            count: self.count(),
        };
        let (root, classes) = self.harmony_at(frame);
        let octave = self.params.at(OCTAVE).round() as i32;
        let mut notes = [0.0_f32; RESONANCE_STRINGS];
        tune(classes, 12 * (octave + 1), &mut notes[..voicing.count]);
        let wanted = assign(&self.pitch[..voicing.count], &notes[..voicing.count]);
        let revoiced = self.voicing != Some(voicing);
        let first = self.voicing.is_none();
        self.voicing = Some(voicing);
        (self.root, self.classes) = (root, classes);
        for index in 0..RESONANCE_STRINGS {
            if self.damping[index] > 0 {
                self.damping[index] -= 1;
                if self.damping[index] == 0
                    && let Some(pitch) = self.pending[index].take()
                {
                    self.configure(index, pitch, voicing);
                }
                // A string still being damped is retuned when it is done, to whatever is
                // wanted then.
                if index < voicing.count {
                    self.pending[index] = Some(wanted[index]);
                }
                continue;
            }
            let target = if index < voicing.count {
                wanted[index]
            } else {
                self.pitch[index]
            };
            let moved = (target - self.pitch[index]).abs() > 1.0e-3;
            if moved && !first && self.energy(index) > 1.0e-10 {
                self.pending[index] = Some(target);
                self.damping[index] = DAMP_TICKS;
                self.damp(index);
            } else if moved || revoiced {
                self.configure(index, target, voicing);
            }
        }
    }

    fn settle(&mut self) -> bool {
        let mut ringing = false;
        for (re, im) in self.re.iter_mut().zip(self.im.iter_mut()) {
            let moving: f32 = re.iter().chain(im.iter()).map(|value| value.abs()).sum();
            if moving.is_finite() && moving > 1.0e-12 {
                ringing = true;
            } else {
                *re = [0.0; LANES];
                *im = [0.0; LANES];
            }
        }
        ringing
    }

    fn publish(&mut self) {
        let count = self.count();
        let mut frame = ResonanceFrame {
            kind: ResonanceKind::Strings,
            input: self.input.get(),
            output: self.output.get(),
            root: self.root,
            classes: self.classes,
            string_count: count,
            ..ResonanceFrame::default()
        };
        for (index, string) in frame.strings.iter_mut().enumerate().take(count) {
            string.pitch = self.pitch[index];
            let mut power = 0.0;
            for partial in 0..PARTIALS {
                let (group, lane) = (index * GROUPS + partial / LANES, partial % LANES);
                let displacement = self.im.get(group).map_or(0.0, |lanes| lanes[lane]);
                power += displacement * displacement;
                // A partial's shape along the string, from the nut to the bridge.
                let number = (partial + 1) as f32;
                for (point, value) in string.points.iter_mut().enumerate() {
                    let at = point as f32 / (RESONANCE_POINTS - 1) as f32;
                    *value += displacement * (PI * number * at).sin();
                }
            }
            string.level = power.sqrt();
        }
        self.capture.publish(&frame);
    }
}

/// `mask` turned so that bit 0 lands on pitch class `root`.
fn rotate(mask: u16, root: i32) -> u16 {
    let shift = root.rem_euclid(12) as u32;
    let wide = u32::from(mask & 0x0FFF) << shift;
    ((wide | (wide >> 12)) & 0x0FFF) as u16
}

/// Which of `notes` each string tuned to `current` should take, moving as few strings as it
/// can: a string already on one of the notes keeps it, and the rest take the notes left over,
/// lowest string to lowest note. A chord change is heard as the strings it actually changes
/// being damped and retuned, and the others ringing on through it, as the common tones of two
/// chords do on a real instrument.
fn assign(current: &[f32], notes: &[f32]) -> [f32; RESONANCE_STRINGS] {
    let mut out = [0.0_f32; RESONANCE_STRINGS];
    let count = current.len().min(notes.len()).min(RESONANCE_STRINGS);
    let mut taken = [false; RESONANCE_STRINGS];
    let mut placed = [false; RESONANCE_STRINGS];
    for (string, pitch) in current.iter().enumerate().take(count) {
        if let Some(note) = (0..count).find(|note| !taken[*note] && notes[*note] == *pitch) {
            taken[note] = true;
            placed[string] = true;
            out[string] = *pitch;
        }
    }
    let mut free = (0..count).filter(|note| !taken[*note]);
    for string in 0..count {
        if !placed[string] {
            out[string] = free.next().map_or(notes[string], |note| notes[note]);
        }
    }
    out
}

/// Fills `out` with the notes of `classes` from `lowest` upwards, one string each.
fn tune(classes: u16, lowest: i32, out: &mut [f32]) {
    let classes = if classes & 0x0FFF == 0 { 1 } else { classes };
    let mut note = lowest;
    for slot in out.iter_mut() {
        while classes & (1 << note.rem_euclid(12)) == 0 {
            note += 1;
        }
        *slot = note as f32;
        note += 1;
    }
}

impl Parameterized for SympatheticStrings {
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

impl Effect for SympatheticStrings {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            Self::ID,
            "Sympathetic Strings",
            "Unplayed strings tuned to the song's chords or key, ringing with what is played in tune",
            PluginCategory::Reverb,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.rate = sane_rate(ctx.sample_rate);
        let groups = RESONANCE_STRINGS * GROUPS;
        for lanes in [
            &mut self.re,
            &mut self.im,
            &mut self.cosine,
            &mut self.sine,
            &mut self.drive,
            &mut self.left,
            &mut self.right,
        ] {
            lanes.clear();
            lanes.resize(groups, [0.0; LANES]);
        }
        self.voicing = None;
        self.reset();
    }

    fn reset(&mut self) {
        self.re.fill([0.0; LANES]);
        self.im.fill([0.0; LANES]);
        self.pending = [None; RESONANCE_STRINGS];
        self.damping = [0; RESONANCE_STRINGS];
        // Retuned from scratch on the next tick, poles and all.
        self.voicing = None;
        self.phase = 0;
        self.ticks = 0;
        self.idle = 0;
        self.heard = false;
        self.asleep = true;
        self.mix = self.params.at(MIX);
        self.input.reset();
        self.output.reset();
    }

    fn process(&mut self, buffer: &mut AudioBuffer, ctx: &ProcessContext) {
        if self.re.is_empty() {
            return;
        }
        let frames = buffer.frame_count().min(ctx.block_frames);
        let stereo = buffer.channel_count() >= 2;
        let groups = RESONANCE_STRINGS * GROUPS;
        let mut start = 0;
        while start < frames {
            if self.phase == 0 {
                self.control(ctx.playhead_samples + start as u64);
            }
            let length = (TICK - self.phase).min(frames - start);
            let mix_from = self.mix;
            self.mix += 0.25 * (self.params.at(MIX) - self.mix);
            let (mut heard, mut out) = (0.0_f32, 0.0_f32);
            let channels = buffer.channels_mut();
            for offset in 0..length {
                let frame = start + offset;
                let left = finite(channels[0][frame]);
                let right = if stereo {
                    finite(channels[1][frame])
                } else {
                    left
                };
                let push = 0.5 * (left + right);
                heard = heard.max(push.abs());
                if push != 0.0 {
                    self.asleep = false;
                }
                if self.asleep {
                    continue;
                }
                let (mut wet_left, mut wet_right) = ([0.0_f32; LANES], [0.0_f32; LANES]);
                let state = self.re.iter_mut().zip(self.im.iter_mut());
                let poles = self.cosine.iter().zip(self.sine.iter());
                let ears = self.left.iter().zip(self.right.iter());
                for ((((re, im), (cosine, sine)), (to_left, to_right)), drive) in state
                    .zip(poles)
                    .zip(ears)
                    .zip(self.drive.iter())
                    .take(groups)
                {
                    for lane in 0..LANES {
                        let turned =
                            cosine[lane] * re[lane] - sine[lane] * im[lane] + drive[lane] * push;
                        im[lane] = sine[lane] * re[lane] + cosine[lane] * im[lane];
                        re[lane] = turned;
                        wet_left[lane] += to_left[lane] * turned;
                        wet_right[lane] += to_right[lane] * turned;
                    }
                }
                let ramp = (offset + 1) as f32 / length as f32;
                let mix = mix_from + (self.mix - mix_from) * ramp;
                let (wet_left, wet_right) = (
                    wet_left.iter().sum::<f32>() * mix,
                    wet_right.iter().sum::<f32>() * mix,
                );
                out = out.max(wet_left.abs()).max(wet_right.abs());
                if stereo {
                    channels[0][frame] = left + wet_left;
                    channels[1][frame] = right + wet_right;
                } else {
                    channels[0][frame] = left + 0.5 * (wet_left + wet_right);
                }
            }
            if heard > 0.0 {
                self.heard = true;
            }
            self.input.feed(heard);
            self.output.feed(out);
            self.phase += length;
            start += length;
            if self.phase == TICK {
                self.phase = 0;
                self.ticks = self.ticks.wrapping_add(1);
                if std::mem::take(&mut self.heard) {
                    self.idle = 0;
                } else if !self.asleep {
                    self.idle += 1;
                    if self.idle >= IDLE_TICKS {
                        self.idle = 0;
                        self.asleep = !self.settle();
                    }
                }
                if self.capture.is_watched() && self.ticks.is_multiple_of(PUBLISH_TICKS) {
                    self.publish();
                }
            }
        }
    }

    fn tail_frames(&self) -> usize {
        // The fundamental's sixty-decibel decay is the longest of any partial's.
        (self.params.at(DECAY) * 1.2 * self.rate) as usize
    }

    fn resonance_monitor(&self) -> Option<Arc<ResonanceMonitor>> {
        Some(self.capture.monitor())
    }

    fn follows_harmony(&self) -> bool {
        true
    }

    fn set_harmony(&mut self, harmony: SharedHarmony) {
        self.harmony = Some(harmony);
    }
}

fn finite(sample: f32) -> f32 {
    if sample.is_finite() { sample } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::rms;
    use auris_core::resonance::HarmonyTimeline;

    const RATE: f64 = 48_000.0;

    fn prepared() -> SympatheticStrings {
        let mut effect = SympatheticStrings::new();
        effect.prepare(&PrepareContext::new(RATE, 512, 2));
        effect
    }

    fn hz(pitch: f32) -> f32 {
        440.0 * 2.0_f32.powf((pitch - 69.0) / 12.0)
    }

    /// Plays `pitch` as a bright tone for `held` frames and then silence for `after`, returning
    /// what the strings left behind once the tone stopped, as the left channel.
    fn ring_after(
        effect: &mut SympatheticStrings,
        pitch: f32,
        held: usize,
        after: usize,
    ) -> Vec<f32> {
        let step = std::f32::consts::TAU * hz(pitch) / RATE as f32;
        let mut tail = Vec::new();
        let mut start = 0;
        while start < held + after {
            let block = 256.min(held + after - start);
            let mut buffer = AudioBuffer::stereo(block, RATE);
            for channel in 0..2 {
                for (offset, sample) in buffer.channel_mut(channel).iter_mut().enumerate() {
                    let n = start + offset;
                    if n < held {
                        let phase = step * n as f32;
                        *sample = 0.3 * (phase.sin() + 0.5 * (2.0 * phase).sin());
                    }
                }
            }
            effect.process(
                &mut buffer,
                &ProcessContext::realtime(RATE, block, start as u64, 120.0, true),
            );
            for (offset, sample) in buffer.channel(0).iter().enumerate() {
                if start + offset >= held {
                    tail.push(*sample);
                }
            }
            start += block;
        }
        tail
    }

    #[test]
    fn scales_turn_to_their_root_and_strings_climb_through_them() {
        assert_eq!(rotate(SCALE_MASKS[0], 2), 0b0000_0010_0000_0100 & 0x0FFF);
        assert_eq!(rotate(0b1, 11), 1 << 11);
        let mut strings = [0.0; 5];
        // C major's triad from C3.
        tune(0b0000_1001_0001, 48, &mut strings);
        assert_eq!(strings, [48.0, 52.0, 55.0, 60.0, 64.0]);
    }

    #[test]
    fn a_chord_change_moves_only_the_strings_it_has_to() {
        // D major to A major over five strings from D3: D F# A D F# becomes the A strings kept
        // and the rest taking C#, E and the A above.
        let mut d = [0.0; 5];
        tune(1 << 2 | 1 << 6 | 1 << 9, 50, &mut d);
        assert_eq!(d, [50.0, 54.0, 57.0, 62.0, 66.0]);
        let mut a = [0.0; 5];
        tune(1 << 9 | 1 << 1 | 1 << 4, 48, &mut a);
        assert_eq!(a, [49.0, 52.0, 57.0, 61.0, 64.0]);
        let moved = assign(&d, &a);
        assert_eq!(moved[2], 57.0, "the common tone stays where it is");
        let mut sorted = moved[..5].to_vec();
        sorted.sort_by(f32::total_cmp);
        assert_eq!(sorted, a);
    }

    #[test]
    fn a_note_in_tune_leaves_the_strings_ringing_and_one_out_of_tune_does_not() {
        // Fixed on a D drone: D and A from D3 upwards.
        let mut effect = prepared();
        effect.set_param(ParamId(TUNING), 2.0);
        effect.set_param(ParamId(MIX), 1.0);
        let in_tune = ring_after(&mut effect, 50.0, 48_000, 24_000);
        effect.reset();
        let out_of_tune = ring_after(&mut effect, 51.0, 48_000, 24_000);
        let (a, b) = (rms(&in_tune[..12_000]), rms(&out_of_tune[..12_000]));
        assert!(a > 10.0 * b, "in tune {a}, out of tune {b}");
        // Ringing, not a burst: the halo is still there half a second after the note stopped.
        assert!(rms(&in_tune[12_000..24_000]) > 0.3 * rms(&in_tune[..12_000]));
        // And at a level that is heard beside the note: within twenty decibels of its 0.24 RMS.
        assert!(a > 0.024, "{a}");
    }

    #[test]
    fn the_strings_follow_the_chords_of_the_song() {
        let mut effect = prepared();
        effect.set_param(ParamId(MIX), 1.0);
        // G major from the start, so a G is in tune and a C# is not.
        let mut harmony =
            auris_core::Harmony::in_key(auris_core::theory::key::Key::parse("C major").unwrap());
        harmony.chords.set_point(
            auris_core::Ticks::ZERO,
            Some(auris_core::theory::numeral::Numeral::parse("V").unwrap()),
        );
        let timeline = HarmonyTimeline::of(&harmony, &auris_core::TempoMap::constant(120.0), RATE);
        effect.set_harmony(Arc::new(timeline));
        let on_chord = rms(&ring_after(&mut effect, 55.0, 48_000, 12_000));
        effect.reset();
        let off_chord = rms(&ring_after(&mut effect, 61.0, 48_000, 12_000));
        assert!(on_chord > 10.0 * off_chord, "{on_chord} {off_chord}");
        effect.control(0);
        assert_eq!(effect.root, 7);
        assert_eq!(
            effect.pitch[0], 50.0,
            "D3 is the lowest note of G major from C3"
        );
    }

    #[test]
    fn a_retuned_string_is_damped_first_and_then_takes_its_new_note() {
        let mut effect = prepared();
        effect.set_param(ParamId(TUNING), 2.0);
        let _ = ring_after(&mut effect, 50.0, 24_000, 0);
        assert_eq!(effect.pitch[0], 50.0);
        // E drone: the lowest string moves from D3 to E3.
        effect.set_param(ParamId(ROOT), 4.0);
        effect.control(0);
        assert_eq!(effect.damping[0], DAMP_TICKS);
        assert_eq!(effect.pitch[0], 50.0);
        for _ in 0..DAMP_TICKS {
            effect.control(0);
        }
        assert_eq!(effect.pitch[0], 52.0);
        assert_eq!(effect.damping[0], 0);
    }

    #[test]
    fn a_watched_bank_publishes_its_strings() {
        let mut effect = prepared();
        let monitor = effect.resonance_monitor().unwrap();
        monitor.watch(true);
        effect.set_param(ParamId(TUNING), 2.0);
        let _ = ring_after(&mut effect, 50.0, 24_000, 0);
        let frame = monitor.read().unwrap();
        assert_eq!(frame.kind, ResonanceKind::Strings);
        assert_eq!(frame.string_count, 11);
        assert_eq!(frame.strings[0].pitch, 50.0);
        assert!(frame.strings[0].level > frame.strings[1].level);
        assert!(frame.strings[0].points.iter().any(|point| *point != 0.0));
        assert_eq!(frame.root, 2);
        assert_eq!(frame.classes, rotate(SCALE_MASKS[0], 2));
    }

    #[test]
    fn silence_in_is_silence_out_and_the_strings_go_to_sleep() {
        let mut effect = prepared();
        effect.set_param(ParamId(DECAY), 1.0);
        let tail = ring_after(&mut effect, 50.0, 4_800, (RATE * 4.0) as usize);
        assert!(
            tail[tail.len() - 4_800..]
                .iter()
                .all(|sample| *sample == 0.0)
        );
        assert!(effect.asleep);
    }
}
