//! The plan every part is written against.
//!
//! Harmony, form and the melodic skeleton are decided once, before any part exists, and then
//! frozen. That is what makes the parts agree with each other without knowing about each other:
//! the bass and the melody both read the same chord at the same tick, so they cannot drift, and
//! each part is a pure function of the frame and its own name.

use auris_core::time::Ticks;

use crate::rhythm::{Grid, Pattern};
use crate::rng::{Key as RngKey, Rng};
use crate::spec::{Mood, SongSpec};
use crate::theory::chart::{ChartOrigin, HarmonicEvent};
use crate::theory::chord::Quality;
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
    /// The key it is in, after any transposition.
    pub key: Key,
    /// How hard it is played.
    pub intensity: f32,
    /// Its chords, positioned from the section's own start.
    pub events: Vec<HarmonicEvent>,
    /// One structural pitch per chord, for the melody to hang on.
    pub skeleton: Vec<i32>,
    /// The parts that play, by name. Empty means all of them.
    pub parts: Vec<String>,
}

impl SectionPlan {
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

    /// The index of the chord sounding at a tick from the section's start.
    pub fn event_index_at(&self, tick: Ticks) -> usize {
        self.events
            .iter()
            .rposition(|event| event.start <= tick)
            .unwrap_or(0)
    }
}

/// The whole plan: every section, in order, with its harmony resolved.
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
    /// Whether the end of the last section joins something rather than being the end.
    ///
    /// A single clip does: whatever the arrangement puts after it, most often another playing of
    /// itself, which is the bar a drum loop wants its fill in. A piece does not — it stops, and
    /// a fill running into silence is a drummer who did not know the song had finished.
    ///
    /// A property of what is being written rather than of how many sections it has: a song with
    /// one section in it is still a song.
    pub joins_on: bool,
}

/// Builds the frame for a spec.
///
/// Everything random here is drawn from a named stream, so adding a part or changing a section
/// leaves the rest of the plan untouched.
pub fn plan(spec: &SongSpec) -> Frame {
    let grid = Grid::new(spec.meter, 4);
    let mut sections = Vec::new();
    let mut start = Ticks::ZERO;
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();

    for name in &spec.form {
        let Some(section) = spec.sections.get(name) else {
            continue;
        };
        let instance = counts.entry(name.as_str()).or_insert(0);
        *instance += 1;
        let instance = *instance;

        let key = spec.key.transposed(section.transpose);
        let chart = spec.chart_for(section).fit_to(section.bars);
        let mut events = chart.resolve(key, grid.bar_ticks());

        // A chart the user wrote or quoted is played as written. Only a chart the composer made
        // up is coloured, because colouring 丸サ進行 would stop it being 丸サ進行.
        if chart.origin == ChartOrigin::Generated {
            colour(&mut events, spec.mood, spec.seed, name, instance);
        }

        let length = grid.bar_ticks() * section.bars as i64;
        let skeleton = skeleton(&events, spec.seed, name, instance);

        sections.push(SectionPlan {
            name: name.clone(),
            instance,
            start,
            length,
            bars: section.bars,
            key,
            intensity: section.intensity,
            events,
            skeleton,
            parts: section.parts.clone(),
        });
        start += length;
    }

    Frame {
        grid,
        sections,
        length: start,
        seed: spec.seed,
        mood: spec.mood,
        joins_on: false,
    }
}

/// Adds sevenths, ninths and borrowed chords in proportion to the mood's tension.
///
/// Every roll is drawn whether or not it can be applied, so pinning one chord's quality does not
/// shift the colour of every chord after it.
fn colour(events: &mut [HarmonicEvent], mood: Mood, seed: u64, section: &str, instance: usize) {
    let mut rng = Rng::stream(
        seed,
        &[
            RngKey::Word("frame"),
            RngKey::Word("harmony"),
            RngKey::Word(section),
            RngKey::Index(instance as u64),
        ],
    );
    for event in events.iter_mut() {
        let seventh = rng.chance(mood.seventh_rate());
        let ninth = rng.chance(mood.ninth_rate());
        let borrow = rng.chance(mood.borrow_rate());
        if !event.numeral.is_colourable() {
            continue;
        }
        if seventh {
            event.chord.quality = event.chord.quality.with_seventh();
        }
        if ninth {
            event.chord.quality = event.chord.quality.with_ninth();
        }
        if borrow {
            // The parallel mode's version of the same degree: this is where a minor iv in a
            // major key comes from, and it is the cheapest colour in the book.
            let parallel = event.key.parallel();
            let borrowed = event.numeral.chord_in(parallel);
            if borrowed.root != event.chord.root || borrowed.quality != event.chord.quality {
                event.chord = borrowed;
            }
        }
    }
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
        let target = low as f32 + (high - low) as f32 * (0.25 + 0.5 * height);
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

/// Whether a chord is one a cadence would want to land on.
pub fn is_stable(quality: Quality) -> bool {
    matches!(
        quality,
        Quality::Major | Quality::Minor | Quality::Major7 | Quality::Major6 | Quality::Minor7
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theory::pitch::PitchClass;

    fn spec(text: &str) -> SongSpec {
        SongSpec::parse(text).expect("the fixture parses")
    }

    #[test]
    fn the_form_lays_the_sections_out_end_to_end() {
        let frame = plan(&spec(
            "
            form: intro verse chorus

            [section intro]
            bars: 4
            [section verse]
            bars: 8
            [section chorus]
            bars: 8
            ",
        ));
        assert_eq!(frame.sections.len(), 3);
        assert_eq!(frame.sections[0].start, Ticks::ZERO);
        assert_eq!(frame.sections[1].start, frame.grid.bar_ticks() * 4);
        assert_eq!(frame.sections[2].start, frame.grid.bar_ticks() * 12);
        assert_eq!(frame.length, frame.grid.bar_ticks() * 20);
    }

    #[test]
    fn a_repeated_section_counts_its_instances() {
        let frame = plan(&spec("form: verse chorus verse chorus"));
        let instances: Vec<usize> = frame.sections.iter().map(|s| s.instance).collect();
        assert_eq!(instances, [1, 1, 2, 2]);
    }

    #[test]
    fn a_section_repeats_its_chart_to_fill_its_bars() {
        let frame = plan(&spec(
            "
            form: verse
            chords: @axis

            [section verse]
            bars: 8
            ",
        ));
        let verse = &frame.sections[0];
        assert_eq!(verse.events.len(), 8, "a four-bar loop played twice");
        assert_eq!(verse.events[0].chord, verse.events[4].chord);
        assert_eq!(verse.events.last().unwrap().end(), verse.length);
    }

    #[test]
    fn a_transposed_section_moves_its_chords_and_nothing_else() {
        let frame = plan(&spec(
            "
            key: C major
            form: chorus

            [section chorus]
            transpose: 2
            ",
        ));
        let chorus = &frame.sections[0];
        assert_eq!(chorus.key.tonic, PitchClass::parse("D").unwrap());
        assert_eq!(chorus.events[0].chord.root, PitchClass::parse("D").unwrap());
    }

    #[test]
    fn a_written_progression_is_never_recoloured() {
        // Whatever the tension, a quoted chart comes out as it was written — which is the whole
        // reason for naming one.
        for tension in ["0.0", "0.5", "1.0"] {
            let frame = plan(&spec(&format!(
                "form: verse\nchords: @marusa\ntension: {tension}"
            )));
            let chords: Vec<String> = frame.sections[0]
                .events
                .iter()
                .take(4)
                .map(|event| event.chord.to_string())
                .collect();
            assert_eq!(
                chords,
                ["Fmaj7", "E7", "Am7", "C7"],
                "tension {tension} rewrote a quoted chart"
            );
        }
    }

    #[test]
    fn the_skeleton_gives_one_pitch_per_chord_inside_the_melody_range() {
        let frame = plan(&spec("form: verse\nchords: @axis"));
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
        let frame = plan(&spec(
            "form: verse\nchords: @canon\n[section verse]\nbars: 8",
        ));
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
        let frame = plan(&spec("form: verse\nchords: @axis"));
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
        let a = plan(&spec("form: verse\nseed: 1\ntension: 0.9"));
        let b = plan(&spec("form: verse\nseed: 1\ntension: 0.9"));
        assert_eq!(a.sections[0].skeleton, b.sections[0].skeleton);

        let c = plan(&spec("form: verse\nseed: 2\ntension: 0.9"));
        // Not a guarantee for every seed pair, but these two do differ, and the test pins that
        // the seed is actually reaching the plan.
        assert_ne!(a.sections[0].skeleton, c.sections[0].skeleton);
    }

    #[test]
    fn the_chord_at_a_tick_is_the_one_sounding_there() {
        let frame = plan(&spec("form: verse\nchords: @axis"));
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
}
