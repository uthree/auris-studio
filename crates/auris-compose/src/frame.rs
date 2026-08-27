//! The plan every part is written against.
//!
//! Harmony, form and the melodic skeleton are decided once, before any part exists, and then
//! frozen. That is what makes the parts agree with each other without knowing about each other:
//! the bass and the melody both read the same chord at the same tick, so they cannot drift, and
//! each part is a pure function of the frame and its own name.

use auris_core::time::Ticks;

use crate::rhythm::{Grid, Pattern};
use crate::rng::{Key as RngKey, Rng};
use crate::spec::{Ending, LeadIn, Mood, SectionSpec, SongSpec};
use crate::theory::chart::{ChartOrigin, HarmonicEvent};
use crate::theory::chord::{Chord, Quality};
use crate::theory::key::Key;
use crate::theory::numeral::{Numeral, degree_of, diatonic_quality, diatonic_seventh};
use crate::theory::pitch::PitchClass;

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
    /// The tempo it is played at, resolved: its own, or the song's.
    ///
    /// Resolved here rather than left as the `Option` a specification holds, because everything
    /// that reads it wants a number — the humanisation converting a wander in milliseconds into
    /// ticks, and the tempo map the finished piece hands to a document. A part looking up "the
    /// tempo" and finding `None` would have to know what to fall back to, and that would be the
    /// second place in the program that knows.
    pub tempo: f64,
    /// How hard it is played.
    pub intensity: f32,
    /// Its chords, positioned from the section's own start.
    pub events: Vec<HarmonicEvent>,
    /// One structural pitch per chord, for the melody to hang on.
    pub skeleton: Vec<i32>,
    /// The parts that play, by name. Empty means all of them.
    pub parts: Vec<String>,
    /// What this section changes about how particular parts play, by part name.
    pub tweaks: std::collections::BTreeMap<String, crate::spec::PartTweak>,
    /// `true` for the held final bar a piece ends on, which is written by its own small writer
    /// rather than by the role writers: a figure over the last chord would be one more bar of
    /// the piece, and what an ending is, is the piece stopping *on* something.
    pub coda: bool,
}

impl SectionPlan {
    /// A part as this section plays it: the roster's, with whatever this section patches.
    ///
    /// Every pass has to go through here and none may read the roster's copy directly, which is
    /// the whole risk this method exists to name. A writer that took the section's density and a
    /// gate applied afterwards from the roster would be one part played two ways at once.
    pub fn played(&self, part: &crate::spec::PartSpec) -> crate::spec::PartSpec {
        match self.tweaks.get(&part.name) {
            Some(tweak) => tweak.applied_to(part),
            None => part.clone(),
        }
    }
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

    // The form resolved before anything is written, because a section has to know what follows it:
    // a key change is prepared in the bars *before* it, and those bars belong to the section that
    // does not modulate. Reading the next entry's key is the whole reason this is two passes.
    let played: Vec<&SectionSpec> = spec
        .form
        .iter()
        .filter_map(|name| spec.sections.get(name))
        .collect();

    for (place, name) in spec
        .form
        .iter()
        .filter(|name| spec.sections.contains_key(*name))
        .enumerate()
    {
        let Some(section) = spec.sections.get(name) else {
            continue;
        };
        let instance = counts.entry(name.as_str()).or_insert(0);
        *instance += 1;
        let instance = *instance;

        let key = spec.key.transposed(section.transpose);
        // Read against the key before it is fitted to the bars: a progression quoted by name is
        // written in a mode, and asked for in the other one it names its chords from the
        // relative key rather than reading its degrees literally. 丸サ進行 in C minor is the loop
        // centred on C minor, not four degrees of an aeolian scale.
        let chart = spec.chart_for(section).spelled_in(key).fit_to(section.bars);
        let mut events = chart.resolve(key, grid.bar_ticks());

        // A chart the user wrote or quoted is played as written. Only a chart the composer made
        // up is coloured, because colouring 丸サ進行 would stop it being 丸サ進行.
        if chart.origin == ChartOrigin::Generated {
            colour(&mut events, spec.mood, spec.seed, name, instance);
        }

        // The turnaround: the composer's own chart leans into an arrival. Only its own — a
        // quoted chart is played as written, which is the same trade `colour` makes — and only
        // where the form actually arrives somewhere, which is the same question the cymbal asks:
        // the harmony, the fill and the crash should all read one join the same way.
        if chart.origin == ChartOrigin::Generated
            && let Some(next) = played.get(place + 1)
            && spec.key.transposed(next.transpose) == key
            && next.intensity >= section.intensity
        {
            let opening = spec
                .chart_for(next)
                .spelled_in(key)
                .bars
                .first()
                .and_then(|bar| bar.first())
                .map(|numeral| numeral.chord_in(key).root);
            if opening == Some(key.tonic) {
                turn_around(&mut events, key);
            }
        }
        // The held ending is an arrival by construction — it opens on the tonic the whole piece
        // has been heading for — so the composer's own chart turns around into it exactly as it
        // does into any other arrival.
        if chart.origin == ChartOrigin::Generated
            && spec.ending == Ending::Held
            && place + 1 == played.len()
        {
            turn_around(&mut events, key);
        }

        // Before the skeleton, because the melody hangs on these chords: a line written against
        // the chord that was there and then played over the dominant that replaced it would be
        // the one part in the band not in on the modulation.
        if let Some(next) = played.get(place + 1) {
            let arriving = spec.key.transposed(next.transpose);
            if next.lead_in == LeadIn::Dominant {
                lead_into(&mut events, key, arriving);
            }
        }

        let length = grid.bar_ticks() * section.bars as i64;
        let skeleton = skeleton(&events, spec.seed, name, instance, spec.mood.brightness);

        sections.push(SectionPlan {
            name: name.clone(),
            instance,
            start,
            length,
            bars: section.bars,
            key,
            tempo: spec.tempo_of(section),
            intensity: section.intensity,
            events,
            skeleton,
            parts: section.parts.clone(),
            tweaks: section.tweaks.clone(),
            coda: false,
        });
        start += length;
    }

    // The ending: one bar of the final key's tonic after the last section, held by the band and
    // struck once by the kick and the cymbal — the bar every performance of anything ends on.
    // The last section used to play its loop out and stop mid-groove, as if the tape ran out.
    //
    // A plan-level section rather than a note pass, because everything downstream already speaks
    // sections: it arrives as a labelled stretch on the timeline, a tonic in the harmony lane,
    // and a clip per part, with nothing translated. The chord is built through one numeral so
    // `event.chord == event.numeral.chord_in(event.key)` holds by construction, like everywhere
    // else.
    if spec.ending == Ending::Held
        && let Some(last) = sections.last()
    {
        let key = last.key;
        let (tempo, intensity) = (last.tempo, last.intensity);
        let numeral = Numeral::new(1, diatonic_quality(key, 1).is_minor());
        let chord = numeral.chord_in(key);
        let length = grid.bar_ticks();
        // The melody's landing: the tonic nearest the middle of its role's range, which is
        // where the skeleton's arch has been living all along.
        let (low, high) = crate::spec::Role::Melody.range();
        let middle = (low + high) / 2;
        let resting = (low..=high)
            .filter(|pitch| PitchClass::new(*pitch) == key.tonic)
            .min_by_key(|pitch| (pitch - middle).abs())
            .unwrap_or(middle);
        sections.push(SectionPlan {
            name: "ending".to_string(),
            instance: 1,
            start,
            length,
            bars: 1,
            key,
            tempo,
            intensity,
            events: vec![HarmonicEvent {
                numeral,
                chord,
                key,
                start: Ticks::ZERO,
                length,
                bar: 0,
            }],
            skeleton: vec![resting],
            parts: Vec::new(),
            tweaks: Default::default(),
            coda: true,
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
///
/// The numeral is brought along with the chord. An event carries both, and they are not two
/// spellings of one fact for the fun of it: the chord is what every part plays, and the numeral is
/// what gets written down — the harmony lane names it, and a Chords clip generated later resolves
/// it again. Colouring one and leaving the other saying what the chord used to be is a document
/// that contradicts its own audio, and it saves that way.
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
        // Which mode the degree is taken from. A borrow moves it into the parallel one, which is
        // where a minor iv in a major key comes from and the cheapest colour in the book.
        let source = if borrow {
            event.key.parallel()
        } else {
            event.key
        };
        // Asked for as the source mode's *own* chord on that degree — see `Numeral::as_diatonic`
        // for why replaying the numeral's case instead answered A♭ minor for a borrowed vi, and
        // never answered anything at all for a borrowed I, IV or V.
        let mut chord = event.numeral.as_diatonic(source).chord_in(source);
        if seventh {
            // The seventh the *key* stacks on that degree, which is the only thing that knows a
            // dominant from a tonic. `Quality::with_seventh` sees a major triad and can only give
            // it a major seventh, so this used to write Vmaj7 — an F♯ in the key of C — on the
            // one chord where the seventh matters most.
            chord.quality = Some(event.numeral)
                .filter(|numeral| numeral.accidental == 0)
                .and_then(|numeral| diatonic_seventh(source, numeral.degree))
                .unwrap_or_else(|| chord.quality.with_seventh());
            // Only over a seventh, which is what `Mood::ninth_rate` already says it is. On its
            // own it made an add9 of a major triad and nothing whatever of a minor one.
            if ninth {
                chord.quality = chord.quality.with_ninth();
            }
        }
        if chord == event.chord {
            continue;
        }
        // Named against the key still in force, from a numeral that means this chord in the mode
        // it was taken from. Resolving and renaming through one value is what makes
        // `event.chord == event.numeral.chord_in(event.key)` true by construction rather than
        // true as long as three branches agree with each other.
        event.numeral = event
            .numeral
            .as_diatonic(source)
            .with_quality(chord.quality)
            .respelled_in(source, event.key);
        event.chord = chord;
    }
}

/// Turns the last chord before a key change into the dominant of the key being arrived at.
///
/// The oldest device in the book, and the reason a modulation can sound like an arrival rather than
/// an edit: a `V7` names its tonic before that tonic has sounded, so the ear is already in the new
/// key by the time the new section begins. Without one the piece steps sideways and the listener
/// hears the join.
///
/// # What is changed, and what is not
///
/// **One event, replaced in place.** Not lengthened, not inserted before: the section keeps its
/// bars, its clips keep their lengths, and everything downstream that counts bars goes on counting
/// the same ones. A chart of one chord per bar therefore gives the last bar to the dominant, which
/// is the amount an arranger would use; a busier chart gives it whatever its final chord had.
///
/// **The section keeps its own key.** The chord is resolved in the key being arrived at and then
/// *renamed* against the key still in force, exactly as a borrowed chord is — so the harmony lane
/// shows one key change, at the bar where it happens, with a chromatic chord leaning into it. A
/// key point half a bar early would be a second modulation nobody wrote.
///
/// It also rewrites a bar of a progression that may have been quoted by name, which nothing else
/// in this crate does. That is the trade, taken deliberately: the modulation was asked for by hand,
/// a structural instruction outranks a chord chart, and there is no way to prepare a key change
/// without changing the chord that prepares it. `lead_in = "none"` is how somebody says otherwise.
fn lead_into(events: &mut [HarmonicEvent], from: Key, to: Key) {
    if from == to {
        return;
    }
    let Some(last) = events.last_mut() else {
        return;
    };
    // A perfect fifth above the tonic being arrived at, built as a dominant seventh. Measured in
    // semitones rather than taken as the arriving key's fifth *degree*, because a degree is only
    // a fifth in a scale that has one: `V7` read in a Locrian key named a root a tritone above
    // the tonic, and in a major-pentatonic one a major sixth above it. Neither prepares anything.
    // What a dominant *is* does not vary with the mode of the key it belongs to.
    // The same construction `chord_in` uses for a secondary dominant, which is the same idea.
    let dominant = Chord::new(to.tonic.transposed(7), Quality::Dominant7);
    // Named against the key still in force, so the harmony lane shows one key change at the bar
    // where it happens with a chromatic chord leaning into it.
    let (degree, accidental) = degree_of(to, dominant.root);
    last.numeral = Numeral {
        accidental,
        ..Numeral::new(degree, false)
    }
    .with_quality(Quality::Dominant7)
    .respelled_in(to, from);
    last.chord = dominant;
}

/// Turns the last chord of a section into the key's own dominant, so the join is a cadence.
///
/// The turnaround, and the reachable half of what a cadence-aware composer means: a piece built
/// on a four-bar loop ran that loop straight across every section join, so nothing in the
/// harmony ever said "here" — the fill rose, the cymbal crashed, and the chords went round as if
/// no join existed. A dominant in the last bar makes the next section's tonic an arrival instead
/// of another lap.
///
/// The caller gates it three ways, and each is a promise. Only a [`ChartOrigin::Generated`]
/// chart — a progression the user quoted is played as written, the same trade [`colour`] makes,
/// and the reason 丸サ進行 never gains a bar it does not have. Only into a section at least as
/// strong — coming down out of a chorus is the one join a band lets pass unmarked, and
/// [`crate::parts`]' cymbal reads the same rule, so the harmony and the kit agree about which
/// joins are arrivals. And only where the next section opens on the tonic, asked of the resolved
/// chord rather than assumed: the composer's own chart quoted into a minor key opens on the
/// *relative* major, and a dominant prepared for a tonic that never comes is a question with the
/// wrong answer.
///
/// A final bar already on the tonic or the dominant is left alone — the first has its own kind
/// of close and the second needs no help.
fn turn_around(events: &mut [HarmonicEvent], key: Key) {
    let Some(last) = events.last_mut() else {
        return;
    };
    // The same construction `lead_into` uses, for the same reason: what a dominant is does not
    // vary with the mode of the key it belongs to.
    let dominant = Chord::new(key.tonic.transposed(7), Quality::Dominant7);
    if last.chord.root == dominant.root || last.chord.root == key.tonic {
        return;
    }
    let (degree, accidental) = degree_of(key, dominant.root);
    last.numeral = Numeral {
        accidental,
        ..Numeral::new(degree, false)
    }
    .with_quality(Quality::Dominant7);
    last.chord = dominant;
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
    use crate::theory::chord::Chord;
    use crate::theory::pitch::PitchClass;

    fn spec(text: &str) -> SongSpec {
        SongSpec::parse(text).expect("the fixture parses")
    }

    #[test]
    fn the_form_lays_the_sections_out_end_to_end() {
        let frame = plan(&spec(
            r#"
            form = "intro verse chorus"

            [section.intro]
            bars = 4
            [section.verse]
            bars = 8
            [section.chorus]
            bars = 8
            "#,
        ));
        assert_eq!(frame.sections.len(), 4, "three of the form, and the ending");
        assert_eq!(frame.sections[0].start, Ticks::ZERO);
        assert_eq!(frame.sections[1].start, frame.grid.bar_ticks() * 4);
        assert_eq!(frame.sections[2].start, frame.grid.bar_ticks() * 12);
        // The held final bar sits after the form and inside the piece's own length.
        assert_eq!(frame.sections[3].start, frame.grid.bar_ticks() * 20);
        assert!(frame.sections[3].coda);
        assert_eq!(frame.length, frame.grid.bar_ticks() * 21);
    }

    #[test]
    fn a_repeated_section_counts_its_instances() {
        let frame = plan(&spec(r#"form = "verse chorus verse chorus""#));
        let instances: Vec<usize> = frame.sections.iter().map(|s| s.instance).collect();
        assert_eq!(instances, [1, 1, 2, 2, 1], "and the ending is played once");
    }

    #[test]
    fn a_lead_in_is_a_fifth_above_the_tonic_it_arrives_at_in_every_mode() {
        // A dominant is a perfect fifth above the tonic and a major-minor seventh on top of it.
        // That is what the chord *is*, and it does not vary with the mode of the key it belongs
        // to — but it used to be read as the arriving key's fifth *degree*, and a degree is only
        // a fifth in a scale that has one. Locrian's fifth is a tritone and a major pentatonic's
        // is a major sixth, so the bar meant to prepare the change named a root that prepares
        // nothing.
        for scale in [
            "major",
            "minor",
            "dorian",
            "locrian",
            "major-pentatonic",
            "whole-tone",
        ] {
            let frame = plan(&spec(&format!(
                r#"
                form = "verse chorus"
                key = "C {scale}"
                [section.verse]
                bars = 2
                [section.chorus]
                bars = 2
                transpose = 2
                "#
            )));
            let arriving = frame.sections[1].key;
            let leading = frame.sections[0]
                .events
                .last()
                .expect("the verse has chords")
                .chord;
            assert_eq!(
                arriving.tonic.distance_up_to(leading.root),
                7,
                "in C {scale} the lead-in was not a fifth above the arriving tonic"
            );
            assert_eq!(leading.quality, Quality::Dominant7, "in C {scale}");
        }
    }

    #[test]
    fn a_quoted_chart_is_never_turned_around() {
        // The turnaround rewrites a bar of the composer's own chart, and only its own: a quoted
        // progression is played as written even into an arrival, which is the same trade the
        // colouring makes and the reason for naming one.
        let frame = plan(&spec(
            r#"
            chords = "@axis"
            form = "verse chorus"
            "#,
        ));
        let last = frame.sections[0].events.last().expect("chords").chord;
        let unchanged = plan(&spec(
            r#"
            chords = "@axis"
            form = "verse"
            "#,
        ));
        assert_eq!(
            last,
            unchanged.sections[0].events.last().expect("chords").chord
        );
    }

    #[test]
    fn the_composers_own_chart_turns_around_into_an_arrival() {
        // No progression chosen is how the composer gets to invent, and what it invented used to
        // run its loop straight across every join: the fill rose, the cymbal crashed, and the
        // harmony went round as if no join existed. Into an arrival the last bar is the key's own
        // dominant, so the next section's tonic arrives instead of coming round again.
        let frame = plan(&spec(r#"form = "verse chorus""#));
        let verse = &frame.sections[0];
        let last = verse.events.last().expect("the verse has chords");
        assert_eq!(
            last.chord,
            Chord::new(PitchClass::parse("G").unwrap(), Quality::Dominant7)
        );
        // The lane and the parts still agree, by construction.
        assert_eq!(last.chord, last.numeral.chord_in(last.key));
        // The melody hangs on the dominant rather than on the chord it replaced — the same
        // ordering promise the lead-in makes, for the same reason.
        let pitch = *verse.skeleton.last().expect("one pitch per chord");
        assert!(
            last.chord.contains_midi(pitch),
            "the tune ends on {pitch}, which is not in {}",
            last.chord
        );
        // With the held ending switched off there is nothing to arrive at, and the loop plays
        // out as written — a turnaround into nothing would be a question nobody answers.
        let alone = plan(&spec(
            r#"
            form = "verse"
            ending = "none"
            "#,
        ));
        assert_ne!(
            alone.sections[0].events.last().expect("chords").chord,
            last.chord,
            "a piece with no ending was turned around into nothing"
        );
    }

    #[test]
    fn a_piece_ends_on_one_held_bar_of_its_tonic() {
        // The last section used to play its loop out and stop mid-groove, as if the tape ran
        // out. The ending is one bar: the final key's tonic, a labelled section the writers
        // treat as a landing rather than as one more bar of the piece.
        let frame = plan(&spec(r#"form = "verse""#));
        assert_eq!(frame.sections.len(), 2);
        let ending = frame.sections.last().unwrap();
        assert!(ending.coda);
        assert_eq!(ending.name, "ending");
        assert_eq!(ending.bars, 1);
        assert_eq!(ending.start + ending.length, frame.length);
        let event = &ending.events[0];
        assert_eq!(event.chord.root, ending.key.tonic);
        assert_eq!(
            event.chord,
            event.numeral.chord_in(event.key),
            "the lane and the parts agree, by construction"
        );
        assert!(
            event.chord.contains_midi(ending.skeleton[0]),
            "the melody's landing is not on the chord"
        );

        // In a minor key the tonic is the minor triad — the ending closes the piece in its own
        // mode rather than picardying it.
        let minor = plan(&spec(
            r#"
            key = "A minor"
            form = "verse"
            "#,
        ));
        let event = &minor.sections.last().unwrap().events[0];
        assert_eq!(event.chord.quality, Quality::Minor);

        // And `ending = "none"` is the loop played out, which is what an exported loop wants.
        let plain = plan(&spec(
            r#"
            form = "verse"
            ending = "none"
            "#,
        ));
        assert_eq!(plain.sections.len(), 1);
    }

    #[test]
    fn coming_down_out_of_a_chorus_is_not_turned_around() {
        // The one join a band lets pass unmarked, and the same rule the cymbal reads: marking a
        // drop with a dominant says the opposite of what the arrangement is doing.
        let frame = plan(&spec(r#"form = "chorus verse""#));
        let last = frame.sections[0].events.last().expect("chords");
        assert_ne!(
            last.chord.quality,
            Quality::Dominant7,
            "a chorus falling into a verse gained a turnaround"
        );
    }

    #[test]
    fn a_section_repeats_its_chart_to_fill_its_bars() {
        let frame = plan(&spec(
            r#"
            form = "verse"
            chords = "@axis"

            [section.verse]
            bars = 8
            "#,
        ));
        let verse = &frame.sections[0];
        assert_eq!(verse.events.len(), 8, "a four-bar loop played twice");
        assert_eq!(verse.events[0].chord, verse.events[4].chord);
        assert_eq!(verse.events.last().unwrap().end(), verse.length);
    }

    #[test]
    fn a_transposed_section_moves_its_chords_and_nothing_else() {
        let frame = plan(&spec(
            r#"
            key = "C major"
            form = "chorus"

            [section.chorus]
            transpose = 2
            "#,
        ));
        let chorus = &frame.sections[0];
        assert_eq!(chorus.key.tonic, PitchClass::parse("D").unwrap());
        assert_eq!(chorus.events[0].chord.root, PitchClass::parse("D").unwrap());
    }

    /// A form of two sections, the second transposed, at whatever lead-in is asked for.
    fn modulating(lead_in: &str) -> Frame {
        plan(&spec(&format!(
            r#"
            key    = "C major"
            chords = "@axis"
            form   = "verse chorus"

            [section.verse]
            bars = 4
            [section.chorus]
            bars      = 4
            transpose = 2
            {lead_in}
            "#
        )))
    }

    #[test]
    fn the_chord_before_a_key_change_is_the_dominant_of_the_key_arrived_at() {
        // The oldest device there is: a `V7` names its tonic before the tonic has sounded, so the
        // ear is already in the new key when the section starts. Without it the piece steps
        // sideways and a listener hears the join as an edit.
        let frame = modulating("");
        let verse = &frame.sections[0];
        let chorus = &frame.sections[1];
        assert_eq!(chorus.key.tonic, PitchClass::parse("D").unwrap());

        let last = verse.events.last().expect("the verse has chords");
        assert_eq!(
            last.chord,
            Chord::new(PitchClass::parse("A").unwrap(), Quality::Dominant7)
        );
        // Named from the key still in force, not from the one being arrived at — the section has
        // one key and the lane draws one change, at the bar where it happens.
        assert_eq!(last.key, verse.key);
        assert_eq!(
            last.chord,
            last.numeral.chord_in(last.key),
            "the lane would draw {} over parts playing {}",
            last.name(),
            last.chord
        );
    }

    #[test]
    fn only_the_last_chord_moves_and_only_where_the_key_does() {
        // The scope of the trade. One event, replaced in place: the bars are the bars they were,
        // and everything before the join is the chart as written.
        let plain = modulating("lead_in = \"none\"");
        let prepared = modulating("");
        let (plain, prepared) = (&plain.sections[0], &prepared.sections[0]);

        assert_eq!(plain.events.len(), prepared.events.len());
        let moved: Vec<usize> = plain
            .events
            .iter()
            .zip(&prepared.events)
            .enumerate()
            .filter(|(_, (a, b))| a.chord != b.chord)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(moved, [plain.events.len() - 1]);
        // And the chorus itself is untouched either way: a lead-in is written into the bars
        // *before* the change.
        assert_eq!(
            modulating("lead_in = \"none\"").sections[1].events[0].chord,
            modulating("").sections[1].events[0].chord
        );
    }

    #[test]
    fn a_form_that_does_not_modulate_is_never_led_into() {
        // The field says how a key change is arrived at, so a piece with no key change has
        // nothing for it to do — whatever it is set to.
        for lead_in in ["", "lead_in = \"dominant\""] {
            let frame = plan(&spec(&format!(
                r#"
                chords = "@axis"
                form   = "verse chorus"

                [section.verse]
                bars = 4
                [section.chorus]
                bars = 4
                {lead_in}
                "#
            )));
            let verse = &frame.sections[0];
            let last = verse.events.last().unwrap();
            assert_eq!(
                last.chord,
                verse.events[verse.events.len() - 1]
                    .numeral
                    .chord_in(verse.key)
            );
            assert_eq!(
                last.key, frame.sections[1].key,
                "the fixture does not modulate"
            );
            assert_eq!(last.chord.quality, Quality::Major, "@axis ends on F");
        }
    }

    #[test]
    fn the_melody_hangs_on_the_chord_the_lead_in_left_behind() {
        // The skeleton is chosen before any part exists and every pitched writer reads it, so a
        // lead-in applied after it would leave the tune on the chord that used to be there —
        // the one part in the band not in on the modulation.
        let frame = modulating("");
        let verse = &frame.sections[0];
        let last = verse.events.last().unwrap();
        let pitch = *verse.skeleton.last().expect("one pitch per chord");
        assert!(
            last.chord.contains_midi(pitch),
            "the tune ends on {pitch}, which is not in {}",
            last.chord
        );
    }

    #[test]
    fn a_written_progression_is_never_recoloured() {
        // Whatever the tension, a quoted chart comes out as it was written — which is the whole
        // reason for naming one.
        for tension in ["0.0", "0.5", "1.0"] {
            let frame = plan(&spec(&format!(
                r#"
                    form = "verse"
                    chords = "@marusa"
                    tension = {tension}
                    "#
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
    fn colouring_carries_the_numeral_along_with_the_chord() {
        // An event holds both, and different halves of the program read them: every part plays
        // the chord, while the numeral is what the document stores, what the harmony lane names
        // and what a Chords clip is written from. Colouring one and leaving the other saying what
        // the chord used to be is a lane painting Fm over parts playing F#m, and it saves that
        // way — so the two naming the same chord is the property, not any particular chord.
        let song = |mood: &str, extra: &str| {
            format!(
                r#"
                form = "verse"
                mood = "{mood}"
                {extra}

                [section.verse]
                bars = 8
                "#
            )
        };
        // Nothing is coloured at a tension of zero, so this is the chart as written, and the
        // count below is how many chords the mood really moved.
        let plain: Vec<String> = plan(&spec(&song("neutral", "tension = 0.0"))).sections[0]
            .events
            .iter()
            .map(|event| event.chord.to_string())
            .collect();

        for mood in ["neutral", "tense"] {
            let frame = plan(&spec(&song(mood, "")));
            let section = &frame.sections[0];
            for event in &section.events {
                assert_eq!(
                    event.chord,
                    event.numeral.chord_in(event.key),
                    "{mood}: the lane writes {} where the parts play {}",
                    event.name(),
                    event.chord
                );
            }
            let moved = section
                .events
                .iter()
                .zip(&plain)
                .filter(|(event, before)| event.chord.to_string() != **before)
                .count();
            assert!(moved > 0, "{mood} coloured nothing, so this proves nothing");
        }
    }

    #[test]
    fn the_skeleton_gives_one_pitch_per_chord_inside_the_melody_range() {
        let frame = plan(&spec(
            r#"
            form = "verse"
            chords = "@axis"
            "#,
        ));
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
            r#"
                form = "verse"
                chords = "@canon"
                [section.verse]
                bars = 8
                "#,
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
        let frame = plan(&spec(
            r#"
            form = "verse"
            chords = "@axis"
            "#,
        ));
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
            format!(
                r#"
                form    = "verse"
                seed    = {seed}
                tension = 0.9
                "#
            )
        };
        let a = plan(&spec(&seeded(1)));
        let b = plan(&spec(&seeded(1)));
        assert_eq!(a.sections[0].skeleton, b.sections[0].skeleton);

        let c = plan(&spec(&seeded(2)));
        // Not a guarantee for every seed pair, but these two do differ, and the test pins that
        // the seed is actually reaching the plan.
        assert_ne!(a.sections[0].skeleton, c.sections[0].skeleton);
    }

    #[test]
    fn the_chord_at_a_tick_is_the_one_sounding_there() {
        let frame = plan(&spec(
            r#"
            form   = "verse"
            chords = "@axis"
            "#,
        ));
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
