//! Playing the computer keyboard as an instrument.
//!
//! Two rows of the alphabet laid out as a piano — `a` is C, `w` is C sharp, and so on up to `;`
//! — so that somebody with no MIDI keyboard on the desk can still play a part in rather than
//! draw it. Logic and GarageBand call it Musical Typing, and the layout here is theirs, key for
//! key: the layout is the part a person already knows, and a keyboard that was nearly the same
//! would be worse than one that was plainly different.
//!
//! So, from the top row down: `1` and `2` bend, `3` to `8` are the modulation wheel, `z` and `x`
//! move the octave, `c` and `v` the velocity, `tab` sustains, and everything else plays.
//!
//! # Why it is here rather than in the frontend
//!
//! Nothing about "the letter `d` means E" needs a window. The table, the octave, the velocity and
//! the bookkeeping of what is currently down are all facts about an instrument being played, and
//! a frontend's whole job is to turn a key event into [`Session::typing_press`] and
//! [`Session::typing_release`]. A second frontend gets the feature by making those two calls.
//!
//! What is genuinely a frontend's problem is stopping the letters reaching everything *else* they
//! are bound to, and that cannot be solved here: it is a fact about that toolkit's key dispatch.
//! [`shadows_musical_typing`] is the one thing this module lends to that job — the question
//! "would the keyboard swallow this keystroke?", answered from the same table the keyboard plays
//! from, so a frontend never grows a second copy of the layout to compare against.
//!
//! # Holding a note is holding a track
//!
//! Every key that is down remembers the [`TrackId`] it was pressed on. Which track a strike goes
//! to is the caller's choice and it can change *while a key is down* — the selection moves, a
//! track is added — and a release that went to wherever the selection is now would leave the
//! original note sounding for ever. The pair is stored together for that reason and for no other.
//!
//! # What the keyboard owns, and what it merely sets
//!
//! The octave and the velocity are the *keyboard's* — they are read when a note is struck and
//! sent nowhere else, so putting the keyboard away leaves them where the hands left them.
//!
//! The bend and the modulation wheel are the *instrument's*. Both are channel state: an
//! instrument holds the last value it was given until it is given another, so a keyboard that
//! stopped without putting them back would leave a track bent or shaking with nothing on screen
//! to say why, and playing the timeline would come out wrong. [`MusicalTyping::release_all`]
//! therefore returns both to zero on every track it moved them on — which is also what makes the
//! bend keys behave like a wheel rather than a latch, since letting go is the same operation.
//!
//! # Nothing here is an edit
//!
//! No command in this file records an undo step, for the reason
//! [`set_loop_enabled`](Session::set_loop_enabled) does not: this is somebody listening, not
//! somebody writing. A held chord is not in the document, and pressing Undo in the middle of one
//! should take back whatever was last written.

use auris_core::TrackId;
use auris_core::project::MODULATION_LIMIT;
use auris_core::theory::pitch::{OCTAVE, midi_name};

use super::Session;

/// What one key of the typing keyboard does.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TypingRole {
    /// Sounds the note this many semitones above the C the keyboard is centred on.
    Note(i32),
    /// Moves the keyboard down an octave.
    OctaveDown,
    /// Moves it up an octave.
    OctaveUp,
    /// Plays the next notes more gently.
    Softer,
    /// Plays them harder.
    Louder,
    /// Holds what is played, while it is itself held.
    Sustain,
    /// Bends down while it is held, and springs back when it is let go.
    BendDown,
    /// Bends up while it is held.
    BendUp,
    /// Sets the modulation wheel to this step, `0` being down and
    /// [`WHEEL_STEPS`]` - 1` all the way up.
    Wheel(u8),
}

/// The keys, in the order they sit under the hands.
///
/// Two rows of a piano: the home row is the white keys from C, the row above it the black ones.
/// `k`, `o`, `l`, `p` and `;` carry on past the octave, which is what puts a tenth under the
/// right hand without moving the keyboard. The number row is the two wheels a synthesiser has to
/// the left of its keys, in the same order: the springy one first, then the one that stays.
///
/// Written the way a keystroke is written — `"tab"` rather than a character — because a frontend
/// compares what the platform reported against this, and a table of [`char`] could not hold the
/// one key here that has a name instead of a glyph.
pub const LAYOUT: &[(&str, TypingRole)] = &[
    ("1", TypingRole::BendDown),
    ("2", TypingRole::BendUp),
    ("3", TypingRole::Wheel(0)),
    ("4", TypingRole::Wheel(1)),
    ("5", TypingRole::Wheel(2)),
    ("6", TypingRole::Wheel(3)),
    ("7", TypingRole::Wheel(4)),
    ("8", TypingRole::Wheel(5)),
    ("a", TypingRole::Note(0)),
    ("w", TypingRole::Note(1)),
    ("s", TypingRole::Note(2)),
    ("e", TypingRole::Note(3)),
    ("d", TypingRole::Note(4)),
    ("f", TypingRole::Note(5)),
    ("t", TypingRole::Note(6)),
    ("g", TypingRole::Note(7)),
    ("y", TypingRole::Note(8)),
    ("h", TypingRole::Note(9)),
    ("u", TypingRole::Note(10)),
    ("j", TypingRole::Note(11)),
    ("k", TypingRole::Note(12)),
    ("o", TypingRole::Note(13)),
    ("l", TypingRole::Note(14)),
    ("p", TypingRole::Note(15)),
    (";", TypingRole::Note(16)),
    ("z", TypingRole::OctaveDown),
    ("x", TypingRole::OctaveUp),
    ("c", TypingRole::Softer),
    ("v", TypingRole::Louder),
    ("tab", TypingRole::Sustain),
];

/// The octave the keyboard starts on, so that `a` is C3 — an octave below middle C.
///
/// Low enough that `k` upwards lands where a melody is comfortable, and high enough that a bass
/// line is one press of `z` away rather than three.
pub const DEFAULT_OCTAVE: i32 = 3;

/// The lowest and highest octave the keyboard can be moved to.
///
/// The top is where it is because the keyboard is seventeen semitones wide: from C8 the last key
/// is E9, which is still a MIDI note, and from any higher it would not be.
pub const OCTAVE_RANGE: std::ops::RangeInclusive<i32> = -1..=8;

/// How hard the keyboard strikes to begin with.
pub const DEFAULT_VELOCITY: f32 = 0.75;

/// How far one press of `c` or `v` moves the velocity.
///
/// An eighth, so the whole range is eight presses across and every one of them is audible. A
/// finer step would be a control that appears not to work.
pub const VELOCITY_STEP: f32 = 0.125;

/// How far `1` and `2` bend, in semitones.
///
/// A whole tone, which is what a bend wheel does on nearly every synthesiser ever sold and so
/// what a hand expects of one. Deliberately *not*
/// [`BEND_LIMIT`](auris_core::project::BEND_LIMIT), which is an octave: that is how far a curve
/// may be *drawn*, where the shape is visible and can be aimed, and an octave arriving on one
/// keypress of a key with no travel to it would be unplayable.
pub const TYPING_BEND: f32 = 2.0;

/// How many positions the modulation keys offer, `3` to `8`.
///
/// Six, because six is how many keys Logic gives it. `3` is the wheel down rather than a first
/// step up, so that there is a key that turns it off — a wheel that could only be moved and never
/// put back would be a trap.
pub const WHEEL_STEPS: u8 = 6;

/// A note the keyboard is asking to be sounded.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Struck {
    /// The track to sound it on.
    pub track: TrackId,
    /// Which note.
    pub pitch: u8,
    /// How hard.
    pub velocity: f32,
}

/// A note the keyboard is asking to be released, and where from.
pub type Release = (TrackId, u8);

/// Everything one key event asks the instruments to do.
///
/// One shape for a press, a release and a letting go of everything, because any of the three can
/// ask for any of these — a press strikes a note *or* moves a wheel, a release lets go of one
/// note or of six, and putting the keyboard away does all of it at once. A caller that handled
/// each field would be right whichever call it came from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Played {
    /// Whether the key is one of the keyboard's, and so must not reach anything else.
    ///
    /// True for `z` as much as for `a`: a control key is claimed even though it sounds nothing,
    /// because a keyboard whose octave switch also toggled a panel would be worse than one with
    /// no octave switch at all.
    pub claimed: bool,
    /// The note to strike.
    pub struck: Option<Struck>,
    /// The notes to release.
    pub released: Vec<Release>,
    /// Where the bend now sits, on each track it moved on, in semitones.
    pub bend: Vec<(TrackId, f32)>,
    /// Where the modulation wheel now sits, on each track it moved on.
    pub modulation: Vec<(TrackId, f32)>,
}

impl Played {
    /// A key that is one of the keyboard's and asks for nothing else.
    fn claimed() -> Self {
        Self {
            claimed: true,
            ..Self::default()
        }
    }
}

/// One key that is down.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Held {
    /// Which key, as it appears in [`LAYOUT`].
    key: &'static str,
    /// The track it was pressed on.
    track: TrackId,
    /// The note it is sounding, if it sounded one.
    note: Option<u8>,
}

/// The computer keyboard, played as an instrument.
///
/// Pure bookkeeping: it decides *what* to sound and sounds nothing itself. [`Session`] turns the
/// answers into engine commands, which is what makes every rule here testable without an audio
/// device.
#[derive(Clone, Debug)]
pub struct MusicalTyping {
    /// Whether the keyboard is being played at all.
    enabled: bool,
    /// The octave `a` sounds C in.
    octave: i32,
    /// How hard the next note is struck.
    velocity: f32,
    /// Whether what is played is being held past its key.
    sustain: bool,
    /// Where the modulation wheel has been set.
    wheel: f32,
    /// Which keys are down.
    held: Vec<Held>,
    /// Notes let go of while the sustain key was down, still sounding.
    sustained: Vec<Release>,
    /// Tracks whose bend or wheel this keyboard has moved, so it can put them back.
    touched: Vec<TrackId>,
}

impl Default for MusicalTyping {
    fn default() -> Self {
        Self {
            enabled: false,
            octave: DEFAULT_OCTAVE,
            velocity: DEFAULT_VELOCITY,
            sustain: false,
            wheel: 0.0,
            held: Vec::new(),
            sustained: Vec::new(),
            touched: Vec::new(),
        }
    }
}

impl MusicalTyping {
    /// Whether the keyboard is being played.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The octave `a` sounds C in.
    pub fn octave(&self) -> i32 {
        self.octave
    }

    /// How hard the next note is struck, in `0.0..=1.0`.
    pub fn velocity(&self) -> f32 {
        self.velocity
    }

    /// Whether notes are being held past their keys.
    pub fn sustain(&self) -> bool {
        self.sustain
    }

    /// Where the modulation wheel sits, in `0.0..=1.0`.
    pub fn wheel(&self) -> f32 {
        self.wheel
    }

    /// How far the bend keys currently have it bent, in semitones.
    ///
    /// Zero with neither held and with both: they are one wheel pushed from two sides, and a
    /// hand on both ends of it has not moved it.
    pub fn bend(&self) -> f32 {
        let direction = |role| {
            self.held
                .iter()
                .any(|held| Self::role(held.key) == Some(role))
        };
        let down = direction(TypingRole::BendDown);
        let up = direction(TypingRole::BendUp);
        match (down, up) {
            (true, false) => -TYPING_BEND,
            (false, true) => TYPING_BEND,
            _ => 0.0,
        }
    }

    /// The name of the note `a` sounds, such as `C3`, for a readout to say where the hands are.
    pub fn octave_name(&self) -> String {
        midi_name(self.root())
    }

    /// The MIDI note `a` sounds.
    ///
    /// Public for the same reason [`Self::sounding`] is: something drawing this keyboard has to
    /// know which note each of its keys is on to light the ones that are down, and a frontend
    /// working that out from [`Self::octave`] would be a second copy of a formula that has to
    /// agree with the one a key is struck by, exactly, or light the wrong key.
    pub fn root(&self) -> i32 {
        (self.octave + 1) * OCTAVE
    }

    /// Which modulation key left the wheel where it is.
    ///
    /// For a drawn keyboard lighting the one of `3` to `8` that is currently in force. `None`
    /// means it sits somewhere none of them could have put it, which nothing here does today —
    /// but the answer is *drawn*, and a keyboard lighting `5` for a value `5` does not produce
    /// would be a readout that lies.
    pub fn wheel_step(&self) -> Option<u8> {
        (0..WHEEL_STEPS).find(|&step| (Self::wheel_at(step) - self.wheel).abs() < 1e-4)
    }

    /// Every note the keyboard has sounding, held or sustained.
    ///
    /// In no particular order, and a pitch may appear twice — the same note struck on two tracks
    /// is two notes. What this is for is drawing a keyboard with the played keys lit.
    pub fn sounding(&self) -> impl Iterator<Item = u8> + '_ {
        let held = self.held.iter().filter_map(|held| held.note);
        held.chain(self.sustained.iter().map(|&(_, pitch)| pitch))
    }

    /// What `key` does, or `None` when it is not one of the keyboard's.
    pub fn role(key: &str) -> Option<TypingRole> {
        LAYOUT
            .iter()
            .find(|(name, _)| *name == key)
            .map(|&(_, role)| role)
    }

    /// What the modulation wheel reads at `step`, `0` being down.
    fn wheel_at(step: u8) -> f32 {
        let top = f32::from(WHEEL_STEPS.saturating_sub(1)).max(1.0);
        (f32::from(step) / top).clamp(0.0, 1.0) * MODULATION_LIMIT
    }

    /// Presses `key`, sounding or moving something on `track` if that is what the key does.
    ///
    /// A key already down does nothing and is still claimed: a platform that repeats a held key
    /// would otherwise machine-gun the note. The guard is here as well as in whatever a frontend
    /// can ask about auto-repeat, because not every platform can be asked.
    pub fn press(&mut self, key: &str, track: TrackId) -> Played {
        let Some((key, role)) = LAYOUT.iter().find(|(name, _)| *name == key).copied() else {
            return Played::default();
        };
        if self.held.iter().any(|held| held.key == key) {
            return Played::claimed();
        }
        // Recorded as down before the role is acted on, because the bend is read *from* the keys
        // that are down: a `2` that had not been written down yet would compute a bend of zero.
        self.held.push(Held {
            key,
            track,
            note: None,
        });

        let mut played = Played::claimed();
        match role {
            TypingRole::Note(semitones) => {
                played.struck = self.strike(track, semitones);
                if let Some(struck) = played.struck {
                    // Held again with the note against it, so the release knows what to let go of.
                    if let Some(held) = self.held.last_mut() {
                        held.note = Some(struck.pitch);
                    }
                }
            }
            TypingRole::OctaveDown => self.shift_octave(-1),
            TypingRole::OctaveUp => self.shift_octave(1),
            TypingRole::Softer => self.shift_velocity(-VELOCITY_STEP),
            TypingRole::Louder => self.shift_velocity(VELOCITY_STEP),
            TypingRole::Sustain => self.sustain = true,
            TypingRole::BendDown | TypingRole::BendUp => {
                played.bend = vec![(track, self.bend())];
                self.touch(track);
            }
            TypingRole::Wheel(step) => {
                self.wheel = Self::wheel_at(step);
                played.modulation = vec![(track, self.wheel)];
                self.touch(track);
            }
        }
        played
    }

    /// Releases `key`, and says what stops or springs back because of it.
    ///
    /// Usually one note or none. Letting go of the sustain key is the exception and returns
    /// everything that was waiting on it; letting go of a bend key returns the bend.
    pub fn release(&mut self, key: &str) -> Played {
        let Some(index) = self.held.iter().position(|held| held.key == key) else {
            return Played::default();
        };
        let held = self.held.remove(index);
        let mut played = Played::claimed();
        match Self::role(held.key) {
            Some(TypingRole::Sustain) => {
                self.sustain = false;
                played.released = std::mem::take(&mut self.sustained);
            }
            Some(TypingRole::BendDown) | Some(TypingRole::BendUp) => {
                // Computed after the removal above, so releasing one of two held bend keys hands
                // the wheel back to the other rather than centring it.
                played.bend = vec![(held.track, self.bend())];
            }
            _ => {
                if let Some(pitch) = held.note {
                    let note = (held.track, pitch);
                    match self.sustain {
                        true => self.sustained.push(note),
                        false => played.released = vec![note],
                    }
                }
            }
        }
        played
    }

    /// Lets go of everything, and puts back everything this keyboard moved.
    ///
    /// The octave and the velocity stay where the hands put them — they are the keyboard's own —
    /// but the bend and the wheel are the instrument's, and are returned to zero on every track
    /// they were moved on. See the module documentation for why the two are treated differently.
    pub fn release_all(&mut self) -> Played {
        let released = self
            .held
            .drain(..)
            .filter_map(|held| Some((held.track, held.note?)))
            .chain(self.sustained.drain(..))
            .collect();
        let put_back: Vec<(TrackId, f32)> = self
            .touched
            .drain(..)
            .map(|track| (track, 0.0))
            .collect::<Vec<_>>();
        self.sustain = false;
        self.wheel = 0.0;
        Played {
            claimed: false,
            struck: None,
            released,
            bend: put_back.clone(),
            modulation: put_back,
        }
    }

    /// Remembers that this keyboard has moved something on `track` that it will have to put back.
    fn touch(&mut self, track: TrackId) {
        if !self.touched.contains(&track) {
            self.touched.push(track);
        }
    }

    /// Strikes the note `semitones` above the keyboard's C, if MIDI has such a note.
    ///
    /// Out of range is nothing, rather than the nearest note: the octave is clamped so that this
    /// cannot happen from the keys, and a pitch invented for a key that has none would put two
    /// keys on one note.
    fn strike(&mut self, track: TrackId, semitones: i32) -> Option<Struck> {
        let pitch = self.root() + semitones;
        if !(0..=127).contains(&pitch) {
            return None;
        }
        let pitch = pitch as u8;
        // A note struck while an older copy of it is sustaining retriggers the one voice the
        // instrument has for that pitch, so the sustained entry now names a note that is not
        // its own. Dropping it hands the note to the key that is holding it.
        self.sustained.retain(|&note| note != (track, pitch));
        Some(Struck {
            track,
            pitch,
            velocity: self.velocity,
        })
    }

    /// Moves the keyboard by `octaves`, within [`OCTAVE_RANGE`].
    fn shift_octave(&mut self, octaves: i32) {
        self.octave = (self.octave + octaves).clamp(*OCTAVE_RANGE.start(), *OCTAVE_RANGE.end());
    }

    /// Moves the striking velocity by `delta`, never as far as silence.
    fn shift_velocity(&mut self, delta: f32) {
        self.velocity = (self.velocity + delta).clamp(VELOCITY_STEP, 1.0);
    }
}

/// Whether the typing keyboard would swallow `keystroke`.
///
/// A frontend asks this about every binding it installs, so the commands on bare letters stop
/// firing while the keyboard is being played and every other binding carries on. Asked about the
/// keystroke rather than about the command, so a key the user has rebound is answered as
/// correctly as a default.
///
/// Only a bare key counts. A modifier means the hand is not playing, and a keystroke of several
/// chunks is a chord nobody plays a note with; both must keep working, because somebody stopping
/// to save in the middle of a take presses the same ⌘S they always do.
pub fn shadows_musical_typing(keystroke: &str) -> bool {
    let bare = !keystroke.contains(char::is_whitespace) && !keystroke.contains('-');
    bare && MusicalTyping::role(keystroke).is_some()
}

impl Session {
    /// The typing keyboard, for a frontend showing what it is doing.
    pub fn typing_keyboard(&self) -> &MusicalTyping {
        &self.typing
    }

    /// Whether the computer keyboard is being played as an instrument.
    pub fn musical_typing(&self) -> bool {
        self.typing.enabled()
    }

    /// Turns the typing keyboard on or off, silencing and unbending whatever it was holding.
    pub fn set_musical_typing(&mut self, enabled: bool) {
        self.release_typed_notes();
        self.typing.enabled = enabled;
    }

    /// Presses `key` on the typing keyboard, playing it on `track`.
    ///
    /// Returns whether the key belonged to the keyboard, which is what tells a frontend to keep
    /// it from everything else. Nothing happens at all while the keyboard is off, so a frontend
    /// that forgets to check cannot play a note with a stray keystroke.
    pub fn typing_press(&mut self, track: TrackId, key: &str) -> bool {
        if !self.typing.enabled() {
            return false;
        }
        let played = self.typing.press(key, track);
        self.perform(played)
    }

    /// Releases `key`, stopping or springing back whatever it was holding.
    ///
    /// Answered even while the keyboard is off, because it may have been switched off with a key
    /// still down — and the release is the last chance that note has to stop.
    pub fn typing_release(&mut self, key: &str) -> bool {
        let played = self.typing.release(key);
        self.perform(played) && self.typing.enabled()
    }

    /// Silences every note the typing keyboard is holding, and puts back what it moved.
    ///
    /// For everything that ends a performance without a key being lifted: the mode going off, the
    /// window losing focus, a panic. The keyboard keeps its octave and its velocity.
    pub fn release_typed_notes(&mut self) {
        let played = self.typing.release_all();
        self.perform(played);
    }

    /// Carries out what the keyboard asked for, and passes its answer through.
    ///
    /// The wheels move before the notes, for two reasons. A note struck in the same breath as a
    /// bend is then struck *into* the bend rather than a moment before it — and the audition
    /// queue an engine command lands in has a fixed size and drops what will not fit, so the
    /// events that must not be lost go in first. A dropped note-off leaves a note sounding, which
    /// Escape clears; a dropped return-to-zero would leave a track bent, which looks like an
    /// instrument that has gone out of tune. (Escape clears that too, as it happens —
    /// `RenderGraph::panic` resets each instrument, and an instrument's reset centres both wheels
    /// — but a thing that needs the panic button is still a thing that went wrong.)
    fn perform(&mut self, played: Played) -> bool {
        for (track, semitones) in played.bend {
            self.pitch_bend(track, semitones);
        }
        for (track, amount) in played.modulation {
            self.modulation(track, amount);
        }
        for (track, pitch) in played.released {
            self.note_off(track, pitch);
        }
        if let Some(struck) = played.struck {
            self.note_on(struck.track, struck.pitch, struck.velocity);
        }
        played.claimed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::{session, undo_depth};
    use auris_core::theory::pitch::MIDDLE_C;

    /// A track to hang held notes on. Which track it is does not matter to the keyboard.
    const TRACK: TrackId = TrackId(1);
    const OTHER: TrackId = TrackId(2);

    fn keyboard() -> MusicalTyping {
        MusicalTyping {
            enabled: true,
            ..MusicalTyping::default()
        }
    }

    /// Presses `key` and lets go of it again, answering with what it struck.
    fn tap(keys: &mut MusicalTyping, key: &str) -> Option<Struck> {
        let struck = keys.press(key, TRACK).struck;
        keys.release(key);
        struck
    }

    /// A session with one instrument track, and the keyboard switched on.
    fn playing() -> (Session, TrackId) {
        let mut session = session();
        let track = session
            .add_default_instrument_track("Lead")
            .expect("a headless session can hold a track");
        session.set_musical_typing(true);
        (session, track)
    }

    #[test]
    fn the_home_row_is_a_c_major_scale_and_the_row_above_it_the_black_keys() {
        let mut keys = keyboard();
        let pitch = |keys: &mut MusicalTyping, key: &str| tap(keys, key).expect("a note key").pitch;

        assert_eq!(pitch(&mut keys, "a"), 48, "a is C3");
        assert_eq!(pitch(&mut keys, "s"), 50, "s is D3");
        assert_eq!(pitch(&mut keys, "d"), 52, "d is E3");
        assert_eq!(pitch(&mut keys, "w"), 49, "w is the black key between them");
        assert_eq!(pitch(&mut keys, "k"), MIDDLE_C as u8, "k is middle C");
        assert_eq!(pitch(&mut keys, ";"), 64, "and the top of the keyboard");
    }

    #[test]
    fn the_layout_is_the_one_logic_and_garageband_use() {
        // Every key of theirs, in their meaning. The value of this keyboard is that a hand that
        // has played one already knows it, so a key quietly moved here is a regression even if
        // what it moved to is defensible.
        let role =
            |key| MusicalTyping::role(key).unwrap_or_else(|| panic!("`{key}` plays nothing"));

        assert_eq!(role("1"), TypingRole::BendDown);
        assert_eq!(role("2"), TypingRole::BendUp);
        assert_eq!(role("3"), TypingRole::Wheel(0), "3 is the wheel down");
        assert_eq!(
            role("8"),
            TypingRole::Wheel(WHEEL_STEPS - 1),
            "8 is all the way up"
        );
        assert_eq!(role("z"), TypingRole::OctaveDown);
        assert_eq!(role("x"), TypingRole::OctaveUp);
        assert_eq!(role("c"), TypingRole::Softer);
        assert_eq!(role("v"), TypingRole::Louder);
        assert_eq!(role("tab"), TypingRole::Sustain);

        // And the notes, in one line each way: the white keys along the home row from C, the
        // black ones above them where a piano has them.
        let whites = ["a", "s", "d", "f", "g", "h", "j", "k", "l", ";"];
        let major = [0, 2, 4, 5, 7, 9, 11, 12, 14, 16];
        for (key, semitones) in whites.iter().zip(major) {
            assert_eq!(
                role(key),
                TypingRole::Note(semitones),
                "the white key `{key}`"
            );
        }
        let blacks = ["w", "e", "t", "y", "u", "o", "p"];
        let accidentals = [1, 3, 6, 8, 10, 13, 15];
        for (key, semitones) in blacks.iter().zip(accidentals) {
            assert_eq!(
                role(key),
                TypingRole::Note(semitones),
                "the black key `{key}`"
            );
        }
    }

    #[test]
    fn the_layout_has_no_key_meaning_two_things_and_no_note_meaning_two_keys() {
        let mut keys: Vec<&str> = LAYOUT.iter().map(|&(key, _)| key).collect();
        let count = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), count, "a key appears twice in the layout");

        let mut notes: Vec<i32> = LAYOUT
            .iter()
            .filter_map(|&(_, role)| match role {
                TypingRole::Note(semitones) => Some(semitones),
                _ => None,
            })
            .collect();
        let count = notes.len();
        notes.sort_unstable();
        notes.dedup();
        assert_eq!(notes.len(), count, "two keys sound the same note");
    }

    #[test]
    fn every_key_of_every_reachable_octave_is_a_midi_note() {
        // Which is what the clamp on the octave is for: a keyboard whose top keys silently did
        // nothing would look broken, and one that folded them back would double a note.
        for octave in OCTAVE_RANGE {
            let mut keys = MusicalTyping {
                enabled: true,
                octave,
                ..MusicalTyping::default()
            };
            for &(key, role) in LAYOUT {
                let TypingRole::Note(_) = role else { continue };
                assert!(
                    tap(&mut keys, key).is_some(),
                    "`{key}` sounds nothing on octave {octave}"
                );
            }
        }
    }

    #[test]
    fn a_key_pressed_and_released_is_one_note_and_one_release() {
        let mut keys = keyboard();

        let played = keys.press("a", TRACK);
        assert!(played.claimed);
        assert_eq!(
            played.struck,
            Some(Struck {
                track: TRACK,
                pitch: 48,
                velocity: DEFAULT_VELOCITY,
            })
        );
        assert_eq!(keys.sounding().collect::<Vec<_>>(), vec![48]);

        assert_eq!(keys.release("a").released, vec![(TRACK, 48)]);
        assert_eq!(keys.sounding().count(), 0);
        assert_eq!(
            keys.release("a"),
            Played::default(),
            "and not a second time"
        );
    }

    #[test]
    fn a_key_held_down_does_not_strike_again() {
        // Auto-repeat: the platform sends a stream of presses for one finger, and a note per
        // repeat would be a drum roll where a held note was meant.
        let mut keys = keyboard();
        assert!(keys.press("a", TRACK).struck.is_some());
        for _ in 0..5 {
            let played = keys.press("a", TRACK);
            assert!(played.claimed, "still the keyboard's key");
            assert_eq!(played.struck, None, "and still the same note");
        }
        assert_eq!(keys.sounding().collect::<Vec<_>>(), vec![48]);
        assert_eq!(keys.release("a").released, vec![(TRACK, 48)]);
    }

    #[test]
    fn a_held_octave_key_steps_the_octave_once() {
        let mut keys = keyboard();
        for _ in 0..5 {
            keys.press("x", TRACK);
        }
        assert_eq!(
            keys.octave(),
            DEFAULT_OCTAVE + 1,
            "once per press, not once per repeat"
        );
        keys.release("x");
        keys.press("x", TRACK);
        assert_eq!(
            keys.octave(),
            DEFAULT_OCTAVE + 2,
            "and again once let go of"
        );
    }

    #[test]
    fn a_chord_is_held_and_let_go_of_a_note_at_a_time() {
        let mut keys = keyboard();
        for key in ["a", "d", "g"] {
            assert!(keys.press(key, TRACK).struck.is_some());
        }
        assert_eq!(keys.sounding().collect::<Vec<_>>(), vec![48, 52, 55]);

        assert_eq!(keys.release("d").released, vec![(TRACK, 52)]);
        assert_eq!(
            keys.sounding().collect::<Vec<_>>(),
            vec![48, 55],
            "the other two are still down"
        );
    }

    #[test]
    fn a_note_is_released_on_the_track_it_was_struck_on() {
        // The selection can move while a key is down. A release that followed it would leave the
        // first track sounding for ever, which is the one bug in a keyboard nobody can work round.
        let mut keys = keyboard();
        keys.press("a", TRACK);
        keys.press("d", OTHER);

        assert_eq!(keys.release("a").released, vec![(TRACK, 48)]);
        assert_eq!(keys.release("d").released, vec![(OTHER, 52)]);
    }

    #[test]
    fn the_octave_keys_move_the_whole_keyboard() {
        let mut keys = keyboard();
        assert_eq!(keys.octave(), DEFAULT_OCTAVE);
        assert_eq!(keys.octave_name(), "C3");

        assert!(
            tap(&mut keys, "x").is_none(),
            "a control key sounds nothing"
        );
        assert_eq!(keys.octave(), DEFAULT_OCTAVE + 1);
        assert_eq!(keys.octave_name(), "C4");
        assert_eq!(
            tap(&mut keys, "a").expect("a note").pitch,
            MIDDLE_C as u8,
            "a is middle C an octave up"
        );

        for _ in 0..20 {
            tap(&mut keys, "z");
        }
        assert_eq!(keys.octave(), *OCTAVE_RANGE.start(), "and stops at the end");
    }

    #[test]
    fn the_velocity_keys_move_how_hard_the_next_note_is_struck() {
        let mut keys = keyboard();
        tap(&mut keys, "v");
        let struck = tap(&mut keys, "a").expect("a note");
        assert!(
            (struck.velocity - (DEFAULT_VELOCITY + VELOCITY_STEP)).abs() < 1e-6,
            "{} is not one step above {DEFAULT_VELOCITY}",
            struck.velocity
        );

        for _ in 0..20 {
            tap(&mut keys, "c");
        }
        assert!(keys.velocity() > 0.0, "never all the way to silence");
        for _ in 0..20 {
            tap(&mut keys, "v");
        }
        assert!(keys.velocity() <= 1.0, "and never past the top");
    }

    #[test]
    fn a_note_already_sounding_keeps_the_velocity_it_was_struck_at() {
        // It has already been sent to the instrument, so nothing about turning the dial afterwards
        // can reach it, and a keyboard implying otherwise would be lying.
        let mut keys = keyboard();
        let first = keys.press("a", TRACK).struck.expect("a note").velocity;
        tap(&mut keys, "v");
        let second = keys.press("d", TRACK).struck.expect("a note").velocity;

        assert!(
            (first - DEFAULT_VELOCITY).abs() < 1e-6,
            "the first note moved"
        );
        assert!(second > first, "and the second one is the louder");
    }

    #[test]
    fn the_bend_keys_bend_while_they_are_held_and_spring_back() {
        let mut keys = keyboard();
        assert_eq!(keys.bend(), 0.0);

        assert_eq!(keys.press("2", TRACK).bend, vec![(TRACK, TYPING_BEND)]);
        assert_eq!(keys.bend(), TYPING_BEND, "up while it is down");
        assert_eq!(keys.release("2").bend, vec![(TRACK, 0.0)], "and back");
        assert_eq!(keys.bend(), 0.0);

        assert_eq!(keys.press("1", TRACK).bend, vec![(TRACK, -TYPING_BEND)]);
        assert_eq!(keys.release("1").bend, vec![(TRACK, 0.0)]);
    }

    #[test]
    fn a_note_struck_while_the_bend_is_held_is_not_unbent_by_it() {
        // The bend is channel state: it was sent when the key went down and the instrument is
        // still holding it, so a note struck now needs nothing sent for it. What must *not*
        // happen is the strike reporting a bend of its own and centring the wheel.
        let mut keys = keyboard();
        keys.press("2", TRACK);
        let played = keys.press("a", TRACK);

        assert!(played.struck.is_some());
        assert!(played.bend.is_empty(), "the strike moved the wheel");
        assert_eq!(keys.bend(), TYPING_BEND, "which is still where it was");
    }

    #[test]
    fn releasing_one_of_two_held_bend_keys_hands_the_wheel_to_the_other() {
        let mut keys = keyboard();
        keys.press("2", TRACK);
        assert_eq!(
            keys.press("1", TRACK).bend,
            vec![(TRACK, 0.0)],
            "a hand on both ends has not moved it"
        );

        assert_eq!(
            keys.release("2").bend,
            vec![(TRACK, -TYPING_BEND)],
            "letting go of up leaves down holding it"
        );
        assert_eq!(keys.release("1").bend, vec![(TRACK, 0.0)]);
    }

    #[test]
    fn the_wheel_keys_set_the_modulation_and_leave_it_there() {
        let mut keys = keyboard();
        assert_eq!(keys.wheel(), 0.0);

        assert_eq!(keys.press("8", TRACK).modulation, vec![(TRACK, 1.0)]);
        assert_eq!(
            keys.release("8"),
            Played::claimed(),
            "a wheel does not spring back"
        );
        assert_eq!(keys.wheel(), 1.0, "it stays where it was put");

        assert_eq!(keys.press("3", TRACK).modulation, vec![(TRACK, 0.0)]);
        assert_eq!(keys.wheel(), 0.0, "and 3 is how it comes back down");
    }

    #[test]
    fn the_wheel_keys_climb_in_order_from_off_to_all_the_way_up() {
        let mut keys = keyboard();
        let mut last = -1.0;
        for key in ["3", "4", "5", "6", "7", "8"] {
            let amount = keys.press(key, TRACK).modulation;
            let &(_, amount) = amount.first().expect("a wheel key moves the wheel");
            assert!(amount > last, "`{key}` is not above the key before it");
            assert!((0.0..=MODULATION_LIMIT).contains(&amount));
            last = amount;
            keys.release(key);
        }
        assert_eq!(last, MODULATION_LIMIT, "8 is all the way up");
    }

    #[test]
    fn the_wheel_names_the_key_that_set_it_and_the_keyboard_names_the_note_it_starts_on() {
        // Both are for something drawing this keyboard, and both are answers a frontend would
        // otherwise work out for itself from a formula it cannot see.
        let mut keys = keyboard();
        assert_eq!(keys.wheel_step(), Some(0), "3, the wheel down");
        for (key, step) in [("4", 1), ("6", 3), ("8", WHEEL_STEPS - 1), ("3", 0)] {
            tap(&mut keys, key);
            assert_eq!(
                keys.wheel_step(),
                Some(step),
                "`{key}` lights the wrong key"
            );
        }

        assert_eq!(keys.root(), 48, "a is C3");
        assert_eq!(midi_name(keys.root()), keys.octave_name());
        tap(&mut keys, "x");
        assert_eq!(
            keys.root(),
            MIDDLE_C,
            "and the note every key is drawn against moves with the octave"
        );
        assert_eq!(
            tap(&mut keys, "a").expect("a note").pitch as i32,
            keys.root(),
            "which is the note it strikes"
        );
    }

    #[test]
    fn sustain_holds_notes_past_their_keys_and_lets_go_of_them_together() {
        let mut keys = keyboard();
        assert!(keys.press("tab", TRACK).claimed);
        assert!(keys.sustain());

        keys.press("a", TRACK);
        assert!(keys.release("a").released.is_empty(), "held by the sustain");
        keys.press("d", TRACK);
        assert!(keys.release("d").released.is_empty());
        assert_eq!(
            keys.sounding().collect::<Vec<_>>(),
            vec![48, 52],
            "both still sounding"
        );

        assert_eq!(keys.release("tab").released, vec![(TRACK, 48), (TRACK, 52)]);
        assert!(!keys.sustain());
        assert_eq!(keys.sounding().count(), 0);
    }

    #[test]
    fn a_note_still_down_when_the_sustain_is_let_go_is_still_down() {
        let mut keys = keyboard();
        keys.press("tab", TRACK);
        keys.press("a", TRACK);

        assert!(
            keys.release("tab").released.is_empty(),
            "nothing was waiting on it"
        );
        assert_eq!(
            keys.sounding().collect::<Vec<_>>(),
            vec![48],
            "the finger is still on it"
        );
        assert_eq!(keys.release("a").released, vec![(TRACK, 48)]);
    }

    #[test]
    fn a_sustained_note_struck_again_is_released_once() {
        // Striking it again retriggers the one voice the instrument has for that pitch. Two
        // entries would mean two releases, and the second would cut a note somebody was holding.
        let mut keys = keyboard();
        keys.press("tab", TRACK);
        keys.press("a", TRACK);
        keys.release("a");
        keys.press("a", TRACK);

        assert_eq!(
            keys.sounding().collect::<Vec<_>>(),
            vec![48],
            "one note, not two"
        );
        assert!(
            keys.release("tab").released.is_empty(),
            "the key has it back"
        );
        assert_eq!(keys.release("a").released, vec![(TRACK, 48)]);
    }

    #[test]
    fn releasing_everything_puts_back_the_notes_and_both_wheels_and_keeps_the_settings() {
        let mut keys = keyboard();
        tap(&mut keys, "x");
        tap(&mut keys, "v");
        keys.press("8", OTHER);
        keys.press("2", TRACK);
        keys.press("tab", TRACK);
        keys.press("a", TRACK);
        keys.release("a");
        keys.press("d", TRACK);

        let played = keys.release_all();
        assert_eq!(played.released, vec![(TRACK, 64), (TRACK, 60)]);
        assert_eq!(
            played.bend,
            vec![(OTHER, 0.0), (TRACK, 0.0)],
            "every track this keyboard bent is put back, not only the last"
        );
        assert_eq!(played.modulation, vec![(OTHER, 0.0), (TRACK, 0.0)]);

        assert_eq!(keys.sounding().count(), 0);
        assert!(!keys.sustain(), "the pedal comes up too");
        assert_eq!(keys.bend(), 0.0);
        assert_eq!(keys.wheel(), 0.0, "the instrument is left as it was found");
        assert_eq!(keys.octave(), DEFAULT_OCTAVE + 1, "but the hands stay put");
        assert!(keys.velocity() > DEFAULT_VELOCITY);
    }

    #[test]
    fn a_key_the_keyboard_does_not_use_is_left_alone() {
        let mut keys = keyboard();
        for key in ["q", "space", "escape", "9", "0", "b"] {
            let played = keys.press(key, TRACK);
            assert_eq!(played, Played::default(), "`{key}` was claimed");
            assert_eq!(keys.release(key), Played::default());
        }
    }

    #[test]
    fn the_keystrokes_a_binding_loses_to_are_the_bare_ones_the_keyboard_plays() {
        for &(key, _) in LAYOUT {
            assert!(
                shadows_musical_typing(key),
                "`{key}` is played but not taken"
            );
        }
        for keystroke in [
            "space",
            "escape",
            "enter",
            "secondary-s",
            "secondary-k",
            "shift-tab",
            "secondary-shift-a",
            "g g",
            "9",
            "b",
        ] {
            assert!(
                !shadows_musical_typing(keystroke),
                "`{keystroke}` was taken and should not have been"
            );
        }
    }

    #[test]
    fn a_session_plays_nothing_until_the_keyboard_is_switched_on() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();

        assert!(!session.musical_typing());
        assert!(!session.typing_press(track, "a"), "off, so not claimed");
        assert_eq!(session.typing_keyboard().sounding().count(), 0);

        session.set_musical_typing(true);
        assert!(session.musical_typing());
        assert!(session.typing_press(track, "a"));
        assert_eq!(
            session.typing_keyboard().sounding().collect::<Vec<_>>(),
            vec![48]
        );
        assert!(session.typing_release("a"));
        assert_eq!(session.typing_keyboard().sounding().count(), 0);
    }

    #[test]
    fn switching_the_keyboard_off_lets_go_of_what_it_was_holding() {
        let (mut session, track) = playing();
        session.typing_press(track, "a");
        session.typing_press(track, "d");
        session.typing_press(track, "8");

        session.set_musical_typing(false);
        assert_eq!(
            session.typing_keyboard().sounding().count(),
            0,
            "a note with no key left to lift is a note that never stops"
        );
        assert_eq!(
            session.typing_keyboard().wheel(),
            0.0,
            "and an instrument left modulated would play the timeline back wrong"
        );
    }

    #[test]
    fn a_release_arriving_after_the_keyboard_went_off_still_lets_go() {
        // Switching the mode off with a finger down is exactly how this happens, and the release
        // is the last chance that note has.
        let (mut session, track) = playing();
        session.typing_press(track, "a");
        session.typing.enabled = false;

        assert!(
            !session.typing_release("a"),
            "the keyboard is off, so the key is not claimed"
        );
        assert_eq!(
            session.typing_keyboard().sounding().count(),
            0,
            "and the note was let go of all the same"
        );
    }

    #[test]
    fn playing_the_keyboard_writes_nothing_to_the_document_and_nothing_to_the_history() {
        let (mut session, track) = playing();
        session.forget_history();
        let before = session.project().clone();

        for key in ["a", "d", "g", "x", "v", "2", "6", "tab"] {
            session.typing_press(track, key);
        }
        for key in ["a", "d", "g", "x", "v", "2", "6", "tab"] {
            session.typing_release(key);
        }
        session.set_musical_typing(false);

        assert_eq!(session.project(), &before, "a performance is not an edit");
        assert_eq!(undo_depth(&mut session), 0, "and not an undo step");
    }
}
