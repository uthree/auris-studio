//! The plan every part is written against.
//!
//! Harmony, phrasing and the melodic skeleton are decided once, before any part exists, and then
//! frozen. That is what makes the parts agree with each other without knowing about each other:
//! the bass and the melody both read the same chord at the same tick, so they cannot drift, and
//! each part is a pure function of the frame and its own name.

use auris_core::time::Ticks;

use crate::rhythm::{Grid, Pattern};
use crate::rng::{Key as RngKey, Rng};
use crate::spec::Mood;
use crate::theory::chart::HarmonicEvent;
use crate::theory::key::Key;

/// One playing of one section.
#[derive(Clone, Debug)]
pub struct SectionPlan {
    /// Which section it is.
    pub name: String,
    /// Which time round it is, counting from one, so a second chorus can differ from the first.
    pub instance: usize,
    /// Where it starts in the song.
    pub start: Ticks,
    /// How long it lasts.
    pub length: Ticks,
    /// How many bars that is.
    pub bars: usize,
    /// Shared phrase boundaries and their functions within this section.
    pub phrases: Vec<crate::phrasing::PhrasePlan>,
    /// The key it is in, after any transposition.
    pub key: Key,
    /// How hard it is played.
    pub intensity: f32,
    /// Its chords, positioned from the section's own start.
    pub events: Vec<HarmonicEvent>,
    /// One structural pitch per chord, for the melody to hang on.
    pub skeleton: Vec<i32>,
}

impl SectionPlan {
    /// The phrase containing a section-relative bar.
    pub fn phrase_at(&self, bar: usize) -> Option<&crate::phrasing::PhrasePlan> {
        self.phrases
            .iter()
            .find(|phrase| phrase.start_bar <= bar && bar < phrase.end_bar())
    }

    /// Whether this bar ends a planned phrase.
    pub fn closes_phrase(&self, bar: usize) -> bool {
        self.phrase_at(bar)
            .is_some_and(|phrase| bar + 1 == phrase.end_bar())
    }

    /// The chord sounding at a tick measured from the section's start.
    ///
    /// `None` when nothing sounds there — before the first chord, or in a stretch somebody
    /// cleared. The composer's own charts tile a section completely, so a hole only exists when
    /// the harmony came from a document; answering with the nearest chord anyway would mean
    /// playing over a silence the person deliberately wrote.
    pub fn chord_at(&self, tick: Ticks) -> Option<&HarmonicEvent> {
        self.events
            .iter()
            .rev()
            .find(|event| event.start <= tick)
            .filter(|event| tick < event.end())
    }

    /// The same section with every chord cut at the bar lines it crosses.
    ///
    /// What the accompaniment writers play over. A chord chart says where the harmony
    /// *changes*, and a chord held for two bars is one chord — but a keyboard player, a bass
    /// player and an arpeggio still play their figure in every bar of it. Writing a part against
    /// the chord's own span instead made the rhythm a function of how the chart was spelled:
    /// `| C | C |` restruck at the bar line and the same harmony written once did not. Cut here,
    /// each part keeps its own bar-by-bar rhythm whatever the chart's spelling, and
    /// [`Self::continues_chord`] tells a writer which cuts are not changes.
    pub fn bar_by_bar(&self, bar: Ticks) -> SectionPlan {
        let mut plan = self.clone();
        if bar <= Ticks::ZERO {
            return plan;
        }
        plan.events.clear();
        for event in &self.events {
            let mut start = event.start;
            let end = event.end();
            while start < end {
                let line = Ticks((start.raw().div_euclid(bar.raw()) + 1) * bar.raw());
                let until = line.min(end);
                plan.events.push(HarmonicEvent {
                    start,
                    length: until - start,
                    bar: start.raw().div_euclid(bar.raw()).max(0) as usize,
                    ..*event
                });
                start = until;
            }
        }
        plan
    }

    /// Whether the chord at `index` only carries on the one before it, with no change of
    /// harmony between them — the second half of a chord [`Self::bar_by_bar`] cut at a bar line,
    /// or a chart that wrote the same chord twice.
    pub fn continues_chord(&self, index: usize) -> bool {
        index
            .checked_sub(1)
            .and_then(|previous| self.events.get(previous))
            .zip(self.events.get(index))
            .is_some_and(|(previous, event)| {
                previous.end() == event.start
                    && previous.chord == event.chord
                    && previous.key == event.key
            })
    }

    /// The index of the chord sounding at a tick from the section's start.
    pub fn event_index_at(&self, tick: Ticks) -> usize {
        self.events
            .iter()
            .rposition(|event| event.start <= tick)
            .unwrap_or(0)
    }
}

/// The whole plan: the stretch being written, with its harmony resolved.
#[derive(Clone, Debug)]
pub struct Frame {
    /// The grid everything is placed on.
    pub grid: Grid,
    /// The sections, in playing order.
    pub sections: Vec<SectionPlan>,
    /// How long the piece is.
    pub length: Ticks,
    /// The seed every stream is drawn from.
    pub seed: u64,
    /// How the piece should feel.
    pub mood: Mood,
}

/// One structural pitch per chord, chosen so the line makes musical sense as a whole.
///
/// A dynamic program rather than a series of local choices: picking each note from its
/// predecessor gives a line that wanders, because nothing is looking ahead to where the phrase
/// has to end. Solving the whole phrase at once is what makes it arrive somewhere.
pub(crate) fn skeleton(
    events: &[HarmonicEvent],
    seed: u64,
    section: &str,
    instance: usize,
    brightness: f32,
) -> Vec<i32> {
    if events.is_empty() {
        return Vec::new();
    }
    // The role's own range, not whichever melody part happens to be in the roster: taking it
    // from a part would mean adding or removing a part silently rewrote every other part, since
    // they all hang off this skeleton.
    let (low, high) = crate::spec::Role::Melody.range();

    // A gentle arch over the phrase: rising to two thirds of the way through, then falling. Most
    // melodies do this, and a skeleton that does it sounds intentional rather than aimless.
    let mut rng = Rng::stream(
        seed,
        &[
            RngKey::Word("frame"),
            RngKey::Word("skeleton"),
            RngKey::Word(section),
            RngKey::Index(instance as u64),
        ],
    );
    let peak = 0.55 + rng.unit() * 0.25;

    let candidates: Vec<Vec<i32>> = events
        .iter()
        .map(|event| {
            let mut pitches: Vec<i32> = (low..=high)
                .filter(|midi| event.chord.contains_midi(*midi))
                .collect();
            if pitches.is_empty() {
                pitches.push(event.chord.nearest_tone((low + high) / 2));
            }
            pitches
        })
        .collect();

    // Viterbi: cost of being on each candidate, and where we came from.
    let mut cost: Vec<Vec<i32>> = Vec::with_capacity(candidates.len());
    let mut from: Vec<Vec<usize>> = Vec::with_capacity(candidates.len());

    for (index, options) in candidates.iter().enumerate() {
        let position = if candidates.len() == 1 {
            0.0
        } else {
            index as f32 / (candidates.len() - 1) as f32
        };
        // Where the arch wants the line to be at this point.
        let height = if position <= peak {
            position / peak
        } else {
            1.0 - (position - peak) / (1.0 - peak).max(0.001)
        };
        // Where in the role's range the arch sits. The arch spans half the range and brightness
        // slides that half: 0 writes the line low, 1 writes it high, and 0.5 leaves it exactly
        // where it has always been. See `Mood::brightness` for why this is what that dial does.
        let floor = 0.5 * brightness.clamp(0.0, 1.0);
        let target = low as f32 + (high - low) as f32 * (floor + 0.5 * height);
        let final_event = index + 1 == candidates.len();

        let mut row = Vec::with_capacity(options.len());
        let mut back = Vec::with_capacity(options.len());
        for pitch in options {
            // Emission: how far this pitch is from the arch, and whether it lands somewhere
            // stable when the phrase is ending.
            let mut emission = ((*pitch as f32 - target).abs() * 10.0) as i32;
            if final_event {
                // The ending chord's own key: after a modulation inside the range, the key the
                // phrase has to sound finished in is the one in force where it ends.
                let degree = events[index]
                    .key
                    .tonic
                    .distance_up_to(crate::theory::pitch::PitchClass::new(*pitch));
                // A phrase that ends on the tonic or the fifth sounds finished; one that ends
                // on the seventh sounds like a question nobody answered.
                emission += match degree {
                    0 => -120,
                    7 => -60,
                    _ => 100,
                };
            }
            let (best, best_index) = match cost.last() {
                None => (emission, 0),
                Some(previous) => {
                    let mut best = i32::MAX;
                    let mut best_index = 0;
                    for (candidate_index, previous_cost) in previous.iter().enumerate() {
                        let step = (pitch - candidates[index - 1][candidate_index]).abs();
                        // Steps are cheap, leaps are dear, and a leap beyond a fifth is dearer
                        // still — which is what stops the line jumping about.
                        let transition = step * 6 + (step - 7).max(0) * 50;
                        let total = previous_cost.saturating_add(transition);
                        if total < best {
                            best = total;
                            best_index = candidate_index;
                        }
                    }
                    (best.saturating_add(emission), best_index)
                }
            };
            row.push(best);
            back.push(best_index);
        }
        cost.push(row);
        from.push(back);
    }

    // Walk the cheapest path back.
    let mut path = vec![0usize; candidates.len()];
    let last = cost.len() - 1;
    path[last] = cost[last]
        .iter()
        .enumerate()
        .min_by_key(|(index, value)| (**value, *index))
        .map(|(index, _)| index)
        .unwrap_or(0);
    for index in (1..candidates.len()).rev() {
        path[index - 1] = from[index][path[index]];
    }
    path.iter()
        .enumerate()
        .map(|(index, choice)| candidates[index][*choice])
        .collect()
}

/// The drum pattern for a voice, at the song's groove.
pub fn groove_pattern(groove: &str, voice: crate::rhythm::DrumVoice) -> Pattern {
    crate::rhythm::groove(groove)
        .map(|groove| groove.pattern(voice))
        .unwrap_or_else(|| Pattern::rests(16))
}

/// How many steps of the song's groove make one of its beats.
///
/// Asked separately from the pattern because it is what says how to lay that pattern over a bar:
/// the compound grooves count their beats in threes, and a reader that assumed four would play
/// them at two thirds of the length they were written at.
pub fn groove_steps_per_beat(groove: &str) -> usize {
    crate::rhythm::groove(groove).map_or(crate::rhythm::GROOVE_STEPS_PER_BEAT, |groove| {
        groove.steps_per_beat
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::Song;
    use crate::theory::pitch::PitchClass;

    #[test]
    fn the_skeleton_gives_one_pitch_per_chord_inside_the_melody_range() {
        let frame = Song::default().frame();
        let verse = &frame.sections[0];
        assert_eq!(verse.skeleton.len(), verse.events.len());
        for (pitch, event) in verse.skeleton.iter().zip(&verse.events) {
            assert!(
                (60..=84).contains(pitch),
                "{pitch} is outside the melody range"
            );
            assert!(
                event.chord.contains_midi(*pitch),
                "{pitch} is not in {}",
                event.chord
            );
        }
    }

    #[test]
    fn the_skeleton_moves_by_steps_rather_than_leaps() {
        let frame = Song {
            chords: "@canon".into(),
            bars: 8,
            ..Song::default()
        }
        .frame();
        let skeleton = &frame.sections[0].skeleton;
        let leaps = skeleton
            .windows(2)
            .filter(|pair| (pair[1] - pair[0]).abs() > 7)
            .count();
        assert!(
            leaps <= 1,
            "a skeleton that leaps everywhere is not a melody: {skeleton:?}"
        );
    }

    #[test]
    fn the_skeleton_ends_somewhere_stable() {
        // A phrase that ends on the seventh sounds like a question nobody answered.
        let frame = Song::default().frame();
        let verse = &frame.sections[0];
        let last = *verse.skeleton.last().unwrap();
        let degree = verse.key.tonic.distance_up_to(PitchClass::new(last));
        assert!(
            matches!(degree, 0 | 7),
            "the phrase ended on degree {degree} above the tonic"
        );
    }

    #[test]
    fn planning_is_reproducible_and_seed_dependent() {
        let seeded = |seed: u64| {
            Song {
                chords: "@canon".into(),
                bars: 8,
                seed,
                ..Song::default()
            }
            .frame()
        };
        let a = seeded(1);
        let b = seeded(1);
        assert_eq!(a.sections[0].skeleton, b.sections[0].skeleton);

        let c = seeded(2);
        // Not a guarantee for every seed pair, but these two do differ, and the test pins that
        // the seed is actually reaching the plan.
        assert_ne!(a.sections[0].skeleton, c.sections[0].skeleton);
    }

    #[test]
    fn the_chord_at_a_tick_is_the_one_sounding_there() {
        let frame = Song::default().frame();
        let verse = &frame.sections[0];
        let bar = frame.grid.bar_ticks();
        assert_eq!(verse.chord_at(Ticks::ZERO).unwrap().chord.to_string(), "C");
        assert_eq!(
            verse.chord_at(bar - Ticks(1)).unwrap().chord.to_string(),
            "C"
        );
        assert_eq!(verse.chord_at(bar).unwrap().chord.to_string(), "G");
        assert_eq!(verse.event_index_at(bar * 2), 2);
    }

    #[test]
    fn a_section_cut_bar_by_bar_keeps_its_harmony_and_marks_what_carries_on() {
        let frame = Song {
            chords: "| I | V |".into(),
            bars: 2,
            ..Song::default()
        }
        .frame();
        let bar = frame.grid.bar_ticks();
        let mut section = frame.sections[0].clone();
        // One I across both bars, then a V starting half way through the second.
        section.events.truncate(1);
        section.events[0].length = bar * 2;
        let barred = section.bar_by_bar(bar);
        assert_eq!(barred.events.len(), 2);
        assert_eq!(barred.events[1].start, bar);
        assert_eq!(barred.events[1].bar, 1);
        assert_eq!(barred.events[1].chord, section.events[0].chord);
        assert!(barred.continues_chord(1));
        assert!(!barred.continues_chord(0));
        assert!(!frame.sections[0].continues_chord(1), "I to V is a change");
    }
}
