//! Turning Auris note events into whatever a plugin's note port speaks.
//!
//! A CLAP note port declares which *dialects* it understands, and a host that guesses wrong sends
//! events into a void: the plugin does not fail, it simply plays nothing. So the dialect is read
//! once, at activation, and every event is translated to it.
//!
//! The two dialects are not equivalent. CLAP's own is the better one for notes — the key, channel
//! and note id are separate fields rather than packed into three bytes, and pitch bend arrives as
//! a *tuning in semitones* instead of a fourteen-bit number the plugin has to scale by a range the
//! host never told it. MIDI is the better one for the modulation wheel, which CLAP has no note
//! event for at all. A plugin that speaks both gets the better half of each, which is why
//! [`NoteLanguage`] is two answers and not one.
//!
//! # Notes a tuning has moved
//!
//! An Auris note-on carries a key and the pitch it sounds at, and the two part company in any
//! tuning but the standard one. A plugin is sent the key *nearest the pitch* — so a plugin that
//! understands nothing more still plays the nearest semitone, and its samples and keyboard
//! tracking are those of where the note sounds — with a note id of its own, and a per-note tuning
//! expression for the rest of the way. That is translation with memory: the note-off has to name
//! the key and id the note went out with, and a pitch bend, which CLAP sends as a tuning to every
//! note at once, would overwrite each note's own offset unless the offsets are sent again on top
//! of it. [`NoteMap`] is that memory: a fixed table of the notes sounding, so none of it allocates
//! on the audio thread.

use auris_core::plugin::{CONTROLLER_MAX, NoteEvent};
use clack_extensions::note_ports::{NoteDialect, NoteDialects};

/// What a plugin is actually sent for one Auris event, or one of several.
///
/// Named rather than pushed straight into a buffer so the choice can be tested without a plugin,
/// an audio thread or a block of samples.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Translated {
    /// A CLAP note-on for one key.
    NoteOn {
        /// MIDI key number.
        key: u16,
        /// The id the note's later events name it by.
        note_id: u32,
        /// Attack strength, 0 to 1, which is CLAP's own scale.
        velocity: f64,
    },
    /// A CLAP note-off for one key.
    NoteOff {
        /// MIDI key number.
        key: u16,
        /// The note's id, or `None` to release whatever plays the key.
        note_id: Option<u32>,
    },
    /// Release everything sounding, letting it decay: a note-off matching every key.
    ReleaseAll,
    /// Stop everything now, without a release stage: a note choke.
    ChokeAll,
    /// Retune every sounding note, in semitones.
    Tuning(f64),
    /// Retune one note, in semitones from its key.
    NoteTuning {
        /// MIDI key number the note went out on.
        key: u16,
        /// The note's id.
        note_id: u32,
        /// Semitones from the key.
        semitones: f64,
    },
    /// Three bytes of MIDI 1.0 on channel 0.
    Midi([u8; 3]),
}

/// Which dialects a port speaks, reduced to the two questions that change what gets sent.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct NoteLanguage {
    /// Notes travel as CLAP note events. Otherwise they are packed into MIDI.
    pub clap_notes: bool,
    /// The port also accepts raw MIDI, which is the only way to move a modulation wheel.
    pub midi: bool,
}

impl NoteLanguage {
    /// `true` when a note between two keys can be played there: only CLAP's own dialect has a
    /// per-note tuning, and MIDI 1.0 plays the nearest key.
    pub fn retunes_notes(self) -> bool {
        self.clap_notes
    }
}

/// How to talk to a port supporting `dialects`, or `None` if there is no shared language.
///
/// `None` is worth having as an answer: a port that speaks only MIDI 2.0 or MPE is one Auris
/// cannot drive, and saying so is better than sending events nobody listens to and leaving the
/// user to wonder why the track is silent.
pub fn language_for(dialects: NoteDialects) -> Option<NoteLanguage> {
    let language = NoteLanguage {
        clap_notes: dialects.supports(NoteDialect::Clap),
        midi: dialects.supports(NoteDialect::Midi),
    };
    match language.clap_notes || language.midi {
        true => Some(language),
        false => None,
    }
}

/// The MIDI pitch bend range Auris assumes when it has to use MIDI at all.
///
/// Two semitones is the General MIDI default, and there is no way to ask a plugin what it is
/// using. This is exactly the assumption the CLAP dialect makes unnecessary — [`Translated::Tuning`]
/// carries semitones — so it applies only to plugins that cannot speak it.
pub const MIDI_BEND_SEMITONES: f32 = 2.0;

/// How many notes [`NoteMap`] follows at once. A note struck past it takes the oldest one's
/// place, and that one is forgotten.
pub const MAX_SOUNDING: usize = 128;

/// The largest note id handed out before counting starts again: CLAP's ids are non-negative
/// `int32_t`, the negative ones meaning "any".
const MAX_NOTE_ID: u32 = i32::MAX as u32;

/// Offsets smaller than this are no offset: a note on its key is sent no tuning of its own.
const ON_THE_KEY: f64 = 1e-6;

/// One note the plugin is playing, as it was sent.
#[derive(Copy, Clone, Debug, Default)]
struct Sounding {
    used: bool,
    /// The Auris key, which the note-off will name.
    key: u8,
    /// The key it went to the plugin on.
    sent: u8,
    note_id: u32,
    /// Semitones from `sent` to where it sounds.
    offset: f64,
    /// Strike order, for releasing the oldest of two at one key first.
    order: u64,
}

/// The notes a plugin is playing, and how to keep talking about them.
///
/// `Copy` so the bridge can run a block's events through a copy first and count what they will
/// become, before sending any of them.
#[derive(Copy, Clone, Debug)]
pub struct NoteMap {
    slots: [Sounding; MAX_SOUNDING],
    next_id: u32,
    order: u64,
    /// The bend in force, which every retuned note's own offset is sent on top of.
    bend: f64,
}

impl Default for NoteMap {
    fn default() -> Self {
        Self {
            slots: [Sounding::default(); MAX_SOUNDING],
            next_id: 0,
            order: 0,
            bend: 0.0,
        }
    }
}

impl NoteMap {
    /// An empty map: nothing sounding, no bend.
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets every note, as a choke or a release of everything does.
    fn clear(&mut self) {
        for slot in &mut self.slots {
            slot.used = false;
        }
    }

    /// How many events `event` becomes, without remembering it.
    pub fn count(&self, event: NoteEvent, language: NoteLanguage) -> usize {
        let mut copy = *self;
        let mut count = 0;
        copy.translate(event, language, |_| {
            count += 1;
            true
        });
        count
    }

    /// Translates `event`, handing each event it becomes to `emit` in the order a plugin must
    /// see them. `false` when `emit` refused any of them; the map remembers the event anyway.
    pub fn translate(
        &mut self,
        event: NoteEvent,
        language: NoteLanguage,
        mut emit: impl FnMut(Translated) -> bool,
    ) -> bool {
        match event {
            NoteEvent::NoteOn {
                key,
                pitch,
                velocity,
                ..
            } => self.note_on(key, pitch, velocity, language, &mut emit),
            NoteEvent::NoteOff { key, .. } => {
                let translated = match self.take_oldest(key) {
                    Some(note) => off(language, note.sent, Some(note.note_id)),
                    None => off(language, key, None),
                };
                emit(translated)
            }
            // Controller 123, "all notes off", is the MIDI spelling of letting them ring out; 120,
            // "all sound off", is the one that cuts them dead.
            NoteEvent::AllNotesOff { .. } => {
                self.clear();
                emit(match language.clap_notes {
                    true => Translated::ReleaseAll,
                    false => Translated::Midi([0xB0, 123, 0]),
                })
            }
            NoteEvent::AllSoundOff { .. } => {
                self.clear();
                emit(match language.clap_notes {
                    true => Translated::ChokeAll,
                    false => Translated::Midi([0xB0, 120, 0]),
                })
            }
            NoteEvent::PitchBend { semitones, .. } => {
                if !language.clap_notes {
                    return emit(Translated::Midi(bend_bytes(semitones)));
                }
                self.bend = f64::from(semitones);
                // Every note, then each retuned one again on top: in that order a plugin ends with
                // each note at its own offset plus the bend.
                let mut sent = emit(Translated::Tuning(self.bend));
                for slot in self.slots.iter().filter(|slot| slot.used) {
                    if slot.offset.abs() > ON_THE_KEY {
                        sent &= emit(Translated::NoteTuning {
                            key: u16::from(slot.sent),
                            note_id: slot.note_id,
                            semitones: slot.offset + self.bend,
                        });
                    }
                }
                sent
            }
            // The one event with no CLAP note equivalent. A plugin that cannot take MIDI simply does
            // not get controllers, which is a limitation to state rather than to fake with a note
            // expression that means something else.
            NoteEvent::Controller { number, value, .. } => match language.midi {
                true => emit(Translated::Midi([
                    0xB0,
                    number.min(CONTROLLER_MAX),
                    to_7bit(value),
                ])),
                false => true,
            },
        }
    }

    fn note_on(
        &mut self,
        key: u8,
        pitch: f32,
        velocity: f32,
        language: NoteLanguage,
        emit: &mut impl FnMut(Translated) -> bool,
    ) -> bool {
        let key = key & 0x7F;
        let pitch = match pitch.is_finite() {
            true => f64::from(pitch),
            false => f64::from(key),
        };
        let sent = pitch.round().clamp(0.0, 127.0) as u8;
        let offset = pitch - f64::from(sent);
        let mut delivered = true;
        // A full table forgets its oldest note rather than releasing it: a note-on is one event,
        // which is what the render graph counts a block's room by. The forgotten note's own
        // release later finds nothing here and names its key to every voice playing it, which is
        // exact in the standard tuning and the nearest thing to it in any other.
        let index = match self.slots.iter().position(|slot| !slot.used) {
            Some(index) => index,
            None => self
                .oldest(|_| true)
                .expect("a full table has an oldest note"),
        };
        let note_id = self.next_id;
        self.next_id = match self.next_id >= MAX_NOTE_ID {
            true => 0,
            false => self.next_id + 1,
        };
        self.slots[index] = Sounding {
            used: true,
            key,
            sent,
            note_id,
            offset,
            order: self.order,
        };
        self.order = self.order.wrapping_add(1);
        if !language.clap_notes {
            return delivered & emit(Translated::Midi([0x90, sent, to_7bit(velocity)]));
        }
        delivered &= emit(Translated::NoteOn {
            key: u16::from(sent),
            note_id,
            velocity: f64::from(velocity.clamp(0.0, 1.0)),
        });
        if offset.abs() > ON_THE_KEY {
            delivered &= emit(Translated::NoteTuning {
                key: u16::from(sent),
                note_id,
                semitones: offset + self.bend,
            });
        }
        delivered
    }

    /// The oldest note `which` accepts.
    fn oldest(&self, which: impl Fn(&Sounding) -> bool) -> Option<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.used && which(slot))
            .max_by_key(|(_, slot)| self.order.wrapping_sub(slot.order))
            .map(|(index, _)| index)
    }

    /// Forgets and returns the oldest note at `key`, the one a note-off for it ends.
    fn take_oldest(&mut self, key: u8) -> Option<Sounding> {
        let index = self.oldest(|slot| slot.key == key)?;
        self.slots[index].used = false;
        Some(self.slots[index])
    }
}

/// A note-off for `key` in `language`.
fn off(language: NoteLanguage, key: u8, note_id: Option<u32>) -> Translated {
    match language.clap_notes {
        true => Translated::NoteOff {
            key: u16::from(key & 0x7F),
            note_id,
        },
        false => Translated::Midi([0x80, key & 0x7F, 0]),
    }
}

/// A 0..1 amount as a MIDI data byte.
fn to_7bit(amount: f32) -> u8 {
    (amount.clamp(0.0, 1.0) * 127.0).round() as u8
}

/// A bend in semitones as a MIDI pitch-wheel message, centred at 8192.
fn bend_bytes(semitones: f32) -> [u8; 3] {
    let normalised = (semitones / MIDI_BEND_SEMITONES).clamp(-1.0, 1.0);
    let raw = (normalised * 8191.0).round() as i32 + 8192;
    let raw = raw.clamp(0, 16383) as u16;
    [0xE0, (raw & 0x7F) as u8, (raw >> 7) as u8]
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: NoteLanguage = NoteLanguage {
        clap_notes: true,
        midi: true,
    };
    const CLAP_ONLY: NoteLanguage = NoteLanguage {
        clap_notes: true,
        midi: false,
    };
    const MIDI_ONLY: NoteLanguage = NoteLanguage {
        clap_notes: false,
        midi: true,
    };

    fn note_on(pitch: u8, velocity: f32) -> NoteEvent {
        NoteEvent::note_on(0, pitch, velocity)
    }

    fn retuned(key: u8, pitch: f32) -> NoteEvent {
        NoteEvent::NoteOn {
            frame: 0,
            key,
            pitch,
            velocity: 1.0,
        }
    }

    fn off_event(key: u8) -> NoteEvent {
        NoteEvent::NoteOff { frame: 0, key }
    }

    /// Everything `event` becomes through `map`.
    fn through(map: &mut NoteMap, event: NoteEvent, language: NoteLanguage) -> Vec<Translated> {
        let mut out = Vec::new();
        assert!(map.translate(event, language, |t| {
            out.push(t);
            true
        }));
        out
    }

    /// Everything `event` becomes on a fresh map.
    fn once(event: NoteEvent, language: NoteLanguage) -> Vec<Translated> {
        through(&mut NoteMap::new(), event, language)
    }

    #[test]
    fn a_port_that_speaks_neither_dialect_gets_no_events() {
        assert_eq!(language_for(NoteDialects::empty()), None);
        assert_eq!(language_for(NoteDialects::MIDI2), None);
        // MPE is MIDI with conventions on top, but the flag on its own does not promise plain
        // MIDI 1.0 will be understood, so it is not claimed as one.
        assert_eq!(language_for(NoteDialects::MIDI_MPE), None);
    }

    #[test]
    fn a_plugin_that_speaks_both_gets_the_better_half_of_each() {
        let language = language_for(NoteDialects::CLAP | NoteDialects::MIDI).expect("a language");
        assert_eq!(language, BOTH);

        // Notes as CLAP events, because the key is a field rather than a packed byte...
        assert_eq!(
            once(note_on(64, 0.5), language),
            [Translated::NoteOn {
                key: 64,
                note_id: 0,
                velocity: 0.5
            }]
        );
        // ...and a controller as MIDI, because CLAP has no note event for one.
        let controller = |number, value| NoteEvent::Controller {
            frame: 0,
            number,
            value,
        };
        assert_eq!(
            once(controller(1, 1.0), language),
            [Translated::Midi([0xB0, 1, 127])]
        );
        // Any controller, not just the wheel: an expression pedal is the same three bytes with a
        // different number in the middle, and that is the whole of why a lane can be drawn on one.
        assert_eq!(
            once(controller(11, 0.5), language),
            [Translated::Midi([0xB0, 11, 64])]
        );
    }

    #[test]
    fn a_clap_only_port_loses_the_wheel_and_keeps_the_bend() {
        assert_eq!(
            once(
                NoteEvent::Controller {
                    frame: 0,
                    number: 1,
                    value: 0.5
                },
                CLAP_ONLY
            ),
            [],
            "dropping it is honest; a note expression that means something else is not"
        );
        // Semitones travel as semitones, with no range for the plugin to guess at.
        assert_eq!(
            once(
                NoteEvent::PitchBend {
                    frame: 0,
                    semitones: 7.0
                },
                CLAP_ONLY
            ),
            [Translated::Tuning(7.0)]
        );
    }

    #[test]
    fn a_midi_only_port_gets_everything_packed_into_three_bytes() {
        assert_eq!(
            once(note_on(60, 1.0), MIDI_ONLY),
            [Translated::Midi([0x90, 60, 127])]
        );
        assert_eq!(
            once(off_event(60), MIDI_ONLY),
            [Translated::Midi([0x80, 60, 0])]
        );
        assert_eq!(
            once(NoteEvent::AllNotesOff { frame: 0 }, MIDI_ONLY),
            [Translated::Midi([0xB0, 123, 0])],
            "let them ring out"
        );
        assert_eq!(
            once(NoteEvent::AllSoundOff { frame: 0 }, MIDI_ONLY),
            [Translated::Midi([0xB0, 120, 0])],
            "and this one cuts them dead"
        );
    }

    #[test]
    fn a_midi_bend_is_centred_and_stays_inside_fourteen_bits() {
        assert_eq!(bend_bytes(0.0), [0xE0, 0x00, 64], "8192 is no bend at all");

        // Full scale in both directions, and nothing beyond it: a bend of an octave through a
        // two-semitone wheel is as far as the wheel goes, not a wrapped-around number.
        assert_eq!(bend_bytes(MIDI_BEND_SEMITONES), [0xE0, 0x7F, 127]);
        assert_eq!(bend_bytes(12.0), bend_bytes(MIDI_BEND_SEMITONES));
        assert_eq!(bend_bytes(-12.0), bend_bytes(-MIDI_BEND_SEMITONES));
        assert_eq!(bend_bytes(-MIDI_BEND_SEMITONES), [0xE0, 0x01, 0]);
    }

    #[test]
    fn a_velocity_outside_the_range_is_brought_back_in() {
        // Nothing in Auris should produce these, but a plugin given a velocity above one is
        // entitled to do anything at all, including divide by it.
        assert_eq!(
            once(note_on(60, 4.0), CLAP_ONLY),
            [Translated::NoteOn {
                key: 60,
                note_id: 0,
                velocity: 1.0
            }]
        );
        assert_eq!(
            once(note_on(60, -1.0), MIDI_ONLY),
            [Translated::Midi([0x90, 60, 0])]
        );
    }

    #[test]
    fn a_retuned_note_goes_out_on_the_nearest_key_with_the_rest_as_its_own_tuning() {
        // 31-tone equal temperament's key 61, a step above middle C at 60.387.
        let mut map = NoteMap::new();
        let sent = through(&mut map, retuned(61, 60.387), CLAP_ONLY);
        let [
            Translated::NoteOn {
                key: 60,
                note_id: 0,
                ..
            },
            Translated::NoteTuning {
                key: 60,
                note_id: 0,
                semitones,
            },
        ] = sent[..]
        else {
            panic!("{sent:?}");
        };
        assert!((semitones - 0.387).abs() < 1e-5);
        // Its release names it as it went out, so the plugin finds the right voice — key 61 is
        // not a key the plugin has ever heard of in this note.
        assert_eq!(
            through(&mut map, off_event(61), CLAP_ONLY),
            [Translated::NoteOff {
                key: 60,
                note_id: Some(0)
            }]
        );
    }

    #[test]
    fn two_keys_on_one_plugin_key_are_told_apart_by_their_ids() {
        // Key 60 on middle C and key 61 a 31st of an octave above it both go out on key 60.
        let mut map = NoteMap::new();
        through(&mut map, retuned(60, 60.0), CLAP_ONLY);
        through(&mut map, retuned(61, 60.387), CLAP_ONLY);
        assert_eq!(
            through(&mut map, off_event(61), CLAP_ONLY),
            [Translated::NoteOff {
                key: 60,
                note_id: Some(1)
            }]
        );
        assert_eq!(
            through(&mut map, off_event(60), CLAP_ONLY),
            [Translated::NoteOff {
                key: 60,
                note_id: Some(0)
            }]
        );
    }

    #[test]
    fn a_bend_is_sent_to_every_note_and_again_on_top_of_each_retuned_one() {
        let mut map = NoteMap::new();
        through(&mut map, retuned(60, 60.0), CLAP_ONLY);
        through(&mut map, retuned(61, 60.25), CLAP_ONLY);
        let bend = NoteEvent::PitchBend {
            frame: 0,
            semitones: 2.0,
        };
        assert_eq!(map.count(bend, CLAP_ONLY), 2);
        assert_eq!(
            through(&mut map, bend, CLAP_ONLY),
            [
                Translated::Tuning(2.0),
                Translated::NoteTuning {
                    key: 60,
                    note_id: 1,
                    semitones: 2.25
                }
            ]
        );
        // A retuned note struck under the bend starts already bent: 61.5 goes out on key 62
        // (rounding away from zero), half a semitone under it, plus two of bend.
        let late = through(&mut map, retuned(62, 61.5), CLAP_ONLY);
        assert_eq!(
            late[1],
            Translated::NoteTuning {
                key: 62,
                note_id: 2,
                semitones: 1.5
            }
        );
    }

    #[test]
    fn a_midi_only_plugin_plays_the_nearest_key_and_releases_that_one() {
        let mut map = NoteMap::new();
        assert_eq!(
            through(&mut map, retuned(91, 72.0), MIDI_ONLY),
            [Translated::Midi([0x90, 72, 127])]
        );
        assert_eq!(
            through(&mut map, off_event(91), MIDI_ONLY),
            [Translated::Midi([0x80, 72, 0])]
        );
    }

    #[test]
    fn a_note_struck_past_the_table_is_still_one_event_and_the_oldest_is_forgotten() {
        let mut map = NoteMap::new();
        for index in 0..MAX_SOUNDING {
            through(&mut map, note_on((index % 128) as u8, 1.0), CLAP_ONLY);
        }
        assert_eq!(map.count(note_on(5, 1.0), CLAP_ONLY), 1);
        through(&mut map, note_on(5, 1.0), CLAP_ONLY);
        // Key 0's note was the oldest; its release now names the key to whatever plays it.
        assert_eq!(
            through(&mut map, off_event(0), CLAP_ONLY),
            [Translated::NoteOff {
                key: 0,
                note_id: None
            }]
        );
    }

    #[test]
    fn counting_a_block_remembers_nothing() {
        let mut map = NoteMap::new();
        assert_eq!(map.count(retuned(61, 60.5), CLAP_ONLY), 2);
        // Still empty: the note-off finds nothing, and falls back to the key.
        assert_eq!(
            through(&mut map, off_event(61), CLAP_ONLY),
            [Translated::NoteOff {
                key: 61,
                note_id: None
            }]
        );
    }
}
