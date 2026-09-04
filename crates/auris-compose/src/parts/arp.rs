//! A broken chord.
//!
//! The shortest of the writers, and its own file for the same reason as the rest: nothing calls
//! it and it calls nothing. The one thing worth knowing before reading it is that its density is
//! a *rate* — how fast the figure climbs — rather than how many of its notes survive, because an
//! arpeggio with holes punched in it is not a sparser arpeggio, it is a broken one.

use crate::frame::{Frame, SectionPlan};
use crate::rng::{Key as RngKey, Rng};
use crate::spec::PartSpec;
use crate::theory::pitch::OCTAVE;

use super::writer::{density, part_grid, phrase_shape, velocity};
use super::{Draft, ScoreSettings};

/// A broken chord.
pub(super) fn arp(
    settings: &ScoreSettings,
    frame: &Frame,
    section: &SectionPlan,
    index: usize,
    part: &PartSpec,
) -> Vec<Draft> {
    let (low, high) = part.range();
    let grid = part_grid(frame, part);
    // How fast the figure runs. An arpeggio's density is the rate it climbs at, not how many of
    // its notes are dropped — dropping them would leave a broken chord with holes in it.
    let busy = density(settings, part, section);
    let step_length = grid.step_ticks()
        * if busy > 0.66 {
            1
        } else if busy > 0.33 {
            2
        } else {
            4
        };
    let mut notes = Vec::new();
    let mut rng = Rng::stream(
        frame.seed,
        &[
            RngKey::Word("part"),
            RngKey::Word(&part.name),
            RngKey::Word("arp"),
            // Not the instance: a repeat of a section runs its arpeggio the same way round.
            RngKey::Word(&section.name),
        ],
    );
    // One binary choice was the whole of this part's variety, so two seeds wrote the same
    // arpeggio five times out of six. A shape and a span are two more.
    let descending = rng.chance(0.3);
    let turns = rng.chance(0.45);
    let span = 1 + rng.below(2) as i32;

    for event in &section.events {
        let mut voicing: Vec<i32> = Vec::new();
        for octave in 0..span {
            for pitch in event.chord.voiced_from(low + octave * OCTAVE) {
                if pitch <= high {
                    voicing.push(pitch);
                }
            }
        }
        voicing.sort_unstable();
        voicing.dedup();
        if voicing.is_empty() {
            continue;
        }
        if descending {
            voicing.reverse();
        }
        // Up and back down again, without repeating the note it turns on.
        if turns && voicing.len() > 2 {
            let back: Vec<i32> = voicing[1..voicing.len() - 1]
                .iter()
                .rev()
                .copied()
                .collect();
            voicing.extend(back);
        }
        // A rhythm the user wrote is played as written: the climb strikes on those steps and
        // only those, walking the chord in the same order it always does.
        if let Some(pattern) = part.rhythm.as_ref() {
            let per_bar = grid.steps_per_bar().max(1);
            let from = grid.step_of(event.start);
            let mut strike = 0usize;
            for offset in 0..grid.step_of(event.length) {
                if pattern.at((from + offset) % per_bar).is_none() {
                    continue;
                }
                let at = event.start + grid.tick_of(offset);
                let pitch = voicing[strike % voicing.len()];
                strike += 1;
                notes.push(Draft {
                    section: index,
                    pitch: pitch.clamp(0, 127) as u8,
                    velocity: (velocity(
                        grid.weight(grid.step_of(at)),
                        section.intensity,
                        settings.dynamics,
                    ) * 0.8
                        * phrase_shape(grid, section, at, settings.dynamics))
                    .clamp(0.05, 1.0),
                    start: section.start + at,
                    length: grid.step_ticks(),
                });
            }
            continue;
        }

        // Even a chord shorter than the chosen rate needs an onset: otherwise a coarse
        // arpeggio silently skips dense harmony changes altogether.
        let count = (event.length.raw() / step_length.raw().max(1)).max(1) as usize;
        for position in 0..count {
            let at = event.start + step_length * position as i64;
            let pitch = voicing[position % voicing.len()];
            notes.push(Draft {
                section: index,
                pitch: pitch.clamp(0, 127) as u8,
                velocity: (velocity(
                    grid.weight(grid.step_of(at)),
                    section.intensity,
                    settings.dynamics,
                ) * 0.8
                    * phrase_shape(grid, section, at, settings.dynamics))
                .clamp(0.05, 1.0),
                start: section.start + at,
                length: step_length,
            });
        }
    }
    let _ = settings;
    notes
}
