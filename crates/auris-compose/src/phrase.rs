//! Writing one part over one stretch of a document's harmony.
//!
//! This is the composer with the song taken out of it. [`compose`](crate::compose) needs a whole
//! specification — a key, a form, a roster, a chart per section — and answers with a whole piece.
//! A person who has written a chord progression onto a timeline and wants a bass line under bars
//! nine to sixteen has none of that and should not have to invent it.
//!
//! So: a recipe, a range, and whatever chords the document already holds under that range. The
//! parts are written by exactly the same code either way — a one-section [`Frame`] is built over
//! the range and handed to [`write_parts`], which is why a phrase written here and the same part
//! written by `compose` come out of the same machinery rather than out of two that have to be
//! kept in step.

use auris_core::harmony::Harmony;
use auris_core::time::{Ticks, TimeSignature};
use auris_core::{ClipPreset, ClipRecipe, Note};

use crate::frame::{Frame, SectionPlan, skeleton};
use crate::parts::{ScoreSettings, write_parts};
use crate::rhythm::Grid;
use crate::spec::{Mood, PartSpec, Role};

/// The roles a preset writes.
///
/// One for everything except the drums, where a kick, a snare and a hat share an instrument and so
/// belong in one clip. Writing them as three parts and merging the notes is what the composer
/// already does for a full arrangement; the only difference here is that they land in one place.
pub fn roles_of(preset: ClipPreset) -> &'static [Role] {
    match preset {
        ClipPreset::Lead => &[Role::Melody],
        ClipPreset::Chords => &[Role::Chords],
        ClipPreset::Pad => &[Role::Pad],
        ClipPreset::Arp => &[Role::Arp],
        ClipPreset::Bass => &[Role::Bass],
        ClipPreset::Drums => &[Role::Kick, Role::Snare, Role::Hat],
    }
}

/// The instrument a preset's clips should be played by, when the track has none yet.
pub fn default_instrument(preset: ClipPreset) -> &'static str {
    roles_of(preset)
        .first()
        .map_or("auris.synth.chiptune", |role| role.default_instrument())
}

/// Writes one part over the harmony under `start .. start + length`.
///
/// The notes come back positioned from the *clip's* own start, which is where a
/// [`MidiClip`](auris_core::MidiClip) wants them, and sorted so that two runs of the same recipe
/// compare equal.
///
/// An empty answer is a real answer: a range with no chords written under it, or one shorter than
/// a bar, has nothing for a part to play, and inventing something would mean inventing harmony the
/// person did not ask for.
pub fn write_phrase(
    harmony: &Harmony,
    start: Ticks,
    length: Ticks,
    meter: TimeSignature,
    recipe: &ClipRecipe,
) -> Vec<Note> {
    let grid = Grid::new(meter, 4);
    let bar_ticks = grid.bar_ticks().raw().max(1);
    let bars = (length.raw().max(0) / bar_ticks) as usize;
    if bars == 0 {
        return Vec::new();
    }

    // The harmony under the range, moved so the range's start is zero — which is the frame of
    // reference a `SectionPlan` uses and, conveniently, the one a clip uses too.
    let mut events = harmony.events_in(start, start + length, meter);
    for event in &mut events {
        event.start -= start;
    }
    if events.is_empty() {
        return Vec::new();
    }

    let key = harmony.key_at(start);
    // Keyed by the preset rather than by a section name, so that changing the preset writes a
    // different part and changing the seed writes a different take of the same one.
    let section_key = recipe.preset.name();
    let skeleton = skeleton(&events, key, recipe.seed, section_key, 1);

    let frame = Frame {
        grid,
        sections: vec![SectionPlan {
            name: section_key.to_string(),
            instance: 1,
            start: Ticks::ZERO,
            length,
            bars,
            key,
            intensity: recipe.intensity.clamp(0.0, 1.0),
            events,
            skeleton,
            parts: Vec::new(),
        }],
        length,
        seed: recipe.seed,
        mood: mood_for(recipe),
    };

    let settings = ScoreSettings {
        mood: frame.mood,
        swing: recipe.swing,
        humanize: recipe.humanize.clamp(0.0, 1.0),
        // One clip is one playing of one section, so there is no repeat to depart from.
        variation: 0.0,
        groove: recipe.groove.clone(),
    };

    let roster: Vec<PartSpec> = roles_of(recipe.preset)
        .iter()
        .map(|role| {
            let mut part = PartSpec::of_role(role.name(), *role);
            // The recipe's dial, not the mood's: a person moving a slider expects that slider to
            // be what decides, rather than to be averaged with something they cannot see.
            part.density = Some(recipe.density.clamp(0.0, 1.0));
            part
        })
        .collect();

    let mut notes: Vec<Note> = write_parts(&settings, &roster, &frame)
        .into_iter()
        .flat_map(|draft| draft.notes)
        .filter(|draft| draft.start >= Ticks::ZERO && draft.start < length)
        .map(|draft| Note {
            pitch: draft.pitch.min(127),
            velocity: draft.velocity.clamp(0.0, 1.0),
            start: draft.start,
            // Truncate rather than overhang: the scheduler drops a note that runs past its clip.
            length: draft.length.min(length - draft.start).max(Ticks(1)),
        })
        .collect();
    notes.sort_by_key(|note| (note.start.raw(), note.pitch));
    notes
}

/// The mood a recipe implies.
///
/// A recipe has two dials where a [`Mood`] has four, because two is what a person can hold in
/// their head while listening. Density is passed to the parts directly, so what is left for the
/// mood to carry is how hard and how loose the playing is.
fn mood_for(recipe: &ClipRecipe) -> Mood {
    let intensity = recipe.intensity.clamp(0.0, 1.0);
    Mood {
        energy: intensity,
        syncopation: 0.15 + intensity * 0.4,
        ..Mood::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::theory::chart::Chart;
    use auris_core::theory::key::Key;

    const BAR: Ticks = Ticks(3840);

    fn four_four() -> TimeSignature {
        TimeSignature::new(4, 4)
    }

    /// Four bars of the axis progression in C major, starting at bar one.
    fn axis() -> Harmony {
        let mut harmony = Harmony::in_key(Key::parse("C major").unwrap());
        harmony.stamp(
            &Chart::parse("| I | V | vi | IV |").unwrap(),
            Ticks::ZERO,
            4,
            four_four(),
        );
        harmony
    }

    fn phrase(preset: ClipPreset, seed: u64) -> Vec<Note> {
        write_phrase(
            &axis(),
            Ticks::ZERO,
            BAR * 4,
            four_four(),
            &ClipRecipe::new(preset, seed),
        )
    }

    #[test]
    fn every_preset_writes_something_playable() {
        for preset in ClipPreset::ALL {
            let notes = phrase(preset, 1);
            assert!(!notes.is_empty(), "{} wrote nothing", preset.name());
            for note in &notes {
                assert!(
                    note.start >= Ticks::ZERO,
                    "{} before the clip",
                    preset.name()
                );
                assert!(note.end() <= BAR * 4, "{} past the clip", preset.name());
                assert!(note.velocity > 0.0 && note.velocity <= 1.0);
            }
        }
    }

    #[test]
    fn the_same_recipe_writes_the_same_phrase_every_time() {
        for preset in ClipPreset::ALL {
            assert_eq!(phrase(preset, 9), phrase(preset, 9), "{}", preset.name());
        }
    }

    /// How much two takes have in common, as a fraction of everything either of them plays.
    ///
    /// Compares which pitch lands on which sixteenth, ignoring the few ticks humanisation shakes
    /// a note by — because a listener does too. Counted over the union of both takes, so a sparse
    /// take whose notes all appear inside a busy one still counts as different.
    fn overlap(preset: ClipPreset, one: u64, other: u64) -> f32 {
        let figure = |notes: Vec<Note>| -> Vec<(u8, i64)> {
            let mut out: Vec<(u8, i64)> = notes
                .iter()
                .map(|note| (note.pitch, note.start.snap_nearest(Ticks(240)).raw()))
                .collect();
            out.sort_unstable();
            out.dedup();
            out
        };
        let (a, b) = (figure(phrase(preset, one)), figure(phrase(preset, other)));
        let shared = a.iter().filter(|entry| b.contains(entry)).count();
        let union = a.len() + b.len() - shared;
        shared as f32 / union.max(1) as f32
    }

    #[test]
    fn another_take_is_a_take_a_listener_would_call_different() {
        // The bug this pins, and it is worth spelling out because the obvious test missed it:
        // four of the six presets used to write *byte for byte the same figure* for a different
        // seed, and differ only in the ticks humanisation shook them by. `assert_ne!` passed
        // happily. To a person pressing "another take" nothing happened at all.
        //
        // So the assertion is about what is heard, averaged over several pairs of seeds. An
        // average and not each pair: a part choosing between a handful of figures will sometimes
        // draw the same one twice, and that is honest variance rather than a fault. What it
        // cannot do, and what this catches, is score the same every time.
        let ceiling = |preset: ClipPreset| match preset {
            // A bass plays the root when the chord changes — that is the job — so two takes over
            // one progression share those notes however different the rest of the line is.
            ClipPreset::Bass => 0.85,
            // A pad holds the chord. All it can vary is the register it holds it in.
            ClipPreset::Pad => 0.75,
            // A kit follows its groove, and the groove is the part somebody chose.
            ClipPreset::Drums => 0.75,
            _ => 0.55,
        };
        for preset in ClipPreset::ALL {
            let pairs = [(1u64, 2), (2, 3), (3, 4), (1, 7), (5, 9)];
            let mean: f32 = pairs
                .iter()
                .map(|(one, other)| overlap(preset, *one, *other))
                .sum::<f32>()
                / pairs.len() as f32;
            assert!(
                mean < ceiling(preset),
                "{}: two takes share {:.0}% of what they play on average",
                preset.name(),
                mean * 100.0
            );
        }
    }

    #[test]
    fn a_different_seed_is_a_different_take_of_the_same_part() {
        // Different notes, but still the same instrument doing the same job — which is what a
        // person pressing "another take" is asking for.
        let first = phrase(ClipPreset::Lead, 1);
        let second = phrase(ClipPreset::Lead, 2);
        assert_ne!(first, second);
        assert!(!second.is_empty());
    }

    #[test]
    fn a_pitched_part_plays_the_chords_underneath_it() {
        // The bass is the strictest case: it should be on the chord, so a wrong reading of the
        // harmony shows up immediately rather than hiding inside a melody's passing notes.
        //
        // Humanisation off, because it nudges notes by a few ticks and a note pushed backwards
        // over a bar line lands under the previous chord. That is correct behaviour and would
        // make this test fail for a reason that has nothing to do with what it is checking.
        let harmony = axis();
        let notes = write_phrase(
            &harmony,
            Ticks::ZERO,
            BAR * 4,
            four_four(),
            &ClipRecipe {
                humanize: 0.0,
                ..ClipRecipe::new(ClipPreset::Bass, 4)
            },
        );
        let mut checked = 0;
        for note in &notes {
            let Some(chord) = harmony.chord_at(note.start) else {
                continue;
            };
            let class = auris_core::theory::pitch::PitchClass::new(i32::from(note.pitch));
            assert!(
                chord.contains(class),
                "the bass played {class} over {chord} at tick {}",
                note.start.raw()
            );
            checked += 1;
        }
        assert!(checked > 4, "only {checked} bass notes to check");
    }

    #[test]
    fn a_range_with_no_chords_under_it_gets_no_notes() {
        // Silence is the honest answer: a part written over nothing would be a part written over
        // harmony that nobody asked for.
        let empty = Harmony::in_key(Key::parse("C major").unwrap());
        let notes = write_phrase(
            &empty,
            Ticks::ZERO,
            BAR * 4,
            four_four(),
            &ClipRecipe::new(ClipPreset::Chords, 1),
        );
        assert!(notes.is_empty());

        // And a range shorter than a bar has nowhere to put a figure.
        let short = write_phrase(
            &axis(),
            Ticks::ZERO,
            Ticks(960),
            four_four(),
            &ClipRecipe::new(ClipPreset::Chords, 1),
        );
        assert!(short.is_empty());
    }

    #[test]
    fn a_phrase_reads_the_harmony_where_it_sits_rather_than_at_the_start_of_the_song() {
        // Bars 5..9 of a song whose key changes at bar 5: the phrase must be in the new key.
        let mut harmony = axis();
        harmony.stamp(
            &Chart::parse("| I | IV |").unwrap(),
            BAR * 4,
            4,
            four_four(),
        );
        harmony
            .keys
            .set_point(BAR * 4, Key::parse("A major").unwrap());

        let notes = write_phrase(
            &harmony,
            BAR * 4,
            BAR * 4,
            four_four(),
            &ClipRecipe::new(ClipPreset::Bass, 4),
        );
        assert!(!notes.is_empty());
        for note in &notes {
            let class = auris_core::theory::pitch::PitchClass::new(i32::from(note.pitch));
            let chord = harmony
                .chord_at(BAR * 4 + note.start)
                .expect("a chord under every note of the range");
            assert!(chord.contains(class), "{class} over {chord}");
        }
    }

    fn at_density(preset: ClipPreset, density: f32) -> usize {
        write_phrase(
            &axis(),
            Ticks::ZERO,
            BAR * 4,
            four_four(),
            &ClipRecipe {
                density,
                ..ClipRecipe::new(preset, 3)
            },
        )
        .len()
    }

    #[test]
    fn the_density_dial_decides_how_busy_a_part_that_writes_its_own_figure_is() {
        let (sparse, busy) = (
            at_density(ClipPreset::Lead, 0.1),
            at_density(ClipPreset::Lead, 1.0),
        );
        assert!(
            busy > sparse * 2,
            "{busy} notes at full density against {sparse} at a tenth"
        );
    }

    #[test]
    fn the_presets_that_follow_a_pattern_do_not_read_the_density_dial_yet() {
        // Pinning what is true rather than what ought to be. Only the parts that invent their own
        // figure read the dial today; the rest take their shape from a rhythm pattern or from the
        // groove, and moving the slider does nothing to them. A person will notice, so this test
        // is here to be deleted by whoever makes it reach them.
        for preset in [ClipPreset::Chords, ClipPreset::Arp, ClipPreset::Bass] {
            assert_eq!(
                at_density(preset, 0.1),
                at_density(preset, 1.0),
                "{} moved, so the dial now reaches it",
                preset.name()
            );
        }
    }

    #[test]
    fn a_drum_kit_takes_its_shape_from_its_groove() {
        let kit = |groove: &str| {
            write_phrase(
                &axis(),
                Ticks::ZERO,
                BAR * 4,
                four_four(),
                &ClipRecipe {
                    groove: groove.to_string(),
                    ..ClipRecipe::new(ClipPreset::Drums, 3)
                },
            )
            .len()
        };
        assert!(kit("sixteen-beat") > kit("sparse"));
        // A groove nobody has heard of leaves the kit silent rather than guessing at one.
        assert_eq!(kit("bossa-nova-from-mars"), 0);
    }
}
