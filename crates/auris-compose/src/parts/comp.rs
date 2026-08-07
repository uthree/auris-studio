//! Chords, either comped in rhythm or held as a pad.
//!
//! One file for three roles — chords, pad and stab — because they are one writer with its dials
//! set differently, and splitting them would mean three copies of the voice leading. What the
//! file is really about is the six figures a keyboard player actually comps: they are written
//! against the beat rather than against a note value, so the same six mean the same six things on
//! whatever grid the part divides its beats into.

use auris_core::time::Ticks;

use crate::frame::{Frame, SectionPlan};
use crate::rng::{Key as RngKey, Rng};
use crate::spec::{PartSpec, Role};
use crate::theory::pitch::{OCTAVE, fold_into};

use super::writer::{bar_onsets, bar_stream, density, part_grid, phrase_shape, velocity};
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
    ///
    /// It used to be a chord on literally every step, which was reachable and too plain to use:
    /// sixteen identical strikes in a bar is a tremolo, not a part. Rolling it the way the melody
    /// rolls its figure keeps the density and buys back a shape.
    Rolled,
}

/// How often the last bar of a four-bar phrase departs from the figure the section chose.
///
/// A turnaround, not a new part. Somewhere to put it is worth having — the fourth bar is where a
/// phrase turns over, and it is the one place a change reads as intent rather than as drift.
const TURNAROUND: f32 = 0.45;

/// Draws one comping figure, weighted by how busy the part was asked to be.
///
/// Sparse reaches for the held chord, busy for the offbeats and the rolled figure. Every figure
/// keeps some weight, because a dial that forbids a choice outright makes every section the same
/// again.
fn pick_figure(rng: &mut Rng, busy: f32) -> CompFigure {
    const FIGURES: [CompFigure; 6] = [
        CompFigure::Held,
        CompFigure::Beats,
        CompFigure::Charleston,
        CompFigure::Offbeats,
        CompFigure::Cross,
        CompFigure::Rolled,
    ];
    FIGURES[rng
        .weighted(&[
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
        ])
        .min(FIGURES.len() - 1)]
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
    let (low, high) = part.range();
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

    // How the part sits, decided once for the section. A pad has no rhythm to vary, so this is
    // the whole of what makes one take of it differ from another: which octave it sits in, and
    // which notes of the chord it chooses to sound.
    let mut choose = bar_stream(settings, frame, part, section, "register", 0);
    let register = (choose.below(3) as i32 - 1) * OCTAVE;
    // How many notes of the chord sound, weighted by how busy the part was asked to be. This is
    // the whole of what the density dial can reach on a pad, which holds one chord and has no
    // rhythm to thin or thicken.
    let busy = density(settings, part, section);
    let voicing_variant = choose.weighted(&[1.0, 0.2 + (1.0 - busy) * 1.8, 0.2 + busy * 1.8]);

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
    let chosen_figure = if written {
        // The written rhythm plays through the rolled figure's machinery, which is already
        // "strike on exactly these steps".
        CompFigure::Rolled
    } else if pad {
        CompFigure::Held
    } else {
        pick_figure(&mut invent, busy)
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

    for event in &section.events {
        // Voiced upward from a floor, so a ninth sounds an octave and a tone above the root
        // rather than being folded into the triad as a second. The floor is whichever octave
        // leaves the chord nearest to where the last one sat — as much voice leading as a part
        // that plays whole chords can honestly claim.
        let centre = if previous.is_empty() {
            (low + high) / 2 + register
        } else {
            previous.iter().sum::<i32>() / previous.len() as i32
        };
        let mut voicing: Vec<i32> = Vec::new();
        let mut best_distance = i32::MAX;
        for octave in -1..=2 {
            let candidate = event.chord.voiced_from(low + octave * OCTAVE);
            if candidate.iter().any(|pitch| *pitch < low || *pitch > high) {
                continue;
            }
            let middle = candidate.iter().sum::<i32>() / candidate.len().max(1) as i32;
            if (middle - centre).abs() < best_distance {
                best_distance = (middle - centre).abs();
                voicing = candidate;
            }
        }
        // Nothing fits the window — an extended chord in a narrow range — so fold each note into
        // it and accept that the spacing suffers.
        if voicing.is_empty() {
            voicing = event
                .chord
                .classes()
                .iter()
                .map(|class| fold_into(class.midi(4), low, high))
                .collect();
        }
        voicing.sort_unstable();
        voicing.dedup();
        // Which notes of the chord actually sound. A player choosing what to leave out is most of
        // what makes one voicing different from another, and for a pad it is nearly all of it.
        match voicing_variant {
            // Drop the fifth: the note the bass is most likely to be covering anyway. A plain
            // triad comes down to root and third, which is a shell voicing and a real thing to
            // play — the guard used to be `> 3`, which made this a no-op on every triad and so
            // on most of what gets written. That left one of the three variants doing nothing,
            // and the density dial with almost nothing to reach on a part that holds one chord.
            1 if voicing.len() > 2 => {
                voicing.remove(2);
            }
            // Double the root an octave up, for a wider chord.
            2 => {
                if let Some(root) = voicing.first().copied()
                    && root + OCTAVE <= high
                {
                    voicing.push(root + OCTAVE);
                }
            }
            _ => {}
        }
        previous.clone_from(&voicing);

        // Which rhythm the chord is struck on. Chosen per bar from the section's own stream, so a
        // repeat of the section comps the same way and a different seed comps differently — the
        // whole of this part used to be one fixed pattern, which made "another take" a button
        // that could not do anything.
        let bar = grid.step_of(event.start) / grid.steps_per_bar().max(1);
        // Four bars is the phrase almost everything is built in, and the fourth is where one
        // turns over. It is the only bar allowed to depart, and only sometimes: anywhere else a
        // change reads as the part losing its place rather than as a player finishing a thought.
        // `variation` reaches this through `bar_stream`, so a repeat can turn around differently.
        let figure = if pad || written || bar % 4 != 3 {
            chosen_figure
        } else {
            let mut rng = bar_stream(settings, frame, part, section, "comp", bar);
            if rng.chance(TURNAROUND) {
                pick_figure(&mut rng, busy)
            } else {
                chosen_figure
            }
        };

        let onsets: Vec<usize> = if figure == CompFigure::Held {
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
                    match figure {
                        CompFigure::Beats => at.is_multiple_of(beat),
                        CompFigure::Offbeats => at % beat == half,
                        CompFigure::Charleston => at == 0 || at == beat + half,
                        CompFigure::Cross => cross.at(at).is_some(),
                        CompFigure::Rolled => rolled.iter().any(|(step, _)| *step == at),
                        CompFigure::Held => false,
                    }
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
        let held = pad || figure == CompFigure::Held;
        let last = onsets.len().saturating_sub(1);
        let mut still_sounding: Vec<(i32, usize)> = Vec::new();

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
            let length = (next - at).min(event.end() - at).max(Ticks(1));
            let weight = grid.weight(grid.step_of(at));
            for pitch in &voicing {
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
                    start: section.start + at,
                    length,
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
    use crate::parts::fixture::{bar_steps, draft, part};

    #[test]
    fn a_comp_at_full_density_is_dense_without_being_a_metronome() {
        // Two claims, and the second is the one that was learned the hard way. The four original
        // figures topped out at the offbeat eighth, so no setting anywhere reached a comp that
        // moved on sixteenths — but the first fix was a chord on *literally* every step, which is
        // a tremolo rather than a part and was too plain to use. What the top of the dial should
        // give is most of the steps, with the holes that make it a rhythm.
        let full = |seed: u64| {
            format!(
                r#"
                    form = "verse"
                    chords = "@axis"
                    humanize = 0
                    seed = {seed}
                    [section.verse]
                    bars = 4
                    intensity = 1.0
                    [[part]]
                    name = "chords"
                    density = 1.0
                    "#
            )
        };
        let mut counts = Vec::new();
        for seed in 1..=8 {
            let (_, frame, parts) = draft(&full(seed));
            let chords = part(&parts, "chords");
            counts.extend((0..4).map(|bar| bar_steps(&frame, chords, bar).len()));
        }
        let steps = 16;
        let busiest = counts.iter().copied().max().unwrap_or(0);
        assert!(
            busiest > steps / 2,
            "the busiest bar in thirty-two struck {busiest} of {steps} steps at full density"
        );
        assert!(
            counts.iter().any(|count| *count < steps),
            "every bar struck every step, which is a tremolo rather than a rhythm"
        );
    }

    #[test]
    fn a_comp_keeps_one_figure_through_a_section_and_turns_it_over_at_the_end() {
        // A keyboard player picks a feel and keeps it. Drawing again every bar was four bars of
        // four different players, and left the section with nothing an ear could hold on to —
        // the same mistake the melody used to make, and the same fix. Only the fourth bar of a
        // phrase may depart, because that is where a phrase turns over.
        let mut steady = 0;
        for seed in 1..=8u64 {
            let (_, frame, parts) = draft(&format!(
                r#"
                    form = "verse"
                    chords = "@axis"
                    humanize = 0
                    variation = 0
                    seed = {seed}
                    [section.verse]
                    bars = 8
                    [[part]]
                    name = "chords"
                    density = 0.8
                    "#
            ));
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
    fn a_pad_holds_what_two_chords_have_in_common_and_a_comp_strikes_it_again() {
        // The difference the two parts existed to have and did not. Both read the same harmony
        // through the same writer, so without this a pad was a comp that happened to have drawn
        // the held figure — which the comp could draw too, and did, four times in ten.
        let roster = r#"
            form = "verse"
            chords = "@axis"
            humanize = 0
            seed = 3
            [section.verse]
            bars = 4
            [[part]]
            name = "pad"
            role = "pad"
            [[part]]
            name = "comp"
            role = "chords"
            "#;
        let (_, frame, parts) = draft(roster);
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
}
