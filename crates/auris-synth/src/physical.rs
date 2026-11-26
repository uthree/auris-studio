//! Sample-free instruments built from the parts a real one is made of.
//!
//! Every instrument here is two things: something that *resonates* and something that *sets it
//! going*. What resonates is linear, and a linear resonator is a set of modes, each a damped
//! rotating phasor — a string's partials, a bar's bending modes, a bell's hum and tierce, a
//! soundboard's hundred-odd resonances are all advanced by the one loop in `rotors`. What sets
//! it going is not linear, and that is where an instrument's character is: a felt hammer that
//! stiffens as it is driven into the string, a bow that grips and lets go once a period, a reed
//! that closes against its mouthpiece, an air jet that flips either side of an edge. Each of
//! those is solved sample by sample, against the resonator it is driving.
//!
//! | Model | What resonates | What sets it going |
//! |---|---|---|
//! | Piano, Hammered Strings | stiff strings in bridge-coupled unisons, on a soundboard | a felt or wooden hammer, solved against the string it strikes |
//! | Guitar, Electric Guitar, Bass | a string in two polarisations, on a body or under a pickup | a finger or a pick, let go; on a bass, a thumb thrown at the string and the frets it comes down on |
//! | Strings | a travelling-wave string per player, on the body of its register | a bow: stick, slip, and the hysteresis between them |
//! | Mallet | a tuned bar over a resonator tube | a yarn or rubber mallet's Hertzian contact |
//! | Bell | the paired modes of a shell | a clapper or a hammer |
//! | Reeds | an air column closed at the reed | the reed as a pressure-controlled valve |
//! | Whistles | an air column open at both ends | a jet across the window, delayed by its own flight |
//!
//! Nothing is sampled, and no spectrum is shaped to match a recording's. The numbers are the
//! kind a maker or a measurement gives — a string's stiffness, a hammer's mass, a plate's first
//! resonances, how long a piano's notes take to die away — and the sound is what follows from
//! them.
//!
//! The polyphonic shell around the engines — voices, MIDI, placement in the stereo field,
//! quality, and the picture a frontend draws — is [`Physical`], one type for all of them.

use std::borrow::Cow;

use auris_core::motion::{
    MOTION_BODY, MOTION_PARTIALS, MOTION_SCOPE, MotionCapture, MotionFrame, MotionGeometry,
    MotionVoice, TOUCH_CONTROLLER,
};
use auris_core::param::db_to_gain;
use auris_core::{
    AudioBuffer, Instrument, NoteEvent, ParamDescriptor, ParamId, ParamUnit, Parameterized,
    PluginCategory, PluginDescriptor, PrepareContext, ProcessContext,
};

use crate::observe::recent_voices;
use crate::params::finite_or;
use crate::render::{StereoImage, downmix_to_extra_channels};
use crate::voice::VoiceState;
use crate::{ParamBank, SegmentRenderer, VoiceAllocator, render_segments};

mod bars;
mod bells;
pub(crate) mod body;
mod bowed;
mod flutes;
mod hammer;
mod legato;
mod plucked;
mod reeds;
pub(crate) mod rotors;
mod slap;
mod stretch;
pub(crate) mod strings;
mod struck;
mod unison;
mod waveguide;

#[cfg(test)]
mod tests;

use legato::Legato;

/// Samples between control updates: 0.67 ms at 48 kHz.
///
/// Envelopes, vibrato, pedals and the bookkeeping of which voices have died away move on this
/// clock, which counts samples since the instrument was reset rather than since the host's last
/// block. A render is therefore the same whatever block size the host asks for.
pub(crate) const TICK: usize = 32;

/// How finely a physical model is simulated.
///
/// Every instrument has two of these controls: one for live playback, where the audio callback's
/// budget is the limit, and one for offline rendering (export, freeze, bounce), where nothing but
/// time is. A level is chosen when a note starts and kept for that note, so changing either
/// control never disturbs a sounding note and never allocates.
///
/// | Model | Draft | Standard | High |
/// |---|---|---|---|
/// | Piano, Hammered Strings | half the partials | every partial that can be heard | partials up to the band edge |
/// | Guitar, Electric Guitar, Bass | half the partials | as the piano | as the piano |
/// | Mallet, Bell | the lowest modes | every mode | as Standard |
/// | Strings, Reeds, Whistles | the loop at the host rate | the loop at twice the host rate | as Standard |
///
/// The bow, the reed and the jet are discontinuous: a string lets go of the bow in a single
/// sample. Solved at the host rate, what that puts above the band edge folds back onto it as a
/// faint inharmonic grit under high notes; solved at twice the rate and brought back through a
/// half-band filter, it is not there. That is what Standard buys over Draft for those models,
/// and it roughly doubles their cost.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Quality {
    /// Cheaper than the full model, for dense live arrangements on a slow machine.
    Draft,
    /// The full model.
    #[default]
    Standard,
    /// More of the physics than a live callback can always afford; the default for rendering.
    High,
}

impl Quality {
    /// Choice labels in parameter-value order.
    pub const CHOICES: &'static [Cow<'static, str>] = &[
        Cow::Borrowed("Draft"),
        Cow::Borrowed("Standard"),
        Cow::Borrowed("High"),
    ];

    /// The level a choice parameter's value names, rounding to the nearest step.
    pub fn from_value(value: f32) -> Self {
        match finite_or(value, 1.0).round() as i32 {
            i32::MIN..=0 => Self::Draft,
            1 => Self::Standard,
            _ => Self::High,
        }
    }

    /// The choice parameter's value for this level.
    pub const fn value(self) -> f32 {
        match self {
            Self::Draft => 0.0,
            Self::Standard => 1.0,
            Self::High => 2.0,
        }
    }
}

/// Which instrument a [`Physical`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Model {
    /// A grand piano: felt hammers, unison strings, dampers and a soundboard.
    Piano,
    /// A hammered dulcimer: light hammers on courses of strings over a shallow box.
    HammeredDulcimer,
    /// An acoustic guitar: plucked strings on a resonant body.
    Guitar,
    /// A solid-body electric guitar heard through its pickup, before any amplifier.
    ElectricGuitar,
    /// A bass, from a pickup under roundwound strings to an upright's body.
    Bass,
    /// Bowed strings: a section seated by register, from basses to violins.
    Violin,
    /// Tuned bars: marimba, vibraphone, xylophone, glockenspiel and a comb of tines.
    Mallet,
    /// Bells: a cast bell, a tubular chime, a handbell and a singing bowl.
    Bell,
    /// Reed woodwinds, from a clarinet's cylinder to a saxophone's cone.
    Clarinet,
    /// Whistles and flutes: an air jet across the open end of a pipe.
    TinWhistle,
}

impl Model {
    /// Every physical instrument, ordered by how it is set going.
    pub const ALL: [Self; 10] = [
        Self::Piano,
        Self::HammeredDulcimer,
        Self::Guitar,
        Self::ElectricGuitar,
        Self::Bass,
        Self::Violin,
        Self::Mallet,
        Self::Bell,
        Self::Clarinet,
        Self::TinWhistle,
    ];

    /// Stable plugin id stored in a project.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Piano => "auris.physical.piano",
            Self::HammeredDulcimer => "auris.physical.hammered_dulcimer",
            Self::Guitar => "auris.physical.guitar",
            Self::ElectricGuitar => "auris.physical.electric_guitar",
            Self::Bass => "auris.physical.bass",
            Self::Violin => "auris.physical.violin",
            Self::Mallet => "auris.physical.mallet",
            Self::Bell => "auris.physical.bell",
            Self::Clarinet => "auris.physical.clarinet",
            Self::TinWhistle => "auris.physical.tin_whistle",
        }
    }

    /// Whether this instrument has dampers that the sustain pedal (controller 64) lifts: the
    /// piano, the hammered strings, the tuned bars and the bells. A keyboard plays the pedal to
    /// these rather than holding their notes for them.
    pub const fn has_dampers(self) -> bool {
        matches!(
            self,
            Self::Piano | Self::HammeredDulcimer | Self::Mallet | Self::Bell
        )
    }

    /// The model a plugin id names.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|model| model.id() == id)
    }

    /// The instrument's name in the browser.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Piano => "Physical Piano",
            Self::HammeredDulcimer => "Physical Hammered Strings",
            Self::Guitar => "Physical Guitar",
            Self::ElectricGuitar => "Electric Guitar",
            Self::Bass => "Physical Bass",
            Self::Violin => "Physical Strings",
            Self::Mallet => "Physical Mallet",
            Self::Bell => "Physical Bell",
            Self::Clarinet => "Physical Reeds",
            Self::TinWhistle => "Physical Whistles",
        }
    }

    const fn description(self) -> &'static str {
        match self {
            Self::Piano => {
                "Felt hammers on bridge-coupled unison strings, with dampers and a soundboard"
            }
            Self::HammeredDulcimer => "Light hammers on courses of strings over a shallow box",
            Self::Guitar => "Plucked strings in two polarisations on a resonant body",
            Self::ElectricGuitar => "Steel strings under a magnetic pickup, before the amplifier",
            Self::Bass => "Plucked bass strings under a pickup or on an upright's body",
            Self::Violin => {
                "Bowed strings: stick-slip friction, a section of players, their bodies"
            }
            Self::Mallet => "Tuned bars over resonator tubes, struck with a mallet",
            Self::Bell => "Struck shells and tubes: paired modes that beat as they ring",
            Self::Clarinet => "A reed valve on a cylinder of air, closed at the reed",
            Self::TinWhistle => "An air jet across the window of an open pipe",
        }
    }

    /// Where the notes sit across the stereo field, heard from the player's seat.
    ///
    /// Keyboards and bars run low to high from left to right; the strings follow orchestral
    /// seating, violins on the left and basses on the right. An instrument held in the hands is
    /// one source, and its width is its body's own.
    pub(crate) const fn stereo_image(self) -> StereoImage {
        match self {
            Self::Piano => StereoImage::new(64.5, 43.5, 0.6),
            Self::HammeredDulcimer => StereoImage::new(68.0, 18.0, -0.35),
            Self::Guitar => StereoImage::new(62.0, 22.0, 0.1),
            Self::ElectricGuitar => StereoImage::new(62.0, 22.0, 0.1),
            Self::Bass => StereoImage::new(45.0, 18.0, 0.1),
            Self::Violin => StereoImage::new(60.0, 28.0, -0.5),
            Self::Mallet => StereoImage::new(64.5, 31.5, 0.6),
            Self::Bell => StereoImage::new(72.0, 18.0, 0.4),
            Self::Clarinet => StereoImage::new(70.0, 22.0, 0.25),
            Self::TinWhistle => StereoImage::new(86.0, 12.0, 0.25),
        }
    }

    /// Whether a note lasts as long as its key is held: bowed and blown, not struck or plucked.
    pub const fn is_sustained(self) -> bool {
        matches!(self, Self::Violin | Self::Clarinet | Self::TinWhistle)
    }

    /// What a frontend draws this instrument's bodies as.
    pub const fn geometry(self) -> MotionGeometry {
        match self {
            Self::Mallet => MotionGeometry::Bar,
            Self::Bell => MotionGeometry::Shell,
            Self::Clarinet | Self::TinWhistle => MotionGeometry::Bore,
            _ => MotionGeometry::String,
        }
    }

    /// Most notes the model sounds at once; the quietest is taken for the next past that.
    pub const fn voices(self) -> usize {
        match self {
            Self::Piano => 32,
            Self::HammeredDulcimer | Self::Mallet | Self::Bell => 24,
            Self::Guitar | Self::ElectricGuitar | Self::Violin => 16,
            Self::Bass | Self::Clarinet | Self::TinWhistle => 12,
        }
    }

    fn engine(self) -> Box<dyn Engine> {
        match self {
            Self::Piano | Self::HammeredDulcimer => Box::new(struck::Struck::new(self)),
            Self::Guitar | Self::ElectricGuitar | Self::Bass => {
                Box::new(plucked::Plucked::new(self))
            }
            Self::Violin => Box::new(bowed::Bowed::new()),
            Self::Mallet => Box::new(bars::Bars::new()),
            Self::Bell => Box::new(bells::Bells::new()),
            Self::Clarinet => Box::new(reeds::Reeds::new()),
            Self::TinWhistle => Box::new(flutes::Flutes::new()),
        }
    }

    fn descriptors(self) -> Vec<ParamDescriptor> {
        match self {
            Self::Piano | Self::HammeredDulcimer => struck::descriptors(self),
            Self::Guitar | Self::ElectricGuitar | Self::Bass => plucked::descriptors(self),
            Self::Violin => bowed::descriptors(),
            Self::Mallet => bars::descriptors(),
            Self::Bell => bells::descriptors(),
            Self::Clarinet => reeds::descriptors(),
            Self::TinWhistle => flutes::descriptors(),
        }
    }

    /// Gain that puts each model where a mix expects it at the same level control.
    ///
    /// Measured, not derived. What an engine puts out is in whatever units its physics is
    /// in, and those have nothing to do with one another: a bridge's velocity, the air at a
    /// tone hole. Each gain here sets a model's default patch, playing the crate's standard
    /// four-bar phrase, at the same integrated loudness as every other built-in instrument —
    /// `loudness_tests` measures it and holds the band — so that changing a track's sound
    /// does not also change how loud it is.
    const fn normalization(self) -> f32 {
        match self {
            Self::Piano => 0.438,
            Self::HammeredDulcimer => 1.616,
            Self::Guitar => 0.806,
            Self::ElectricGuitar => 0.733,
            Self::Bass => 0.776,
            Self::Violin => 1.345,
            Self::Mallet => 0.634,
            Self::Bell => 0.329,
            Self::Clarinet => 2.481,
            Self::TinWhistle => 2.903,
        }
    }
}

/// One note as an engine is asked to start it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Note {
    /// Which key: what its release and its damper answer to.
    pub key: u8,
    /// Where it sounds, in semitones on the MIDI scale. The key's own number in the standard
    /// tuning; anything a tuning says otherwise. Every continuous quantity of an instrument —
    /// its stringing, its voicing, where its hammer strikes — follows this rather than the key,
    /// because a string tuned to a pitch is the string of that pitch whatever key plays it.
    pub pitch: f32,
    /// How hard, above zero and at most one.
    pub velocity: f32,
    /// Where it sits at unit width, from -1 on the left to 1 on the right.
    pub pan: f32,
    /// Where a frontend touched the body to make it, from zero to one along it.
    pub touch: Option<f32>,
    /// Whether the voice was still sounding another note.
    pub stolen: bool,
    /// A number that differs from note to note and is the same in every render: what a player
    /// does not repeat exactly, repeated exactly.
    pub seed: u32,
}

/// The key nearest a sounding pitch, for a table measured key by key — a piano's stretch is
/// one number per string, and a retuned note takes the number of the string it sounds nearest.
pub(crate) fn nearest_key(pitch: f32) -> u8 {
    pitch.round().clamp(0.0, 127.0) as u8
}

/// What the player is doing besides pressing keys.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Performance {
    /// Pitch bend in semitones.
    pub bend: f32,
    /// Controller 11: how hard the bow is drawn or the breath blown, and how loud the rest are.
    pub expression: f32,
    /// Controller 1: the modulation wheel.
    pub modulation: f32,
    /// Controller 64: how far down the sustain pedal is, from resting (0) to the floor (1).
    ///
    /// Read through [`Performance::lift`], never as a switch: between the two ends the dampers
    /// only brush the strings, which is the half pedal a pianist plays with.
    pub pedal: f32,
    /// Controller 66: the sostenuto pedal, holding up only the dampers that were up when it
    /// went down.
    pub sostenuto: bool,
    /// Controller 67: the soft pedal, which on a grand shifts the action so the hammers strike
    /// one string fewer.
    pub soft: bool,
    /// Controller 68: the legato footswitch. While it is down, a note played before the last
    /// is let go moves the sounding string or air column to it instead of starting another.
    pub legato: bool,
    /// Controller 75, the decay time of the General MIDI 2 sound controllers: resting at the
    /// middle, and shorter below it. A plucked string hears it as a palm laid on the strings at
    /// the bridge, which is how a guitarist shortens a note while it is held.
    pub decay: f32,
    /// Controller 74, brightness: resting at the middle. A bowed string hears it as where the
    /// bow is drawn, nearer the bridge above the middle and over the fingerboard below it.
    pub brightness: f32,
    /// Controller 65: portamento. While it is on, a slur slides from one note to the next in
    /// the time controller 5 gives instead of moving the finger at once.
    pub portamento: bool,
    /// Controller 5: the portamento time, from nothing to all the way up.
    pub portamento_time: f32,
    /// Controller 2, the breath controller: how hard a wind player blows, under the expression
    /// pedal. It rests all the way up, so a part that never sends it is blown as the expression
    /// says; a wind controller sends it, and with no breath a pipe is silent.
    pub breath: f32,
    /// The stereo width control.
    pub width: f32,
}

/// Pedal travel, as a fraction of controller 64, at which the dampers begin to leave the
/// strings. The travel before it is the pedal's free play.
const DAMPERS_LEAVE: f32 = 0.25;
/// Pedal travel at which the dampers are clear of the strings.
const DAMPERS_CLEAR: f32 = 0.75;
/// The breath controller's travel below which a pipe no longer speaks.
const BREATH_SPEAKS: f32 = 0.1;
/// The quickest and slowest portamento, as the time constant of a slide, in seconds.
const PORTAMENTO_FASTEST: f32 = 0.02;
const PORTAMENTO_SLOWEST: f32 = 1.5;

/// Moves `slid`, the key a slurred line is sounding, toward `key`: at once, or with portamento
/// on, over the time it gives. Called once a control tick, at `ticks_per_second`.
pub(crate) fn slide(slid: &mut f32, key: f32, performance: &Performance, ticks_per_second: f32) {
    match performance.portamento_seconds() {
        Some(seconds) => {
            *slid += (key - *slid) * (1.0 - (-1.0 / (seconds * ticks_per_second)).exp());
        }
        None => *slid = key,
    }
}

/// How a blown instrument is tongued, in parameter-value order.
pub(crate) const TONGUINGS: &[Cow<'static, str>] =
    &[Cow::Borrowed("Ordinary"), Cow::Borrowed("Flutter-tongue")];
/// Rolls of the tongue a second in a flutter.
pub(crate) const FLUTTER_HZ: f32 = 24.0;
/// The quickest a player's breath can follow the tongue, in seconds.
pub(crate) const FLUTTER_TURN: f32 = 0.004;

/// How much of the breath a fluttering tongue lets past `seconds` into a note: all of it
/// between rolls, and all but `depth` of it as the tongue comes up, `rolls` times a second.
pub(crate) fn flutter(seconds: f32, rolls: f32, depth: f32) -> f32 {
    let turn = (seconds * rolls).fract();
    1.0 - depth * (0.5 - 0.5 * (std::f32::consts::TAU * turn).cos())
}

impl Performance {
    /// How far the sustain pedal holds the dampers off the strings: 0 with them resting on the
    /// strings, 1 with them clear, and in between a damper that only brushes the string — the
    /// half pedal, which shortens a note without stopping it.
    pub fn lift(&self) -> f32 {
        let travel =
            ((self.pedal - DAMPERS_LEAVE) / (DAMPERS_CLEAR - DAMPERS_LEAVE)).clamp(0.0, 1.0);
        travel * travel * (3.0 - 2.0 * travel)
    }

    /// How much of a resting damper's grip the sustain pedal leaves on the strings: all of it
    /// with the pedal up, none with it down. A damper's grip goes with how hard its felt is
    /// pressed on the string, and that falls away faster than the damper rises: half lifted,
    /// it takes a quarter of what it would resting.
    pub fn damping(&self) -> f32 {
        let resting = 1.0 - self.lift();
        resting * resting
    }

    /// How long a slurred slide takes with portamento on, in seconds — the time constant of its
    /// approach to the new note — or `None` with it off, when the finger moves at once.
    ///
    /// Controller 5 at the bottom is a quick slide, a fiftieth of a second, and all the way up
    /// a slow one of a second and a half; it rises as the square, so the useful slides are
    /// spread across the travel.
    pub fn portamento_seconds(&self) -> Option<f32> {
        self.portamento.then(|| {
            let time = self.portamento_time.clamp(0.0, 1.0);
            PORTAMENTO_FASTEST + (PORTAMENTO_SLOWEST - PORTAMENTO_FASTEST) * time * time
        })
    }

    /// How much of a pipe's threshold the breath controller still reaches: all of it above the
    /// bottom tenth of its travel, where the breath gives out and the pipe stops speaking.
    pub fn speaking(&self) -> f32 {
        (self.breath / BREATH_SPEAKS).clamp(0.0, 1.0)
    }

    /// How hard the next blow, strike or pluck is: the instrument's own `control`, moved by
    /// controller 74 — a harder mallet, hammer or pick above its middle, a softer one below,
    /// as far as half the control's range either way.
    pub fn hardness(&self, control: f32) -> f32 {
        (control + self.brightness.clamp(0.0, 1.0) - 0.5).clamp(0.0, 1.0)
    }

    /// How hard a palm is laid on the strings: none with controller 75 at or above its middle,
    /// all the way at the bottom of it.
    pub fn palm(&self) -> f32 {
        ((0.5 - self.decay) * 2.0).clamp(0.0, 1.0)
    }
}

impl Default for Performance {
    fn default() -> Self {
        Self {
            bend: 0.0,
            expression: 1.0,
            modulation: 0.0,
            pedal: 0.0,
            sostenuto: false,
            soft: false,
            legato: false,
            decay: 0.5,
            brightness: 0.5,
            portamento: false,
            portamento_time: 0.0,
            breath: 1.0,
            width: 1.0,
        }
    }
}

/// The part of an instrument that is its own: voices that resonate, and what sets them going.
///
/// Everything here but [`Engine::prepare`] runs on the audio thread and allocates nothing.
pub(crate) trait Engine: Send {
    /// Sizes `voices` voices for `rate`. The only method that allocates.
    fn prepare(&mut self, rate: f32, voices: usize);

    /// Silences everything at once.
    fn reset(&mut self);

    /// Reads the instrument's controls, after any of them has changed and before the first note.
    fn configure(&mut self, params: &ParamBank);

    /// Starts `note` on `voice`.
    fn start(&mut self, voice: usize, note: &Note, quality: Quality);

    /// Moves a sounding voice to `note` without starting it again: a slur. `false` when this
    /// instrument cannot, and the note is started afresh instead.
    fn glide(&mut self, _voice: usize, _note: &Note) -> bool {
        false
    }

    /// The key of `voice` was let go.
    fn stop(&mut self, voice: usize);

    /// The key at `pitch` has come up. Most instruments only care about the note it played,
    /// which `stop` has already told them of; a piano's key also holds a damper off strings
    /// that may long since have stopped sounding.
    fn lift(&mut self, _pitch: u8) {}

    /// Fades `voice` out within a couple of milliseconds, whatever it was doing.
    fn silence(&mut self, voice: usize);

    /// The player's bend, expression, modulation, pedal or width changed.
    fn perform(&mut self, performance: &Performance);

    /// Adds every voice to `mid` and `side`, at most [`TICK`] samples of them. `tick` marks a
    /// block that starts on the control clock.
    fn render(&mut self, mid: &mut [f32], side: &mut [f32], tick: bool);

    /// How loud `voice` is, for choosing which to steal. Zero once it has died away.
    fn level(&self, voice: usize) -> f32;

    /// Whether `voice` still answers its key: `false` once it is only ringing out.
    fn is_held(&self, voice: usize) -> bool;

    /// Fills in the picture of `voice`: where it is excited, how, and the shape it is in.
    fn observe(&self, voice: usize, picture: &mut MotionVoice);

    /// The bow or breath pressure a frontend shows beside the picture; zero where there is none.
    fn pressure(&self) -> f32 {
        0.0
    }

    /// Fills in where the lowest modes of the instrument's soundboard or box are in their
    /// swing. An instrument with no such body leaves them at rest.
    fn observe_body(&self, _body: &mut [f32; MOTION_BODY]) {}
}

/// Seconds a struck instrument's picture of its blow covers.
pub(crate) const BLOW_WINDOW: f32 = 0.006;

/// Dials a calibration run turns between renders, so that a search over a voicing does not
/// rebuild the crate for every candidate. An application build has none: each dial reads as
/// the default written beside it, and the compiler folds it away.
#[cfg(feature = "physical-calibration")]
pub mod calibration {
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Number of dials.
    pub const DIALS: usize = 128;
    /// A bit pattern no finite value has.
    const UNSET: u32 = u32::MAX;
    static VALUES: [AtomicU32; DIALS] = [const { AtomicU32::new(UNSET) }; DIALS];

    /// Sets dial `index`, or with `None` returns it to its default. A dial is read when an
    /// instrument is prepared and when a note starts.
    pub fn set(index: usize, value: Option<f32>) {
        if let Some(slot) = VALUES.get(index) {
            slot.store(value.map_or(UNSET, f32::to_bits), Ordering::Relaxed);
        }
    }

    /// Returns every dial to its default.
    pub fn clear() {
        for slot in &VALUES {
            slot.store(UNSET, Ordering::Relaxed);
        }
    }

    pub(crate) fn dial(index: usize, default: f32) -> f32 {
        match VALUES.get(index).map(|slot| slot.load(Ordering::Relaxed)) {
            Some(bits) if bits != UNSET => f32::from_bits(bits),
            _ => default,
        }
    }
}

#[cfg(not(feature = "physical-calibration"))]
pub(crate) mod calibration {
    #[inline(always)]
    pub(crate) fn dial(_index: usize, default: f32) -> f32 {
        default
    }
}

/// Which dial is which. Those that come in eights hold one value for each C from C1 to C8.
pub(crate) mod dials {
    /// Decades by which the hammers' felt is stiffer than the voicing as built.
    pub(crate) const FELT: usize = 0;
    /// Added to the exponent of the felt's force law.
    pub(crate) const EXPONENT: usize = 8;
    /// Multiplies how far along the string the hammers strike.
    pub(crate) const STRIKE: usize = 16;
    /// Decibels added to a register's level.
    pub(crate) const LEVEL: usize = 24;
    /// Decibels added to the knock of the blow on the frame.
    pub(crate) const KNOCK: usize = 32;
    /// Decibels added to a body's tilt at 50 Hz and at each octave above it: ten of them.
    pub(crate) const TILT: usize = 40;
    /// Hammer speed at full velocity, and the decibels of speed the range of velocity spans.
    pub(crate) const SPEED: usize = 50;
    pub(crate) const SPEED_LAW: usize = 51;
    /// The felt's hysteresis.
    pub(crate) const FELT_LOSS: usize = 52;
    /// Multiplies what the bridge takes from the motion a unison shares.
    pub(crate) const BRIDGE: usize = 53;
    /// Multiplies the strings' own loss.
    pub(crate) const AFTERSOUND: usize = 61;
    /// Added to the exponent of the bridge's loss over partial number.
    pub(crate) const BRIDGE_SLOPE: usize = 69;
    /// Multiplies the internal friction of the partials above the fundamental.
    pub(crate) const FRICTION: usize = 70;
    /// Where the internal friction stops growing as the square of frequency, in kilohertz.
    pub(crate) const FRICTION_KNEE: usize = 71;
    /// What the bridge takes from a wound string's second plane against its first, and how
    /// strongly that plane is heard against the first.
    pub(crate) const PLANE_BRIDGE: usize = 72;
    pub(crate) const PLANE_HEARD: usize = 73;
    /// What the bridge takes from a wound string's first plane against a unison's shared motion.
    pub(crate) const PLANE_FIRST: usize = 74;
    /// The power of the soundboard's conductance, against its own trend, that the bridge's
    /// loss of each partial follows: zero for a bridge that drains every partial alike.
    pub(crate) const JAGGED: usize = 75;
    /// How far the stiffness of the strings of one unison stands apart, as a share of it.
    pub(crate) const SPREAD: usize = 76;
    /// How loud a low string's longitudinal motion is, how sharp its modes are, and how much
    /// less it grows than the square of the blow.
    pub(crate) const STRETCH: usize = 77;
    pub(crate) const STRETCH_Q: usize = 78;
    pub(crate) const STRETCH_TOUCH: usize = 79;
    /// The slapped bass: the force of a full pluck, the saddle's height, how far a pop pulls,
    /// the thumb's speed, its mass and its flesh's damping, what a fret gives back, and how
    /// many frets a string is held to.
    pub(crate) const SLAP_PLUCK: usize = 80;
    pub(crate) const SLAP_SADDLE: usize = 81;
    pub(crate) const SLAP_PULL: usize = 82;
    pub(crate) const SLAP_THUMB_SPEED: usize = 83;
    pub(crate) const SLAP_THUMB_MASS: usize = 84;
    pub(crate) const SLAP_FLESH: usize = 85;
    pub(crate) const SLAP_BOUNCE: usize = 86;
    pub(crate) const SLAP_FRETS: usize = 87;
    /// The reeds: where the breath's roughness falls away, how rough it is, and how far over
    /// what a note needs a player blows to start it.
    pub(crate) const REED_ROUGH_HZ: usize = 90;
    pub(crate) const REED_ROUGH: usize = 91;
    pub(crate) const REED_KICK: usize = 92;
    /// Cents the hardest breath sends a reed's note sharp, which the player takes out.
    pub(crate) const REED_SHARP: usize = 93;
    /// The bow: the nearest the bridge a string holds its motion, and how much of the way a
    /// bow could not go becomes weight.
    pub(crate) const BOW_NEAREST: usize = 94;
    pub(crate) const BOW_LEAN: usize = 95;
    /// The guitar's pluck: the power of a string's speed its corner rises by at a middling
    /// pluck, and what a unit of velocity adds to that power.
    pub(crate) const PLUCK_SNAP: usize = 96;
    pub(crate) const PLUCK_HARDER: usize = 97;
    /// The power of how much shorter a stopped string sounds by which it decays faster.
    pub(crate) const PLUCK_STOPPED: usize = 98;
}

/// A dial for each C from C1 to C8, read at a pitch between them along straight lines.
pub(crate) fn dial_across(first: usize, pitch: f32, default: f32) -> f32 {
    let position = ((pitch - 24.0) / 12.0).clamp(0.0, 7.0);
    let index = (position as usize).min(6);
    let fraction = position - index as f32;
    let low = calibration::dial(first + index, default);
    let high = calibration::dial(first + index + 1, default);
    low + (high - low) * fraction
}
/// Scale of a soundboard's or a box's modes in the picture, which brings them to the size
/// the strings that drive them are drawn at.
pub(crate) const BODY_DISPLAY: f32 = 0.1;

/// A blow that rises and falls as half a sine over `seconds`, as its picture: what a mallet
/// does to a bar, whose own motion is too slow to push back within the blow.
pub(crate) fn blow_scope(seconds: f32, strength: f32, scope: &mut [f32; MOTION_SCOPE]) {
    for (index, sample) in scope.iter_mut().enumerate() {
        let at = (index as f32 + 0.5) / MOTION_SCOPE as f32 * BLOW_WINDOW;
        *sample = if at < seconds {
            strength * (std::f32::consts::PI * at / seconds.max(1.0e-6)).sin()
        } else {
            0.0
        };
    }
}

/// Sizes of the first harmonics of one period of a wave, which `at` gives from zero at the
/// period's start to one at its end: the spectrum of an instrument that is a wave going
/// round and not a sum of partials. Sixty-four samples of the period, so harmonics above the
/// thirty-second fold down among the ones shown; a bowed or blown wave has little up there.
pub(crate) fn harmonics(mut at: impl FnMut(f32) -> f32, sizes: &mut [f32; MOTION_PARTIALS]) {
    const POINTS: usize = 64;
    let mut wave = [0.0_f32; POINTS];
    let mut turns = [(0.0_f32, 0.0_f32); POINTS];
    for (index, (sample, turn)) in wave.iter_mut().zip(turns.iter_mut()).enumerate() {
        let along = index as f32 / POINTS as f32;
        *sample = at(along);
        *turn = (std::f32::consts::TAU * along).sin_cos();
    }
    for (harmonic, size) in sizes.iter_mut().enumerate() {
        let (mut re, mut im) = (0.0_f32, 0.0_f32);
        for (index, sample) in wave.iter().enumerate() {
            let (sine, cosine) = turns[(harmonic + 1) * index % POINTS];
            re += sample * cosine;
            im += sample * sine;
        }
        *size = 2.0 * re.hypot(im) / POINTS as f32;
    }
}

/// A polyphonic physical instrument, registered once for each [`Model`].
///
/// Velocity changes what the exciter does, not just how loud the result is: a harder hammer blow
/// is shorter and so brighter, a faster bow presses harder. Controller 1 deepens the vibrato of
/// the bowed and blown models, 7 sets channel volume, 11 is expression — bow speed and breath as
/// well as level — and 64 lifts the piano's dampers and the vibraphone's. State is ordinary
/// plugin parameters, so automation, undo, save and load need no special format.
pub struct Physical {
    model: Model,
    params: ParamBank,
    engine: Box<dyn Engine>,
    allocator: VoiceAllocator,
    performance: Performance,
    /// Channel volume, squared from controller 7.
    volume: f32,
    /// Expression as it is applied, chasing the controller.
    expression: f32,
    expression_step: f32,
    gain: f32,
    rate: f32,
    /// Samples rendered since the last reset.
    clock: usize,
    /// Notes started since the last reset.
    notes: u32,
    /// Where a frontend touched the body, waiting for the note it excites.
    touch: Option<f32>,
    legato: Legato,
    /// Where each voice's note sounds, for the picture: the allocator keeps only its key.
    sounding: [f32; crate::voice::MAX_VOICES],
    motion: MotionCapture,
    motion_frames: usize,
    /// Stands in for the right channel when the output buffer is mono.
    scratch: Vec<f32>,
    level_param: u32,
    width_param: u32,
    quality_param: u32,
    render_quality_param: u32,
    legato_param: Option<u32>,
    /// Whether the block being rendered is an offline one, which picks the render quality.
    offline: bool,
}

impl std::fmt::Debug for Physical {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Physical")
            .field("model", &self.model)
            .field("voices", &self.allocator.active_count())
            .finish_non_exhaustive()
    }
}

impl Physical {
    /// Builds an unprepared physical instrument. All voice storage is allocated in `prepare`.
    pub fn new(model: Model) -> Self {
        let mut descriptors = model.descriptors();
        let legato_param = descriptors
            .iter()
            .find(|descriptor| descriptor.key == "legato")
            .map(|descriptor| descriptor.id.0);
        let level_param = descriptors.len() as u32;
        let width_param = level_param + 1;
        let quality_param = level_param + 2;
        let render_quality_param = level_param + 3;
        descriptors.push(
            ParamDescriptor::new(level_param, "level", "Level", -60.0, 6.0, -12.0)
                .with_unit(ParamUnit::Decibels),
        );
        descriptors.push(ParamDescriptor::new(
            width_param,
            "width",
            "Stereo Width",
            0.0,
            2.0,
            1.0,
        ));
        descriptors.push(
            ParamDescriptor::new(
                quality_param,
                "quality",
                "Live Quality",
                0.0,
                2.0,
                Quality::Standard.value(),
            )
            .with_choices(Quality::CHOICES),
        );
        descriptors.push(
            ParamDescriptor::new(
                render_quality_param,
                "render_quality",
                "Render Quality",
                0.0,
                2.0,
                Quality::High.value(),
            )
            .with_choices(Quality::CHOICES),
        );
        let params = ParamBank::new(descriptors);
        let mut engine = model.engine();
        engine.configure(&params);
        Self {
            model,
            gain: db_to_gain(params.at(level_param)) * model.normalization(),
            params,
            engine,
            allocator: VoiceAllocator::new(),
            performance: Performance::default(),
            volume: 1.0,
            expression: 1.0,
            expression_step: 1.0,
            rate: 48_000.0,
            clock: 0,
            notes: 0,
            touch: None,
            legato: Legato::default(),
            sounding: [0.0; crate::voice::MAX_VOICES],
            motion: MotionCapture::default(),
            motion_frames: 0,
            scratch: Vec::new(),
            level_param,
            width_param,
            quality_param,
            render_quality_param,
            legato_param,
            offline: false,
        }
    }

    /// Which instrument this is.
    pub fn model(&self) -> Model {
        self.model
    }

    /// The level a note started now is simulated at: the render quality while rendering
    /// offline, the live quality otherwise.
    pub fn quality(&self) -> Quality {
        Quality::from_value(self.params.at(if self.offline {
            self.render_quality_param
        } else {
            self.quality_param
        }))
    }

    /// Whether a note played over a held one slurs to it: the instrument's own legato control,
    /// or the legato footswitch. Only an instrument that has the control can slur at all.
    fn is_legato(&self) -> bool {
        self.legato_param
            .is_some_and(|param| self.performance.legato || self.params.at(param) >= 0.5)
    }

    fn note(&mut self, key: u8, pitch: f32, velocity: f32, stolen: bool) -> Note {
        self.notes = self.notes.wrapping_add(1);
        Note {
            key,
            pitch,
            velocity,
            pan: self.model.stereo_image().position(pitch),
            touch: self.touch.take(),
            stolen,
            // From the pitch rather than the key, so a note sounds the same whichever key a
            // tuning has playing it.
            seed: self
                .notes
                .wrapping_mul(0x9E37_79B9)
                .wrapping_add(u32::from(nearest_key(pitch)) << 20),
        }
    }

    /// Moves the legato voice to `key`, sounding at `pitch`, keeping its string or its air
    /// column sounding.
    fn glide(&mut self, index: usize, key: u8, pitch: f32, velocity: f32) -> bool {
        if self.engine.level(index) <= 0.0 || !self.engine.is_held(index) {
            return false;
        }
        if !self.allocator.retarget(index, key, velocity) {
            return false;
        }
        if let Some(sounding) = self.sounding.get_mut(index) {
            *sounding = pitch;
        }
        let note = self.note(key, pitch, velocity, false);
        self.engine.glide(index, &note)
    }

    fn note_on(&mut self, key: u8, pitch: f32, velocity: f32) {
        let key = key.min(127);
        let pitch = crate::params::sounding_pitch(key, pitch);
        let velocity = finite_or(velocity, 0.0).clamp(0.0, 1.0);
        if velocity == 0.0 {
            self.note_off(key);
            return;
        }
        // Which keys are down is kept whether or not the line is slurred, so the footswitch
        // can go down over a key already held and slur from it.
        self.legato.note_on(key, pitch, velocity);
        if self.is_legato()
            && let Some(index) = self.legato.voice
            && self.glide(index, key, pitch, velocity)
        {
            return;
        }
        let Some(assignment) = self.allocator.note_on(key, velocity) else {
            return;
        };
        if let Some(sounding) = self.sounding.get_mut(assignment.index) {
            *sounding = pitch;
        }
        let note = self.note(key, pitch, velocity, assignment.stolen);
        let quality = self.quality();
        self.engine.start(assignment.index, &note, quality);
        if self.is_legato() {
            self.legato.voice = Some(assignment.index);
        }
    }

    fn note_off(&mut self, key: u8) {
        let key = key.min(127);
        let counted = self.legato.note_off(key);
        if self.is_legato() {
            if !counted {
                return;
            }
            let line = self.legato.voice.filter(|index| {
                self.allocator
                    .slot(*index)
                    .is_some_and(|slot| slot.state == VoiceState::Held && slot.pitch == key)
            });
            if let Some(index) = line {
                if let Some(down) = self.legato.last() {
                    // The key underneath is still down: the line falls back to it.
                    self.glide(index, down.key, down.pitch, down.velocity);
                    return;
                }
                self.legato.voice = None;
            }
            // A key the line has moved off holds no voice; one struck before the footswitch
            // went down holds its own, and lets it go.
        }
        for index in self.allocator.note_off(key) {
            if self.legato.voice == Some(index) {
                self.legato.voice = None;
            }
            self.engine.stop(index);
        }
    }

    fn release_everything(&mut self, at_once: bool) {
        self.legato.clear();
        let pedals = (self.performance.pedal, self.performance.sostenuto);
        if pedals != (0.0, false) {
            self.performance.pedal = 0.0;
            self.performance.sostenuto = false;
            self.engine.perform(&self.performance);
        }
        for index in self.allocator.release_all() {
            if at_once {
                self.engine.silence(index);
            } else {
                self.engine.stop(index);
            }
        }
        for pitch in 0..=127 {
            self.engine.lift(pitch);
        }
    }

    /// Frees the voices that have died away and tells the allocator how loud the rest are.
    fn tick(&mut self) {
        for index in 0..self.allocator.capacity() {
            if self
                .allocator
                .slot(index)
                .is_none_or(|slot| slot.state == VoiceState::Free)
            {
                continue;
            }
            let level = self.engine.level(index);
            if level > 0.0 {
                self.allocator.set_level(index, level);
            } else {
                self.allocator.retire(index);
                if self.legato.voice == Some(index) {
                    self.legato.voice = None;
                }
            }
        }
    }

    fn publish_motion(&mut self) {
        let mut frame = MotionFrame {
            geometry: self.model.geometry(),
            active: self.allocator.active_count(),
            expression: self.expression,
            pressure: self.engine.pressure(),
            pedal: self.performance.lift() > 0.0,
            ..MotionFrame::default()
        };
        let engine = &self.engine;
        let selected = recent_voices(&self.allocator, |index| engine.level(index) > 0.0);
        for (picture, index) in frame.voices.iter_mut().zip(selected) {
            let Some(index) = index else {
                break;
            };
            let Some(sounding) = self.sounding.get(index) else {
                break;
            };
            picture.pitch = sounding + self.performance.bend;
            picture.held = self.engine.is_held(index);
            self.engine.observe(index, picture);
        }
        self.engine.observe_body(&mut frame.body);
        self.motion.publish(&frame);
    }
}

impl Parameterized for Physical {
    fn parameters(&self) -> &[ParamDescriptor] {
        self.params.descriptors()
    }

    fn param(&self, id: ParamId) -> f32 {
        self.params.get(id)
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        let was_legato = self.is_legato();
        if !self.params.set(id, value) {
            return;
        }
        if was_legato != self.is_legato() {
            // The bookkeeping of which keys are down belongs to one mode or the other.
            self.release_everything(false);
        }
        if id.0 == self.level_param {
            self.gain = db_to_gain(self.params.at(self.level_param)) * self.model.normalization();
        } else if id.0 == self.width_param {
            self.performance.width = self.params.at(self.width_param);
            self.engine.perform(&self.performance);
        } else if id.0 < self.level_param {
            self.engine.configure(&self.params);
        }
    }
}

impl SegmentRenderer for Physical {
    fn handle_event(&mut self, event: &NoteEvent) {
        match *event {
            NoteEvent::NoteOn {
                key,
                pitch,
                velocity,
                ..
            } => self.note_on(key, pitch, velocity),
            NoteEvent::NoteOff { key, .. } => {
                self.note_off(key);
                self.engine.lift(key.min(127));
            }
            NoteEvent::AllNotesOff { .. } => self.release_everything(false),
            NoteEvent::AllSoundOff { .. } => self.release_everything(true),
            NoteEvent::PitchBend { semitones, .. } => {
                self.performance.bend = finite_or(semitones, 0.0).clamp(-24.0, 24.0);
                self.engine.perform(&self.performance);
            }
            NoteEvent::Controller { number, value, .. } => {
                let value = finite_or(value, 0.0).clamp(0.0, 1.0);
                match number {
                    1 => self.performance.modulation = value,
                    7 => {
                        self.volume = value * value;
                        return;
                    }
                    11 => {
                        self.performance.expression = value;
                        if self.allocator.active_count() == 0 {
                            // Nothing is sounding to hear the change glide.
                            self.expression = value;
                        }
                    }
                    64 => self.performance.pedal = value,
                    66 => self.performance.sostenuto = value >= 0.5,
                    75 => self.performance.decay = value,
                    74 => self.performance.brightness = value,
                    65 => self.performance.portamento = value >= 0.5,
                    5 => self.performance.portamento_time = value,
                    2 => self.performance.breath = value,
                    67 => self.performance.soft = value >= 0.5,
                    68 => {
                        let legato = value >= 0.5;
                        if legato == self.performance.legato {
                            return;
                        }
                        self.performance.legato = legato;
                        if !self.is_legato() {
                            // The line ends here; whatever it still holds is an ordinary note.
                            self.legato.voice = None;
                        }
                        return;
                    }
                    TOUCH_CONTROLLER => {
                        self.touch = Some(value);
                        return;
                    }
                    _ => return,
                }
                self.engine.perform(&self.performance);
            }
        }
    }

    fn render_segment(&mut self, out: &mut AudioBuffer, start: usize, end: usize) {
        let Some((first, rest)) = out.channels_mut().split_first_mut() else {
            return;
        };
        let Some(left) = first.get_mut(start..end) else {
            return;
        };
        let stereo = !rest.is_empty();
        // Taken out for the block so the mono path can borrow it beside the engine.
        let mut scratch = std::mem::take(&mut self.scratch);
        let right = match rest.first_mut() {
            Some(channel) => channel.get_mut(start..end),
            None => scratch.get_mut(..end - start),
        };
        let Some(right) = right else {
            left.fill(0.0);
            self.scratch = scratch;
            return;
        };
        let mut done = 0;
        while done < left.len() {
            let phase = self.clock % TICK;
            let count = (left.len() - done).min(TICK - phase);
            if phase == 0 {
                self.tick();
            }
            let (Some(mid), Some(side)) = (
                left.get_mut(done..done + count),
                right.get_mut(done..done + count),
            ) else {
                break;
            };
            mid.fill(0.0);
            side.fill(0.0);
            self.engine.render(mid, side, phase == 0);
            for (mid, side) in mid.iter_mut().zip(side.iter_mut()) {
                self.expression +=
                    (self.performance.expression - self.expression) * self.expression_step;
                let gain = self.gain * self.volume * self.expression;
                // Left is `1 - pan` of a note and right `1 + pan`: mid less side, and mid plus.
                let (to_left, to_right) = ((*mid - *side) * gain, (*mid + *side) * gain);
                if stereo {
                    *mid = to_left;
                    *side = to_right;
                } else {
                    *mid = (to_left + to_right) * 0.5;
                }
            }
            self.clock = self.clock.wrapping_add(count);
            done += count;
        }
        self.scratch = scratch;
        downmix_to_extra_channels(out, start, end);
    }
}

impl Instrument for Physical {
    fn motion_monitor(&self) -> Option<std::sync::Arc<auris_core::motion::MotionMonitor>> {
        Some(self.motion.monitor())
    }

    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::instrument(
            self.model.id(),
            self.model.name(),
            self.model.description(),
            PluginCategory::Synth,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.rate = crate::sample_rate_f32(ctx.sample_rate).clamp(8_000.0, 192_000.0);
        let voices = self.model.voices();
        self.allocator.prepare(voices);
        self.engine.prepare(self.rate, voices);
        self.engine.configure(&self.params);
        self.scratch = vec![0.0; ctx.max_block_frames];
        // Ten milliseconds: a pedal swept from end to end never zips.
        self.expression_step = 1.0 - (-1.0 / (0.010 * self.rate)).exp();
        self.reset();
    }

    fn reset(&mut self) {
        self.engine.reset();
        self.allocator.clear();
        self.performance = Performance {
            width: self.params.at(self.width_param),
            ..Performance::default()
        };
        self.engine.perform(&self.performance);
        self.volume = 1.0;
        self.expression = 1.0;
        self.clock = 0;
        self.notes = 0;
        self.touch = None;
        self.legato.clear();
        self.motion_frames = 0;
        self.publish_motion();
    }

    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, ctx: &ProcessContext) {
        let frames = ctx.block_frames.min(out.frame_count());
        self.offline = ctx.is_offline;
        render_segments(self, events, out, frames);
        if self.motion.is_watched() {
            self.motion_frames = self.motion_frames.saturating_add(frames);
            if self.motion_frames >= (self.rate / 30.0) as usize {
                self.motion_frames = 0;
                self.publish_motion();
            }
        }
    }

    fn active_voices(&self) -> usize {
        self.allocator.active_count()
    }
}

/// A touch anywhere along a string, folded onto the half the excitation position measures: a
/// string fixed at both ends sounds the same excited at a quarter from either end.
pub(crate) fn touch_position(at: f32) -> f32 {
    let at = finite_or(at, 0.25).clamp(0.0, 1.0);
    at.min(1.0 - at).clamp(0.02, 0.5)
}

/// A voice's last sample, faded out over two milliseconds when the voice is taken for another
/// note: the difference between a stolen voice and a click.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Declick {
    value: f32,
    step: f32,
}

impl Declick {
    /// Starts fading from `value`, which is where the voice's output just was.
    pub(crate) fn from(value: f32, rate: f32) -> Self {
        Self {
            value: finite_or(value, 0.0),
            step: finite_or(value, 0.0) / (0.002 * rate).max(1.0),
        }
    }

    /// The next sample of the fade; zero once it is over.
    pub(crate) fn next(&mut self) -> f32 {
        if self.value == 0.0 {
            return 0.0;
        }
        let now = self.value;
        self.value -= self.step;
        if self.value * now <= 0.0 {
            self.value = 0.0;
        }
        now
    }
}

/// A gain that falls from one to nothing in two milliseconds, for a voice silenced outright.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Mute {
    gain: f32,
    step: f32,
}

impl Default for Mute {
    fn default() -> Self {
        Self {
            gain: 1.0,
            step: 0.0,
        }
    }
}

impl Mute {
    /// Starts the fade.
    pub(crate) fn close(&mut self, rate: f32) {
        self.step = 1.0 / (0.002 * rate).max(1.0);
    }

    /// Opens fully again, for the voice's next note.
    pub(crate) fn open(&mut self) {
        *self = Self::default();
    }

    /// The gain for the next sample.
    pub(crate) fn next(&mut self) -> f32 {
        self.gain = (self.gain - self.step).max(0.0);
        self.gain
    }

    /// Whether the fade has run out.
    pub(crate) fn is_closed(&self) -> bool {
        self.gain <= 0.0
    }
}

/// The frequency of MIDI `pitch`, which may be fractional, in hertz.
pub(crate) fn hertz(pitch: f32) -> f32 {
    auris_core::plugin::pitch_to_hz(pitch)
}
