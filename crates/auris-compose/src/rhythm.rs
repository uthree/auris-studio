//! Where notes fall in time: the grid, the patterns written on it, and the grooves.

use auris_core::time::{Ticks, TimeSignature};

/// The subdivision everything is placed on.
///
/// A bar is a whole number of steps, so a pattern is a list of booleans and a position is an
/// index. Sixteenths are the default because that is what a drum pattern is written in.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Grid {
    /// The time signature the bar is counted in.
    pub signature: TimeSignature,
    /// How many steps one *quarter note* divides into.
    ///
    /// Held against the quarter and not against the beat, so that a step is a fixed note value in
    /// every meter: four is a sixteenth whether the bar is 4/4 or 6/8. It used to divide the note
    /// the denominator names, which made a "sixteenth" in 6/8 a thirty-second — the grid came out
    /// twice as fine as everything placing notes on it believed.
    pub steps_per_quarter: u32,
}

impl Default for Grid {
    fn default() -> Self {
        Self {
            signature: TimeSignature::default(),
            steps_per_quarter: 4,
        }
    }
}

impl Grid {
    /// A grid in `signature` whose step is a quarter note divided `steps_per_quarter` ways.
    pub fn new(signature: TimeSignature, steps_per_quarter: u32) -> Self {
        Self {
            signature,
            steps_per_quarter: steps_per_quarter.max(1),
        }
    }

    /// Ticks in one step.
    pub fn step_ticks(self) -> Ticks {
        Ticks(Ticks::QUARTER.raw() / i64::from(self.steps_per_quarter))
    }

    /// Ticks in one bar.
    pub fn bar_ticks(self) -> Ticks {
        self.signature.ticks_per_bar()
    }

    /// How many steps make up one *felt* beat — six sixteenths to a dotted quarter of 6/8.
    ///
    /// A derived number rather than a stored one, because it is a fact about the meter and the
    /// step together. Every part that asks "am I on a beat" asks this.
    pub fn steps_per_beat(self) -> usize {
        let step = self.step_ticks().raw().max(1);
        (self.signature.beat_ticks().raw() / step).max(1) as usize
    }

    /// How many steps make up one bar.
    pub fn steps_per_bar(self) -> usize {
        let step = self.step_ticks().raw().max(1);
        (self.bar_ticks().raw() / step) as usize
    }

    /// The tick a step index falls on.
    pub fn tick_of(self, step: usize) -> Ticks {
        self.step_ticks() * step as i64
    }

    /// The step index a tick falls on, rounding down.
    pub fn step_of(self, tick: Ticks) -> usize {
        let step = self.step_ticks().raw().max(1);
        (tick.raw().max(0) / step) as usize
    }

    /// `true` when the *step* is a triplet of the quarter rather than a division by two.
    ///
    /// Asked of the subdivision and not of the meter. A compound bar also has three steps to the
    /// eighth, but that is the meter being compound and not a triplet grid laid over it — reading
    /// this off `steps_per_beat` would call every 6/8 a triplet and switch swing off in 12/8 for
    /// the wrong reason.
    pub fn is_triplet(self) -> bool {
        self.steps_per_quarter.is_multiple_of(3)
    }

    /// How strong a beat this step is, from 0 for the weakest to 4 for the downbeat.
    ///
    /// This is the metric hierarchy: a note on the downbeat carries more weight than one on the
    /// beat, which carries more than one on an eighth, and so on. Almost every musical decision
    /// downstream — which notes may be dissonant, which get accented, which survive thinning —
    /// asks this one question.
    pub fn weight(self, step: usize) -> u8 {
        let per_bar = self.steps_per_bar().max(1);
        let position = step % per_bar;
        if position == 0 {
            return 4;
        }
        let per_beat = self.steps_per_beat();
        if position.is_multiple_of(per_beat) {
            // A beat, with the halfway point of the bar stronger than the rest.
            let beat = position / per_beat;
            let beats = per_bar / per_beat.max(1);
            return if beats.is_multiple_of(2) && beat == beats / 2 {
                3
            } else {
                2
            };
        }
        // Inside a beat: a step landing on a simpler division of it is stronger than one that
        // only exists at the finest level.
        let inside = position % per_beat;
        // A compound beat divides in three and in nothing else. Its halfway point is a *dotted*
        // eighth into a dotted quarter — a syncopation against the meter, not a position the
        // meter offers — so asking the halves family there would hand a real beat's weight to the
        // one step in 6/8 that most needs to be heard as a departure.
        let halves = !self.signature.is_compound()
            && per_beat.is_multiple_of(2)
            && inside.is_multiple_of(per_beat / 2);
        let thirds = per_beat.is_multiple_of(3) && inside.is_multiple_of(per_beat / 3);
        if halves || thirds { 1 } else { 0 }
    }
}

/// How hard a hit is struck.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Accent {
    /// A normal hit.
    Normal,
    /// A hit that leads the bar.
    Strong,
    /// A ghost note, felt more than heard.
    Ghost,
}

impl Accent {
    /// What this accent multiplies a velocity by.
    pub fn scale(self) -> f32 {
        match self {
            Accent::Normal => 1.0,
            Accent::Strong => 1.15,
            Accent::Ghost => 0.55,
        }
    }
}

/// A rhythm written as one character per step.
///
/// `x` is a hit, `X` a strong one, `o` a ghost note and `~`, `.` or `-` a rest. Spaces are free,
/// so a pattern can be grouped by beat and still read as one bar:
///
/// ```
/// # use auris_compose::rhythm::Pattern;
/// let backbeat = Pattern::parse("~ ~ ~ ~ X ~ ~ ~ ~ ~ ~ ~ X ~ ~ ~").unwrap();
/// assert_eq!(backbeat.onsets(), vec![4, 12]);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    /// One entry per step: `None` is a rest.
    pub steps: Vec<Option<Accent>>,
}

impl Pattern {
    /// An empty pattern of `steps` rests.
    pub fn rests(steps: usize) -> Self {
        Self {
            steps: vec![None; steps],
        }
    }

    /// Reads a pattern, ignoring whitespace.
    pub fn parse(text: &str) -> Option<Self> {
        let mut steps = Vec::new();
        for character in text.chars() {
            match character {
                ' ' | '\t' | '|' => continue,
                'x' => steps.push(Some(Accent::Normal)),
                'X' => steps.push(Some(Accent::Strong)),
                'o' => steps.push(Some(Accent::Ghost)),
                '~' | '.' | '-' => steps.push(None),
                _ => return None,
            }
        }
        (!steps.is_empty()).then_some(Self { steps })
    }

    /// How many steps long the pattern is.
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// `true` when the pattern has no steps at all.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// The step indices that carry a hit.
    pub fn onsets(&self) -> Vec<usize> {
        self.steps
            .iter()
            .enumerate()
            .filter_map(|(index, accent)| accent.map(|_| index))
            .collect()
    }

    /// The accent at `step`, wrapping so a short pattern repeats under a long bar.
    pub fn at(&self, step: usize) -> Option<Accent> {
        if self.steps.is_empty() {
            return None;
        }
        self.steps[step % self.steps.len()]
    }

    /// The accent at `step` for a pattern that is a whole *bar* rather than a repeating cell.
    ///
    /// [`Self::at`] wraps, which is right for the four-step cell somebody writes under a bar four
    /// times its length and wrong for a groove. A groove is one bar of drumming — a downbeat at
    /// the top, a turnaround at the end — and the built-in ones are all sixteen steps, which is a
    /// bar of 4/4 and a bar of nothing else. Wrapped under a 6/8 bar of twenty-four steps it
    /// restarted at the fifth eighth, putting a second downbeat where the bar has no beat at all;
    /// under 5/4 and 7/8 the same, and under 3/4 the turnaround simply never played.
    ///
    /// So the bar's first beat takes the groove's first, its last beat takes the groove's last,
    /// and the beats between cycle through the middle. A shorter bar drops middle beats and a
    /// longer one repeats them, which is what a drummer does with a pattern in a meter it was not
    /// written for.
    ///
    /// It is not a substitute for a groove written in the meter. A 4/4 rock beat mapped onto 6/8
    /// keeps its footing but it is still a four-beat idea in a two-beat bar; compound time wants
    /// its own patterns, and this is what stops the absence of them sounding like a fault.
    pub fn at_in_bar(
        &self,
        step: usize,
        steps_per_bar: usize,
        steps_per_beat: usize,
        own_steps_per_beat: usize,
    ) -> Option<Accent> {
        let steps_per_beat = steps_per_beat.max(1);
        let own = own_steps_per_beat.max(1);
        if self.steps.is_empty() || steps_per_bar == 0 {
            return None;
        }
        // A step inside the bar's beat, expressed inside the pattern's. They are the same length
        // in a simple meter and they are not in a compound one — a dotted quarter holds six
        // sixteenths where the groove's beat holds four — so the pattern is stretched over the
        // longer beat rather than counted out in it and running off the end.
        let across = |step: usize| (step % steps_per_beat) * own / steps_per_beat;
        // Stretching lands two bar steps on one pattern step. Only the first of them strikes, or
        // a single hit would come out as a flam nobody wrote.
        if !step.is_multiple_of(steps_per_beat) && across(step) == across(step - 1) {
            return None;
        }
        let pattern_beats = self.steps.len().div_ceil(own);
        let bar_beats = steps_per_bar.div_ceil(steps_per_beat);
        let beat = step / steps_per_beat;
        let middle = pattern_beats.saturating_sub(2);
        let mapped = if pattern_beats == bar_beats || pattern_beats < 2 || bar_beats < 2 {
            // Same count of beats, or too short for a first and a last to be different beats:
            // beat for beat is already the right answer, and is what a one-beat cell wants in any
            // meter. The step *inside* the beat is still stretched, which is the whole difference
            // between this and a plain wrap — a groove in eighths under a bar counted in
            // sixteenths lines up beat with beat and then reads its own eighths within them.
            beat
        } else if beat == 0 {
            0
        } else if beat + 1 >= bar_beats {
            pattern_beats - 1
        } else if middle == 0 {
            0
        } else {
            1 + (beat - 1) % middle
        };
        self.at(mapped * own + across(step))
    }

    /// How many hits the pattern has.
    pub fn hits(&self) -> usize {
        self.steps.iter().filter(|accent| accent.is_some()).count()
    }

    /// The pattern written back out, one character per step.
    pub fn to_text(&self) -> String {
        self.steps
            .iter()
            .map(|accent| match accent {
                Some(Accent::Normal) => 'x',
                Some(Accent::Strong) => 'X',
                Some(Accent::Ghost) => 'o',
                None => '~',
            })
            .collect()
    }
}

/// The Euclidean rhythm of `hits` spread over `steps`, rotated by `rotation`.
///
/// Bjorklund's algorithm, which distributes hits as evenly as the numbers allow. `(3,8)` is the
/// tresillo, `(5,8)` the cinquillo, `(2,5)`, `(5,16)` and the rest of the world's dance rhythms
/// fall out of the same two numbers — which is why it is worth having when a pattern has not
/// been written by hand.
pub fn euclid(hits: usize, steps: usize, rotation: usize) -> Pattern {
    if steps == 0 {
        return Pattern::rests(0);
    }
    let hits = hits.min(steps);
    let mut pattern = Pattern::rests(steps);
    if hits == 0 {
        return pattern;
    }
    // The standard closed form, which agrees with Bjorklund's algorithm on the rhythms anyone
    // has a name for: a hit lands wherever `(i * hits) mod steps` falls below `hits`.
    for index in 0..steps {
        if (index * hits) % steps < hits {
            pattern.steps[(index + rotation) % steps] = Some(Accent::Normal);
        }
    }
    pattern
}

/// Which drum a hit is played on.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum DrumVoice {
    /// The kick drum.
    Kick,
    /// The snare.
    Snare,
    /// A closed hi-hat.
    ClosedHat,
    /// An open hi-hat.
    OpenHat,
    /// A crash cymbal.
    Crash,
    /// A tom or other percussion.
    Percussion,
}

impl DrumVoice {
    /// The MIDI pitch this voice is written at.
    ///
    /// These are the General MIDI percussion numbers, which is what every DAW and every drum
    /// machine agrees on, even though the instrument here synthesises rather than samples.
    pub fn pitch(self) -> u8 {
        match self {
            DrumVoice::Kick => 36,
            DrumVoice::Snare => 38,
            DrumVoice::ClosedHat => 42,
            DrumVoice::OpenHat => 46,
            DrumVoice::Crash => 49,
            DrumVoice::Percussion => 47,
        }
    }
}

/// A named drum pattern set.
pub struct Groove {
    /// The name the text format writes.
    pub name: &'static str,
    /// What it is, for a listing.
    pub description: &'static str,
    /// The kick pattern, in sixteenths.
    pub kick: &'static str,
    /// The snare pattern.
    pub snare: &'static str,
    /// The hi-hat pattern.
    pub hat: &'static str,
    /// How much to swing the offbeats, as a percentage where 50 is straight.
    pub swing: u8,
    /// How many steps of this groove make one of *its* beats.
    ///
    /// Not the bar's beat, and the two differ in compound time: a groove in sixteenths of a simple
    /// beat counts four, one in eighths of a dotted beat counts three. It is what
    /// [`Pattern::at_in_bar`] needs to lay one over the other, and it is a property of the pattern
    /// rather than of the song, which is why it is written here beside the pattern it describes.
    pub steps_per_beat: usize,
}

impl Groove {
    /// The pattern for one voice.
    pub fn pattern(&self, voice: DrumVoice) -> Pattern {
        let text = match voice {
            DrumVoice::Kick => self.kick,
            DrumVoice::Snare => self.snare,
            DrumVoice::ClosedHat | DrumVoice::OpenHat => self.hat,
            DrumVoice::Crash | DrumVoice::Percussion => "",
        };
        Pattern::parse(text).unwrap_or_else(|| Pattern::rests(16))
    }
}

/// How many steps a pattern of unknown provenance is assumed to count to its beat.
///
/// Sixteenths, because that is what a drum pattern is written in and what every simple-meter
/// groove below uses. A groove states its own in [`Groove::steps_per_beat`]; this is the answer
/// for a name that matches none of them.
pub const GROOVE_STEPS_PER_BEAT: usize = 4;

/// Every groove the composer knows by name.
///
/// Quoted rather than derived: these are the patterns a kit actually plays, and the whole value of
/// naming one is that it comes out sounding like itself.
pub const GROOVES: &[Groove] = &[
    Groove {
        name: "basic-rock",
        description: "Four on the snare's two and four, eighths on the hat",
        kick: "x ~ ~ ~ ~ ~ x ~ ~ ~ x ~ ~ ~ ~ ~",
        snare: "~ ~ ~ ~ X ~ ~ ~ ~ ~ ~ ~ X ~ ~ ~",
        hat: "x ~ o ~ x ~ o ~ x ~ o ~ x ~ o ~",
        swing: 50,
        steps_per_beat: 4,
    },
    Groove {
        name: "eight-beat",
        description: "The straight eight-beat every J-rock song is built on",
        kick: "x ~ ~ ~ ~ ~ ~ ~ x ~ ~ ~ ~ ~ ~ ~",
        snare: "~ ~ ~ ~ X ~ ~ ~ ~ ~ ~ ~ X ~ ~ ~",
        hat: "x ~ x ~ x ~ x ~ x ~ x ~ x ~ x ~",
        swing: 50,
        steps_per_beat: 4,
    },
    Groove {
        name: "sixteen-beat",
        description: "Busier hats and a syncopated kick",
        kick: "x ~ ~ x ~ ~ x ~ ~ ~ x ~ x ~ ~ ~",
        snare: "~ ~ ~ ~ X ~ ~ o ~ ~ ~ ~ X ~ o ~",
        hat: "x o x o x o x o x o x o x o x o",
        swing: 50,
        steps_per_beat: 4,
    },
    Groove {
        name: "four-on-the-floor",
        description: "A kick on every beat, for house and its descendants",
        kick: "x ~ ~ ~ x ~ ~ ~ x ~ ~ ~ x ~ ~ ~",
        snare: "~ ~ ~ ~ X ~ ~ ~ ~ ~ ~ ~ X ~ ~ ~",
        hat: "~ ~ x ~ ~ ~ x ~ ~ ~ x ~ ~ ~ x ~",
        swing: 50,
        steps_per_beat: 4,
    },
    Groove {
        name: "shuffle",
        description: "A swung eight-beat",
        kick: "x ~ ~ ~ ~ ~ x ~ x ~ ~ ~ ~ ~ ~ ~",
        snare: "~ ~ ~ ~ X ~ ~ ~ ~ ~ ~ ~ X ~ ~ ~",
        hat: "x ~ o ~ x ~ o ~ x ~ o ~ x ~ o ~",
        swing: 66,
        steps_per_beat: 4,
    },
    Groove {
        name: "breakbeat",
        description: "A broken kick and a snare that lands early, with ghosts around it",
        kick: "x ~ ~ ~ ~ ~ x ~ ~ ~ x ~ ~ ~ ~ ~",
        snare: "~ ~ ~ ~ X ~ ~ o ~ ~ ~ o X ~ ~ o",
        hat: "x ~ x ~ x ~ x ~ x ~ x ~ x ~ x ~",
        swing: 50,
        steps_per_beat: 4,
    },
    Groove {
        name: "bossa-nova",
        description: "The clave on the rim over a surdo on every beat",
        kick: "x ~ ~ ~ x ~ ~ ~ x ~ ~ ~ x ~ ~ ~",
        snare: "x ~ ~ x ~ ~ x ~ ~ ~ x ~ x ~ ~ ~",
        hat: "x ~ x ~ x ~ x ~ x ~ x ~ x ~ x ~",
        swing: 50,
        steps_per_beat: 4,
    },
    Groove {
        name: "half-time",
        description: "The backbeat moved to bar's centre, which halves the felt tempo",
        kick: "x ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~",
        snare: "~ ~ ~ ~ ~ ~ ~ ~ X ~ ~ ~ ~ ~ ~ ~",
        hat: "x ~ ~ ~ x ~ ~ ~ x ~ ~ ~ x ~ ~ ~",
        swing: 50,
        steps_per_beat: 4,
    },
    Groove {
        name: "sparse",
        description: "Almost nothing: for intros and ambient sections",
        kick: "x ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~",
        snare: "~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~",
        hat: "x ~ ~ ~ ~ ~ ~ ~ x ~ ~ ~ ~ ~ ~ ~",
        swing: 50,
        steps_per_beat: 4,
    },
    // The compound ones, counted in eighths of a dotted beat rather than sixteenths of a plain
    // one. Mapping a 4/4 groove onto 6/8 keeps its footing but stays a four-beat idea in a
    // two-beat bar; these are the two-beat and four-beat ideas themselves. Nothing chooses one
    // automatically — a song in 6/8 has to name it, the way it names any other groove.
    Groove {
        name: "six-eight",
        description: "Two dotted beats, with the hat counting the eighths between them",
        kick: "x ~ ~ ~ ~ ~",
        snare: "~ ~ ~ X ~ ~",
        hat: "x o o x o o",
        swing: 50,
        steps_per_beat: 3,
    },
    Groove {
        name: "slow-blues",
        description: "The 12/8 shuffle: a backbeat under a ride that skips",
        kick: "x ~ ~ ~ ~ ~ x ~ ~ ~ ~ ~",
        snare: "~ ~ ~ X ~ ~ ~ ~ ~ X ~ ~",
        hat: "x ~ o x ~ o x ~ o x ~ o",
        swing: 50,
        steps_per_beat: 3,
    },
];

/// The groove with this name, matched the way every other vocabulary in the format is.
pub fn groove(name: &str) -> Option<&'static Groove> {
    let name = name.trim().to_ascii_lowercase();
    GROOVES.iter().find(|groove| groove.name == name)
}

/// How far a note on `step` is delayed by swinging at `percent`.
///
/// Swing is felt at the eighth, not the sixteenth: the *second eighth of each beat* is late, and
/// the sixteenths inside it move with it. `percent` says where that eighth lands inside its
/// beat — 50 is straight, 66.7 puts it on the third triplet.
pub fn swing_offset(grid: Grid, step: usize, percent: u8) -> Ticks {
    // A triplet grid is already sitting where swing is trying to push a straight one, so there is
    // nothing left to delay. It also has no half-beat to measure the delay in: the unit below
    // would round to a single step and every other triplet would be shoved off the beat, which is
    // not a swung triplet but a broken one.
    if grid.is_triplet() {
        return Ticks::ZERO;
    }
    // Compound time is already the shuffle a swing dial is reaching for: a 6/8 beat *is* three
    // eighths, which is where swinging a 4/4 beat is trying to get to. There is no straight pair
    // left to delay, and delaying the middle of three would push the beat about rather than swing
    // it. The dial goes quiet here for the same reason it does over a triplet grid.
    if grid.signature.is_compound() {
        return Ticks::ZERO;
    }
    // Half a beat, in steps: the unit that gets swung.
    let unit = (grid.steps_per_beat() / 2).max(1);
    if (step / unit).is_multiple_of(2) {
        return Ticks::ZERO;
    }
    let unit_ticks = grid.step_ticks().raw() * unit as i64;
    let shift = (f64::from(percent) / 100.0 - 0.5) * 2.0 * unit_ticks as f64;
    Ticks(shift.round() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::time::TICKS_PER_QUARTER;

    fn four_four() -> Grid {
        Grid::default()
    }

    #[test]
    fn a_bar_of_four_four_is_sixteen_sixteenths() {
        let grid = four_four();
        assert_eq!(grid.steps_per_bar(), 16);
        assert_eq!(grid.step_ticks(), Ticks(TICKS_PER_QUARTER / 4));
        assert_eq!(grid.bar_ticks(), Ticks(TICKS_PER_QUARTER * 4));
        assert_eq!(grid.tick_of(4), Ticks(TICKS_PER_QUARTER));
        assert_eq!(grid.step_of(Ticks(TICKS_PER_QUARTER)), 4);
        assert_eq!(grid.step_of(Ticks(TICKS_PER_QUARTER + 1)), 4, "rounds down");
    }

    #[test]
    fn three_four_has_twelve_steps_and_no_halfway_beat() {
        let grid = Grid::new(TimeSignature::new(3, 4), 4);
        assert_eq!(grid.steps_per_bar(), 12);
        assert_eq!(grid.weight(0), 4);
        assert_eq!(grid.weight(4), 2, "beat two of three is just a beat");
        assert_eq!(grid.weight(8), 2);
    }

    #[test]
    fn a_triplet_beat_divides_exactly_and_keeps_its_hierarchy() {
        // A triplet is a position, not a rounding error: 960 ticks per quarter divides by three.
        let grid = Grid::new(TimeSignature::new(4, 4), 3);
        assert!(grid.is_triplet());
        assert_eq!(grid.step_ticks(), Ticks(TICKS_PER_QUARTER / 3));
        assert_eq!(grid.step_ticks() * 3, Ticks(TICKS_PER_QUARTER), "no drift");
        assert_eq!(grid.steps_per_bar(), 12);

        assert_eq!(grid.weight(0), 4, "the downbeat");
        assert_eq!(grid.weight(6), 3, "halfway through the bar");
        assert_eq!(grid.weight(3), 2, "beat two");
        // The two offbeat triplets are the level immediately below the beat, which is exactly
        // what an offbeat eighth is on a straight grid — so they carry the same weight.
        assert_eq!(grid.weight(1), 1);
        assert_eq!(grid.weight(2), 1);
    }

    #[test]
    fn a_beat_of_six_is_felt_in_halves_and_in_thirds_at_once() {
        // Sixteenth triplets: the halfway point is the offbeat eighth and the thirds are the
        // eighth triplets. Both are real positions, and only the steps between them are the
        // finest level. A rule that asked one question would have flattened four of the five.
        let grid = Grid::new(TimeSignature::new(4, 4), 6);
        assert_eq!(grid.steps_per_bar(), 24);
        assert_eq!(grid.weight(0), 4);
        assert_eq!(grid.weight(12), 3, "halfway through the bar");
        assert_eq!(grid.weight(6), 2, "beat two");
        assert_eq!(grid.weight(3), 1, "the offbeat eighth");
        assert_eq!(grid.weight(2), 1, "an eighth triplet");
        assert_eq!(grid.weight(4), 1);
        assert_eq!(grid.weight(1), 0, "only a sixteenth triplet");
        assert_eq!(grid.weight(5), 0);
    }

    #[test]
    fn an_eighth_grid_still_calls_its_offbeat_an_offbeat() {
        let grid = Grid::new(TimeSignature::new(4, 4), 2);
        assert!(!grid.is_triplet());
        assert_eq!(grid.steps_per_bar(), 8);
        assert_eq!(grid.weight(4), 3, "halfway through the bar");
        assert_eq!(grid.weight(1), 1, "the offbeat eighth");
    }

    #[test]
    fn the_metric_hierarchy_runs_downbeat_halfway_beat_eighth_sixteenth() {
        let grid = four_four();
        assert_eq!(grid.weight(0), 4, "the downbeat");
        assert_eq!(grid.weight(8), 3, "halfway through the bar");
        assert_eq!(grid.weight(4), 2, "beat two");
        assert_eq!(grid.weight(12), 2, "beat four");
        assert_eq!(grid.weight(2), 1, "an offbeat eighth");
        assert_eq!(grid.weight(1), 0, "a sixteenth");
        assert_eq!(grid.weight(15), 0);
        // And it repeats bar by bar.
        assert_eq!(grid.weight(16), 4);
        assert_eq!(grid.weight(24), 3);
    }

    #[test]
    fn a_pattern_reads_one_character_per_step() {
        let pattern = Pattern::parse("x ~ ~ ~ X ~ o ~").unwrap();
        assert_eq!(pattern.len(), 8);
        assert_eq!(pattern.onsets(), vec![0, 4, 6]);
        assert_eq!(pattern.hits(), 3);
        assert_eq!(pattern.at(0), Some(Accent::Normal));
        assert_eq!(pattern.at(4), Some(Accent::Strong));
        assert_eq!(pattern.at(6), Some(Accent::Ghost));
        assert_eq!(pattern.at(1), None);
    }

    #[test]
    fn a_pattern_repeats_under_a_longer_bar() {
        let pattern = Pattern::parse("x ~").unwrap();
        assert_eq!(pattern.at(0), Some(Accent::Normal));
        assert_eq!(pattern.at(2), Some(Accent::Normal), "wrapped around");
        assert_eq!(pattern.at(3), None);
        assert_eq!(Pattern::rests(4).at(0), None);
    }

    #[test]
    fn a_pattern_round_trips_through_its_text() {
        let text = "x~~~X~o~";
        assert_eq!(Pattern::parse(text).unwrap().to_text(), text);
        // Spacing and bar lines are free.
        assert_eq!(Pattern::parse("x ~ ~ ~ | X ~ o ~").unwrap().to_text(), text);
        assert!(
            Pattern::parse("x q x").is_none(),
            "an unknown mark is an error"
        );
        assert!(Pattern::parse("   ").is_none());
    }

    #[test]
    fn euclidean_rhythms_are_the_ones_they_are_named_for() {
        // The tresillo: three hits over eight steps.
        assert_eq!(euclid(3, 8, 0).onsets(), vec![0, 3, 6]);
        // The cinquillo.
        assert_eq!(euclid(5, 8, 0).onsets(), vec![0, 2, 4, 5, 7]);
        // Four on the floor is the degenerate case.
        assert_eq!(euclid(4, 16, 0).onsets(), vec![0, 4, 8, 12]);
        // (2,5) comes out as the mirror of the form Toussaint tabulates; it is the same rhythm
        // read from the other end, and every rotation of it is reachable through `rotation`.
        assert_eq!(euclid(2, 5, 0).onsets(), vec![0, 3]);
    }

    #[test]
    fn euclidean_edges_do_not_panic_or_overrun() {
        assert_eq!(euclid(0, 8, 0).hits(), 0);
        assert_eq!(euclid(8, 8, 0).hits(), 8, "every step");
        assert_eq!(
            euclid(99, 8, 0).hits(),
            8,
            "more hits than steps is clamped"
        );
        assert_eq!(euclid(3, 0, 0).len(), 0);
        // A rotation moves the hits without changing how many there are.
        assert_eq!(euclid(3, 8, 2).hits(), 3);
        assert_eq!(euclid(3, 8, 2).onsets(), vec![0, 2, 5]);
        assert_eq!(
            euclid(3, 8, 8).onsets(),
            euclid(3, 8, 0).onsets(),
            "a full turn"
        );
    }

    #[test]
    fn a_compound_bar_is_counted_in_dotted_beats_of_real_sixteenths() {
        // Four steps to the quarter is a sixteenth in any meter. A 6/8 bar is three quarters, so
        // twelve of them — it used to be twenty-four, because the step divided the *eighth* the
        // denominator names and came out a thirty-second.
        let six_eight = Grid::new(TimeSignature::new(6, 8), 4);
        assert_eq!(six_eight.step_ticks(), Ticks(Ticks::QUARTER.raw() / 4));
        assert_eq!(six_eight.steps_per_bar(), 12);
        assert_eq!(
            six_eight.steps_per_beat(),
            6,
            "a dotted quarter of sixteenths"
        );
        assert_eq!(six_eight.steps_per_bar() / six_eight.steps_per_beat(), 2);

        // The metric hierarchy of those twelve steps. Two beats; the eighths inside each are real
        // positions; the step halfway through a dotted quarter is not one the meter offers.
        let weights: Vec<u8> = (0..12).map(|step| six_eight.weight(step)).collect();
        assert_eq!(weights, [4, 0, 1, 0, 1, 0, 3, 0, 1, 0, 1, 0]);

        // 12/8 is four of the same beat.
        let twelve_eight = Grid::new(TimeSignature::new(12, 8), 4);
        assert_eq!(twelve_eight.steps_per_bar(), 24);
        assert_eq!(twelve_eight.steps_per_beat(), 6);

        // 4/4 is what it always was, which is why every fixture held.
        let four_four = Grid::new(TimeSignature::default(), 4);
        assert_eq!(four_four.steps_per_bar(), 16);
        assert_eq!(four_four.steps_per_beat(), 4);
        assert_eq!(
            (0..16)
                .map(|step| four_four.weight(step))
                .collect::<Vec<_>>(),
            [4, 0, 1, 0, 2, 0, 1, 0, 3, 0, 1, 0, 2, 0, 1, 0]
        );

        // And a compound meter is not a triplet grid: the subdivision is what decides that.
        assert!(!six_eight.is_triplet());
        assert!(Grid::new(TimeSignature::new(6, 8), 3).is_triplet());

        // Swing is silent in compound time — the bar is already the shuffle it aims at.
        assert_eq!(swing_offset(six_eight, 3, 67), Ticks::ZERO);
        assert_ne!(swing_offset(four_four, 2, 67), Ticks::ZERO);
    }

    #[test]
    fn a_groove_is_mapped_onto_the_bar_rather_than_wrapped_round_it() {
        // A bar-long pattern with the downbeat and the turnaround marked, and nothing between.
        let groove = Pattern::parse("x ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ x ~ ~ ~").expect("parses");
        let hits = |steps_per_bar: usize| -> Vec<usize> {
            (0..steps_per_bar)
                .filter(|step| groove.at_in_bar(*step, steps_per_bar, 4, 4).is_some())
                .collect()
        };

        // 4/4 is the meter it was written in, so nothing moves.
        assert_eq!(hits(16), [0, 12]);

        // 6/8 is twenty-four steps. Wrapping put the groove's *downbeat* back at step 16 — a
        // second bar line two thirds of the way through the bar, which is the thing a listener
        // hears as the drummer losing the meter.
        assert_eq!(
            groove.at(16),
            Some(Accent::Normal),
            "the plain wrap is what this test exists to be different from"
        );
        assert_eq!(hits(24), [0, 20], "6/8 grew a downbeat mid-bar");

        // 5/4 and 7/8 the same, and 3/4 keeps the turnaround it used to lose entirely.
        assert_eq!(hits(20), [0, 16], "5/4");
        assert_eq!(hits(28), [0, 24], "7/8");
        assert_eq!(hits(12), [0, 8], "3/4 dropped its turnaround");

        // And a real 6/8 bar, which is twelve sixteenths in two dotted-quarter beats while the
        // groove is sixteen sixteenths in four beats of four. The groove's beat is stretched over
        // the longer one, and each of its steps strikes once — so the bar comes out with the
        // downbeat on its first beat and the turnaround on its second, which is what two beats of
        // a 4/4 pattern reduce to and what a drummer would play.
        assert_eq!(
            (0..12)
                .filter(|step| groove.at_in_bar(*step, 12, 6, 4).is_some())
                .collect::<Vec<_>>(),
            [0, 6]
        );

        // The first and last beat of the bar always carry the first and last beat of the groove,
        // in every meter, which is the property the mapping is for.
        for steps_per_bar in [8, 12, 16, 20, 24, 28] {
            let bar = hits(steps_per_bar);
            assert_eq!(bar.first(), Some(&0), "no downbeat in {steps_per_bar}");
            assert_eq!(
                bar.last(),
                Some(&(steps_per_bar - 4)),
                "no turnaround in {steps_per_bar}"
            );
        }
    }

    #[test]
    fn a_written_rhythm_is_a_cell_and_still_repeats() {
        // The counterpart. Somebody who writes four steps means them four times under a 4/4 bar,
        // and mapping that onto the bar would play it once and pad the rest.
        let cell = Pattern::parse("x ~ x ~").expect("parses");
        assert_eq!(
            (0..16)
                .filter(|step| cell.at(*step).is_some())
                .collect::<Vec<_>>(),
            [0, 2, 4, 6, 8, 10, 12, 14]
        );
    }

    #[test]
    fn every_groove_parses_and_is_a_whole_number_of_bars() {
        let mut seen = Vec::new();
        for groove in GROOVES {
            assert!(
                !seen.contains(&groove.name),
                "`{}` is listed twice",
                groove.name
            );
            seen.push(groove.name);
            // One bar, and the same bar for every voice. The length itself is the groove's own
            // business — sixteen sixteenths of 4/4, six eighths of 6/8 — but a snare longer than
            // the kick under it would be a groove that drifts against itself.
            let bar = groove.pattern(DrumVoice::Kick).len();
            assert!(
                bar.is_multiple_of(groove.steps_per_beat),
                "`{}` is {bar} steps, which is not a whole number of its {}-step beats",
                groove.name,
                groove.steps_per_beat
            );
            for voice in [DrumVoice::Kick, DrumVoice::Snare, DrumVoice::ClosedHat] {
                let pattern = groove.pattern(voice);
                assert_eq!(
                    pattern.len(),
                    bar,
                    "`{}` has a {voice:?} pattern of {} steps against a bar of {bar}",
                    groove.name,
                    pattern.len()
                );
            }
            assert!(
                groove.pattern(DrumVoice::Kick).hits() > 0,
                "{}",
                groove.name
            );
            assert!((40..=80).contains(&groove.swing), "{}", groove.name);
        }
        assert!(groove("basic-rock").is_some());
        assert!(groove("nonsense").is_none());
    }

    #[test]
    fn a_compound_groove_lands_on_the_dotted_beats_of_the_bar_it_was_written_for() {
        let blues = groove("slow-blues").expect("listed");
        assert_eq!(blues.steps_per_beat, 3);
        // Twelve eighths: four dotted beats, the backbeat on the second and fourth of them.
        assert_eq!(blues.pattern(DrumVoice::Snare).onsets(), vec![3, 9]);

        // And laid over a real 12/8 bar of sixteenths, where a dotted beat is six steps and the
        // groove's eighth is two of them. The backbeat has to stay on the dotted beats.
        let grid = Grid::new(TimeSignature::new(12, 8), 4);
        assert_eq!(grid.steps_per_beat(), 6);
        let snare = blues.pattern(DrumVoice::Snare);
        let struck: Vec<_> = (0..grid.steps_per_bar())
            .filter(|step| {
                snare
                    .at_in_bar(*step, grid.steps_per_bar(), grid.steps_per_beat(), 3)
                    .is_some()
            })
            .collect();
        assert_eq!(struck, vec![6, 18], "the second and fourth dotted beats");
    }

    #[test]
    fn a_compound_groove_is_not_stretched_into_a_bar_that_does_not_want_it() {
        // Six eighths under a 6/8 bar of sixteenths: two dotted beats either way, so the mapping
        // is a plain doubling and the hat still counts every eighth.
        let six = groove("six-eight").expect("listed");
        let grid = Grid::new(TimeSignature::new(6, 8), 4);
        assert_eq!(grid.steps_per_bar(), 12);
        let hat = six.pattern(DrumVoice::ClosedHat);
        let struck: Vec<_> = (0..grid.steps_per_bar())
            .filter(|step| {
                hat.at_in_bar(*step, grid.steps_per_bar(), grid.steps_per_beat(), 3)
                    .is_some()
            })
            .collect();
        assert_eq!(struck, vec![0, 2, 4, 6, 8, 10]);
    }

    #[test]
    fn the_backbeat_falls_on_two_and_four() {
        let rock = groove("basic-rock").unwrap();
        assert_eq!(
            rock.pattern(DrumVoice::Snare).onsets(),
            vec![4, 12],
            "beats two and four of a sixteenth-step bar"
        );
        assert_eq!(rock.pattern(DrumVoice::Kick).onsets()[0], 0);
    }

    #[test]
    fn four_on_the_floor_puts_a_kick_on_every_beat() {
        let house = groove("four-on-the-floor").unwrap();
        assert_eq!(house.pattern(DrumVoice::Kick).onsets(), vec![0, 4, 8, 12]);
        assert_eq!(
            house.pattern(DrumVoice::ClosedHat).onsets(),
            vec![2, 6, 10, 14],
            "the hat sits on the offbeats"
        );
    }

    #[test]
    fn drum_voices_use_the_general_midi_numbers() {
        assert_eq!(DrumVoice::Kick.pitch(), 36);
        assert_eq!(DrumVoice::Snare.pitch(), 38);
        assert_eq!(DrumVoice::ClosedHat.pitch(), 42);
    }

    #[test]
    fn swing_delays_the_offbeat_eighth_and_leaves_the_beat_alone() {
        let grid = four_four();
        assert_eq!(swing_offset(grid, 0, 66), Ticks::ZERO, "the downbeat");
        assert_eq!(
            swing_offset(grid, 1, 66),
            Ticks::ZERO,
            "still the first eighth"
        );
        assert_eq!(swing_offset(grid, 4, 66), Ticks::ZERO, "beat two");
        assert_eq!(
            swing_offset(grid, 2, 50),
            Ticks::ZERO,
            "straight is no swing"
        );

        // The offbeat eighth is what gets delayed. An eighth is 480 ticks, so at 66 % the shift
        // is 480 * (0.66 - 0.5) * 2 = 154, which lands it just shy of the third triplet.
        assert_eq!(swing_offset(grid, 2, 66), Ticks(154));
        assert!(
            swing_offset(grid, 2, 66) > Ticks::ZERO,
            "swing delays, never rushes"
        );
        // The sixteenth inside a swung eighth moves with it.
        assert_eq!(swing_offset(grid, 3, 66), Ticks(154));
        assert_eq!(
            swing_offset(grid, 6, 66),
            Ticks(154),
            "every offbeat eighth"
        );
    }

    #[test]
    fn a_triplet_grid_is_never_swung() {
        // Swing exists to push a straight offbeat toward the third triplet. On a grid that is
        // already there it has nothing to do, and the half-beat unit it measures in does not
        // exist — so it would shove every other triplet off the beat instead.
        for steps_per_beat in [3, 6] {
            let grid = Grid::new(TimeSignature::new(4, 4), steps_per_beat);
            for step in 0..grid.steps_per_bar() {
                assert_eq!(
                    swing_offset(grid, step, 66),
                    Ticks::ZERO,
                    "step {step} of a grid in {steps_per_beat}s"
                );
            }
        }
    }

    #[test]
    fn accents_scale_a_velocity_the_way_they_read() {
        assert!(Accent::Strong.scale() > Accent::Normal.scale());
        assert!(Accent::Ghost.scale() < Accent::Normal.scale());
        assert_eq!(Accent::Normal.scale(), 1.0);
    }
}
