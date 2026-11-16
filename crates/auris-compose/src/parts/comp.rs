//! Chords, either comped in rhythm or held as a pad.
//!
//! One file for three roles — chords, pad and stab — because they are one writer with its dials
//! set differently, and splitting them would mean three copies of everything below. What the
//! file is really about is the six figures a keyboard player actually comps: they are written
//! against the beat rather than against a note value, so the same six mean the same six things on
//! whatever grid the part divides its beats into. Which notes a chord sounds, and how its voices
//! are led from the chord before, is `voicing`, which the arpeggio reads as well.

use auris_core::time::Ticks;

use crate::PerformanceStyle;
use crate::frame::{Frame, SectionPlan};
use crate::rng::{Key as RngKey, Rng};
use crate::spec::{PartSpec, Role};
use crate::theory::chord::Chord;
use crate::theory::pitch::{OCTAVE, PitchClass};

use super::voicing::voiced;
use super::writer::{
    band_pushes, bar_onsets, bar_stream, breath_before, density, lift_before, part_grid,
    phrase_shape, velocity,
};
use super::{Draft, ScoreSettings};

/// How a chord is struck through a bar.
///
/// A part that only ever played the chord on every beat wrote the same bar for every seed, so
/// asking it for another take gave back what it had already given. These are the ways a keyboard
/// player actually comps, and one of them is chosen per bar.
///
/// Every one is written against the *beat* rather than against a note value, so the same six
/// figures mean the same six things whether the part is dividing its beats in two, three, four or
/// six. That is what lets a triplet grid be a setting rather than a separate set of figures.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum CompFigure {
    /// Held for as long as the chord lasts.
    Held,
    /// Once on every beat.
    Beats,
    /// On the second half of each beat, which pushes the music forward.
    Offbeats,
    /// Beat one and the half-beat after beat two: the Charleston, and half of pop music.
    Charleston,
    /// A euclidean rhythm across the bar: the tresillo and its relatives, which put three
    /// against the bar's four and are where a comp gets its lift without getting busier.
    Cross,
    /// A rhythm rolled from the metric hierarchy: most of the steps, with the holes that make it
    /// a rhythm. This is the fast one — with the gate most of the way down it is the release-cut
    /// piano dance music is built on.
    Rolled,
}

/// How many takes of a comp let a chord go before the next strike; the rest hold every chord
/// into the next. See `lift_before` and `breath_before`.
///
/// Read off the chordal parts of a pop and rock score corpus: the keyboard and guitar tracks
/// that strike two and a half notes at a time or more, 269 sixteen-bar windows. Fifty-nine of
/// them never rest, 22 in a hundred, and the rest do: a strike is followed by a rest 38 times
/// in a hundred and a part sounds for 80 hundredths of the time. It was half the takes, the
/// bass's number borrowed, with a rest only before a syncopated strike, and a comp sounded 94
/// hundredths of the time.
const LIFT: f32 = 0.78;

/// How often the last bar of a planned phrase departs from the figure the section chose.
///
/// A turnaround belongs at the shared phrase boundary, including short and uneven phrases.
const TURNAROUND: f32 = 0.45;

/// Draws one comping figure, weighted by how busy the part was asked to be.
///
/// Sparse reaches for the held chord, busy for the offbeats and the rolled figure. Every figure
/// keeps some weight, because a dial that forbids a choice outright makes every section the same
/// again.
fn pick_figure(rng: &mut Rng, busy: f32, style: Option<PerformanceStyle>) -> CompFigure {
    const FIGURES: [CompFigure; 6] = [
        CompFigure::Held,
        CompFigure::Beats,
        CompFigure::Charleston,
        CompFigure::Offbeats,
        CompFigure::Cross,
        CompFigure::Rolled,
    ];
    let mut weights = [
        // Weighted far below where it started. Now that a figure lasts a whole section,
        // drawing the held chord means holding one chord for the whole of it — which is a
        // pad played by the wrong part, and the pad is the part that does it properly:
        // it sustains what two chords have in common instead of striking them again.
        0.1 + (1.0 - busy) * 0.8,
        1.0,
        0.2 + busy,
        0.2 + busy * 1.6,
        0.2 + busy * 1.4,
        // Squared, so the fast one is somewhere the dial has to be pushed rather than
        // somewhere a middling setting wanders into. It is the loudest thing a comp can do.
        0.05 + busy * busy * 3.0,
    ];
    // These are preferences, not exclusions: a sparse jazz passage can still hold a chord.
    // Rhythm and articulation stay separate, so changing a sound never chooses a new figure.
    let vocabulary = match style {
        Some(PerformanceStyle::JazzTrio) => [0.9, 0.3, 2.6, 1.8, 0.8, 0.2],
        Some(PerformanceStyle::CityPop) => [0.3, 0.5, 1.5, 2.0, 1.8, 0.8],
        Some(PerformanceStyle::Rock) => [0.6, 3.0, 0.5, 0.7, 0.3, 1.0],
        Some(PerformanceStyle::Ambient) => [8.0, 0.3, 0.4, 0.3, 0.5, 0.1],
        Some(PerformanceStyle::Orchestral) => [3.5, 1.5, 0.4, 0.3, 0.6, 0.3],
        Some(PerformanceStyle::Synthwave | PerformanceStyle::Chiptune) => {
            [0.4, 1.8, 0.5, 1.6, 1.0, 1.2]
        }
        Some(PerformanceStyle::PopBand) => [0.7, 1.2, 1.4, 1.2, 0.8, 0.7],
        None => [1.0; 6],
    };
    for (weight, preference) in weights.iter_mut().zip(vocabulary) {
        *weight *= preference;
    }
    FIGURES[rng.weighted(&weights).min(FIGURES.len() - 1)]
}

/// How many notes a chord sounds when the part leaves it to the chord: its own tones, and one
/// more at the busy end of the density dial, read as it was set.
///
/// Density is all a pad's one held chord lets the dial reach, and a tone doubled on top is what
/// "more" means to a chord that is not struck more often. Never fewer than the chord has: a
/// thinner chord is asked for with the note count, not drifted into by a dial set for the rhythm.
fn auto_voices(chord: Chord, dial: f32) -> usize {
    let own = chord.quality.intervals().len();
    if dial >= 0.75 { (own + 1).min(6) } else { own }
}

/// The lowest a chord part's own bass note goes: the bottom string of a bass guitar.
///
/// Below this a piano's left hand is rumble rather than a pitch, and a voicing pushed down by the
/// octave dial would otherwise drag its root out of anything a speaker can reproduce.
const LOWEST_ROOT: i32 = 28;

/// The chord's bass note placed under a voicing: the highest one at least an octave below the
/// voicing's lowest note, which is where a pianist's left hand sits under the right.
///
/// The slash bass when the chord has one — `C/E` is asked for with E underneath, and a C there
/// would be a different chord. `None` when the voicing is already so low that nothing fits above
/// [`LOWEST_ROOT`] and below it.
fn low_root(chord: Chord, voicing: &[i32]) -> Option<i32> {
    let lowest = *voicing.iter().min()?;
    let class = chord.bass_class();
    let ceiling = lowest - OCTAVE;
    // How far the ceiling sits above the nearest note of the class at or below it.
    let mut root = ceiling - class.distance_up_to(PitchClass::new(ceiling));
    while root < LOWEST_ROOT {
        root += OCTAVE;
    }
    (root < lowest).then_some(root)
}

/// How late each note of a strummed chord begins, for `count` notes sounding `length`, lowest
/// first.
///
/// `strum` apart, unless the chord is struck too briefly for that: then the strum is squeezed so
/// the top note still begins inside the first half of the strike. A strum that ran past the
/// strike would leave its top notes sounding only after the chord has been let go — or never.
fn strum_offsets(count: usize, strum: Ticks, length: Ticks) -> Vec<Ticks> {
    if count < 2 || strum <= Ticks::ZERO {
        return vec![Ticks::ZERO; count];
    }
    let steps = count as i64 - 1;
    let step = strum.raw().min(length.raw() / 2 / steps).max(0);
    (0..count as i64).map(|index| Ticks(index * step)).collect()
}

/// Chords, either comped in rhythm or held as a pad.
pub(super) fn comp(
    settings: &ScoreSettings,
    frame: &Frame,
    section: &SectionPlan,
    index: usize,
    part: &PartSpec,
) -> Vec<Draft> {
    let grid = part_grid(frame, part);
    let range = part.chord_range();
    let target = part.chord_centre();
    let mut notes: Vec<Draft> = Vec::new();
    // A rhythm the user wrote overrides the generated one — that is the field's stated
    // contract, and the melody and the drums already keep it. It overrides a pad's stillness
    // too: a pad's "rhythm" is that it has none, and writing one asks for strikes.
    let written = part.rhythm.is_some();
    let pad = part.role == Role::Pad && !written;
    let mut previous: Vec<i32> = Vec::new();
    // Which voice of the last chord is still sounding, and where its note is, so that a pad can
    // let it run on. See the loop below: this is most of what makes a pad a pad.
    let mut sustaining: Vec<(i32, usize)> = Vec::new();

    // How the part sits, decided once for the section: which of the voicings nearly as good as
    // the best its first chord takes. A pad has no rhythm to vary, so this and the note count are
    // the whole of what makes one take of it differ from another. Where it sits is not drawn —
    // that is the centre, which is somebody's choice rather than the seed's.
    let mut choose = bar_stream(frame, part, section, "register", 0);
    let busy = density(settings, part, section);
    // The dial as it was set, before the role and the section's intensity scale it: a pad's
    // share of that is never more than four tenths, and the note count answers the slider.
    let dial = part.density.unwrap_or_else(|| settings.mood.density());
    // The upper quarter of the dial fills the subdivision, independently of loudness.
    // At full density even a quiet section must reach every step. Authored rests and pads
    // retain their own timing contract.
    let drive = if pad || written {
        0.0
    } else {
        crate::arrangement::chord_subdivision_fill(
            part.density.unwrap_or_else(|| settings.mood.density()),
        )
    };

    // How the part comps, drawn once for the section and then restated over every chord in it.
    //
    // Per bar was wrong, and wrong in the way the melody used to be wrong: a keyboard player
    // picks a feel and keeps it, so a comp that drew again every bar was four bars of four
    // different players and left the section with nothing an ear could hold on to. Keyed by the
    // section's name and not by which playing of it this is, so a second chorus comps like the
    // first.
    let mut invent = Rng::stream(
        frame.seed,
        &[
            RngKey::Word("part"),
            RngKey::Word(&part.name),
            RngKey::Word("comp"),
            RngKey::Word(&section.name),
        ],
    );
    //
    // Above three quarters of the dial the figure is still the figure, with the steps it leaves
    // empty filling in round it. It used to be thrown away there for the rolled one: a comp at
    // 0.75 struck four times a bar and at 0.76 seven, and from there on no style had a
    // vocabulary and no section a push, because one figure had replaced all six.
    let chosen_figure = if written {
        // The written rhythm plays through the rolled figure's machinery, which is already
        // "strike on exactly these steps".
        CompFigure::Rolled
    } else if pad {
        CompFigure::Held
    } else {
        pick_figure(&mut invent, busy, settings.style)
    };
    // The rolled figure's own rhythm — or the written one, played as written. Drawn from the
    // same stream so that it belongs to the section too, and drawn whether or not it is wanted,
    // so that a turnaround reaching for it later finds the section's rhythm rather than a
    // different one — and so the stream does not shift under everything else depending on which
    // figure came out.
    let rolled = bar_onsets(
        grid,
        part.rhythm.as_ref(),
        busy,
        settings.mood.syncopation,
        &mut invent,
    );
    // The steps of the bar the dial's upper quarter strikes on top of the figure.
    let mut driven = vec![false; grid.steps_per_bar()];
    if drive > 0.0 {
        let mut fill = bar_stream(frame, part, section, "comp-density", 0);
        for step in driven.iter_mut() {
            // Draw on every step so raising density only opens more opportunities in this
            // stream, instead of rescrambling the following decisions.
            *step = fill.chance(drive);
        }
    }
    // Whether this section's comp pushes: each chord change struck half a beat early and held
    // over the line, the anticipation every keyboard player owns. A property of the section and
    // not of the chord — a player who pushes, pushes, and a per-change roll would also break the
    // promise that the figure holds through a phrase, since the push lands its strike in the bar
    // *before* the change. That was the comp's own draw, once for the section. It is the
    // band's now, asked of each change as it comes: the bass pushes the same ones, and a
    // change that is pushed is pushed every time the progression comes round to it, which
    // keeps the figure's promise from the other side. See `band_pushes`.
    // Half a felt beat, the distance every push is early by.
    let push_early = grid.step_ticks() * (grid.steps_per_beat() / 2).max(1) as i64;
    // Whether this take lets a chord go before a syncopated strike.
    let lifting = invent.chance(LIFT) && !pad && !written;

    for (event_index, event) in section.events.iter().enumerate() {
        // The middle is pulled to the centre on every chord, not only the first, and the voice
        // leading chooses among the inversions that keep it there.
        let voices = match part.voices {
            0 => auto_voices(event.chord, dial),
            asked => usize::from(asked),
        };
        let voicing = voiced(
            event.chord,
            &previous,
            range,
            target,
            voices,
            settings.style,
            Some(&mut choose),
        );
        previous.clone_from(&voicing);
        // The root under the voicing is struck with it but is not part of the voice leading:
        // `previous` keeps only the voicing, so the next chord is led from the right hand and
        // the left hand simply finds the next root under wherever that lands.
        let mut voicing = voicing;
        if part.low_root
            && let Some(root) = low_root(event.chord, &voicing)
        {
            voicing.insert(0, root);
        }
        // Lowest first, which is the order a strum reaches them in.
        voicing.sort_unstable();

        // Which rhythm the chord is struck on. Chosen per bar from the section's own stream, so a
        // repeat of the section comps the same way and a different seed comps differently — the
        // whole of this part used to be one fixed pattern, which made "another take" a button
        // that could not do anything.
        let bar = grid.step_of(event.start) / grid.steps_per_bar().max(1);
        // The shared phrase plan supplies the turn, including unequal phrase lengths.
        let figure = if pad || written || !section.closes_phrase(bar) {
            chosen_figure
        } else {
            let mut rng = bar_stream(frame, part, section, "comp", bar);
            if rng.chance(TURNAROUND) {
                pick_figure(&mut rng, busy, settings.style)
            } else {
                chosen_figure
            }
        };

        // Whether this change is pushed: the section's style, wherever there is half a beat
        // before the chord to borrow. The first chord of a section has none — its half-beat
        // belongs to the section before, which has already been written.
        // A dense chart can leave less than half a beat between changes. Borrow at most half of
        // that smaller gap, so both the outgoing chord and its anticipation retain an audible
        // span instead of pushing the newcomer back past the old chord's own onset.
        let pushed_by = section.events[..event_index]
            .last()
            .map(|previous| {
                push_early.min(Ticks(
                    event
                        .start
                        .raw()
                        .saturating_sub(previous.start.raw())
                        .max(0)
                        / 2,
                ))
            })
            .unwrap_or(push_early);
        // Only a change is anticipated: restriking a chord that is already sounding half a
        // beat early is a stumble, not a push.
        let pushing = section.events[..event_index]
            .last()
            .is_some_and(|previous| band_pushes(previous, event, settings.mood.syncopation));
        let push = pushing
            && !section.continues_chord(event_index)
            && !pad
            && !written
            // Every step struck leaves nothing to anticipate with: the push would take the
            // strike on the line away, and full density promises that one.
            && drive < 1.0
            && pushed_by > Ticks::ZERO
            && event.start >= pushed_by;
        // A held chord is held until the dial starts filling the bar in round it.
        let held_whole = figure == CompFigure::Held && drive == 0.0;
        let mut onsets: Vec<usize> = if held_whole {
            vec![0]
        } else {
            let beat = grid.steps_per_beat().max(1);
            let half = (beat / 2).max(1);
            let per_bar = grid.steps_per_bar().max(1);
            let from = grid.step_of(event.start);
            // Three hits to the bar's eight, which is the tresillo on an eighth grid and the
            // 3-3-2 of every dance record on a sixteenth one. Rounded up so a grid of twelve
            // gets five rather than the four that would just be the beats again.
            let cross = crate::rhythm::euclid((per_bar * 3).div_ceil(8).max(2), per_bar, 0);
            // Measured against the bar rather than against the chord, so a figure stays in step
            // with the beat when two chords share a bar.
            let mut chosen: Vec<usize> = (0..grid.step_of(event.length))
                .filter(|offset| {
                    let at = (from + offset) % per_bar;
                    let figured = match figure {
                        CompFigure::Beats => at.is_multiple_of(beat),
                        CompFigure::Offbeats => at % beat == half,
                        CompFigure::Charleston => at == 0 || at == beat + half,
                        CompFigure::Cross => cross.at(at).is_some(),
                        CompFigure::Rolled => rolled.iter().any(|(step, _)| *step == at),
                        CompFigure::Held => false,
                    };
                    figured || driven.get(at).copied().unwrap_or(false)
                })
                .collect();
            // A chord nobody strikes is a chord nobody hears change, so its own start always
            // sounds whatever the figure says — unless the rhythm was written by hand, whose
            // rests are as much the instruction as its hits.
            if !written && !chosen.contains(&0) {
                chosen.insert(0, 0);
            }
            chosen
        };
        // The pushed strike replaces the one on the chord's own start rather than doubling it:
        // an anticipation restruck on the line is not an anticipation, it is a stutter.
        if push {
            onsets.retain(|offset| *offset != 0);
        }
        let held = pad || held_whole;
        let last = onsets.len().saturating_sub(1);
        let mut still_sounding: Vec<(i32, usize)> = Vec::new();

        // The push: the chord struck half a beat before its own start and held over the line,
        // which is the anticipation every keyboard player owns and the one thing a comp does
        // that the chart does not say. Whatever was still sounding is let go at the strike —
        // two voicings overlapping is not an anticipation, it is a smear.
        if push {
            let boundary = section.start + event.start - pushed_by;
            // The old chord's own strikes inside the borrowed half-beat go entirely — an
            // offbeat figure lands one exactly where the push lands, and the two voicings
            // struck together are the smear this block exists to prevent. Safe to drop by
            // index because only a pad holds indices into `notes`, and a pad never pushes.
            notes.retain(|note| note.start < boundary);
            for note in notes.iter_mut() {
                if note.start + note.length > boundary {
                    note.length = boundary - note.start;
                }
            }
            let sounds_to = onsets
                .first()
                .map(|onset| event.start + grid.tick_of(*onset))
                .unwrap_or_else(|| event.end());
            let weight = grid.weight(grid.step_of(event.start));
            let length = (sounds_to - (event.start - pushed_by)).max(Ticks(1));
            let delays = strum_offsets(voicing.len(), auris_core::strum_ticks(part.strum), length);
            for (pitch, delay) in voicing.iter().zip(&delays) {
                notes.push(Draft {
                    section: index,
                    pitch: (*pitch).clamp(0, 127) as u8,
                    velocity: (velocity(weight, section.intensity, settings.dynamics)
                        * if held { 0.7 } else { 0.9 }
                        * phrase_shape(grid, section, event.start - pushed_by, settings.dynamics))
                    .clamp(0.05, 1.0),
                    start: boundary + *delay,
                    length: (length - *delay).max(Ticks(1)),
                });
            }
        }

        for (position, onset) in onsets.iter().enumerate() {
            let at = event.start + grid.tick_of(*onset);
            if at >= event.end() {
                continue;
            }
            // To wherever the next chord in this figure begins, and to the end of the chord for
            // the last of them. A fixed beat was right for a figure that struck once a beat and
            // never oftener; sixteen chords in a bar would each have run over the fifteen behind
            // it, and the wall of sound that came out could have been one held note. This is also
            // what gives the gate something to be a fraction *of*.
            let next = if position < last {
                event.start + grid.tick_of(onsets[position + 1])
            } else {
                event.end()
            };
            let mut length = (next - at).min(event.end() - at).max(Ticks(1));
            // Let go on the beat before a syncopated strike, so the strike is a push and not a
            // slur, or half a beat before a backbeat, so the snare has its attack to itself —
            // unless that is less than half a beat away, because a chord shorter
            // than that is a click, and one let go anywhere else leaves no beat free. Only
            // inside the chord: the strike that opens the next one is on its line, or is the
            // section's own push, which lets this one go itself.
            if lifting && position < last {
                let here = grid.step_of(at);
                let half = (grid.steps_per_beat() / 2).max(1);
                let there = grid.step_of(next);
                if let Some(boundary) =
                    lift_before(grid, here, there).or_else(|| breath_before(grid, here, there))
                    && boundary >= here + half
                {
                    length = (grid.tick_of(boundary) - at).max(Ticks(1));
                }
            }
            let weight = grid.weight(grid.step_of(at));
            // Every note of a strum still lets go where the strike does: the strum delays the
            // attacks, it does not move the chord.
            let delays = strum_offsets(voicing.len(), auris_core::strum_ticks(part.strum), length);
            for (pitch, delay) in voicing.iter().zip(&delays) {
                // A pad holds whatever two chords have in common rather than striking it again.
                // This is most of what makes a pad a pad and not a comp playing whole notes: the
                // voices with somewhere to go move, and the ones without stay exactly where they
                // are. A chord part restrikes every voice, which is what a keyboard player does
                // and what an ear hears as the chord *changing* rather than as it drifting.
                if let Some((_, sounding)) =
                    sustaining.iter().copied().find(|(voice, _)| voice == pitch)
                {
                    let ends = section.start + event.end();
                    notes[sounding].length = (ends - notes[sounding].start).max(Ticks(1));
                    still_sounding.push((*pitch, sounding));
                    continue;
                }
                notes.push(Draft {
                    section: index,
                    pitch: (*pitch).clamp(0, 127) as u8,
                    velocity: (velocity(weight, section.intensity, settings.dynamics)
                        * if held { 0.7 } else { 0.9 }
                        * phrase_shape(grid, section, at, settings.dynamics))
                    .clamp(0.05, 1.0),
                    start: section.start + at + *delay,
                    length: (length - *delay).max(Ticks(1)),
                });
                if pad {
                    still_sounding.push((*pitch, notes.len() - 1));
                }
            }
        }
        // Only a pad carries voices forward; leaving this empty is what makes every other part
        // strike every note of every chord.
        sustaining = if pad { still_sounding } else { Vec::new() };
    }
    let _ = settings;
    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{Song, bar_steps, part};
    use crate::spec::{PartSpec, Role};

    /// The chords alone over `chart` for `bars` bars at `seed`, with the syncopation dial set.
    fn chords(chart: &str, bars: usize, seed: u64, syncopation: f32) -> Song {
        let mut song = Song {
            chords: chart.to_string(),
            bars,
            seed,
            ..Song::with_parts([PartSpec::of_role("chords", Role::Chords)])
        };
        song.settings.mood.syncopation = syncopation;
        song
    }

    #[test]
    fn comping_styles_choose_different_rhythmic_vocabularies() {
        let count = |style, figure| {
            (0..256)
                .filter(|seed| {
                    let mut rng = Rng::stream(*seed, &[RngKey::Word("vocabulary")]);
                    pick_figure(&mut rng, 0.6, Some(style)) == figure
                })
                .count()
        };
        assert!(
            count(PerformanceStyle::Ambient, CompFigure::Held)
                > count(PerformanceStyle::JazzTrio, CompFigure::Held) * 3
        );
        assert!(
            count(PerformanceStyle::Rock, CompFigure::Beats)
                > count(PerformanceStyle::JazzTrio, CompFigure::Beats) * 3
        );
    }

    #[test]
    fn a_comp_at_full_density_strikes_every_sixteenth() {
        let full = |seed: u64| Song {
            seed,
            intensity: 1.0,
            ..Song::with_parts([Song::part(Role::Chords, |chords| {
                chords.density = Some(1.0)
            })])
        };
        let mut counts = Vec::new();
        for seed in 1..=8 {
            let (frame, parts) = full(seed).draft();
            let chords = part(&parts, "chords");
            counts.extend((0..4).map(|bar| bar_steps(&frame, chords, bar).len()));
        }
        let steps = 16;
        assert!(counts.iter().all(|count| *count == steps), "{counts:?}");
    }

    #[test]
    fn the_dial_fills_in_round_the_figure_past_three_quarters() {
        // The figure used to be thrown away for the rolled one the moment the dial crossed
        // 0.75: four strikes a bar at 0.75 and seven at 0.76, and no idiom above it.
        let strikes = |density: f32, style: Option<PerformanceStyle>| -> f32 {
            let mut total = 0;
            for seed in 0..48u64 {
                // Syncopation at zero, so that no stretch pushes: a push takes the strike on the
                // line away, which is its own subject.
                let mut song = chords("@axis", 8, seed, 0.0);
                song.parts[0].density = Some(density);
                song.settings.style = style;
                let (frame, parts) = song.draft();
                let comp = part(&parts, "chords");
                total += (0..8)
                    .map(|bar| bar_steps(&frame, comp, bar).len())
                    .sum::<usize>();
            }
            total as f32 / (48.0 * 8.0)
        };
        let mut last = strikes(0.70, None);
        for density in [0.75, 0.76, 0.80, 0.85, 0.90, 0.95, 1.0] {
            let now = strikes(density, None);
            assert!(
                now >= last - 0.2 && now - last < 3.5,
                "from {last} strikes a bar to {now} at {density}"
            );
            last = now;
        }
        assert!(strikes(0.76, None) - strikes(0.75, None) < 1.0);
        assert_eq!(strikes(1.0, None), 16.0);
        // An idiom still has its vocabulary up there: a trio's comp and a rock band's are not
        // the same bar.
        let trio = strikes(0.8, Some(PerformanceStyle::JazzTrio));
        let rock = strikes(0.8, Some(PerformanceStyle::Rock));
        assert!(
            (trio - rock).abs() > 0.05,
            "{trio} and {rock} strikes a bar"
        );
    }

    #[test]
    fn a_comp_that_lifts_lets_go_before_a_syncopated_strike_or_a_backbeat() {
        let mut lifting = 0;
        for seed in 0..64u64 {
            let (frame, parts) = chords("@axis", 8, seed, 0.0).draft();
            let grid = frame.grid;
            let comp = part(&parts, "chords");
            let beat = grid.tick_of(grid.steps_per_beat());
            let mut starts: Vec<_> = comp.notes.iter().map(|note| note.start).collect();
            starts.sort_unstable();
            starts.dedup();
            let mut rested = false;
            for note in &comp.notes {
                let end = note.start + note.length;
                let Some(next) = starts.iter().find(|start| **start > note.start) else {
                    continue;
                };
                if end >= *next {
                    continue;
                }
                rested = true;
                // It lets go on the beat before a strike off the beat, or half a beat before a
                // strike on a backbeat, and has sounded for half a beat at least by then.
                let arriving = grid.weight(grid.step_of(*next));
                if arriving <= 1 {
                    assert_eq!(
                        end.raw() % beat.raw(),
                        0,
                        "seed {seed}: let go off the beat before a syncopated strike"
                    );
                } else {
                    assert_eq!(arriving, 2, "seed {seed}: let go before a strong beat");
                    assert_eq!(
                        (*next - end).raw() * 2,
                        beat.raw(),
                        "seed {seed}: the breath before a backbeat is half a beat"
                    );
                }
                assert!(note.length.raw() * 2 >= beat.raw(), "seed {seed}: a click");
                // And never across a bar line: the chord that follows is struck on its line.
                assert_eq!(
                    note.start.raw() / grid.bar_ticks().raw(),
                    next.raw() / grid.bar_ticks().raw()
                );
            }
            lifting += usize::from(rested);
        }
        // Nearly eight takes in ten lift, and a lift shows wherever the figure has a strike
        // off the beat or on a backbeat, which a held chord has not.
        assert!((32..=56).contains(&lifting), "{lifting} of 64 takes let go");

        // A pad has nothing to lift for, and a rhythm somebody wrote says where its rests are.
        for seed in 0..16u64 {
            let mut pad = chords("@axis", 4, seed, 0.0);
            pad.parts = vec![PartSpec::of_role("pad", Role::Pad)];
            let (frame, parts) = pad.draft();
            let total: i64 = part(&parts, "pad")
                .notes
                .iter()
                .map(|n| n.length.raw())
                .sum();
            assert!(total >= frame.grid.bar_ticks().raw() * 4 * 3);
            let mut written = chords("@axis", 4, seed, 0.0);
            written.parts[0].rhythm = crate::rhythm::Pattern::parse("x ~ ~ x ~ ~ x ~");
            let (_, parts) = written.draft();
            let notes = &part(&parts, "chords").notes;
            let mut starts: Vec<_> = notes.iter().map(|note| note.start).collect();
            starts.sort_unstable();
            starts.dedup();
            for note in notes {
                if let Some(next) = starts.iter().find(|start| **start > note.start) {
                    assert_eq!(
                        note.start + note.length,
                        *next,
                        "a written rhythm was lifted"
                    );
                }
            }
        }
    }

    #[test]
    fn a_comp_keeps_one_figure_through_a_section_and_turns_it_over_at_the_end() {
        // A keyboard player picks a feel and keeps it. Drawing again every bar was four bars of
        // four different players, and left the section with nothing an ear could hold on to —
        // the same mistake the melody used to make, and the same fix. Only the fourth bar of a
        // phrase may depart, because that is where a phrase turns over.
        // Syncopation is pinned to zero because the push is a licensed departure of its own: a
        // pushing section borrows every bar's last half-beat for the next bar's chord, which is
        // the *style* holding, not the figure drifting — and it is asserted where it lives.
        let mut steady = 0;
        for seed in 1..=8u64 {
            let mut song = chords("@axis", 8, seed, 0.0);
            song.parts[0].density = Some(0.8);
            let (frame, parts) = song.draft();
            let chords = part(&parts, "chords");
            // Bars 0, 1 and 2 of each four-bar phrase are never allowed to differ from each
            // other. The chords move underneath them, but the steps struck do not.
            for phrase in 0..2 {
                let first = bar_steps(&frame, chords, phrase * 4);
                for bar in 1..3 {
                    assert_eq!(
                        bar_steps(&frame, chords, phrase * 4 + bar),
                        first,
                        "seed {seed} changed figure inside a phrase, at bar {}",
                        phrase * 4 + bar
                    );
                }
                if bar_steps(&frame, chords, phrase * 4 + 3) == first {
                    steady += 1;
                }
            }
        }
        // And the turnaround is a departure rather than a rule: most closing bars carry straight
        // on. A fourth bar that always changed would be a figure eight bars long, not a phrase.
        assert!(
            steady > 0,
            "every closing bar in sixteen phrases departed from its figure"
        );
    }

    #[test]
    fn a_syncopated_comp_pushes_the_change_and_a_straight_one_never_does() {
        // Which changes are pushed is read off the changes, so it is the charts that are
        // varied here and not the seeds: a quarter of all changes at the top of the dial.
        const CHARTS: [&str; 6] = [
            "@axis",
            "@royal-road",
            "@marusa",
            "@koakuma",
            "@canon",
            "@komuro",
        ];
        let take =
            |chart: &str, syncopation: f32, seed: u64| chords(chart, 8, seed, syncopation).draft();

        // At the top of the dial, some change is pushed: a strike half a beat before a chord
        // change whose pitch the outgoing chord does not own and the arriving one does. It rings
        // over the line, nothing of the old chord sounds under or beside it, and the line itself
        // is not struck again — an anticipation restruck on the line is a stutter.
        let mut pushed_anywhere = false;
        for (seed, chart) in (1..=12u64).zip(CHARTS.iter().cycle()) {
            let (frame, parts) = take(chart, 1.0, seed);
            let chords = part(&parts, "chords");
            let section = &frame.sections[0];
            let half = frame.grid.step_ticks() * 2;
            for event in section.events.iter().skip(1) {
                let line = section.start + event.start;
                let early: Vec<&crate::parts::Draft> = chords
                    .notes
                    .iter()
                    .filter(|note| {
                        note.start == line - half
                            && event.chord.contains_midi(i32::from(note.pitch))
                            && !section
                                .chord_at(note.start - section.start)
                                .is_some_and(|old| old.chord.contains_midi(i32::from(note.pitch)))
                    })
                    .collect();
                if early.is_empty() {
                    continue;
                }
                pushed_anywhere = true;
                for note in &early {
                    assert!(
                        note.start + note.length > line,
                        "seed {seed}: a push that does not cross the line is just an offbeat"
                    );
                }
                for note in &chords.notes {
                    if note.start < line - half {
                        assert!(
                            note.start + note.length <= line - half,
                            "seed {seed}: the old chord smears under the push at {}",
                            line.raw()
                        );
                    }
                }
                assert!(
                    !chords.notes.iter().any(|note| note.start == line),
                    "seed {seed}: the pushed change was struck again on the line"
                );
            }
        }
        assert!(
            pushed_anywhere,
            "no change in six charts was pushed at full syncopation"
        );

        // And square playing never anticipates: every strike belongs to the chord sounding
        // where it starts.
        for (seed, chart) in (1..=12u64).zip(CHARTS.iter().cycle()) {
            let (frame, parts) = take(chart, 0.0, seed);
            let chords = part(&parts, "chords");
            let section = &frame.sections[0];
            for note in &chords.notes {
                let event = section
                    .chord_at(note.start - section.start)
                    .expect("a chord under every strike");
                assert!(
                    event.chord.contains_midi(i32::from(note.pitch)),
                    "seed {seed}: a square comp struck {} outside {} at {}",
                    note.pitch,
                    event.chord,
                    note.start.raw()
                );
            }
        }
    }

    #[test]
    fn a_dense_chart_keeps_each_chord_audible_when_the_next_one_pushes() {
        // The change into the third chord has to be one the band pushes, and which those
        // are is read off the two chords: so the second and third are varied, not the seed.
        const DEGREES: [&str; 6] = ["I", "ii", "iii", "IV", "V", "vi"];
        let mut exercised = false;
        let dense = DEGREES
            .iter()
            .flat_map(|second| DEGREES.iter().map(move |third| (second, third)))
            .filter(|(second, third)| second != third)
            .map(|(second, third)| format!("| I {second} {third} IV V vi vii I bII |"));
        for (seed, chart) in (1u64..).zip(dense) {
            let (frame, parts) = chords(&chart, 1, seed, 1.0).draft();
            let section = &frame.sections[0];
            let chords = part(&parts, "chords");
            let [first, second, third, ..] = section.events.as_slice() else {
                panic!("the dense chart needs at least three changes");
            };
            let third_arrives_early = chords.notes.iter().any(|note| {
                note.start < third.start
                    && note.start >= second.start
                    && third.chord.contains_midi(i32::from(note.pitch))
                    && !second.chord.contains_midi(i32::from(note.pitch))
            });
            if !third_arrives_early {
                continue;
            }
            exercised = true;
            assert!(
                chords.notes.iter().any(|note| {
                    note.start <= second.start
                        && note.start + note.length > second.start
                        && second.chord.contains_midi(i32::from(note.pitch))
                        && !third.chord.contains_midi(i32::from(note.pitch))
                }),
                "seed {seed}: the second chord was erased by the third chord's push; first starts at {}",
                first.start.raw()
            );
        }
        assert!(exercised, "no dense chart had its third chord pushed");
    }

    #[test]
    fn a_comp_may_turn_over_in_the_sections_own_last_bar() {
        // Six bars may divide into unequal or shorter phrases. The planner's boundaries are
        // the places that may depart; an interior bar keeps the established figure.
        // Syncopation pinned for the same reason as the figure-constancy test above: a pushing
        // section moves strikes across bar lines, and this test reads bars back step for step.
        let mut departed = 0;
        for seed in 1..=8u64 {
            let mut song = chords("@axis", 6, seed, 0.0);
            song.parts[0].density = Some(0.7);
            let (frame, parts) = song.draft();
            let chords = part(&parts, "chords");
            let first = bar_steps(&frame, chords, 0);
            for bar in (1..6).filter(|bar| !frame.sections[0].closes_phrase(*bar)) {
                assert_eq!(
                    bar_steps(&frame, chords, bar),
                    first,
                    "seed {seed} changed figure at bar {bar}, which closes nothing"
                );
            }
            if bar_steps(&frame, chords, 5) != first {
                departed += 1;
            }
        }
        assert!(
            departed > 0,
            "no seed in eight turned over in the section's own last bar"
        );
    }

    #[test]
    fn a_pad_holds_what_two_chords_have_in_common_and_a_comp_strikes_it_again() {
        // The difference the two parts existed to have and did not. Both read the same harmony
        // through the same writer, so without this a pad was a comp that happened to have drawn
        // the held figure — which the comp could draw too, and did, four times in ten.
        let (frame, parts) = Song {
            seed: 3,
            ..Song::with_parts([
                PartSpec::of_role("pad", Role::Pad),
                PartSpec::of_role("comp", Role::Chords),
            ])
        }
        .draft();
        let section = &frame.sections[0];
        assert!(section.events.len() >= 4, "a chord per bar to tie across");

        // A pad's note runs past the chord it started under whenever the next chord keeps it.
        let pad = part(&parts, "pad");
        let ties = pad
            .notes
            .iter()
            .filter(|note| {
                let starts = section.chord_at(note.start - section.start);
                let ends = section.chord_at(note.start + note.length - section.start - Ticks(1));
                starts.map(|event| event.start) != ends.map(|event| event.start)
            })
            .count();
        assert!(ties > 0, "the pad restruck every voice of every chord");

        // And every one of the pad's notes begins on a chord change: it never strikes inside one.
        for note in &pad.notes {
            let at = note.start - section.start;
            assert!(
                section.events.iter().any(|event| event.start == at),
                "the pad struck at {} which is not a chord change",
                at.raw()
            );
        }
    }

    #[test]
    fn a_chord_held_over_the_bar_line_is_accompanied_like_one_written_twice() {
        // The document stores `| I | I |` as one I lasting two bars, and the accompaniment must
        // not hear the difference: every part keeps its figure going in the second bar.
        for seed in 1..=6u64 {
            let mut song = Song {
                chords: "| I | I | V | V |".into(),
                seed,
                ..Song::with_parts(
                    [Role::Chords, Role::Pad, Role::Bass, Role::Arp, Role::Kick]
                        .map(|role| PartSpec::of_role(role.name(), role)),
                )
            };
            song.settings.mood.syncopation = 0.8;
            let (frame, parts) = song.draft();
            let mut merged = frame.clone();
            let events = &mut merged.sections[0].events;
            assert_eq!(events.len(), 4, "the chart spells each bar");
            events[0].length = events[0].length + events[1].length;
            events[2].length = events[2].length + events[3].length;
            events.remove(3);
            events.remove(1);
            let again = crate::parts::write_parts(&song.settings, &song.parts, &merged);
            for name in ["chords", "pad", "bass", "arp"] {
                let notes = |drafts: &[crate::parts::PartDraft]| {
                    part(drafts, name)
                        .notes
                        .iter()
                        .map(|note| (note.start.raw(), note.pitch, note.length.raw()))
                        .collect::<Vec<_>>()
                };
                assert_eq!(notes(&parts), notes(&again), "seed {seed}: {name}");
            }
        }
    }

    #[test]
    fn a_comp_never_pushes_a_chord_that_is_already_sounding() {
        for seed in 1..=12u64 {
            let (frame, parts) = chords("| I | I | I | I |", 4, seed, 1.0).draft();
            let bar = frame.grid.bar_ticks();
            let chords = part(&parts, "chords");
            for line in 1..4 {
                let at = bar * line;
                assert!(
                    !chords
                        .notes
                        .iter()
                        .any(|note| note.start < at && note.start + note.length > at),
                    "seed {seed}: a strike ran over bar line {line} as if anticipating a change"
                );
            }
        }
    }

    /// Every chord the part strikes, as the pitches struck together at each onset.
    fn strikes(song: &Song, name: &str) -> Vec<Vec<i32>> {
        let (_, parts) = song.draft();
        let mut by_onset: std::collections::BTreeMap<i64, Vec<i32>> = Default::default();
        for note in &part(&parts, name).notes {
            by_onset
                .entry(note.start.raw())
                .or_default()
                .push(i32::from(note.pitch));
        }
        by_onset.into_values().collect()
    }

    #[test]
    fn a_chord_left_to_itself_sounds_all_of_its_tones() {
        // A triad came out as two notes in more than half of all strikes: one voicing in three
        // dropped the fifth outright, and nothing on screen could ask for the third note back.
        for role in [Role::Chords, Role::Pad] {
            for seed in 0..24u64 {
                let song = Song {
                    seed,
                    chords: "| I | vi | IV | V |".into(),
                    ..Song::with_parts([PartSpec::of_role(role.name(), role)])
                };
                let (frame, parts) = song.draft();
                let section = &frame.sections[0];
                let drafted = part(&parts, role.name());
                for event in &section.events {
                    // What sounds just after the change, which counts a pad's held voices and a
                    // pushed comp's anticipation as well as what is struck on the line.
                    let at = section.start + event.start + Ticks(1);
                    let struck = drafted
                        .notes
                        .iter()
                        .filter(|note| note.start <= at && note.start + note.length > at)
                        .count();
                    assert!(
                        struck >= 3,
                        "{role:?} seed {seed} struck {} as {struck} notes",
                        event.chord
                    );
                }
            }
        }
    }

    #[test]
    fn a_chord_part_sounds_as_many_notes_as_it_was_asked_for() {
        for role in [Role::Chords, Role::Pad] {
            for voices in auris_core::CHORD_VOICES {
                for seed in 0..6u64 {
                    let song = Song {
                        seed,
                        chords: "| I | vi7 | IVmaj7 | V7 |".into(),
                        ..Song::with_parts([Song::part(role, |part| part.voices = voices)])
                    };
                    for strike in strikes(&song, role.name()) {
                        // A pad holds the tones two chords share, so its later strikes are only
                        // the voices that moved; the count is of what sounds, which a comp
                        // restrikes in full.
                        if role == Role::Chords {
                            assert_eq!(
                                strike.len(),
                                usize::from(voices),
                                "{role:?} seed {seed}: {strike:?}"
                            );
                        } else {
                            assert!(strike.len() <= usize::from(voices), "{strike:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_voicings_average_to_the_note_asked_for() {
        for centre in [43, 52, 60, 65, 71, 84] {
            for voices in [0u8, 3, 5] {
                for seed in 0..6u64 {
                    let song = Song {
                        seed,
                        bars: 8,
                        chords: "| I | vi | IV | V | iii | vi | ii | V |".into(),
                        ..Song::with_parts([Song::part(Role::Chords, |part| {
                            part.centre = Some(centre);
                            part.voices = voices;
                        })])
                    };
                    let means: Vec<f64> = strikes(&song, "chords")
                        .iter()
                        .map(|strike| {
                            strike.iter().map(|pitch| f64::from(*pitch)).sum::<f64>()
                                / strike.len() as f64
                        })
                        .collect();
                    let overall = means.iter().sum::<f64>() / means.len() as f64;
                    assert!(
                        (overall - f64::from(centre)).abs() <= 1.5,
                        "centre {centre}, {voices} voices, seed {seed}: averaged {overall:.1}"
                    );
                    for mean in &means {
                        assert!(
                            (mean - f64::from(centre)).abs() <= 4.0,
                            "centre {centre}, {voices} voices, seed {seed}: a chord at {mean:.1}"
                        );
                    }
                }
            }
        }
    }
}
