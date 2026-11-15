//! The bass line.
//!
//! Locked to the kick *pattern* rather than to the kick *part*: reading the groove keeps the two
//! together without making one part depend on another's notes, which is the rule the whole module
//! is built on. That is also why the arithmetic that reads a groove on the drums' grid rather than
//! the bass's own lives here and not with the kit — it is the bass following, not the drummer
//! leading.

use auris_core::time::Ticks;

use crate::PerformanceStyle;
use crate::frame::{Frame, SectionPlan};
use crate::rhythm::DrumVoice;
use crate::rng::Rng;
use crate::spec::PartSpec;
use crate::theory::chord::Chord;
use crate::theory::chord_scale::ChordScale;
use crate::theory::pitch::{OCTAVE, PitchClass, fold_into};

use super::writer::{
    band_pushes, bar_stream, density, lift_before, part_grid, phrase_shape, velocity,
};
use super::{Draft, ScoreSettings};

/// The octave a bass leaps to from `root`, staying inside `low..=high`.
///
/// Up where there is room and down where there is not, which is what a player does: the octave
/// above a high root is off the end of the instrument and the one below is right there.
///
/// It used to fold `root + OCTAVE` back into the range, and the range is exactly two octaves wide
/// with the roster's roots sitting in the upper one — so for every root from F upward the octave
/// above fell outside, `fold_into` subtracted it straight back, and the answer was the root
/// itself. The figure silently stopped being an octave for four of the seven diatonic degrees,
/// the subdominant and the dominant among them: the bass restruck the note it was already on
/// while the recipe said it was leaping.
fn octave_leap(root: i32, low: i32, high: i32) -> i32 {
    if root + OCTAVE <= high {
        root + OCTAVE
    } else if root - OCTAVE >= low {
        root - OCTAVE
    } else {
        // A range narrower than an octave holds no leap at all. Restriking the root is the honest
        // answer, and the only one left.
        root
    }
}

/// Voices a chord tone above or below its bass anchor when that octave fits the instrument.
fn voice_on_side(pitch: i32, root: i32, above: bool, low: i32, high: i32) -> i32 {
    let preferred = |pitch| if above { pitch > root } else { pitch < root };
    let step = if above { OCTAVE } else { -OCTAVE };
    let mut candidate = pitch;
    while !preferred(candidate) {
        candidate += step;
        if !(low..=high).contains(&candidate) {
            return pitch;
        }
    }
    candidate
}

/// The shape of a bass line through a bar.
///
/// Same reason as [`CompFigure`](super::comp::CompFigure): the bass followed the kick and
/// alternated root and fifth, which is one bass line and not a choice of them. And the same
/// lifetime: one is drawn for a section and kept.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum BassFigure {
    /// The root, and nothing else. Solid, and what most of rock does.
    Root,
    /// The root, with the fifth on the one hit where the chord turns: the oldest bass line
    /// there is. See `answering_hit`.
    Fifth,
    /// The root, jumping an octave on the weak hits.
    Octave,
    /// Root and fifth, stepping into the next chord on the last hit before it.
    Approach,
    /// A quarter-note line that reads the chord upward and steps into the next one: the walking
    /// bass. The one figure that does not follow the kick — the walk *is* the timekeeping, which
    /// is the whole reason a trio can play without one.
    Walk,
    /// A repeated root carrying the pulse, rather than a held root or an octave walk.
    Pulse,
}

/// A section keeps its bass vocabulary while the chord changes underneath it. Density sets how
/// active the line is; style changes the relative likelihood of coherent ways to play it.
fn pick_figure(rng: &mut Rng, busy: f32, style: Option<PerformanceStyle>) -> BassFigure {
    const FIGURES: [BassFigure; 6] = [
        BassFigure::Root,
        BassFigure::Fifth,
        BassFigure::Approach,
        BassFigure::Octave,
        BassFigure::Walk,
        BassFigure::Pulse,
    ];
    let mut weights = [
        // Weighted where the comp weights its held chord, and for the comp's reason: a figure
        // lasts a whole section now, so drawing this one is whole notes for the whole of it.
        // At the neutral dial it was the likeliest line a bass could draw, and in a pop and
        // rock score corpus two sixteen-bar stretches in a hundred are played that way.
        0.1 + (1.0 - busy) * 0.8,
        1.0,
        0.2 + busy,
        0.2 + busy * 1.6,
        0.1 + busy * 1.3,
        0.2 + busy,
    ];
    let vocabulary = match style {
        Some(PerformanceStyle::JazzTrio) => [0.4, 0.6, 1.2, 0.3, 6.0, 0.3],
        Some(PerformanceStyle::CityPop) => [0.3, 0.8, 2.8, 1.6, 0.5, 0.7],
        Some(PerformanceStyle::Rock) => [0.5, 1.0, 0.8, 0.6, 0.15, 4.0],
        Some(PerformanceStyle::Ambient) => [6.0, 0.6, 0.5, 0.2, 0.15, 0.3],
        Some(PerformanceStyle::Orchestral) => [3.0, 1.2, 1.0, 0.5, 0.3, 0.8],
        Some(PerformanceStyle::Synthwave | PerformanceStyle::Chiptune) => {
            [0.3, 0.8, 0.6, 2.0, 0.15, 3.0]
        }
        Some(PerformanceStyle::PopBand) => [0.7, 1.2, 1.8, 0.7, 0.3, 1.6],
        // With no idiom named, the figures are weighted by what the bass lines of a pop and
        // rock score corpus play, where every one but the root-and-fifth pair used to be as
        // likely as its dial made it. The walk is an idiom of its own: quarter notes through
        // the third over a kit playing a backbeat is a jazz player sitting in, and a take
        // nobody asked for. The held root was one take in seven, and two of the corpus's
        // intervals in a hundred are a bar long. The octave figure was one take in four and
        // put an octave leap into 15 intervals in a hundred, where the corpus leaps one in
        // five. And the pulse is the line those scores play most: 46 of their intervals in a
        // hundred are an eighth apart and 42 are the same note again, where these takes had
        // 22 and 28.
        None => [0.5, 1.0, 1.0, 0.4, 0.2, 2.5],
    };
    for (weight, preference) in weights.iter_mut().zip(vocabulary) {
        *weight *= preference;
    }
    FIGURES[rng.weighted(&weights)]
}

/// The pitches of a walking line: `count` beats over one chord, from its root toward the next.
///
/// A walking bass is a line rather than a series of answers, so it is built whole: beat one
/// states the root, the middle beats read the chord — third then fifth heading up to a chord
/// above, fifth then third heading down to one below, round again where there are beats left —
/// each placed in the octave nearest the note before it, and the last beat steps to one scale
/// note beside the next chord's root. What comes out is the root–third–fifth–approach of every
/// method book one way, and G–F–D–B into C the other, which is the same idea upside down.
///
/// With nowhere to go — the last chord of a piece — the last beat reads the chord like the
/// middle ones, because an approach into nothing is a question nobody answers.
fn walk_line(
    chord: &Chord,
    scale: &ChordScale,
    root: i32,
    target: Option<i32>,
    count: usize,
    low: i32,
    high: i32,
) -> Vec<i32> {
    let classes = chord.classes();
    let direction = target.map_or(1, |next| if next >= root { 1 } else { -1 });
    let mut line = Vec::with_capacity(count);
    let mut previous = root;
    for position in 0..count {
        let pitch = if position == 0 {
            root
        } else if position + 1 == count
            && let Some(next) = target
        {
            approach(scale, previous, next, low, high)
        } else {
            let others = classes.len().saturating_sub(1).max(1);
            let step = (position - 1) % others;
            // Read toward where the line is going: third then fifth on the way up is fifth
            // then third on the way down.
            let index = if direction >= 0 {
                1 + step
            } else {
                classes.len().saturating_sub(1 + step)
            };
            let class = classes.get(index).copied().unwrap_or(chord.root);
            climbing(class, previous, direction, low, high)
        };
        previous = pitch;
        line.push(pitch);
    }
    line
}

/// The octave of `class` nearest `previous`, inside the range. Ties break toward `direction`,
/// which is the way the walking line is heading.
fn climbing(class: PitchClass, previous: i32, direction: i32, low: i32, high: i32) -> i32 {
    let mut best = fold_into(class.midi(2), low, high);
    let mut nearest = i32::MAX;
    for octave in 0..9 {
        let pitch = class.midi(octave);
        if pitch < low || pitch > high {
            continue;
        }
        // Twice the distance, plus one against the line's direction: -d loses the tie to +d
        // heading up, and wins it heading down, and nothing else changes.
        let distance =
            (pitch - previous).abs() * 2 + i32::from((pitch - previous).signum() != direction);
        if distance < nearest {
            nearest = distance;
            best = pitch;
        }
    }
    best
}

/// One scale note beside `target`, for the last beat of a walking line.
///
/// A walking line does not land on the next chord's root — the next chord does that — it stops
/// one step beside it so that the change answers the line. The semitone wins where the scale
/// offers one, which is what puts a leading note under an arriving tonic; at the same distance
/// the side the line comes from wins; and the note the line is already standing on is taken only
/// when nothing else fits, because an approach that does not move is not approaching.
fn approach(scale: &ChordScale, previous: i32, target: i32, low: i32, high: i32) -> i32 {
    let side = if previous > target { 1 } else { -1 };
    let mut candidates = Vec::with_capacity(4);
    for distance in 1..=2 {
        for step in [side, -side] {
            candidates.push(target + step * distance);
        }
    }
    let mut fitting: Vec<i32> = candidates
        .into_iter()
        .filter(|pitch| (low..=high).contains(pitch) && scale.contains(PitchClass::new(*pitch)))
        .collect();
    if fitting.len() > 1 {
        fitting.retain(|pitch| *pitch != previous);
    }
    fitting
        .into_iter()
        .min_by_key(|pitch| (pitch - target).abs())
        .unwrap_or_else(|| fold_into(target, low, high))
}

/// Which hit of a chord's figure leaves the root for the fifth: the first one, after the
/// chord's own start, in the second half of the chord.
///
/// A root-and-fifth line is mostly root. It used to give the fifth to every hit off a beat,
/// and a kick is mostly off the beat: over basic-rock's kick the line was root, fifth, fifth in
/// every bar — a bass standing on the fifth and visiting the root. One hit answers, where the
/// bar turns, and the rest hold the chord down: root, root, fifth over that kick, root and
/// fifth on one and three over an eight-beat's.
fn answering_hit(onsets: &[usize], steps: usize) -> Option<usize> {
    onsets
        .iter()
        .position(|offset| *offset > 0 && *offset * 2 >= steps)
}

/// How many bars a bass line's pickup comes round in.
///
/// Two, like the kit's cell: a bar that states the figure and a bar that says it again and
/// steps into what follows. With one figure for the section and nothing else, a bass wrote the
/// same bar sixteen times — the bar before it 99 times in a hundred, where the corpus lines
/// repeat the bar before them 39 times and the bar *two* before them 52.
const PICKUP_EVERY: usize = 2;

/// How many takes pick up. The rest play the figure plain, which is a line too.
const PICKUP: f32 = 0.6;

/// The dial setting from which a pulse runs eighths rather than quarters, whatever the idiom.
///
/// Just under the middle, so a bass nobody has touched plays the eighths a pop bass plays. Rock
/// and the sequenced idioms run them at any setting.
const PULSE_EIGHTHS_FROM: f32 = 0.45;

/// The dial setting from which the run into a new phrase is two sixteenths, not two eighths.
const QUICK_RUN_FROM: f32 = 0.66;

/// The scale note under `pitch`, a step away: what a line stands on before it steps up to it.
fn step_below(scale: &ChordScale, pitch: i32) -> Option<i32> {
    (1..=3)
        .map(|step| pitch - step)
        .find(|below| scale.contains(PitchClass::new(*below)))
}

/// The bass line.
///
/// Locked to the kick pattern rather than to the kick *part*: reading the groove keeps the two
/// together without making one part depend on another's notes.
pub(super) fn bass(
    settings: &ScoreSettings,
    frame: &Frame,
    section: &SectionPlan,
    index: usize,
    part: &PartSpec,
) -> Vec<Draft> {
    let (low, high) = part.range();
    let grid = part_grid(frame, part);
    let kick = crate::frame::groove_pattern(&settings.groove, DrumVoice::Kick);
    // Asked in ticks and answered on the *drums'* grid, not the bass's. A groove is read by index,
    // so a bass dividing its beats any other way would have wrapped the pattern partway through
    // the bar and followed a kick nobody was playing.
    //
    // Mapped onto the bar the same way the drummer maps it, too. Reading the raw index instead
    // wrapped a groove shorter than the bar and truncated one longer, so in every meter the
    // groove was not written for the bass followed a kick the kit was not striking.
    let drums = frame.grid;
    let drum_bar = drums.bar_ticks().raw().max(1);
    let own = crate::frame::groove_steps_per_beat(&settings.groove);
    let kick_at = |at: Ticks| {
        let step = drums.step_of(Ticks(at.raw().rem_euclid(drum_bar)));
        kick.at_in_bar(step, drums.steps_per_bar(), drums.steps_per_beat(), own)
            .is_some()
    };
    let mut notes: Vec<Draft> = Vec::new();

    // How the part plays, drawn once for the section and kept over every chord in it.
    //
    // Per phrase was wrong, and wrong the way the comp's per-bar draw was: a player picks a line
    // and keeps it. Drawn again every four bars, not one sixteen-bar take in 432 kept one
    // figure, and four in five put four bars of whole notes beside four bars of something
    // busy — the busiest phrase of a take carried four times the notes of its sparsest, where
    // the corpus lines carry not quite twice. The comp was already drawing once a section, so
    // under one stretch of chords the keyboard held its figure and the bass changed players.
    let busy = density(settings, part, section);
    let mut choose = bar_stream(frame, part, section, "figure", 0);
    let section_figure = pick_figure(&mut choose, busy, settings.style);
    // Whether this player lets go before a syncopated hit or holds into it: a property of the
    // take, like the figure. See `lift_before`.
    let lifting = choose.chance(0.5);
    // Without a chosen idiom, a take can voice its fifth above or below the root, and keeps
    // that relationship throughout. Styled parts retain the register their vocabulary
    // establishes.
    let fifth_above = bar_stream(frame, part, section, "voicing", 0).chance(0.5);
    // Whether this take's second bar answers its first. See `PICKUP_EVERY`.
    let picks_up = bar_stream(frame, part, section, "pickup", 0).chance(PICKUP);
    // The dial as it was set, for the two things it switches rather than scales.
    let dial = part.density.unwrap_or_else(|| settings.mood.density());
    // Whether the band pushes this stretch's chord changes, and whether the chord in hand was
    // pushed into by the one before it. A rhythm somebody wrote says where its notes are.
    let pushing = part.rhythm.is_none() && band_pushes(section, settings.mood.syncopation);
    let mut pushed_in = false;

    for (position_in_section, event) in section.events.iter().enumerate() {
        let root = fold_into(event.chord.bass_class().midi(part.octave), low, high);
        // The chord's own fifth, read off the chord rather than assumed perfect and measured
        // from the chord's root rather than from a slash bass. A blind `root + 7` played F# over
        // a B diminished and a C over a G/F — notes in neither the chord nor the key.
        let fifth_class = event
            .chord
            .classes()
            .get(2)
            .copied()
            .unwrap_or(event.chord.root);
        let fifth = fold_into(fifth_class.midi(part.octave), low, high);

        let steps = grid.step_of(event.length).max(1);
        // The groove is written as one bar, so it is read modulo the bar rather than modulo its
        // own length — otherwise a meter that is not sixteen steps drifts against the drums.
        let per_bar = grid.steps_per_bar().max(1);
        let first = grid.step_of(event.start) % per_bar;
        let bar = grid.step_of(event.start) / grid.steps_per_bar().max(1);
        let fifth = if settings.style.is_none() {
            voice_on_side(fifth, root, fifth_above, low, high)
        } else {
            fifth
        };
        // Only the closing bar of a phrase may turn the established line into an approach,
        // and each closing bar asks for itself.
        let mut figure = section_figure;
        if section.closes_phrase(bar)
            && matches!(figure, BassFigure::Root | BassFigure::Fifth)
            && bar_stream(frame, part, section, "turn", bar).chance(0.35)
        {
            figure = BassFigure::Approach;
        }

        // The figure decides how busy the line is as well as what it plays. Two lines that hit
        // the same beats and differ only on the weak ones are the same line to a listener.
        // A rhythm the user wrote replaces the figure's rhythm outright — the figure still
        // chooses the notes, which is the half of the job the field never claimed.
        let mut onsets: Vec<usize> = match part.rhythm.as_ref() {
            Some(pattern) => (0..steps)
                .filter(|offset| pattern.at((first + offset) % per_bar).is_some())
                .collect(),
            None => match figure {
                // One note under the chord, held: the sound of a bass player staying out of
                // the way.
                BassFigure::Root => Vec::new(),
                // Follow the kick, which is what locks a rhythm section together.
                BassFigure::Fifth | BassFigure::Approach => (0..steps)
                    .filter(|offset| kick_at(event.start + grid.tick_of(*offset)))
                    .collect(),
                // The kick, and the half-beats between it: a busier, walking feel.
                BassFigure::Octave => (0..steps)
                    .filter(|offset| {
                        kick_at(event.start + grid.tick_of(*offset))
                            || ((first + offset) % per_bar)
                                .is_multiple_of(grid.steps_per_beat().max(1))
                    })
                    .collect(),
                // Every beat and nothing else. The walk does not follow the kick because the
                // walk is the timekeeping: quarter notes whatever the groove is doing, which is
                // what the figure *is*.
                BassFigure::Walk => (0..steps)
                    .filter(|offset| {
                        ((first + offset) % per_bar).is_multiple_of(grid.steps_per_beat().max(1))
                    })
                    .collect(),
                BassFigure::Pulse => {
                    let division = if dial >= PULSE_EIGHTHS_FROM
                        || matches!(
                            settings.style,
                            Some(
                                PerformanceStyle::Rock
                                    | PerformanceStyle::Synthwave
                                    | PerformanceStyle::Chiptune
                            )
                        ) {
                        (grid.steps_per_beat() / 2).max(1)
                    } else {
                        grid.steps_per_beat().max(1)
                    };
                    (0..steps)
                        .filter(|offset| {
                            kick_at(event.start + grid.tick_of(*offset))
                                || (first + offset).is_multiple_of(division)
                        })
                        .collect()
                }
            },
        };
        // Always sound the chord's start, so a change of chord is heard whatever the figure —
        // except under a written rhythm, whose rests are part of the instruction.
        // And except where the chord before pushed into this one: the change has been heard,
        // half a beat ago, and striking it again on the line is a stutter.
        if pushed_in {
            onsets.retain(|offset| *offset != 0);
        } else if part.rhythm.is_none() && !onsets.contains(&0) {
            onsets.insert(0, 0);
        }
        // The pickup: in every second bar the last half-beat steps into the bar after it. A
        // held root and a walk are left alone — the one is a player keeping out of the way and
        // the other approaches every chord already.
        //
        // Where a phrase closes the pickup is a run: two notes up the scale into the bar
        // after it, eighths, or sixteenths once the dial is past two thirds. A take that
        // picked up the same way every second bar wrote a bar and a half of different rhythms
        // in sixteen, where the corpus lines write five; the end of a phrase is where a bass
        // player says something else.
        let half = (grid.steps_per_beat() / 2).max(1);
        let closing = section.closes_phrase(bar);
        // The push: the next chord's root, half a beat before its line and held over it. The
        // comp strikes the same change at the same moment — see `band_pushes` — and a bass
        // that waited for the line would be restriking the old root under the new chord. A
        // walk does not push: it is on every beat by definition.
        let pushes_out = pushing
            && figure != BassFigure::Walk
            && steps > half
            && section
                .events
                .get(position_in_section + 1)
                .is_some_and(|next| next.start == event.end())
            && !section.continues_chord(position_in_section + 1);
        let picking_up = picks_up
            && part.rhythm.is_none()
            && (bar % PICKUP_EVERY == PICKUP_EVERY - 1 || closing)
            // A push is already the way into the next bar, and takes the same half-beat.
            && !pushes_out
            && !matches!(figure, BassFigure::Root | BassFigure::Walk)
            && first + steps == per_bar
            && steps > half;
        let stride = if dial >= QUICK_RUN_FROM {
            (half / 2).max(1)
        } else {
            half
        };
        let running_up = picking_up && closing && steps > 2 * stride;
        if running_up {
            // The run owns the end of the bar: nothing of the figure is struck inside it.
            onsets.retain(|offset| *offset < steps - 2 * stride);
            onsets.extend([steps - 2 * stride, steps - stride]);
        } else if picking_up && !onsets.contains(&(steps - half)) {
            onsets.push(steps - half);
            onsets.sort_unstable();
        }
        if pushes_out {
            onsets.retain(|offset| *offset < steps - half);
            onsets.push(steps - half);
        }
        // Stepping into whatever comes next, on the last hit before it: what the approach
        // figure does under every chord, and the pickup in its own bar.
        let approaching = figure == BassFigure::Approach || picking_up;

        // Where the next chord's root is, for a line that steps into it.
        let target = section
            .events
            .get(position_in_section + 1)
            .map(|next| fold_into(next.chord.bass_class().midi(part.octave), low, high));

        // The whole walk at once, because a walking bass is a line rather than a series of
        // answers: each beat is chosen by where the last one was and where the next chord is,
        // and neither is a question one onset can answer for itself.
        let walk = (figure == BassFigure::Walk).then(|| {
            let scale = ChordScale::new(event.key, event.chord);
            walk_line(&event.chord, &scale, root, target, onsets.len(), low, high)
        });

        let last = onsets.len().saturating_sub(1);
        // The note that pushed into this chord is held to the first thing the chord plays for
        // itself, or through all of it.
        if pushed_in && let Some(held) = notes.last_mut() {
            let until = onsets
                .first()
                .map_or(event.length, |offset| grid.tick_of(*offset));
            held.length = (section.start + event.start + until - held.start).max(Ticks(1));
        }
        let answering = answering_hit(&onsets, steps);
        for (position, offset) in onsets.iter().enumerate() {
            let at = event.start + grid.tick_of(*offset);
            let next = onsets
                .get(position + 1)
                .map(|next| grid.tick_of(*next))
                .unwrap_or(event.length);
            let mut length = (next - grid.tick_of(*offset)).max(grid.step_ticks());
            // A rhythm somebody wrote keeps its notes as long as it spelled them: where it
            // wanted a rest it wrote one.
            if lifting
                && part.rhythm.is_none()
                && let Some(next) = onsets.get(position + 1)
                && let Some(boundary) = lift_before(grid, first + offset, first + next)
            {
                length = grid.tick_of(boundary - (first + offset));
            }
            let strong = position == 0 || grid.weight(grid.step_of(at)) >= 2;
            let answers = answering == Some(position);
            let pitch = match figure {
                // Stepping into whatever comes next, on the last hit before it. A bass player
                // reaching for the next chord is the sound of a line going somewhere, and it is
                // the one thing here that needs to know what the next chord is.
                //
                // From inside the key, not a semitone below. A chromatic approach is what a jazz
                // player would reach for, but every other note this crate writes belongs to the
                // chord or to the key, and one part quietly breaking that would be a wrong note
                // to anybody reading the piano roll. Inside the scale the chord implies, so an
                // approach into a dominant does not walk through the degree it just altered.
                _ if pushes_out && position == last => target.unwrap_or(root),
                _ if approaching && position == last && last > 0 => target
                    .and_then(|next| {
                        let scale = ChordScale::new(event.key, event.chord);
                        step_below(&scale, next).map(|pitch| fold_into(pitch, low, high))
                    })
                    .unwrap_or(fifth),
                // The note before it in a run: one more step down the same scale.
                _ if running_up && position + 1 == last && last > 1 => target
                    .and_then(|next| {
                        let scale = ChordScale::new(event.key, event.chord);
                        let under = step_below(&scale, next)?;
                        step_below(&scale, under).map(|pitch| fold_into(pitch, low, high))
                    })
                    .unwrap_or(root),
                BassFigure::Root | BassFigure::Pulse => root,
                BassFigure::Fifth | BassFigure::Approach => {
                    if answers {
                        fifth
                    } else {
                        root
                    }
                }
                BassFigure::Octave => {
                    if strong {
                        root
                    } else {
                        octave_leap(root, low, high)
                    }
                }
                BassFigure::Walk => walk
                    .as_ref()
                    .and_then(|line| line.get(position))
                    .copied()
                    .unwrap_or(root),
            };
            notes.push(Draft {
                section: index,
                pitch: pitch.clamp(0, 127) as u8,
                velocity: (velocity(
                    grid.weight(grid.step_of(at)),
                    section.intensity,
                    settings.dynamics,
                ) * phrase_shape(grid, section, at, settings.dynamics))
                .clamp(0.05, 1.0),
                start: section.start + at,
                // Floored at a tick, not at a step. A step is often *longer* than the room left
                // in the chord — nine chords in a four-four bar leave 426 ticks and an eighth-note
                // grid asks for 480 — and flooring there put the note straight back over the
                // boundary the `min` had just brought it inside, so two chords' bass notes sounded
                // on top of each other. Every other writer in this crate floors at `Ticks(1)`,
                // which can never exceed a `min` that is itself a real length.
                length: length.min(event.end() - at).max(Ticks(1)),
            });
        }
        pushed_in = pushes_out;
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{Song, part};
    use crate::spec::{PartSpec, Role};

    #[test]
    fn fifth_voicings_keep_the_chord_tone_on_the_selected_side_when_it_fits() {
        let (low, high) = crate::spec::Role::Bass.range();
        for root in low..=high {
            for interval in [6, 7, 8] {
                let fifth = fold_into(root + interval, low, high);
                for above in [false, true] {
                    let pitch = voice_on_side(fifth, root, above, low, high);
                    assert!((low..=high).contains(&pitch));
                    assert_eq!(PitchClass::new(pitch), PitchClass::new(fifth));
                    let available = (low..=high).any(|candidate| {
                        PitchClass::new(candidate) == PitchClass::new(fifth)
                            && if above {
                                candidate > root
                            } else {
                                candidate < root
                            }
                    });
                    if available {
                        assert!(
                            if above { pitch > root } else { pitch < root },
                            "root {root}, fifth {fifth}, above {above}: got {pitch}"
                        );
                    }
                }
            }
        }
    }

    /// The bass alone for sixteen bars at `seed`, in `style`.
    fn line(seed: u64, style: Option<PerformanceStyle>) -> Song {
        let mut song = Song {
            seed,
            bars: 16,
            ..Song::with_parts([PartSpec::of_role("bass", Role::Bass)])
        };
        song.settings.style = style;
        song
    }

    #[test]
    fn a_bass_keeps_one_figure_through_a_section() {
        // Drawn again every four bars, not one sixteen-bar take in 432 kept one figure, and
        // four in five put four bars of whole notes beside four bars of something busy. A
        // player picks a line and keeps it: every bar that states the figure is the first bar,
        // and every bar that picks up is the second. Only a phrase's closing bar may turn.
        use crate::fixture::bar_steps;
        for style in [
            None,
            Some(PerformanceStyle::PopBand),
            Some(PerformanceStyle::Rock),
            Some(PerformanceStyle::JazzTrio),
        ] {
            for seed in 0..16 {
                let (frame, parts) = line(seed, style).draft();
                let bass = part(&parts, "bass");
                let section = &frame.sections[0];
                assert!(
                    section.phrases.len() > 1,
                    "more than one phrase to hold across"
                );
                for bar in (0..16).filter(|bar| !section.closes_phrase(*bar)) {
                    assert_eq!(
                        bar_steps(&frame, bass, bar),
                        bar_steps(&frame, bass, bar % PICKUP_EVERY),
                        "{style:?}, seed {seed}: bar {bar} changed its bass figure"
                    );
                }
            }
        }
    }

    #[test]
    fn a_root_and_fifth_line_is_mostly_root() {
        // The fifth went to every hit off a beat, and a kick is mostly off the beat: over
        // basic-rock's kick the line was root, fifth, fifth. One hit answers now.
        assert_eq!(answering_hit(&[0, 6, 10], 16), Some(2), "root, root, fifth");
        assert_eq!(answering_hit(&[0, 8], 16), Some(1), "one and three");
        assert_eq!(answering_hit(&[0, 4, 8, 12], 16), Some(2));
        assert_eq!(
            answering_hit(&[0], 16),
            None,
            "a held root has nothing to answer"
        );
        assert_eq!(
            answering_hit(&[0, 2], 8),
            None,
            "nothing in the chord's second half"
        );

        // And in the line: whatever the figure and the seed, most of what a bass plays under a
        // chord is that chord's bass note.
        for seed in 0..16u64 {
            let (frame, parts) = line(seed, None).draft();
            let bass = part(&parts, "bass");
            let section = &frame.sections[0];
            // A walk reads the whole chord on purpose and is asserted where it lives.
            let third = section.events[0].chord.classes().get(1).copied();
            if bass
                .notes
                .iter()
                .take(4)
                .any(|note| Some(PitchClass::new(i32::from(note.pitch))) == third)
            {
                continue;
            }
            let roots = bass
                .notes
                .iter()
                .filter(|note| {
                    section
                        .chord_at(note.start - section.start)
                        .is_some_and(|event| {
                            event.chord.bass_class() == PitchClass::new(i32::from(note.pitch))
                        })
                })
                .count();
            assert!(
                roots * 2 > bass.notes.len(),
                "seed {seed}: {roots} of {} notes are the root",
                bass.notes.len()
            );
        }
    }

    #[test]
    fn a_note_lets_go_on_the_beat_before_a_syncopated_hit() {
        let grid = crate::rhythm::Grid::default();
        assert_eq!(
            lift_before(grid, 0, 6),
            Some(4),
            "a quarter, then an eighth of rest"
        );
        assert_eq!(lift_before(grid, 6, 10), Some(8));
        assert_eq!(lift_before(grid, 10, 16), None, "into a downbeat it holds");
        assert_eq!(lift_before(grid, 0, 4), None, "into a beat it holds");
        assert_eq!(
            lift_before(grid, 0, 2),
            None,
            "there is no beat between to let go on"
        );
        assert_eq!(lift_before(grid, 4, 6), None);

        // Some takes rest and some hold, and none ever overlaps itself. Sounding all of the
        // time is what every take used to do, where the corpus lines sound for 87 hundredths.
        let (mut resting, mut holding) = (0, 0);
        for seed in 0..32u64 {
            let (frame, parts) = line(seed, None).draft();
            let bass = part(&parts, "bass");
            let sounding: i64 = bass.notes.iter().map(|note| note.length.raw()).sum();
            if sounding < frame.length.raw() {
                resting += 1;
            } else {
                holding += 1;
            }
            for pair in bass.notes.windows(2) {
                assert!(
                    pair[0].start + pair[0].length <= pair[1].start,
                    "seed {seed}: the note at {} overlaps the next",
                    pair[0].start.raw()
                );
            }
        }
        assert!(
            resting > 4 && holding > 4,
            "{resting} takes rest and {holding} hold"
        );
    }

    #[test]
    fn the_second_bar_steps_into_the_bar_after_it() {
        // One figure for the section and nothing else was the same bar sixteen times. In the
        // takes that pick up, every second bar adds the last half-beat and plays it a scale
        // step under the root it is going to.
        use crate::fixture::bar_steps;
        let (mut picking_up, mut plain) = (0, 0);
        for seed in 0..32u64 {
            let (frame, parts) = line(seed, Some(PerformanceStyle::PopBand)).draft();
            let bass = part(&parts, "bass");
            let section = &frame.sections[0];
            let (first, second) = (bar_steps(&frame, bass, 0), bar_steps(&frame, bass, 1));
            if first == second {
                plain += 1;
                continue;
            }
            // A walk approaches every chord already, and so never picks up; a walking take
            // whose bars differ is one whose phrase turned, which is not this test's business.
            if first == vec![0, 4, 8, 12] {
                continue;
            }
            picking_up += 1;
            assert!(
                first.iter().all(|step| second.contains(step)),
                "seed {seed}: the pickup bar lost a note of the figure"
            );
            assert!(second.contains(&14), "seed {seed}: bar two is {second:?}");
            let bar = frame.grid.bar_ticks();
            let pickup = bass
                .notes
                .iter()
                .find(|note| note.start == bar * 2 - Ticks(480))
                .expect("the pickup itself");
            let arriving = section
                .chord_at(bar * 2)
                .expect("a chord in bar three")
                .chord
                .bass_class();
            let below = PitchClass::new(i32::from(pickup.pitch)).distance_up_to(arriving);
            assert!(
                (1..=3).contains(&below),
                "seed {seed}: the pickup sits {below} semitones under the root it goes to"
            );
        }
        assert!(
            picking_up > 4 && plain > 4,
            "{picking_up} takes pick up and {plain} play the figure plain"
        );
    }

    #[test]
    fn a_pulse_runs_eighths_from_the_middle_of_the_dial() {
        // A repeated root on every beat was the unstyled pulse at any setting, and eighths the
        // property of two idioms. Eighths are what the corpus's bass lines play most.
        use crate::fixture::bar_steps;
        let pumping = |density: f32| -> usize {
            (0..64u64)
                .filter(|seed| {
                    let mut song = line(*seed, None);
                    song.parts[0].density = Some(density);
                    let (frame, parts) = song.draft();
                    bar_steps(&frame, part(&parts, "bass"), 0) == vec![0, 2, 4, 6, 8, 10, 12, 14]
                })
                .count()
        };
        assert_eq!(pumping(0.3), 0, "a sparse pulse stays on the beat");
        let middle = pumping(0.5);
        assert!(
            (12..=44).contains(&middle),
            "{middle} of 64 unstyled takes run eighths"
        );
    }

    #[test]
    fn a_bass_that_picks_up_runs_into_a_new_phrase() {
        // The pickup came round every second bar and never changed, so a take had a bar and a
        // half of different rhythms in sixteen. Where a phrase closes it is two notes up the
        // scale into the bar after: eighths, and sixteenths once the dial is past two thirds.
        use crate::fixture::bar_steps;
        for (density, run) in [(0.5, [12usize, 14]), (0.8, [14, 15])] {
            let mut running = 0;
            for seed in 0..48u64 {
                let mut song = line(seed, Some(PerformanceStyle::PopBand));
                song.parts[0].density = Some(density);
                let (frame, parts) = song.draft();
                let bass = part(&parts, "bass");
                let (plain, pickup, close) = (
                    bar_steps(&frame, bass, 0),
                    bar_steps(&frame, bass, 1),
                    bar_steps(&frame, bass, 3),
                );
                // A take that plays the figure plain, a held root or a walk has no run.
                if plain == pickup && !plain.contains(&14) || plain == vec![0, 4, 8, 12] {
                    continue;
                }
                let bar = frame.grid.bar_ticks();
                let step = frame.grid.step_ticks();
                let at = |index: usize| {
                    bass.notes
                        .iter()
                        .find(|note| note.start == bar * 3 + step * index as i64)
                        .map(|note| i32::from(note.pitch))
                };
                let (Some(first), Some(second)) = (at(run[0]), at(run[1])) else {
                    continue;
                };
                let arriving = bass
                    .notes
                    .iter()
                    .find(|note| note.start == bar * 4)
                    .map(|note| i32::from(note.pitch))
                    .expect("the downbeat of bar five");
                if !(first < second && (1..=3).contains(&(second - first))) {
                    continue;
                }
                running += 1;
                assert!(
                    close
                        .iter()
                        .all(|step| *step < run[0] || run.contains(step)),
                    "seed {seed}: the figure struck inside the run: {close:?}"
                );
                // The line may fold at the edge of the instrument, so it is the pitch class
                // the run arrives under.
                let under = (arriving - second).rem_euclid(12);
                assert!(
                    (1..=3).contains(&under),
                    "seed {seed}: {second} into {arriving}"
                );
            }
            assert!(
                running >= 8,
                "{running} of 48 takes ran into bar five at {density}"
            );
        }
    }

    #[test]
    fn with_no_idiom_named_a_bass_seldom_walks() {
        // Quarter notes through the third over a kit playing a backbeat is a jazz player
        // sitting in. The figure stays reachable, and stays the jazz trio's own.
        let walks = |style| {
            (0..512)
                .filter(|seed| {
                    let mut rng = Rng::stream(*seed, &[]);
                    pick_figure(&mut rng, 0.6, style) == BassFigure::Walk
                })
                .count()
        };
        let plain = walks(None);
        assert!(plain > 0, "the walk is out of reach");
        assert!(plain < 40, "{plain} of 512 unstyled takes walk");
        assert!(walks(Some(PerformanceStyle::JazzTrio)) > plain * 8);
    }

    #[test]
    fn bass_styles_distinguish_pulse_walk_and_sustain() {
        let count = |style, figure| {
            (0..256)
                .filter(|seed| {
                    let mut rng = Rng::stream(*seed, &[]);
                    pick_figure(&mut rng, 0.6, Some(style)) == figure
                })
                .count()
        };
        assert!(
            count(PerformanceStyle::JazzTrio, BassFigure::Walk)
                > count(PerformanceStyle::Rock, BassFigure::Walk) * 4
        );
        assert!(
            count(PerformanceStyle::Rock, BassFigure::Pulse)
                > count(PerformanceStyle::Ambient, BassFigure::Pulse) * 3
        );
        assert!(
            count(PerformanceStyle::Ambient, BassFigure::Root)
                > count(PerformanceStyle::JazzTrio, BassFigure::Root) * 3
        );
    }

    #[test]
    fn the_octave_figure_always_moves_an_octave() {
        // The range a bass actually writes in, which is exactly two octaves wide.
        let (low, high) = crate::spec::Role::Bass.range();
        assert_eq!(
            high - low,
            2 * OCTAVE,
            "the range this test is about changed"
        );

        // Every root the roster can sit on — `Role::Bass` puts them in octave 2, so 36..=47.
        for root in 36..=47 {
            let leapt = octave_leap(root, low, high);
            assert_eq!(
                (leapt - root).abs(),
                OCTAVE,
                "the root {root} did not move an octave"
            );
            assert!(
                (low..=high).contains(&leapt),
                "the root {root} leapt out of the range"
            );
        }

        // Which way it goes: up while there is room above, down once there is not. Both are an
        // octave and both are what a player would reach for.
        assert_eq!(octave_leap(36, low, high), 48);
        assert_eq!(octave_leap(40, low, high), 52);
        assert_eq!(
            octave_leap(41, low, high),
            29,
            "F used to answer with itself"
        );
        assert_eq!(
            octave_leap(47, low, high),
            35,
            "B used to answer with itself"
        );

        // A window with no octave in it has no leap, and says so rather than pretending.
        assert_eq!(octave_leap(40, 36, 44), 40);
    }

    #[test]
    fn the_walking_line_reads_the_chord_and_steps_into_the_next() {
        use crate::theory::chord::Quality;
        use crate::theory::key::Key;

        let key = Key::parse("C major").unwrap();
        let (low, high) = crate::spec::Role::Bass.range();

        // Heading up: C to F is the method-book line — root, third, fifth, and the step below
        // the arriving root. E twice is not a repeat: the notes are two beats apart.
        let c = Chord::new(PitchClass::parse("C").unwrap(), Quality::Major);
        let up = walk_line(&c, &ChordScale::new(key, c), 36, Some(41), 4, low, high);
        assert_eq!(up, vec![36, 40, 43, 40]);

        // Heading down: G7 to C reads the chord the other way — G, F, D — and arrives on the
        // leading note, so the tonic lands a semitone above the line that prepared it.
        let g7 = Chord::new(PitchClass::parse("G").unwrap(), Quality::Dominant7);
        let down = walk_line(&g7, &ChordScale::new(key, g7), 43, Some(36), 4, low, high);
        assert_eq!(down, vec![43, 41, 38, 35]);

        // Every note of both belongs to the chord or the key, which is the crate's own rule.
        for line in [&up, &down] {
            for pitch in line {
                let class = PitchClass::new(*pitch);
                assert!(
                    key.scale.contains(key.tonic, class) || c.contains(class) || g7.contains(class),
                    "{pitch} is outside the harmony"
                );
            }
        }

        // Three beats is the same idea one note shorter, and the last chord of a piece has
        // nothing to approach, so it reads the chord to the end instead.
        assert_eq!(
            walk_line(&c, &ChordScale::new(key, c), 36, Some(41), 3, low, high),
            vec![36, 40, 43],
            "root, third, and an approach that will not stand on the note it just played"
        );
        let nowhere = walk_line(&c, &ChordScale::new(key, c), 36, None, 4, low, high);
        assert_eq!(
            nowhere,
            vec![36, 40, 43, 40],
            "third again, not an approach"
        );
    }

    #[test]
    fn a_busy_bass_can_walk() {
        // The figure has to be reachable: some bar of some seed walks quarter notes through
        // the chord's third, which no other figure plays — the rest are root, fifth and octave.
        let mut walked = false;
        for seed in 1..=16u64 {
            let mut song = Song {
                seed,
                ..Song::with_parts([Song::part(Role::Bass, |bass| bass.density = Some(1.0))])
            };
            // In the idiom that walks: with none named the figure is one draw in twenty, and
            // a section draws once.
            song.settings.style = Some(PerformanceStyle::JazzTrio);
            let (frame, parts) = song.draft();
            let bass = part(&parts, "bass");
            let section = &frame.sections[0];
            for event in &section.events {
                let third = event.chord.classes().get(1).copied();
                let in_event: Vec<&Draft> = bass
                    .notes
                    .iter()
                    .filter(|note| {
                        note.start >= section.start + event.start
                            && note.start < section.start + event.end()
                    })
                    .collect();
                if third.is_some_and(|third| {
                    in_event
                        .iter()
                        .any(|note| PitchClass::new(i32::from(note.pitch)) == third)
                }) {
                    walked = true;
                }
            }
        }
        assert!(walked, "no bar in sixteen seeds walked through a third");
    }

    #[test]
    fn the_bass_sounds_every_chord_change() {
        let (frame, parts) = Song::default().draft();
        let bass = part(&parts, "bass");
        let section = &frame.sections[0];
        for event in &section.events {
            let at = section.start + event.start;
            assert!(
                bass.notes.iter().any(|note| note.start == at),
                "the bass missed the change at {}",
                event.start.raw()
            );
        }
    }

    #[test]
    fn the_bass_plays_the_sounding_bass_of_a_slash_chord() {
        let (frame, parts) = Song {
            chords: "@koakuma".into(),
            ..Song::default()
        }
        .draft();
        let bass = part(&parts, "bass");
        let section = &frame.sections[0];
        // Bar two is V over the subdominant, so the bass must play the subdominant.
        let event = &section.events[1];
        let expected = event.chord.bass_class();
        let note = bass
            .notes
            .iter()
            .find(|note| note.start == section.start + event.start)
            .expect("a note at the change");
        assert_eq!(PitchClass::new(i32::from(note.pitch)), expected);
    }

    #[test]
    fn no_bass_note_outlives_the_chord_it_belongs_to() {
        // More chords in a bar than the grid has steps, which the chart syntax allows and a dial
        // can ask for: nine chords in four four leave 426 ticks each, and an eighth-note step is
        // 480. Flooring the length at a whole step re-inflated every note past its chord, so eight
        // of the nine overlapped the next by 54 ticks — two roots sounding at once on one
        // instrument, which is the one thing a bass part must not do.
        let (frame, parts) = Song {
            chords: "| I ii iii IV V vi vii I bII |".into(),
            bars: 1,
            ..Song::with_parts([Song::part(Role::Bass, |bass| {
                bass.subdivision = auris_core::Subdivision::Eighth;
            })])
        }
        .draft();
        let bass = part(&parts, "bass");
        let section = &frame.sections[0];
        assert!(bass.notes.len() >= 9, "one note per chord at least");

        for event in &section.events {
            let from = section.start + event.start;
            let to = section.start + event.end();
            for note in bass
                .notes
                .iter()
                .filter(|note| note.start >= from && note.start < to)
            {
                assert!(
                    note.start + note.length <= to,
                    "a note at {} ran {} ticks past its chord's end at {}",
                    note.start.raw(),
                    (note.start + note.length - to).raw(),
                    to.raw()
                );
            }
        }

        // And the general form of it: no two bass notes overlap at all.
        let mut sorted = bass.notes.clone();
        sorted.sort_by_key(|note| note.start);
        for pair in sorted.windows(2) {
            assert!(
                pair[0].start + pair[0].length <= pair[1].start,
                "the note at {} overlaps the one at {}",
                pair[0].start.raw(),
                pair[1].start.raw()
            );
        }
    }

    #[test]
    fn the_bass_follows_the_kick_in_an_odd_meter() {
        // The groove is a bar long, so it has to be read modulo the bar rather than modulo its
        // own sixteen steps, or it drifts against the drums in anything but four four.
        let (frame, parts) = Song {
            meter: auris_core::TimeSignature::new(3, 4),
            ..Song::default()
        }
        .draft();
        let bass = part(&parts, "bass");
        let kick = part(&parts, "kick");
        assert!(!bass.notes.is_empty() && !kick.notes.is_empty());
        // Every kick that survived thinning should have a bass note with it somewhere in the bar.
        let bar = frame.grid.bar_ticks();
        for note in &kick.notes {
            let bar_index = note.start.raw() / bar.raw();
            assert!(
                bass.notes
                    .iter()
                    .any(|other| other.start.raw() / bar.raw() == bar_index),
                "no bass in the bar with a kick at {}",
                note.start.raw()
            );
        }
    }
}
