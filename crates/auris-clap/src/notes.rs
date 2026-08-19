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

use auris_core::plugin::{CONTROLLER_MAX, NoteEvent};
use clack_extensions::note_ports::{NoteDialect, NoteDialects};

/// What a plugin is actually sent for one Auris event.
///
/// Named rather than pushed straight into a buffer so the choice can be tested without a plugin,
/// an audio thread or a block of samples.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Translated {
    /// A CLAP note-on for one key.
    NoteOn {
        /// MIDI key number.
        key: u16,
        /// Attack strength, 0 to 1, which is CLAP's own scale.
        velocity: f64,
    },
    /// A CLAP note-off for one key.
    NoteOff {
        /// MIDI key number.
        key: u16,
    },
    /// Release everything sounding, letting it decay: a note-off matching every key.
    ReleaseAll,
    /// Stop everything now, without a release stage: a note choke.
    ChokeAll,
    /// Retune every sounding note, in semitones.
    Tuning(f64),
    /// Three bytes of MIDI 1.0 on channel 0.
    Midi([u8; 3]),
    /// Nothing this port can be told. The event is dropped, not approximated.
    Nothing,
}

/// Which dialects a port speaks, reduced to the two questions that change what gets sent.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct NoteLanguage {
    /// Notes travel as CLAP note events. Otherwise they are packed into MIDI.
    pub clap_notes: bool,
    /// The port also accepts raw MIDI, which is the only way to move a modulation wheel.
    pub midi: bool,
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

/// What `event` becomes for a port speaking `language`.
pub fn translate(event: NoteEvent, language: NoteLanguage) -> Translated {
    match event {
        NoteEvent::NoteOn {
            pitch, velocity, ..
        } => match language.clap_notes {
            true => Translated::NoteOn {
                key: pitch as u16,
                velocity: velocity.clamp(0.0, 1.0) as f64,
            },
            false => Translated::Midi([0x90, pitch & 0x7F, to_7bit(velocity)]),
        },
        NoteEvent::NoteOff { pitch, .. } => match language.clap_notes {
            true => Translated::NoteOff { key: pitch as u16 },
            false => Translated::Midi([0x80, pitch & 0x7F, 0]),
        },
        // Controller 123, "all notes off", is the MIDI spelling of letting them ring out; 120,
        // "all sound off", is the one that cuts them dead.
        NoteEvent::AllNotesOff { .. } => match language.clap_notes {
            true => Translated::ReleaseAll,
            false => Translated::Midi([0xB0, 123, 0]),
        },
        NoteEvent::AllSoundOff { .. } => match language.clap_notes {
            true => Translated::ChokeAll,
            false => Translated::Midi([0xB0, 120, 0]),
        },
        NoteEvent::PitchBend { semitones, .. } => match language.clap_notes {
            true => Translated::Tuning(semitones as f64),
            false => Translated::Midi(bend_bytes(semitones)),
        },
        // The one event with no CLAP note equivalent. A plugin that cannot take MIDI simply does
        // not get controllers, which is a limitation to state rather than to fake with a note
        // expression that means something else.
        NoteEvent::Controller { number, value, .. } => match language.midi {
            true => Translated::Midi([0xB0, number.min(CONTROLLER_MAX), to_7bit(value)]),
            false => Translated::Nothing,
        },
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
        NoteEvent::NoteOn {
            frame: 0,
            pitch,
            velocity,
        }
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
            translate(note_on(64, 0.5), language),
            Translated::NoteOn {
                key: 64,
                velocity: 0.5
            }
        );
        // ...and a controller as MIDI, because CLAP has no note event for one.
        assert_eq!(
            translate(
                NoteEvent::Controller {
                    frame: 0,
                    number: 1,
                    value: 1.0
                },
                language
            ),
            Translated::Midi([0xB0, 1, 127])
        );
        // Any controller, not just the wheel: an expression pedal is the same three bytes with a
        // different number in the middle, and that is the whole of why a lane can be drawn on one.
        assert_eq!(
            translate(
                NoteEvent::Controller {
                    frame: 0,
                    number: 11,
                    value: 0.5
                },
                language
            ),
            Translated::Midi([0xB0, 11, 64])
        );
    }

    #[test]
    fn a_clap_only_port_loses_the_wheel_and_keeps_the_bend() {
        assert_eq!(
            translate(
                NoteEvent::Controller {
                    frame: 0,
                    number: 1,
                    value: 0.5
                },
                CLAP_ONLY
            ),
            Translated::Nothing,
            "dropping it is honest; a note expression that means something else is not"
        );
        // Semitones travel as semitones, with no range for the plugin to guess at.
        assert_eq!(
            translate(
                NoteEvent::PitchBend {
                    frame: 0,
                    semitones: 7.0
                },
                CLAP_ONLY
            ),
            Translated::Tuning(7.0)
        );
    }

    #[test]
    fn a_midi_only_port_gets_everything_packed_into_three_bytes() {
        assert_eq!(
            translate(note_on(60, 1.0), MIDI_ONLY),
            Translated::Midi([0x90, 60, 127])
        );
        assert_eq!(
            translate(
                NoteEvent::NoteOff {
                    frame: 0,
                    pitch: 60
                },
                MIDI_ONLY
            ),
            Translated::Midi([0x80, 60, 0])
        );
        assert_eq!(
            translate(NoteEvent::AllNotesOff { frame: 0 }, MIDI_ONLY),
            Translated::Midi([0xB0, 123, 0]),
            "let them ring out"
        );
        assert_eq!(
            translate(NoteEvent::AllSoundOff { frame: 0 }, MIDI_ONLY),
            Translated::Midi([0xB0, 120, 0]),
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
            translate(note_on(60, 4.0), CLAP_ONLY),
            Translated::NoteOn {
                key: 60,
                velocity: 1.0
            }
        );
        assert_eq!(
            translate(note_on(60, -1.0), MIDI_ONLY),
            Translated::Midi([0x90, 60, 0])
        );
    }
}
