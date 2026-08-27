//! The final bar.
//!
//! Its own writer because an ending is not more of the piece: the role writers state figures,
//! comp rhythms and grooves, and one more bar of any of that is one more bar, not a close. What
//! a band actually does on the last bar is land — the chord held, the root under it, the kick
//! and the cymbal once — and that is small enough to be written directly.
//!
//! The cymbal is not here: [`super::joins`] already asks whether arriving somewhere is worth
//! striking something for, and the ending is an arrival by construction, so the crash reaches
//! the final bar through the same rule that reaches every other join.

use auris_core::time::Ticks;

use crate::frame::SectionPlan;
use crate::rhythm::DrumVoice;
use crate::spec::{PartSpec, Role};
use crate::theory::chord::Chord;
use crate::theory::pitch::{OCTAVE, PitchClass, fold_into};

use super::writer::velocity;
use super::{Draft, ScoreSettings};

/// The chord of the final bar, voiced where the part has been living.
///
/// The same search the comp runs for every chord: each octave placement that fits the range,
/// the one whose middle sits nearest the range's own. There is no previous voicing to lead
/// from — this writer sees one bar — so the middle of the range is the neutral answer.
fn resting_voicing(chord: &Chord, low: i32, high: i32) -> Vec<i32> {
    let middle = (low + high) / 2;
    let mut best: Vec<i32> = Vec::new();
    let mut nearest = i32::MAX;
    for octave in -1..=2 {
        let candidate = chord.voiced_from(low + octave * OCTAVE);
        if candidate.iter().any(|pitch| *pitch < low || *pitch > high) {
            continue;
        }
        let centre = candidate.iter().sum::<i32>() / candidate.len().max(1) as i32;
        if (centre - middle).abs() < nearest {
            nearest = (centre - middle).abs();
            best = candidate;
        }
    }
    if best.is_empty() {
        best = chord
            .classes()
            .iter()
            .map(|class| fold_into(class.midi(4), low, high))
            .collect();
    }
    best.sort_unstable();
    best.dedup();
    best
}

/// What one part plays in the held final bar.
pub(super) fn coda(
    settings: &ScoreSettings,
    section: &SectionPlan,
    index: usize,
    part: &PartSpec,
) -> Vec<Draft> {
    let Some(event) = section.events.first() else {
        return Vec::new();
    };
    let (low, high) = part.range();
    // The downbeat's own weight: this is the one bar that is nothing but a downbeat.
    let struck = velocity(4, section.intensity, settings.dynamics);

    // The kick lands once with the chord; the snare and the hat have nothing to keep time for,
    // and a stab held for a bar would not be a stab.
    if part.role == Role::Kick {
        return vec![Draft {
            section: index,
            pitch: part.drum_note().unwrap_or_else(|| DrumVoice::Kick.pitch()),
            velocity: struck.clamp(0.08, 1.0),
            start: section.start,
            length: Ticks(120),
        }];
    }
    if part.role.is_drum() || part.role == Role::Stab {
        return Vec::new();
    }

    let pitches: Vec<i32> = match part.role {
        // The sounding bass of the chord, where this part's octave puts it.
        Role::Bass => vec![fold_into(
            event.chord.bass_class().midi(part.octave),
            low,
            high,
        )],
        // The tune and the arp land on one note: the skeleton's own resting pitch, folded into
        // the part's range — the arp's floor is the melody's, so the fold is usually nothing.
        Role::Melody | Role::Arp => {
            let resting = section
                .skeleton
                .first()
                .copied()
                .unwrap_or((low + high) / 2);
            vec![fold_into(resting, low, high)]
        }
        // The chords and the pad hold the voicing.
        _ => resting_voicing(&event.chord, low, high),
    };
    let held = if matches!(part.role, Role::Chords | Role::Pad) {
        // The same factor the comp plays a held chord at, so the ending is not suddenly louder
        // than every chord that led to it.
        struck * 0.7
    } else {
        struck
    };
    pitches
        .into_iter()
        .map(|pitch| Draft {
            section: index,
            pitch: pitch.clamp(0, 127) as u8,
            velocity: held.clamp(0.05, 1.0),
            start: section.start,
            length: section.length.max(Ticks(1)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theory::chord::Quality;

    #[test]
    fn the_resting_voicing_fits_the_range_and_sits_near_its_middle() {
        let chord = Chord::new(PitchClass::parse("C").unwrap(), Quality::Major7);
        let (low, high) = Role::Chords.range();
        let voicing = resting_voicing(&chord, low, high);
        assert!(voicing.len() >= 3, "{voicing:?}");
        assert!(voicing.iter().all(|pitch| (low..=high).contains(pitch)));
        let centre = voicing.iter().sum::<i32>() / voicing.len() as i32;
        assert!(
            (centre - (low + high) / 2).abs() <= OCTAVE,
            "the voicing sits at {centre}, far from the middle of {low}..{high}"
        );
        // A window too narrow for any whole voicing still answers with the chord folded in.
        let squeezed = resting_voicing(&chord, 60, 63);
        assert!(!squeezed.is_empty());
        assert!(squeezed.iter().all(|pitch| (60..=63).contains(pitch)));
    }
}
