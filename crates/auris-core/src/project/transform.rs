//! Non-destructive changes to how a clip's notes are performed.
//!
//! A [`NoteTransform`] never touches the notes a clip stores. It is applied as the clip answers
//! [`sounding_notes`](super::MidiClip::sounding_notes), so the renderer and the MIDI writer hear
//! it and the piano roll keeps showing the text that was written — the score does not change, and
//! this is the performer's half of that contract. Everything random draws from a seed the
//! transform stores, so the same file is the same performance on every open; and the humanisation
//! is applied *after* looping is expanded, keyed by the pass, so a two-bar loop does not repeat
//! its own accidents every two bars the way a baked wobble would.
//!
//! The constants here are not mirrored from the composer any more; they are the only copy. The
//! composer stopped baking its feel into the notes it writes and instead installs transforms on
//! the clips it delivers, so an amount of looseness means the same thing whether a phrase was
//! written or played by hand — because it is the same code either way. What the composer decides
//! is *which* transforms a part starts with; `auris_compose`'s `perform` module is that table,
//! and the two are measured together (`docs/evaluation.md`).

use serde::{Deserialize, Serialize};

use crate::rng::{Key, Rng};
use crate::time::{TICKS_PER_QUARTER, Ticks};

use super::clip::Note;
use super::recipe::Subdivision;

/// How far a note's timing wanders at full humanisation, as a standard deviation in
/// milliseconds.
///
/// Calibrated when it was still the composer's own constant: six, because the presets' default
/// of 0.35 lands at about 2 ms — where a band that is playing well sits — and the jitter's own
/// three-sigma bound keeps the top of the dial under 18, which is as far as "sloppy" goes
/// before it stops being one band. The measurement behind it is in the repository's history;
/// re-measure (`docs/evaluation.md`) before moving it.
const WANDER_MS: f32 = 6.0;

/// How far a note's velocity wanders at full humanisation, as a standard deviation on the
/// 0-to-1 scale. Calibrated alongside [`WANDER_MS`].
const VELOCITY_WANDER: f32 = 0.06;

/// One non-destructive change to how a clip's notes are performed.
///
/// Stored on the clip in a stack and applied in order, each one a pure function of a note. The
/// stack changes what is *heard* — playback and export both — while the notes the piano roll
/// shows stay exactly as written. Freezing the stack writes what is heard into the text and
/// clears it, the same trade [`freezing a recipe`](super::ClipRecipe) makes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NoteTransform {
    /// Wanders timing and velocity, so the clip stops sounding quantised.
    ///
    /// The wander is a *time*, not a count of ticks: the dial reaches the same number of
    /// milliseconds at every tempo, which is why performing a clip needs to know its tempo at
    /// all. Applied per loop pass, so a looped clip is loose differently on every repeat.
    Humanize {
        /// How far things wander, from 0 for the text as written to 1 for a sloppy band.
        amount: f32,
        /// The number every draw comes from. A different one is a different take.
        seed: u64,
    },
    /// Moves every note by the same amount, the way a player sits against the beat.
    ///
    /// A lean is not a wobble: the hat a little early, the snare a little late, by the same
    /// number of ticks in every bar, which is a thing a drummer does on purpose and reads as a
    /// feel. It is deliberate where [`Humanize`](Self::Humanize) is loose, so it is its own
    /// transform — a stack can lean without wandering, and freezing writes exactly the lean in.
    /// The composer installs one per part from its own table of who pushes and who drags.
    Lean {
        /// Ticks late (positive) or early (negative), read clamped to a quarter note either
        /// way. In ticks rather than milliseconds deliberately: a lean is part of how the part
        /// sits in the bar, so it scales with the music's own grid rather than with the clock.
        ticks: i64,
    },
    /// Delays the offbeats, turning a straight grid into a groove.
    ///
    /// Only notes sitting exactly on the grid's offbeat steps move — anything already off the
    /// grid is left where the player put it.
    Swing {
        /// Where the offbeat lands within its pair of steps, as a percentage. 50 is straight,
        /// 67 is about a triplet feel, and values are read clamped to 50..=75.
        percent: u8,
        /// The grid whose offbeats are delayed. Triplet grids do not swing: a triplet is
        /// already the feel swing reaches for, and the transform leaves them untouched.
        subdivision: Subdivision,
    },
    /// Moves every pitch by the same interval.
    Transpose {
        /// Semitones up (positive) or down (negative). A pitch pushed past the MIDI range is
        /// clamped to its edge rather than dropped — a note falling silent reads as a bug.
        semitones: i32,
    },
    /// Shortens every note, detaching legato playing into separate strokes.
    Gate {
        /// Fraction of each note's written length that is held, read clamped to 0.05..=1.0.
        /// A note never shrinks below one tick.
        amount: f32,
    },
}

/// Performs one note through a transform stack.
///
/// `pass` is which loop pass the note sounds in, and is what makes a looped clip's humanisation
/// differ from repeat to repeat. `bpm` is the tempo in force at the clip — the humanisation dial
/// is milliseconds, and nothing can turn milliseconds into ticks without it. Positions stay
/// relative to the clip, exactly as [`playable_notes`](super::MidiClip::playable_notes) leaves
/// them.
///
/// An empty stack is the identity, bit for bit: a clip with no transforms performs its text.
pub fn performed(mut note: Note, transforms: &[NoteTransform], pass: u64, bpm: f64) -> Note {
    for transform in transforms {
        note = match transform {
            NoteTransform::Humanize { amount, seed } => humanized(note, *amount, *seed, pass, bpm),
            NoteTransform::Lean { ticks } => leaned(note, *ticks),
            NoteTransform::Swing {
                percent,
                subdivision,
            } => swung(note, *percent, *subdivision),
            NoteTransform::Transpose { semitones } => transposed(note, *semitones),
            NoteTransform::Gate { amount } => gated(note, *amount),
        };
    }
    note
}

/// The wander, drawn from a stream named by where the note is.
///
/// Named by position and pitch rather than by how many notes came before, so editing bar one
/// does not re-time the whole clip — the composer's own rule, kept for the same reason. The
/// velocity draw follows the timing draw out of the same stream whether either is used, which
/// is the roll-anyway rule: turning the dial to zero and back must land on the same take.
fn humanized(mut note: Note, amount: f32, seed: u64, pass: u64, bpm: f64) -> Note {
    let amount = amount.clamp(0.0, 1.0);
    if amount <= 0.0 {
        return note;
    }
    let mut rng = Rng::stream(
        seed,
        &[
            Key::Word("humanize"),
            Key::Index(pass),
            Key::Index(note.start.raw().max(0) as u64),
            Key::Index(u64::from(note.pitch)),
        ],
    );
    // How many ticks go by in a millisecond at this tempo — the whole reason `bpm` is here.
    let ticks_per_ms = (TICKS_PER_QUARTER as f64 * bpm.max(0.0) / 60_000.0) as f32;
    let wander = rng.jitter(WANDER_MS * amount * ticks_per_ms);
    note.start = (note.start + Ticks(wander.round() as i64)).max_zero();
    let scale = 1.0 + rng.jitter(VELOCITY_WANDER * amount);
    note.velocity = (note.velocity * scale).clamp(0.05, 1.0);
    note
}

/// The lean, clamped to a quarter note either way and held at the clip's own start.
fn leaned(mut note: Note, ticks: i64) -> Note {
    let ticks = ticks.clamp(-TICKS_PER_QUARTER, TICKS_PER_QUARTER);
    note.start = (note.start + Ticks(ticks)).max_zero();
    note
}

/// The delay, applied to exact offbeat hits only.
fn swung(mut note: Note, percent: u8, subdivision: Subdivision) -> Note {
    if subdivision.is_triplet() {
        return note;
    }
    let percent = percent.clamp(50, 75);
    let step = TICKS_PER_QUARTER / i64::from(subdivision.steps_per_beat());
    let pair = step * 2;
    if note.start.raw().rem_euclid(pair) == step {
        let landed = (pair as f32 * f32::from(percent) / 100.0).round() as i64;
        note.start += Ticks(landed - step);
    }
    note
}

/// The interval, clamped at the ends of the MIDI range.
fn transposed(mut note: Note, semitones: i32) -> Note {
    note.pitch = (i32::from(note.pitch) + semitones).clamp(0, 127) as u8;
    note
}

/// The shortening, floored at one tick so a note cannot vanish.
fn gated(mut note: Note, amount: f32) -> Note {
    let amount = amount.clamp(0.05, 1.0);
    note.length = Ticks(((note.length.raw() as f32 * amount).round() as i64).max(1));
    note
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(pitch: u8, start: i64, length: i64) -> Note {
        Note {
            velocity: 0.8,
            ..Note::new(pitch, Ticks(start), Ticks(length))
        }
    }

    #[test]
    fn an_empty_stack_is_the_identity() {
        let original = note(60, 480, 240);
        assert_eq!(performed(original.clone(), &[], 0, 120.0), original);
    }

    #[test]
    fn humanize_at_zero_is_the_text_as_written() {
        let stack = [NoteTransform::Humanize {
            amount: 0.0,
            seed: 7,
        }];
        let original = note(60, 480, 240);
        assert_eq!(performed(original.clone(), &stack, 0, 120.0), original);
    }

    #[test]
    fn humanize_wanders_within_three_sigma_of_the_dial() {
        // At 120 BPM a millisecond is 1.92 ticks, so the full dial's 6 ms sigma is 11.52 ticks
        // and the jitter's own 3-sigma bound is 34.56 — nothing may land further out than 35.
        let stack = [NoteTransform::Humanize {
            amount: 1.0,
            seed: 3,
        }];
        let mut moved = 0;
        for start in (0..96_000).step_by(240) {
            let played = performed(note(60, start, 240), &stack, 0, 120.0);
            let offset = (played.start.raw() - start).abs();
            assert!(offset <= 35, "note at {start} wandered {offset} ticks");
            if offset > 0 {
                moved += 1;
            }
            assert!((0.05..=1.0).contains(&played.velocity));
        }
        assert!(moved > 300, "only {moved} of 400 notes moved at full dial");
    }

    #[test]
    fn humanize_is_the_same_performance_on_every_open() {
        let stack = [NoteTransform::Humanize {
            amount: 0.5,
            seed: 11,
        }];
        let first = performed(note(64, 960, 240), &stack, 2, 100.0);
        let second = performed(note(64, 960, 240), &stack, 2, 100.0);
        assert_eq!(first, second);
    }

    #[test]
    fn each_loop_pass_is_loose_in_its_own_way() {
        let stack = [NoteTransform::Humanize {
            amount: 1.0,
            seed: 5,
        }];
        let passes: Vec<Ticks> = (0..8)
            .map(|pass| performed(note(60, 480, 240), &stack, pass, 120.0).start)
            .collect();
        let mut distinct = passes.clone();
        distinct.sort();
        distinct.dedup();
        assert!(
            distinct.len() > 1,
            "every pass wobbled identically: {passes:?}"
        );
    }

    #[test]
    fn a_lean_moves_every_note_alike_and_is_held_at_the_edges() {
        let drag = [NoteTransform::Lean { ticks: 10 }];
        assert_eq!(
            performed(note(60, 480, 240), &drag, 0, 120.0).start,
            Ticks(490)
        );
        assert_eq!(
            performed(note(60, 0, 240), &drag, 3, 120.0).start,
            Ticks(10)
        );
        // Early off the front of the clip is held at zero, like the humanise wander.
        let push = [NoteTransform::Lean { ticks: -8 }];
        assert_eq!(
            performed(note(60, 4, 240), &push, 0, 120.0).start,
            Ticks::ZERO
        );
        // A file carrying a wild number is read clamped, not honoured.
        let wild = [NoteTransform::Lean { ticks: 100_000 }];
        assert_eq!(
            performed(note(60, 0, 240), &wild, 0, 120.0).start,
            Ticks(TICKS_PER_QUARTER)
        );
    }

    #[test]
    fn swing_delays_the_offbeat_and_leaves_the_downbeat() {
        let stack = [NoteTransform::Swing {
            percent: 67,
            subdivision: Subdivision::Eighth,
        }];
        // The offbeat eighth lands at two thirds of the pair: 960 × 0.67 = 643.
        let offbeat = performed(note(60, 480, 240), &stack, 0, 120.0);
        assert_eq!(offbeat.start, Ticks(643));
        let downbeat = performed(note(60, 0, 240), &stack, 0, 120.0);
        assert_eq!(downbeat.start, Ticks::ZERO);
        // A note already off the grid belongs to the player and does not move.
        let played = performed(note(60, 500, 240), &stack, 0, 120.0);
        assert_eq!(played.start, Ticks(500));
    }

    #[test]
    fn swing_at_fifty_is_straight_and_triplets_do_not_swing() {
        let straight = [NoteTransform::Swing {
            percent: 50,
            subdivision: Subdivision::Sixteenth,
        }];
        let original = note(60, 240, 120);
        assert_eq!(performed(original.clone(), &straight, 0, 120.0), original);
        let triplet = [NoteTransform::Swing {
            percent: 70,
            subdivision: Subdivision::EighthTriplet,
        }];
        let on_triplet = note(60, 320, 160);
        assert_eq!(
            performed(on_triplet.clone(), &triplet, 0, 120.0),
            on_triplet
        );
    }

    #[test]
    fn transpose_moves_the_pitch_and_clamps_at_the_edges() {
        let up = [NoteTransform::Transpose { semitones: 7 }];
        assert_eq!(performed(note(60, 0, 240), &up, 0, 120.0).pitch, 67);
        let far = [NoteTransform::Transpose { semitones: 80 }];
        assert_eq!(performed(note(60, 0, 240), &far, 0, 120.0).pitch, 127);
        let down = [NoteTransform::Transpose { semitones: -80 }];
        assert_eq!(performed(note(60, 0, 240), &down, 0, 120.0).pitch, 0);
    }

    #[test]
    fn gate_shortens_but_never_erases() {
        let half = [NoteTransform::Gate { amount: 0.5 }];
        assert_eq!(
            performed(note(60, 0, 480), &half, 0, 120.0).length,
            Ticks(240)
        );
        let tight = [NoteTransform::Gate { amount: 0.05 }];
        assert_eq!(performed(note(60, 0, 4), &tight, 0, 120.0).length, Ticks(1));
    }

    #[test]
    fn the_stack_applies_in_order() {
        // Swing first sees the note on the grid and moves it; humanize-first would have moved
        // it off the grid and the swing would have left it alone. Order is audible, so it is
        // the user's.
        let swing_then_gate = [
            NoteTransform::Swing {
                percent: 67,
                subdivision: Subdivision::Eighth,
            },
            NoteTransform::Gate { amount: 0.5 },
        ];
        let played = performed(note(60, 480, 240), &swing_then_gate, 0, 120.0);
        assert_eq!((played.start, played.length), (Ticks(643), Ticks(120)));
    }

    #[test]
    fn a_stored_transform_names_its_kind_in_words() {
        // The file format: a reader of the JSON should meet "humanize", not a bare index.
        let json = serde_json::to_string(&NoteTransform::Humanize {
            amount: 0.5,
            seed: 9,
        })
        .unwrap();
        assert!(json.contains(r#""kind":"humanize""#), "{json}");
        let back: NoteTransform = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back,
            NoteTransform::Humanize {
                amount: 0.5,
                seed: 9
            }
        );
    }
}
