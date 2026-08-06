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
use auris_core::time::{SignatureMap, TempoMap, Ticks, TimeSignature};
use auris_core::{ClipPreset, ClipRecipe, Note, Subdivision};

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
        ClipPreset::Stab => &[Role::Stab],
        ClipPreset::Bass => &[Role::Bass],
        ClipPreset::Drums => &[Role::Kick, Role::Snare, Role::Hat],
        ClipPreset::Kick => &[Role::Kick],
        ClipPreset::Snare => &[Role::Snare],
        ClipPreset::Hat => &[Role::Hat],
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
/// `section` is the label of the timeline stretch the clip sits in, with which occurrence of
/// that label it is — the composer's hint. Every stream the part draws from is keyed by it, so
/// two clips written into stretches both called サビ draw the same figures, and a clip in the
/// *second* サビ varies exactly as a repeated section does under [`compose`](crate::compose):
/// the motif holds, the details may turn. With no section named, the preset keys the streams as
/// before, so an unstructured timeline writes what it always wrote.
///
/// `tempo` is the one in force at `start`, in beats per minute — the caller reads it off the
/// document's tempo map, exactly as it reads `meter` off the signature map. A clip is written in
/// ticks and would not otherwise care how fast they go by, but the humanisation dial is a *time*:
/// [`ScoreSettings::humanize`](crate::parts::ScoreSettings::humanize) asks for a wander of so many
/// milliseconds, and nothing can turn that into ticks without knowing the tempo. Passing the
/// wrong one does not write different notes; it writes them with the wrong amount of looseness,
/// which at 64 BPM is nearly twice what the dial asked for.
///
/// One number for the whole clip, even where the map changes half way through it. A clip is one
/// figure played once — the same reason it takes one `meter` — and a wander that widened in the
/// middle of a bar would be a change of feel that nothing on the panel accounts for. The tempo it
/// begins at is the one a person hears it counted in.
///
/// An empty answer is a real answer: a range with no chords written under it, or one shorter than
/// a bar, has nothing for a part to play, and inventing something would mean inventing harmony the
/// person did not ask for.
pub fn write_phrase(
    harmony: &Harmony,
    start: Ticks,
    length: Ticks,
    meter: TimeSignature,
    tempo: f64,
    recipe: &ClipRecipe,
    section: Option<(&str, usize)>,
) -> Vec<Note> {
    // The reference grid: the meter at the default subdivision. It decides the bar and the drums,
    // both of which every subdivision agrees on. Which grid a part's own figures land on is the
    // part's business, and is set on the roster below.
    let grid = Grid::new(meter, Subdivision::default().steps_per_beat());
    let bar_ticks = grid.bar_ticks().raw().max(1);
    let bars = (length.raw().max(0) / bar_ticks) as usize;
    if bars == 0 {
        return Vec::new();
    }

    // The harmony under the range, moved so the range's start is zero — which is the frame of
    // reference a `SectionPlan` uses and, conveniently, the one a clip uses too.
    //
    // One meter for the whole range rather than the document's map: `Grid` above has already
    // baked this meter into every figure the parts will play, so a clip is written in the meter
    // it begins in and a change part way through it does not reshape the bars behind it. The
    // caller passes the signature in force at `start`.
    let mut events = harmony.events_in(start, start + length, &SignatureMap::constant(meter));
    for event in &mut events {
        event.start -= start;
    }
    if events.is_empty() {
        return Vec::new();
    }

    // The key at the range's start goes on the section plan, but nothing melodic hangs off it
    // any more: the skeleton and the melody's scale walk both read each event's own key, which
    // is what lets a modulation inside the range move the scale at the tick it moves the chords.
    let key = harmony.key_at(start);
    // Keyed by the timeline's own section when it names one — the label is the composer's
    // hint, and what makes two clips in stretches both called サビ recognisably one idea — and
    // by the preset otherwise, so that changing the preset writes a different part and an
    // unstructured timeline writes what it always wrote.
    let (section_key, instance) = match section {
        Some((label, instance)) => (label, instance.max(1)),
        None => (recipe.preset.name(), 1),
    };
    let skeleton = skeleton(&events, recipe.seed, section_key, instance);

    let frame = Frame {
        grid,
        sections: vec![SectionPlan {
            name: section_key.to_string(),
            instance,
            start: Ticks::ZERO,
            length,
            bars,
            key,
            // Held to the range a timeline can hold rather than taken on trust. This is the one
            // setting that does not come off the recipe, so a caller with no map to read is the
            // way a nonsense tempo gets in, and `TempoMap` is where what counts as nonsense is
            // decided.
            tempo: tempo.clamp(TempoMap::MIN_BPM, TempoMap::MAX_BPM),
            intensity: recipe.intensity.clamp(0.0, 1.0),
            events,
            skeleton,
            parts: Vec::new(),
        }],
        length,
        seed: recipe.seed,
        mood: mood_for(recipe),
        // A clip is one bar of an arrangement rather than the end of a piece: whatever follows
        // it, including another playing of itself, is something for the last bar to lead into.
        joins_on: true,
    };

    let settings = ScoreSettings {
        mood: frame.mood,
        swing: recipe.swing,
        humanize: recipe.humanize.clamp(0.0, 1.0),
        dynamics: recipe.dynamics.clamp(0.0, 1.0),
        fill: recipe.fill.clamp(0.0, 1.0),
        // One clip is one playing of one section, so there is no repeat to depart from.
        variation: 0.0,
        groove: recipe.groove.clone(),
    };

    let roster: Vec<PartSpec> = roles_of(recipe.preset)
        .iter()
        .map(|role| {
            let mut part = PartSpec::of_role(role.name(), *role);
            // A register somebody asked for, on top of the one the role implies. The one the
            // part chooses for itself is drawn from the seed, which is right for a take and no
            // use at all when the answer wanted is "the same thing, an octave up".
            part.octave += recipe.octave.clamp(-2, 2);
            // The recipe's dial, not the mood's: a person moving a slider expects that slider to
            // be what decides, rather than to be averaged with something they cannot see.
            part.density = Some(recipe.density.clamp(0.0, 1.0));
            // Likewise the recipe's, and not the role's default: choosing the stab preset is what
            // set them, and a dial that snapped back to the role's idea would be one that undid
            // the choice the moment anything else on the panel was touched.
            part.subdivision = recipe.subdivision;
            part.gate = recipe.gate;
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
    Mood {
        energy: recipe.intensity.clamp(0.0, 1.0),
        // The recipe's own, not a number derived from how hard the part is played. Deriving it
        // was defensible while only the melody read it; the comping figures read it now, so
        // "square" and "awkward" is a choice somebody should be able to make on its own.
        syncopation: recipe.syncopation.clamp(0.0, 1.0),
        ..Mood::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::theory::chart::Chart;
    use auris_core::theory::key::Key;

    const BAR: Ticks = Ticks(3840);

    /// The tempo every test that is not about the tempo writes at.
    ///
    /// Nothing but the humanisation reads it, so it is the same 120 the rest of the workspace
    /// defaults to and no test below has to think about it.
    const TEMPO: f64 = 120.0;

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
            TEMPO,
            &ClipRecipe::new(preset, seed),
            None,
        )
    }

    #[test]
    fn a_single_drum_voice_writes_only_itself() {
        // What the whole kit could not be asked for one piece at a time. A kit on one track is
        // three voices no fader can separate; three tracks is a mix.
        let pitches = |preset| {
            let mut out: Vec<u8> = phrase(preset, 3).iter().map(|note| note.pitch).collect();
            out.sort_unstable();
            out.dedup();
            out
        };
        assert_eq!(pitches(ClipPreset::Kick), vec![36]);
        assert_eq!(pitches(ClipPreset::Snare), vec![38]);
        assert_eq!(pitches(ClipPreset::Hat), vec![42]);
        // And together they are the kit: the same three voices the one preset writes at once.
        assert_eq!(pitches(ClipPreset::Drums), vec![36, 38, 42]);
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

    /// How many takes a timing measurement is pooled over.
    ///
    /// One four-bar lead holds about twenty notes, which is not enough to measure a spread to the
    /// few per cent asked for below. Different seeds rather than one seed measured harder: a seed
    /// decides which figure is played, and the notes of one figure sit on a handful of steps.
    const TAKES: u64 = 24;

    /// How far every note of `preset` moved from where a machine would have put it, in
    /// milliseconds, pooled over [`TAKES`] takes.
    ///
    /// The technique is `parts::tests::displacements`, which measures the same thing for a whole
    /// song. The same recipe at `humanize` 0 is the machine: the dial reaches the timing and the
    /// strength of the stroke and nothing else, so the two takes hold the same notes and each one
    /// pairs with itself. Paired by pitch and then by time, because humanisation moves a note and
    /// cannot repitch it, whereas the order a clip comes back in — by time, then by pitch — is
    /// exactly the one two notes struck together can be swapped in.
    fn displacements(preset: ClipPreset, tempo: f64, humanize: f32) -> Vec<f32> {
        // A tick is a 960th of a quarter note, and a quarter note is 60000/tempo milliseconds.
        let ms_per_tick = 62.5 / tempo as f32;
        let take = |seed: u64, humanize: f32| {
            let mut notes = write_phrase(
                &axis(),
                Ticks::ZERO,
                BAR * 4,
                four_four(),
                tempo,
                &ClipRecipe {
                    humanize,
                    ..ClipRecipe::new(preset, seed)
                },
                None,
            );
            notes.sort_by_key(|note| (note.pitch, note.start.raw()));
            notes
        };
        let mut out = Vec::new();
        for seed in 0..TAKES {
            let played = take(seed, humanize);
            let written = take(seed, 0.0);
            assert_eq!(
                played.len(),
                written.len(),
                "seed {seed}: humanising changed how many notes {} plays",
                preset.name()
            );
            for (played, written) in played.iter().zip(&written) {
                assert_eq!(played.pitch, written.pitch, "seed {seed}: mispaired");
                out.push((played.start - written.start).raw() as f32 * ms_per_tick);
            }
        }
        out
    }

    /// The standard deviation of a set of displacements, about their own mean.
    ///
    /// About the mean rather than about zero, because a part's constant lean is not wander: a
    /// melody sits the same few ticks ahead of the beat in every bar of the clip, and what a
    /// listener hears as loose is the scatter around that rather than where the middle of it sits.
    fn spread(values: &[f32]) -> f32 {
        assert!(!values.is_empty(), "nothing was measured");
        let mean = values.iter().sum::<f32>() / values.len() as f32;
        let variance =
            values.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / values.len() as f32;
        variance.sqrt()
    }

    #[test]
    fn one_clip_is_the_same_feel_at_any_tempo() {
        // What the tempo travels down here for, measured the way
        // `parts::tests::the_same_dial_is_the_same_feel_at_any_tempo` measures it for a whole
        // song. The clip was the one place the promise did not hold: `write_phrase` had no tempo
        // to build a `ScoreSettings` from and wrote 120 into it, so a clip generated in a 64 BPM
        // project wandered by 120/64 of what was asked — 7.0 ms at the recipe's own default of
        // 0.25, where the dial asks for 3.75. Two projects, one dial, two amounts of looseness,
        // and nothing on the panel to explain the difference.
        //
        // Three times apart, which spans everything the presets ask for and then some. The two
        // takes hold the same notes and draw the same numbers out of the same streams, because
        // the wander is keyed by where a note is written rather than by the clock; all the tempo
        // changes is what those numbers are multiplied by. So this is not two samples of one
        // distribution being compared, and the tolerance does not have to cover sampling error.
        for preset in [
            ClipPreset::Lead,
            ClipPreset::Chords,
            ClipPreset::Arp,
            ClipPreset::Bass,
        ] {
            let slow = spread(&displacements(preset, 60.0, 0.5));
            let fast = spread(&displacements(preset, 180.0, 0.5));
            // Five per cent, and the same reasoning as the song's: what is left between the two
            // is rounding a displacement onto a whole tick, which at 60 BPM is a grid of 1.04 ms
            // and at 180 one of 0.35, and rounding onto a grid of width w adds w²/12 to a
            // variance of about 51. Measured, the four presets disagree by under half a per
            // cent. Five per cent leaves room for the note or two per clip that lean off the
            // front and are held at zero, which is the one displacement a tempo can truncate.
            assert!(
                (slow - fast).abs() < 0.05 * slow.max(fast),
                "{}: {slow:.2} ms at 60 BPM against {fast:.2} ms at 180",
                preset.name()
            );
        }

        // And the wander is the one that was asked for, rather than merely the same at both
        // tempos. 7.5 ms is `parts`' own `WANDER_MS` of 15 at half the dial; the number is
        // written out here because that constant is private to the module that decides it, and a
        // clip agreeing with it is exactly what is at stake — a phrase is meant to be written by
        // the same machinery as a whole song and to feel like it.
        //
        // Fifteen per cent, which is what the song's test allows and for the same reasons: the
        // jitter is clamped at three sigma, a note that leans off the front of the clip is held
        // at tick zero, and twenty notes a take is a sample rather than a distribution. Measured,
        // the lead sits at 7.55 ms and the comp, which is not asserted on here, at 7.12.
        for (tempo, measured) in [
            (60.0, spread(&displacements(ClipPreset::Lead, 60.0, 0.5))),
            (180.0, spread(&displacements(ClipPreset::Lead, 180.0, 0.5))),
        ] {
            assert!(
                (measured - 7.5).abs() < 7.5 * 0.15,
                "{tempo} BPM: a clip wandered by {measured:.2} ms against the 7.5 asked for"
            );
        }
    }

    #[test]
    fn a_tempo_no_timeline_could_hold_is_held_to_one_that_could() {
        // `tempo` is the one setting that does not come off the recipe, so a caller with nothing
        // to read it from is how a zero gets in — and a zero would multiply the wander to nothing
        // and quietly turn the dial off. It is held to what `TempoMap` accepts instead, which is
        // the same range the document itself is held to.
        let at = |tempo: f64| {
            write_phrase(
                &axis(),
                Ticks::ZERO,
                BAR * 4,
                four_four(),
                tempo,
                &ClipRecipe::new(ClipPreset::Lead, 3),
                None,
            )
        };
        assert_eq!(at(0.0), at(TempoMap::MIN_BPM), "a floor, not a silence");
        assert_eq!(at(1e9), at(TempoMap::MAX_BPM), "and a ceiling above it");
        // The floor is a tempo and not a machine: something still moves at it.
        assert_ne!(
            at(0.0),
            write_phrase(
                &axis(),
                Ticks::ZERO,
                BAR * 4,
                four_four(),
                TEMPO,
                &ClipRecipe {
                    humanize: 0.0,
                    ..ClipRecipe::new(ClipPreset::Lead, 3)
                },
                None,
            ),
            "the clamped tempo humanised by nothing at all"
        );
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
            // A pad holds the chord. All it can vary is the register and which notes of the chord
            // it sounds, so two takes of one are alike by design — it is a texture, not a part
            // with a figure in it. The ceiling is here to catch a pad that never changes at all,
            // which is what it used to do, and not to demand one that changes a lot.
            ClipPreset::Pad => 0.92,
            // A kit follows its groove, and the groove is the part somebody chose. These two
            // ceilings used to be 0.75 and 0.85, and they were only that low because the kit was
            // being *sampled* rather than played: a third of the hits the groove asked for were
            // dropped at the neutral dial, and it was that dropping — not any musical decision —
            // that made one take differ from the next. Playing the groove costs most of it back.
            // Measured now: kit 84 %, kick 83, snare 87, hat 79. What still makes two takes
            // different is the melody, the voicings, the fills and which weak steps survive.
            ClipPreset::Drums => 0.90,
            // One drum voice on its own has less room still. A lone snare is a backbeat, and a
            // backbeat played differently is a different groove rather than another take of this
            // one. The ceiling is here to catch a voice that never changes at all, not to demand
            // one that changes as much as a melody.
            ClipPreset::Kick | ClipPreset::Snare | ClipPreset::Hat => 0.92,
            _ => 0.55,
        };
        // Every offender, not the first: with ten presets a stop at the first hides the rest, and
        // the number each one scores is what a person needs to judge whether a ceiling is right.
        let over: Vec<String> = ClipPreset::ALL
            .into_iter()
            .filter_map(|preset| {
                let pairs = [(1u64, 2), (2, 3), (3, 4), (1, 7), (5, 9)];
                let mean: f32 = pairs
                    .iter()
                    .map(|(one, other)| overlap(preset, *one, *other))
                    .sum::<f32>()
                    / pairs.len() as f32;
                (mean >= ceiling(preset)).then(|| {
                    format!(
                        "{} shares {:.0}% (ceiling {:.0}%)",
                        preset.name(),
                        mean * 100.0,
                        ceiling(preset) * 100.0
                    )
                })
            })
            .collect();
        assert!(
            over.is_empty(),
            "two takes are too alike: {}",
            over.join(", ")
        );
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
            TEMPO,
            &ClipRecipe {
                humanize: 0.0,
                ..ClipRecipe::new(ClipPreset::Bass, 4)
            },
            None,
        );
        let key = harmony.key_at(Ticks::ZERO);
        let mut checked = 0;
        for note in &notes {
            let Some(chord) = harmony.chord_at(note.start) else {
                continue;
            };
            let class = auris_core::theory::pitch::PitchClass::new(i32::from(note.pitch));
            // In the chord, or at least in the key. A bass line stepping into the next chord
            // passes through a note the current chord does not contain, which is a bass line
            // rather than a wrong note — but it stays inside the key, which every part here does.
            assert!(
                chord.contains(class) || key.scale.contains(key.tonic, class),
                "the bass played {class} over {chord} in {} at tick {}",
                key.to_text(),
                note.start.raw()
            );
            checked += 1;
        }
        assert!(checked > 4, "only {checked} bass notes to check");
    }

    #[test]
    fn a_stab_is_a_short_chord_struck_often() {
        // The preset exists to be chosen rather than dialled in, so what it writes with nobody
        // touching a dial is the whole of what it is worth. Two properties, and it is the second
        // that names it: many strikes, each one released before the next arrives.
        let stab = phrase(ClipPreset::Stab, 3);
        let chords = phrase(ClipPreset::Chords, 3);
        assert!(
            stab.len() > chords.len(),
            "a stab wrote {} notes against a comp's {}",
            stab.len(),
            chords.len()
        );

        let mut by_pitch: std::collections::BTreeMap<u8, Vec<&Note>> =
            std::collections::BTreeMap::new();
        for note in &stab {
            by_pitch.entry(note.pitch).or_default().push(note);
        }
        let mut checked = 0;
        for (pitch, notes) in &by_pitch {
            for pair in notes.windows(2) {
                assert!(
                    pair[0].end() <= pair[1].start,
                    "{pitch} was still sounding at {} when it was struck again",
                    pair[1].start.raw()
                );
                checked += 1;
            }
        }
        assert!(checked > 8, "only {checked} pairs of notes to check");
    }

    #[test]
    fn a_triplet_recipe_writes_notes_a_straight_one_cannot() {
        // The dial is the reason the grid became a setting: a third of a beat is 320 ticks, and
        // nothing a sixteenth grid can reach lands between two of them.
        //
        // Over several seeds, because a clip draws one figure for the whole of itself: a seed
        // that draws the held chord puts every note on a downbeat, which is on both grids at
        // once and a perfectly good comp. What must be true is that the setting is reachable.
        let triplet = |seed: u64| {
            write_phrase(
                &axis(),
                Ticks::ZERO,
                BAR * 4,
                four_four(),
                TEMPO,
                &ClipRecipe {
                    humanize: 0.0,
                    density: 1.0,
                    subdivision: auris_core::Subdivision::EighthTriplet,
                    ..ClipRecipe::new(ClipPreset::Chords, seed)
                },
                None,
            )
        };
        let mut off_the_straight_grid = 0;
        for seed in 1..=8u64 {
            let notes = triplet(seed);
            assert!(!notes.is_empty(), "seed {seed} wrote nothing");
            assert!(
                notes.iter().all(|note| note.start.raw() % 320 == 0),
                "seed {seed} landed off its own grid"
            );
            if notes.iter().any(|note| note.start.raw() % 240 != 0) {
                off_the_straight_grid += 1;
            }
        }
        assert!(
            off_the_straight_grid > 0,
            "not one of eight seeds put a note where a straight grid could not reach"
        );
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
            TEMPO,
            &ClipRecipe::new(ClipPreset::Chords, 1),
            None,
        );
        assert!(notes.is_empty());

        // And a range shorter than a bar has nowhere to put a figure.
        let short = write_phrase(
            &axis(),
            Ticks::ZERO,
            Ticks(960),
            four_four(),
            TEMPO,
            &ClipRecipe::new(ClipPreset::Chords, 1),
            None,
        );
        assert!(short.is_empty());
    }

    #[test]
    fn a_section_label_keys_the_material() {
        // Two stretches with identical harmony: the label decides whether they are the same
        // idea. Same label and occurrence, same take; different labels, different figures; no
        // label at all, exactly what the preset alone always wrote.
        let mut harmony = axis();
        harmony.stamp(
            &Chart::parse("| I | V | vi | IV |").unwrap(),
            BAR * 4,
            4,
            four_four(),
        );
        let recipe = ClipRecipe {
            humanize: 0.0,
            ..ClipRecipe::new(ClipPreset::Lead, 5)
        };
        let write = |start: Ticks, section: Option<(&str, usize)>| {
            write_phrase(
                &harmony,
                start,
                BAR * 4,
                four_four(),
                TEMPO,
                &recipe,
                section,
            )
        };

        assert_eq!(
            write(Ticks::ZERO, Some(("サビ", 1))),
            write(BAR * 4, Some(("サビ", 1))),
            "the same label over the same harmony is the same take, wherever it sits"
        );
        assert_ne!(
            write(Ticks::ZERO, Some(("Aメロ", 1))),
            write(Ticks::ZERO, Some(("サビ", 1))),
            "a different label is a different idea"
        );
        // Nothing is asserted between occurrence 1 and occurrence 2 on purpose: a clip writes
        // with `variation: 0.0` — one clip is one playing — so the second サビ comes out the
        // same take, give or take the skeleton's arch, exactly as a compose repeat without
        // variation would. Which is what 「2番のサビ」 usually means.
        assert_eq!(
            write(Ticks::ZERO, None),
            write(Ticks::ZERO, Some((recipe.preset.name(), 1))),
            "no label keys by the preset, exactly as before"
        );
    }

    #[test]
    fn a_stretch_with_no_chords_is_silent_in_the_melody_too() {
        // The comp, the bass and the arp walk the events and always fell silent over a hole;
        // the melody asked `chord_at`, which used to answer with the nearest chord it could
        // find — and played over bars the person deliberately left empty. Both flavours of
        // hole: bars before the first chord, and a stretch cleared out of the middle.
        let mut leading = Harmony::in_key(Key::parse("C major").unwrap());
        leading.stamp(&Chart::parse("| I | V |").unwrap(), BAR * 2, 2, four_four());

        let mut cleared = axis();
        cleared.chords.clear_range(BAR, BAR * 3);

        for (name, harmony, silent_from, silent_to) in [
            ("leading", &leading, Ticks::ZERO, BAR * 2),
            ("cleared", &cleared, BAR, BAR * 3),
        ] {
            for seed in 1..=4u64 {
                let notes = write_phrase(
                    harmony,
                    Ticks::ZERO,
                    BAR * 4,
                    four_four(),
                    TEMPO,
                    &ClipRecipe {
                        humanize: 0.0,
                        ..ClipRecipe::new(ClipPreset::Lead, seed)
                    },
                    None,
                );
                assert!(
                    !notes.is_empty(),
                    "{name}: seed {seed} wrote nothing at all"
                );
                for note in &notes {
                    assert!(
                        note.start < silent_from || note.start >= silent_to,
                        "{name}: seed {seed} put a note at tick {} where no chord sounds",
                        note.start.raw()
                    );
                }
            }
        }
    }

    #[test]
    fn a_key_change_inside_the_range_moves_the_scale_with_it() {
        // The other key test puts the change exactly at the range's start — the one position a
        // key frozen at the start got right. Here it lands in the middle, where the melody's
        // weak steps used to walk the old key's scale over the new key's chords: one degree up
        // from a B anchor in C major is C natural, which E major does not contain.
        let mut harmony = axis();
        harmony.stamp(
            &Chart::parse("| I | IV |").unwrap(),
            BAR * 4,
            4,
            four_four(),
        );
        harmony
            .keys
            .set_point(BAR * 4, Key::parse("E major").unwrap());

        let mut after_the_change = 0;
        for seed in 1..=4u64 {
            let notes = write_phrase(
                &harmony,
                Ticks::ZERO,
                BAR * 8,
                four_four(),
                TEMPO,
                &ClipRecipe {
                    humanize: 0.0,
                    ..ClipRecipe::new(ClipPreset::Lead, seed)
                },
                None,
            );
            for note in &notes {
                let key = harmony.key_at(note.start);
                let chord = harmony
                    .chord_at(note.start)
                    .expect("a chord under every note of the range");
                let class = auris_core::theory::pitch::PitchClass::new(i32::from(note.pitch));
                assert!(
                    chord.contains(class) || key.scale.contains(key.tonic, class),
                    "seed {seed} played {class} over {chord} in {} at tick {}",
                    key.to_text(),
                    note.start.raw()
                );
                if note.start >= BAR * 4 {
                    after_the_change += 1;
                }
            }
        }
        assert!(
            after_the_change > 8,
            "only {after_the_change} notes after the modulation to judge"
        );
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
            TEMPO,
            &ClipRecipe::new(ClipPreset::Bass, 4),
            None,
        );
        assert!(!notes.is_empty());
        let key = harmony.key_at(BAR * 4);
        assert_eq!(key.to_text(), "A major", "the phrase reads the new key");
        for note in &notes {
            let class = auris_core::theory::pitch::PitchClass::new(i32::from(note.pitch));
            let chord = harmony
                .chord_at(BAR * 4 + note.start)
                .expect("a chord under every note of the range");
            assert!(
                chord.contains(class) || key.scale.contains(key.tonic, class),
                "{class} over {chord} in {}",
                key.to_text()
            );
        }
    }

    /// How many notes a preset writes at a density, averaged over several takes.
    ///
    /// Averaged because the dial weighs a choice rather than making it: at a low setting a part
    /// reaches more often for the sparse figure, not always. One seed would be measuring which
    /// figure that seed happened to draw.
    fn at_density(preset: ClipPreset, density: f32) -> f32 {
        let seeds = 1..=8u64;
        let count = seeds.clone().count() as f32;
        seeds
            .map(|seed| {
                write_phrase(
                    &axis(),
                    Ticks::ZERO,
                    BAR * 4,
                    four_four(),
                    TEMPO,
                    &ClipRecipe {
                        density,
                        ..ClipRecipe::new(preset, seed)
                    },
                    None,
                )
                .len() as f32
            })
            .sum::<f32>()
            / count
    }

    #[test]
    fn the_density_dial_reaches_every_pitched_preset() {
        // It used to move the melody and nothing else, because the other parts followed a fixed
        // pattern and had no notion of being asked for more or less. They choose figures now, so
        // the dial weighs that choice: sparse reaches for the held chord and the root alone, busy
        // for the offbeats and the octave line.
        for preset in [
            ClipPreset::Lead,
            ClipPreset::Chords,
            ClipPreset::Pad,
            ClipPreset::Arp,
            ClipPreset::Bass,
        ] {
            let (sparse, busy) = (at_density(preset, 0.05), at_density(preset, 1.0));
            assert!(
                busy > sparse,
                "{}: {busy:.1} notes at full density against {sparse:.1} at a twentieth",
                preset.name()
            );
        }
    }

    #[test]
    fn a_drum_kit_leans_on_its_groove_rather_than_choosing_another_one() {
        // The dial used to stop short of the kit, on the grounds that thinning a groove would
        // wreck it. It does not thin arbitrarily: it goes from the weakest hits upward and never
        // takes a downbeat, and above the middle it fills the free steps with ghost notes, which
        // is how a drummer gets busier without playing something else.
        let (sparse, busy) = (
            at_density(ClipPreset::Drums, 0.05),
            at_density(ClipPreset::Drums, 1.0),
        );
        assert!(
            busy > sparse,
            "the dial does not reach the kit: {busy:.1} against {sparse:.1}"
        );

        // What it is *not* is a second way to spell the groove. A busier groove is busier than a
        // sparse one at the same setting, whatever the setting.
        let kit = |groove: &str, density: f32| {
            write_phrase(
                &axis(),
                Ticks::ZERO,
                BAR * 4,
                four_four(),
                TEMPO,
                &ClipRecipe {
                    groove: groove.to_string(),
                    density,
                    ..ClipRecipe::new(ClipPreset::Drums, 3)
                },
                None,
            )
            .len()
        };
        for density in [0.05, 0.5, 1.0] {
            assert!(
                kit("sixteen-beat", density) > kit("sparse", density),
                "the groove stopped deciding the shape at {density}"
            );
        }
    }

    #[test]
    fn a_quiet_kit_never_loses_its_downbeat() {
        // Thinning takes the weak hits first and stops before the one the bar stands on. A kit
        // whose downbeat could be thinned away would be a kit that sometimes lost the bar.
        let notes = write_phrase(
            &axis(),
            Ticks::ZERO,
            BAR * 4,
            four_four(),
            TEMPO,
            &ClipRecipe {
                density: 0.0,
                humanize: 0.0,
                intensity: 0.0,
                ..ClipRecipe::new(ClipPreset::Drums, 5)
            },
            None,
        );
        for bar in 0..4 {
            assert!(
                notes.iter().any(|note| note.start == BAR * bar),
                "bar {} lost its downbeat at the bottom of every dial",
                bar + 1
            );
        }
    }

    #[test]
    fn a_busy_kit_fills_in_with_ghosts_rather_than_with_more_of_the_groove() {
        // Above the middle, the free steps take quiet hits. Loud ones would not be a busier
        // groove, they would be a different one — so what arrives has to be softer than what the
        // groove itself wrote, and that is the whole of the claim.
        let kit = |density: f32| {
            write_phrase(
                &axis(),
                Ticks::ZERO,
                BAR * 4,
                four_four(),
                TEMPO,
                &ClipRecipe {
                    density,
                    humanize: 0.0,
                    groove: "eight-beat".to_string(),
                    ..ClipRecipe::new(ClipPreset::Drums, 2)
                },
                None,
            )
        };
        let plain = kit(0.5);
        let busy = kit(1.0);
        assert!(
            busy.len() > plain.len(),
            "the top of the dial added nothing: {} against {}",
            busy.len(),
            plain.len()
        );

        // Every step the groove already had is still struck at the level it was.
        let softest = |notes: &[Note]| {
            notes
                .iter()
                .map(|note| (note.velocity * 1000.0) as i32)
                .min()
                .unwrap_or(0)
        };
        assert!(
            softest(&busy) < softest(&plain),
            "what was added was not quieter than what was there"
        );
    }

    #[test]
    fn a_drum_clip_ends_on_a_fill_and_the_dial_says_how_long() {
        // A loop's last bar joins another playing of itself, which is exactly the bar a fill
        // belongs in — and the clip never got one, because the rule was written for the last
        // section of a piece and a clip is one section.
        let kit = |fill: f32| {
            write_phrase(
                &axis(),
                Ticks::ZERO,
                BAR * 4,
                four_four(),
                TEMPO,
                &ClipRecipe {
                    fill,
                    humanize: 0.0,
                    ..ClipRecipe::new(ClipPreset::Drums, 4)
                },
                None,
            )
        };
        let last_bar = |notes: &[Note]| notes.iter().filter(|note| note.start >= BAR * 3).count();

        assert!(
            last_bar(&kit(1.0)) > last_bar(&kit(0.5)),
            "a longer fill was not longer"
        );
        assert!(
            last_bar(&kit(0.5)) > last_bar(&kit(0.0)),
            "the middle of the dial ran no fill at all"
        );

        // At zero the last bar is the groove and nothing else, which is what a loop that is not
        // meant to announce its own end wants.
        //
        // Within a hit or two of the first bar rather than equal to it. Thinning draws afresh
        // each bar, so two bars of one groove were never obliged to match — they happened to
        // while so little of the groove survived that both bars came out as the strong steps
        // alone. A fill puts four to eight extra hits in the bar, which is well clear.
        let none = kit(0.0);
        assert!(!none.is_empty(), "turning the fill off silenced the kit");
        let first = none.iter().filter(|note| note.start < BAR).count();
        assert!(
            last_bar(&none).abs_diff(first) <= 2,
            "the last bar has {} hits against the first bar's {first}: a fill ran with the dial \
             at zero",
            last_bar(&none)
        );
    }

    #[test]
    fn a_drum_kit_takes_its_shape_from_its_groove() {
        let kit = |groove: &str| {
            write_phrase(
                &axis(),
                Ticks::ZERO,
                BAR * 4,
                four_four(),
                TEMPO,
                &ClipRecipe {
                    groove: groove.to_string(),
                    ..ClipRecipe::new(ClipPreset::Drums, 3)
                },
                None,
            )
            .len()
        };
        assert!(kit("sixteen-beat") > kit("sparse"));
        // A groove nobody has heard of leaves the kit silent rather than guessing at one.
        assert_eq!(kit("bossa-nova-from-mars"), 0);
    }
}
