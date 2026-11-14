//! Playing a MIDI keyboard into the engine.
//!
//! A hardware keyboard is the typing keyboard with real keys: something a person plays, heard
//! the moment it is played and written down only when a take is listening. Everything that is
//! the same — which track a note goes to, holding a note on the track it was struck on, what a
//! take does with it — is the session's, and is shared with the typing keyboard there. What is
//! here is the part that cannot wait for the session.
//!
//! # Why the notes do not go through the session
//!
//! A frontend services the session on a timer, and the desktop's timer is thirty milliseconds
//! long. A note that waited for it would sound up to thirty milliseconds late, and late by a
//! different amount every time, which a player hears long before they can say what is wrong.
//! So [`MidiInputs`] answers every message on the thread the MIDI backend delivers it on: it
//! sends the engine command itself, through the same queue the session's own `NoteOn` uses, and
//! notes *when* it did so for the session to read later. That thread is not the audio thread —
//! it may lock and allocate — and the one lock it takes is held for the length of a message.
//!
//! # When a note was played
//!
//! The frame being *heard* when the message arrived, read from the output clock exactly as the
//! typing keyboard's strikes are, and stamped on the [`MidiEvent`] there and then. The session
//! drains the events thirty milliseconds later, and a stamp taken at the drain would place
//! every note on the timer's grid instead of the player's.
//!
//! # The sustain pedal
//!
//! Held here, not by the instruments. A built-in voice has no idea what controller 64 means,
//! and a take that wrote the pedal as a controller would play back with notes the instrument
//! never held. So a note let go of under the pedal is released when the pedal comes up, and a
//! take writes down the note as long as it was *heard* — the same thing the typing keyboard's
//! sustain key does.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use auris_core::TrackId;
use crossbeam_channel::{Receiver, Sender, TrySendError};

use crate::clock::OutputClock;
use crate::command::EngineCommand;
use crate::error::EngineError;
use crate::handle::EngineHandle;

/// How far a full push of a keyboard's bend wheel bends, in semitones.
///
/// Two, because that is the range the MIDI specification gives a channel until it is told
/// otherwise, and so the range every keyboard's wheel is built and labelled for.
pub const MIDI_BEND_RANGE: f32 = 2.0;

/// The controller a sustain pedal sends.
pub const CC_SUSTAIN: u8 = 64;

/// How many played events may wait for the session before the oldest are lost.
///
/// Several seconds of the densest playing a hand can manage, wheels included. A session that is
/// never serviced — a command line run that happened to open the inputs — loses what it never
/// read rather than growing without limit.
const EVENT_CAPACITY: usize = 8192;

/// The name this program gives itself to the MIDI backend.
const CLIENT_NAME: &str = "Auris Studio";

/// How hard a key has to be struck for how loud a note.
///
/// Keyboards differ, and so do hands: a small keyboard's keys are often stiff enough that its
/// full range is out of reach, which is what a softer curve gives back.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum VelocityResponse {
    /// The keyboard's own reading raised to this power: 1 is as it comes, below 1 makes a light
    /// touch louder, above 1 makes it quieter.
    Curve(f32),
    /// Every note at this velocity, however it was struck.
    Fixed(f32),
}

impl Default for VelocityResponse {
    fn default() -> Self {
        Self::Curve(1.0)
    }
}

impl VelocityResponse {
    /// The velocity, 0 to 1, a strike read as `raw` (1 to 127) sounds at.
    pub fn shape(self, raw: u8) -> f32 {
        let linear = f32::from(raw.min(127)) / 127.0;
        match self {
            Self::Curve(power) if power.is_finite() && power > 0.0 => linear.powf(power),
            Self::Curve(_) => linear,
            Self::Fixed(velocity) => velocity.clamp(0.0, 1.0),
        }
    }
}

/// One MIDI message, as far as an instrument is concerned.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MidiMessage {
    /// A key went down.
    NoteOn {
        /// Channel, 0 to 15.
        channel: u8,
        /// Note number, 0 to 127.
        pitch: u8,
        /// How hard, 1 to 127. A strike of zero is a [`Self::NoteOff`].
        velocity: u8,
    },
    /// A key came up.
    NoteOff {
        /// Channel, 0 to 15.
        channel: u8,
        /// Note number, 0 to 127.
        pitch: u8,
    },
    /// A controller moved.
    Controller {
        /// Channel, 0 to 15.
        channel: u8,
        /// Controller number, 0 to 127.
        number: u8,
        /// Where it is, 0 to 127.
        value: u8,
    },
    /// The bend wheel moved.
    PitchBend {
        /// Channel, 0 to 15.
        channel: u8,
        /// Where it is, from -8192 (all the way down) to 8191, zero at rest.
        value: i16,
    },
}

impl MidiMessage {
    /// Reads one message off the wire, or `None` for one no instrument answers.
    ///
    /// Clock, active sensing, aftertouch and program changes come back `None`. A note-on with
    /// velocity zero is a note-off, which is how most keyboards send their releases.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let (&status, data) = bytes.split_first()?;
        let channel = status & 0x0f;
        let first = *data.first()? & 0x7f;
        let second = data.get(1).map(|byte| byte & 0x7f);
        match status & 0xf0 {
            0x80 => Some(Self::NoteOff {
                channel,
                pitch: first,
            }),
            0x90 => match second? {
                0 => Some(Self::NoteOff {
                    channel,
                    pitch: first,
                }),
                velocity => Some(Self::NoteOn {
                    channel,
                    pitch: first,
                    velocity,
                }),
            },
            0xb0 => Some(Self::Controller {
                channel,
                number: first,
                value: second?,
            }),
            0xe0 => {
                let raw = i16::from(first) | (i16::from(second?) << 7);
                Some(Self::PitchBend {
                    channel,
                    value: raw - 8192,
                })
            }
            _ => None,
        }
    }
}

/// Where what is played is sounded: a track, by its position in the graph and by its id.
///
/// Both, because the engine addresses a track by position and the session by id, and the two
/// are read at the same moment so that they cannot disagree.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LiveTarget {
    /// Position of the track in the project, which is how the engine addresses it.
    pub index: usize,
    /// The track itself, which is how the session addresses it.
    pub track: TrackId,
}

/// What an instrument was asked to do by something played on a keyboard.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Performed {
    /// A note was struck.
    NoteOn {
        /// Which note.
        pitch: u8,
        /// How hard, 0 to 1.
        velocity: f32,
    },
    /// A note stopped sounding — its key came up, or the pedal holding it did.
    NoteOff {
        /// Which note.
        pitch: u8,
    },
    /// The bend moved, in semitones.
    PitchBend(f32),
    /// A controller moved.
    Controller {
        /// Which controller.
        number: u8,
        /// Where it now is, 0 to 1.
        value: f32,
    },
}

impl Performed {
    /// The engine command that makes a track's instrument do this.
    fn command(self, track: usize) -> EngineCommand {
        match self {
            Self::NoteOn { pitch, velocity } => EngineCommand::NoteOn {
                track,
                pitch,
                velocity,
            },
            Self::NoteOff { pitch } => EngineCommand::NoteOff { track, pitch },
            Self::PitchBend(semitones) => EngineCommand::PitchBend { track, semitones },
            Self::Controller { number, value } => EngineCommand::Controller {
                track,
                number,
                value,
            },
        }
    }
}

/// Something a keyboard played, where it was sounded and when.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MidiEvent {
    /// The track it was sounded on.
    pub track: TrackId,
    /// What was sounded.
    pub performed: Performed,
    /// The virtual frame being heard when it arrived, as
    /// [`EngineHandle::heard_frame_now`] reads it.
    pub frame: f64,
}

/// The bookkeeping between a keyboard and the instruments it plays.
///
/// Pure: it decides what to sound and sounds nothing, which is what lets every rule here be
/// tested without a keyboard plugged in. [`MidiInputs`] owns one and carries out its answers.
#[derive(Clone, Debug, Default)]
pub struct LivePlayer {
    /// Keys that are down.
    held: Vec<Key>,
    /// Notes whose keys came up under the pedal, still sounding.
    sustained: Vec<Key>,
    /// Channels whose pedal is down.
    pedal: u16,
    /// Tracks this keyboard has bent or moved a controller on, and which controllers, so that
    /// letting go of everything can put them back.
    touched: Vec<(LiveTarget, Option<u8>)>,
    /// How a strike's reading becomes a velocity.
    response: VelocityResponse,
    /// How many octaves up (or down, below zero) a key sounds from the note it sends.
    octave: i8,
}

/// A key that is down, or that the pedal is holding.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Key {
    /// The channel it came on.
    channel: u8,
    /// The note the keyboard sent for it.
    key: u8,
    /// The track the note went to.
    target: LiveTarget,
    /// The note that sounded: the key moved by the octave in force when it went down.
    sounded: u8,
}

impl Key {
    fn is(&self, channel: u8, key: u8) -> bool {
        self.channel == channel && self.key == key
    }

    fn release(&self) -> (LiveTarget, Performed) {
        (
            self.target,
            Performed::NoteOff {
                pitch: self.sounded,
            },
        )
    }
}

/// How far an octave shift can go either way.
///
/// Four, which takes the middle C of any keyboard to either end of the piano and a 25-key
/// keyboard's whole span to the top or bottom of MIDI's — further would only put keys off the end.
pub const MIDI_OCTAVE_LIMIT: i8 = 4;

impl LivePlayer {
    /// Moves what every key sounds by `octave` octaves, from the next key down.
    ///
    /// Keys already down keep sounding the note they struck and release it, as an instrument's
    /// own octave switch does.
    pub fn set_octave(&mut self, octave: i8) {
        self.octave = octave.clamp(-MIDI_OCTAVE_LIMIT, MIDI_OCTAVE_LIMIT);
    }

    /// The octave shift in force.
    pub fn octave(&self) -> i8 {
        self.octave
    }

    /// Chooses how a strike's reading becomes a velocity.
    pub fn set_response(&mut self, response: VelocityResponse) {
        self.response = response;
    }

    /// Answers one message, played towards `target`.
    ///
    /// A key that comes up is released on the track it went down on, wherever the target has
    /// moved since; with no target, a new note sounds nowhere but a release still lands.
    pub fn play(
        &mut self,
        message: MidiMessage,
        target: Option<LiveTarget>,
    ) -> Vec<(LiveTarget, Performed)> {
        match message {
            MidiMessage::NoteOn {
                channel,
                pitch,
                velocity,
            } => {
                let Some(target) = target else {
                    return Vec::new();
                };
                // A key shifted past either end of MIDI's range sounds nothing, rather than a
                // note folded back into it that nobody played.
                let Ok(sounded) = u8::try_from(i16::from(pitch) + 12 * i16::from(self.octave))
                else {
                    return Vec::new();
                };
                if sounded > 127 {
                    return Vec::new();
                }
                let mut out = Vec::with_capacity(2);
                // Struck again while it was still sounding — through the pedal, or a second
                // keyboard — ends the first note where the second begins.
                for list in [&mut self.held, &mut self.sustained] {
                    if let Some(position) = list.iter().position(|held| held.is(channel, pitch)) {
                        out.push(list.remove(position).release());
                    }
                }
                self.held.push(Key {
                    channel,
                    key: pitch,
                    target,
                    sounded,
                });
                out.push((
                    target,
                    Performed::NoteOn {
                        pitch: sounded,
                        velocity: self.response.shape(velocity),
                    },
                ));
                out
            }
            MidiMessage::NoteOff { channel, pitch } => {
                let Some(position) = self.held.iter().position(|held| held.is(channel, pitch))
                else {
                    return Vec::new();
                };
                let held = self.held.remove(position);
                if self.pedal & (1 << channel) != 0 {
                    self.sustained.push(held);
                    return Vec::new();
                }
                vec![held.release()]
            }
            MidiMessage::Controller {
                channel,
                number: CC_SUSTAIN,
                value,
            } => {
                if value >= 64 {
                    self.pedal |= 1 << channel;
                    return Vec::new();
                }
                self.pedal &= !(1 << channel);
                let (released, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.sustained)
                    .into_iter()
                    .partition(|held| held.channel == channel);
                self.sustained = kept;
                released.iter().map(Key::release).collect()
            }
            MidiMessage::Controller { number, value, .. } => {
                let Some(target) = target else {
                    return Vec::new();
                };
                self.touch(target, Some(number));
                vec![(
                    target,
                    Performed::Controller {
                        number,
                        value: f32::from(value) / 127.0,
                    },
                )]
            }
            MidiMessage::PitchBend { value, .. } => {
                let Some(target) = target else {
                    return Vec::new();
                };
                self.touch(target, None);
                let reach = if value < 0 { 8192.0 } else { 8191.0 };
                vec![(
                    target,
                    Performed::PitchBend(f32::from(value) / reach * MIDI_BEND_RANGE),
                )]
            }
        }
    }

    /// Stops every note this keyboard is sounding and puts back every wheel it moved.
    ///
    /// For everything that ends a performance without a key coming up: the input being switched
    /// off, a keyboard unplugged with a chord held down.
    pub fn release_all(&mut self) -> Vec<(LiveTarget, Performed)> {
        let mut out: Vec<_> = self
            .held
            .drain(..)
            .chain(self.sustained.drain(..))
            .map(|held| held.release())
            .collect();
        self.pedal = 0;
        for (target, controller) in self.touched.drain(..) {
            out.push((
                target,
                match controller {
                    None => Performed::PitchBend(0.0),
                    Some(number) => Performed::Controller { number, value: 0.0 },
                },
            ));
        }
        out
    }

    /// The notes sounding now, wherever they went.
    pub fn sounding(&self) -> impl Iterator<Item = u8> + '_ {
        self.held
            .iter()
            .chain(self.sustained.iter())
            .map(|held| held.sounded)
    }

    /// Remembers that a wheel on `target` has been moved away from rest.
    fn touch(&mut self, target: LiveTarget, controller: Option<u8>) {
        if !self.touched.contains(&(target, controller)) {
            self.touched.push((target, controller));
        }
    }
}

/// The engine a keyboard is sounding into: the queue, and the clock to stamp events by.
struct Route {
    commands: Sender<EngineCommand>,
    clock: Arc<OutputClock>,
    playhead: Arc<AtomicU64>,
    count_in: Arc<AtomicU64>,
}

impl Route {
    fn of(engine: &EngineHandle) -> Self {
        Self {
            commands: engine.commands.clone(),
            clock: Arc::clone(&engine.clock),
            playhead: Arc::clone(&engine.playhead),
            count_in: Arc::clone(&engine.count_in),
        }
    }

    /// The frame being heard now, falling back to the one rendered, as the session reads it.
    fn heard_frame(&self) -> f64 {
        self.clock.heard_now().unwrap_or_else(|| {
            self.playhead.load(Ordering::Relaxed) as f64
                - self.count_in.load(Ordering::Relaxed) as f64
        })
    }
}

/// Everything the MIDI threads share with the session.
#[derive(Default)]
struct Shared {
    route: Option<Route>,
    target: Option<LiveTarget>,
    player: LivePlayer,
    /// Set by every message that sounded something, cleared by whoever reads it.
    active: bool,
}

/// Carries out what the player answered: sounds it, and writes it down for the session.
fn carry_out(
    shared: &mut Shared,
    performed: Vec<(LiveTarget, Performed)>,
    log: &Sender<MidiEvent>,
) {
    if performed.is_empty() {
        return;
    }
    shared.active = true;
    let Some(route) = shared.route.as_ref() else {
        return;
    };
    let frame = route.heard_frame();
    for (target, performed) in performed {
        if let Err(error) = route.commands.try_send(performed.command(target.index)) {
            log::debug!("live MIDI command dropped: {error}");
        }
        let event = MidiEvent {
            track: target.track,
            performed,
            frame,
        };
        if let Err(TrySendError::Full(_)) = log.try_send(event) {
            log::debug!("live MIDI event log is full; an event was not kept");
        }
    }
}

/// Answers one message from a keyboard: plays it, sounds it and writes it down.
fn receive(shared: &Mutex<Shared>, log: &Sender<MidiEvent>, bytes: &[u8]) {
    let Some(message) = MidiMessage::parse(bytes) else {
        return;
    };
    let mut shared = shared.lock().unwrap_or_else(PoisonError::into_inner);
    let target = shared.target;
    let performed = shared.player.play(message, target);
    carry_out(&mut shared, performed, log);
}

/// The MIDI inputs this program is listening to, and what they are playing.
///
/// Opening one is the whole of setting it up: from then on, whatever is played on it sounds on
/// the [`target`](Self::set_target) the moment it arrives, and waits in [`Self::drain`] for
/// whoever is writing it down.
pub struct MidiInputs {
    shared: Arc<Mutex<Shared>>,
    connections: Vec<(String, midir::MidiInputConnection<()>)>,
    /// A client kept for listing inputs, made the first time it is asked for. One for the
    /// life of the session rather than one per look, because a frontend looks every few
    /// seconds to notice a keyboard plugged in, and some backends make a client slowly.
    scanner: Option<midir::MidiInput>,
    log: Sender<MidiEvent>,
    events: Receiver<MidiEvent>,
}

impl Default for MidiInputs {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for MidiInputs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MidiInputs")
            .field("connected", &self.connected().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl MidiInputs {
    /// Listening to nothing yet.
    pub fn new() -> Self {
        let (log, events) = crossbeam_channel::bounded(EVENT_CAPACITY);
        Self {
            shared: Arc::default(),
            connections: Vec::new(),
            scanner: None,
            log,
            events,
        }
    }

    /// The names of the MIDI inputs the system has now, in the order it lists them.
    pub fn available(&mut self) -> Result<Vec<String>, EngineError> {
        if self.scanner.is_none() {
            self.scanner = Some(
                midir::MidiInput::new(CLIENT_NAME)
                    .map_err(|error| EngineError::Midi(error.to_string()))?,
            );
        }
        let input = self.scanner.as_ref().expect("made above");
        Ok(input
            .ports()
            .iter()
            .filter_map(|port| input.port_name(port).ok())
            .collect())
    }

    /// The inputs being listened to, by name.
    pub fn connected(&self) -> impl Iterator<Item = &str> {
        self.connections.iter().map(|(name, _)| name.as_str())
    }

    /// Starts listening to the input called `name`. Listening to one twice is listening to it.
    pub fn connect(&mut self, name: &str) -> Result<(), EngineError> {
        if self.connected().any(|open| open == name) {
            return Ok(());
        }
        let mut input = midir::MidiInput::new(CLIENT_NAME)
            .map_err(|error| EngineError::Midi(error.to_string()))?;
        // Clock and active sensing arrive many times a second and mean nothing to an
        // instrument; leaving them at the backend saves waking for them.
        input.ignore(midir::Ignore::TimeAndActiveSense);
        let port = input
            .ports()
            .into_iter()
            .find(|port| input.port_name(port).is_ok_and(|found| found == name))
            .ok_or_else(|| EngineError::Midi(format!("no MIDI input called `{name}`")))?;
        let shared = Arc::clone(&self.shared);
        let log = self.log.clone();
        let connection = input
            .connect(
                &port,
                "auris-input",
                move |_stamp, bytes, _| receive(&shared, &log, bytes),
                (),
            )
            .map_err(|error| EngineError::Midi(error.to_string()))?;
        self.connections.push((name.to_string(), connection));
        Ok(())
    }

    /// Stops listening to the input called `name`, if it was being listened to.
    ///
    /// Notes it was holding keep sounding until [`Self::release_all`]; the player cannot tell
    /// which keyboard struck what, and an input that has vanished is usually one that has been
    /// unplugged, which is for a frontend to answer with a release of everything.
    pub fn disconnect(&mut self, name: &str) {
        if let Some(position) = self.connections.iter().position(|(open, _)| open == name) {
            let (_, connection) = self.connections.remove(position);
            connection.close();
        }
    }

    /// Stops listening to every input, and silences what they were holding.
    pub fn close(&mut self) {
        for (_, connection) in self.connections.drain(..) {
            connection.close();
        }
        self.release_all();
    }

    /// Points the inputs at the engine to sound into and the track to sound on.
    ///
    /// Called again whenever either changes — the engine is replaced when the output device is,
    /// and a track's position moves when one before it is removed.
    pub fn set_target(&self, engine: &EngineHandle, target: Option<LiveTarget>) {
        let mut shared = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        shared.route = Some(Route::of(engine));
        shared.target = target;
    }

    /// Answers `bytes` exactly as if a keyboard had just sent them.
    ///
    /// For anything that plays like a keyboard without being one — an on-screen keyboard, a
    /// test — and wants the same sustain, the same routing and the same take.
    pub fn receive(&self, bytes: &[u8]) {
        receive(&self.shared, &self.log, bytes);
    }

    /// Silences every note the inputs are holding and puts back every wheel they moved.
    pub fn release_all(&self) {
        let mut shared = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        let performed = shared.player.release_all();
        carry_out(&mut shared, performed, &self.log);
    }

    /// Moves what every key sounds by `octave` octaves, from the next key down.
    pub fn set_octave(&self, octave: i8) {
        self.shared
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .player
            .set_octave(octave);
    }

    /// Chooses how a strike's reading becomes a velocity, from the next note on.
    pub fn set_velocity_response(&self, response: VelocityResponse) {
        self.shared
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .player
            .set_response(response);
    }

    /// Everything played since the last call, oldest first.
    pub fn drain(&self) -> Vec<MidiEvent> {
        self.events.try_iter().collect()
    }

    /// Whether anything has been played since the last call.
    pub fn take_activity(&self) -> bool {
        std::mem::take(
            &mut self
                .shared
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .active,
        )
    }

    /// The notes sounding now.
    pub fn sounding(&self) -> Vec<u8> {
        self.shared
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .player
            .sounding()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEAD: LiveTarget = LiveTarget {
        index: 0,
        track: TrackId(1),
    };
    const PAD: LiveTarget = LiveTarget {
        index: 1,
        track: TrackId(2),
    };

    fn on(pitch: u8) -> MidiMessage {
        MidiMessage::NoteOn {
            channel: 0,
            pitch,
            velocity: 100,
        }
    }

    fn off(pitch: u8) -> MidiMessage {
        MidiMessage::NoteOff { channel: 0, pitch }
    }

    fn pedal(down: bool) -> MidiMessage {
        MidiMessage::Controller {
            channel: 0,
            number: CC_SUSTAIN,
            value: if down { 127 } else { 0 },
        }
    }

    #[test]
    fn messages_are_read_off_the_wire() {
        assert_eq!(
            MidiMessage::parse(&[0x90, 60, 100]),
            Some(MidiMessage::NoteOn {
                channel: 0,
                pitch: 60,
                velocity: 100
            })
        );
        assert_eq!(
            MidiMessage::parse(&[0x93, 60, 0]),
            Some(MidiMessage::NoteOff {
                channel: 3,
                pitch: 60
            }),
            "a strike of nothing is a release"
        );
        assert_eq!(MidiMessage::parse(&[0x80, 61, 64]), Some(off(61)));
        assert_eq!(
            MidiMessage::parse(&[0xb0, 1, 127]),
            Some(MidiMessage::Controller {
                channel: 0,
                number: 1,
                value: 127
            })
        );
        assert_eq!(
            MidiMessage::parse(&[0xe0, 0, 0x40]),
            Some(MidiMessage::PitchBend {
                channel: 0,
                value: 0
            }),
            "the middle of the wheel is no bend"
        );
        assert_eq!(
            MidiMessage::parse(&[0xe0, 0, 0]),
            Some(MidiMessage::PitchBend {
                channel: 0,
                value: -8192
            })
        );
        assert_eq!(MidiMessage::parse(&[0xf8]), None, "clock");
        assert_eq!(MidiMessage::parse(&[0xc0, 5]), None, "program change");
        assert_eq!(MidiMessage::parse(&[0x90, 60]), None, "truncated");
        assert_eq!(MidiMessage::parse(&[]), None);
    }

    #[test]
    fn a_key_sounds_on_the_target_and_is_released_where_it_went_down() {
        let mut player = LivePlayer::default();
        let struck = player.play(on(60), Some(LEAD));
        assert_eq!(struck.len(), 1);
        assert_eq!(struck[0].0, LEAD);
        let Performed::NoteOn { pitch, velocity } = struck[0].1 else {
            panic!("a note was struck");
        };
        assert_eq!(pitch, 60);
        assert!((velocity - 100.0 / 127.0).abs() < 1e-6);
        // The selection moved while the key was down.
        let released = player.play(off(60), Some(PAD));
        assert_eq!(released, vec![(LEAD, Performed::NoteOff { pitch: 60 })]);
        assert_eq!(player.sounding().count(), 0);
    }

    #[test]
    fn with_nowhere_to_play_nothing_sounds_and_nothing_is_held() {
        let mut player = LivePlayer::default();
        assert!(player.play(on(60), None).is_empty());
        assert!(player.play(off(60), Some(LEAD)).is_empty());
    }

    #[test]
    fn the_pedal_holds_released_notes_until_it_comes_up() {
        let mut player = LivePlayer::default();
        player.play(pedal(true), Some(LEAD));
        player.play(on(60), Some(LEAD));
        player.play(on(64), Some(LEAD));
        assert!(
            player.play(off(60), Some(LEAD)).is_empty(),
            "held by the pedal"
        );
        assert!(player.play(off(64), Some(LEAD)).is_empty());
        assert_eq!(player.sounding().count(), 2);
        let released = player.play(pedal(false), Some(LEAD));
        assert_eq!(
            released,
            vec![
                (LEAD, Performed::NoteOff { pitch: 60 }),
                (LEAD, Performed::NoteOff { pitch: 64 })
            ]
        );
        assert_eq!(player.sounding().count(), 0);
    }

    #[test]
    fn a_key_still_down_when_the_pedal_comes_up_keeps_sounding() {
        let mut player = LivePlayer::default();
        player.play(pedal(true), Some(LEAD));
        player.play(on(60), Some(LEAD));
        assert!(player.play(pedal(false), Some(LEAD)).is_empty());
        assert_eq!(
            player.play(off(60), Some(LEAD)),
            vec![(LEAD, Performed::NoteOff { pitch: 60 })]
        );
    }

    #[test]
    fn striking_a_sustained_note_again_ends_the_first() {
        let mut player = LivePlayer::default();
        player.play(pedal(true), Some(LEAD));
        player.play(on(60), Some(LEAD));
        player.play(off(60), Some(LEAD));
        let again = player.play(on(60), Some(LEAD));
        assert_eq!(again[0], (LEAD, Performed::NoteOff { pitch: 60 }));
        assert!(matches!(again[1].1, Performed::NoteOn { pitch: 60, .. }));
        assert_eq!(player.sounding().count(), 1);
    }

    #[test]
    fn a_velocity_curve_reshapes_the_strike_and_a_fixed_one_ignores_it() {
        assert_eq!(VelocityResponse::default().shape(127), 1.0);
        let soft = VelocityResponse::Curve(0.5).shape(32);
        let linear = VelocityResponse::Curve(1.0).shape(32);
        let hard = VelocityResponse::Curve(2.0).shape(32);
        assert!(soft > linear && linear > hard);
        assert_eq!(VelocityResponse::Fixed(0.8).shape(10), 0.8);
        let mut player = LivePlayer::default();
        player.set_response(VelocityResponse::Fixed(0.5));
        assert_eq!(
            player.play(on(60), Some(LEAD))[0].1,
            Performed::NoteOn {
                pitch: 60,
                velocity: 0.5
            }
        );
    }

    #[test]
    fn an_octave_shift_moves_every_key_and_a_held_key_releases_what_it_struck() {
        let mut player = LivePlayer::default();
        player.set_octave(1);
        assert!(matches!(
            player.play(on(60), Some(LEAD))[0].1,
            Performed::NoteOn { pitch: 72, .. }
        ));
        // Moved back while the key is still down: the release still finds the note it struck.
        player.set_octave(0);
        assert_eq!(
            player.play(off(60), Some(LEAD)),
            vec![(LEAD, Performed::NoteOff { pitch: 72 })]
        );
        assert!(matches!(
            player.play(on(60), Some(LEAD))[0].1,
            Performed::NoteOn { pitch: 60, .. }
        ));
    }

    #[test]
    fn a_key_shifted_off_the_end_of_midi_sounds_nothing() {
        let mut player = LivePlayer::default();
        player.set_octave(MIDI_OCTAVE_LIMIT);
        assert!(player.play(on(120), Some(LEAD)).is_empty());
        assert!(player.play(off(120), Some(LEAD)).is_empty());
        player.set_octave(-MIDI_OCTAVE_LIMIT);
        assert!(player.play(on(10), Some(LEAD)).is_empty());
        player.set_octave(100);
        assert_eq!(player.octave(), MIDI_OCTAVE_LIMIT, "clamped");
    }

    #[test]
    fn the_wheel_bends_by_the_range_the_specification_gives_it() {
        let mut player = LivePlayer::default();
        let bend = |player: &mut LivePlayer, value| match player
            .play(MidiMessage::PitchBend { channel: 0, value }, Some(LEAD))[0]
            .1
        {
            Performed::PitchBend(semitones) => semitones,
            other => panic!("not a bend: {other:?}"),
        };
        assert_eq!(bend(&mut player, 0), 0.0);
        assert!((bend(&mut player, 8191) - MIDI_BEND_RANGE).abs() < 1e-6);
        assert!((bend(&mut player, -8192) + MIDI_BEND_RANGE).abs() < 1e-6);
    }

    #[test]
    fn letting_go_of_everything_silences_notes_and_puts_the_wheels_back() {
        let mut player = LivePlayer::default();
        player.play(on(60), Some(LEAD));
        player.play(pedal(true), Some(LEAD));
        player.play(on(62), Some(PAD));
        player.play(off(62), Some(PAD));
        player.play(
            MidiMessage::PitchBend {
                channel: 0,
                value: 4000,
            },
            Some(LEAD),
        );
        player.play(
            MidiMessage::Controller {
                channel: 0,
                number: 1,
                value: 90,
            },
            Some(PAD),
        );
        let out = player.release_all();
        assert!(out.contains(&(LEAD, Performed::NoteOff { pitch: 60 })));
        assert!(out.contains(&(PAD, Performed::NoteOff { pitch: 62 })));
        assert!(out.contains(&(LEAD, Performed::PitchBend(0.0))));
        assert!(out.contains(&(
            PAD,
            Performed::Controller {
                number: 1,
                value: 0.0
            }
        )));
        assert!(player.release_all().is_empty(), "and only once");
        // The pedal is up again too.
        player.play(on(60), Some(LEAD));
        assert_eq!(player.play(off(60), Some(LEAD)).len(), 1);
    }

    #[test]
    fn what_is_played_is_sounded_and_written_down() {
        let (device, engine) = crate::device::start_silent(&crate::AudioSettings::default());
        let commands = device
            .idle_commands
            .as_ref()
            .expect("a silent engine queues");
        let inputs = MidiInputs::new();
        inputs.set_target(&engine, Some(LEAD));
        inputs.receive(&[0x90, 60, 100]);
        assert!(matches!(
            commands.try_recv(),
            Ok(EngineCommand::NoteOn {
                track: 0,
                pitch: 60,
                ..
            })
        ));
        let events = inputs.drain();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].track, LEAD.track);
        assert!(inputs.take_activity());
        assert!(!inputs.take_activity(), "activity is read once");
        inputs.release_all();
        assert!(matches!(
            commands.try_recv(),
            Ok(EngineCommand::NoteOff {
                track: 0,
                pitch: 60
            })
        ));
    }
}
