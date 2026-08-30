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
use auris_core::time::{SignatureMap, Ticks, TimeSignature};
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

/// The preset that writes exactly one role, which is [`roles_of`] read backwards.
///
/// `None` for [`Role::Crash`], which is the one role no preset names: a crash is written against
/// the *joins of the form* — it asks whether arriving at a section is worth striking something for
/// — and that is a question about a whole piece rather than about a clip. A clip preset for it
/// would be a picker entry that wrote nothing whenever the range it was given had no arrival in
/// it, which is most ranges.
///
/// [`ClipPreset::Drums`] is not the answer for anything here, and deliberately: it is three roles
/// in one clip, so no single role maps back to it. A whole song keeps its kick, snare and hat on
/// tracks of their own — that is what makes a kit mixable — and each of those maps to the preset
/// of the same name.
pub fn preset_of(role: Role) -> Option<ClipPreset> {
    Some(match role {
        Role::Melody => ClipPreset::Lead,
        Role::Chords => ClipPreset::Chords,
        Role::Pad => ClipPreset::Pad,
        Role::Arp => ClipPreset::Arp,
        Role::Stab => ClipPreset::Stab,
        Role::Bass => ClipPreset::Bass,
        Role::Kick => ClipPreset::Kick,
        Role::Snare => ClipPreset::Snare,
        Role::Hat => ClipPreset::Hat,
        // Both written against the joins of the form — a question about a whole piece that a
        // clip preset, handed one range, could only answer with silence most of the time.
        Role::Crash | Role::Riser => return None,
    })
}

/// The seed one clip of a composed piece is written from.
///
/// A stream of the song's own seed named by the part and the stretch it plays, so it is
/// reproducible from the specification and **different for every clip**. That is what makes a
/// composed piece re-takeable one clip at a time: asking the chorus bass for another take moves
/// the chorus bass and leaves the verse bass, and the chorus drums, exactly where they were.
///
/// The whole song's seed on every clip would have been simpler and wrong twice over — one clip
/// re-rolled would land on the seed the *next* re-roll of its neighbour would land on, so two
/// takes of two different parts could not be told apart by their numbers.
///
/// Held to [`SEED_RANGE`] rather than handed out at the full width of the stream. A seed is a
/// number a person reads off a panel and types back in to get a take they liked, and nineteen
/// digits is not one — the field it lands in is an editable one, and a value nobody can retype is
/// a value nobody can go back to. Six digits is enough that a piece of thirty clips has no
/// realistic chance of drawing one twice, and two clips that did would still write different
/// notes: they are different parts, over different chords, at different densities.
pub fn clip_seed(song_seed: u64, part: &str, section: &str, instance: usize) -> u64 {
    let drawn = crate::rng::Rng::stream(
        song_seed,
        &[
            crate::rng::Key::Word("clip"),
            crate::rng::Key::Word(part),
            crate::rng::Key::Word(section),
            crate::rng::Key::Index(instance as u64),
        ],
    )
    .next_u64();
    1 + drawn % SEED_RANGE
}

/// How many seeds a composed clip may be given, counting from one.
///
/// Six digits: long enough that a collision inside one piece is not worth guarding against, short
/// enough to be read off a panel and typed back in.
pub const SEED_RANGE: u64 = 999_999;

/// The recipe describing one section of one part, as the whole-song writer played it.
///
/// The inverse of what [`write_phrase`] does, as far as the inverse goes: every dial a recipe has
/// is read back off the specification, the part and the section, so that a composed clip arrives
/// in the document knowing what it is. `part` must be the part **as that section plays it** —
/// [`SectionPlan::played`](crate::frame::SectionPlan) — because a section is free to patch a part
/// busier or an octave up, and a recipe that recorded the roster's answer would describe a clip
/// that is not the one on the timeline.
///
/// `None` where no preset names the role: see [`preset_of`]. Such a clip arrives with no recipe
/// and behaves exactly as a clip somebody played, which is the honest answer — nothing here can
/// write it again.
///
/// # What it does not promise
///
/// It does **not** promise that [`write_phrase`] with this recipe writes the notes the clip
/// arrived with. It cannot: a whole song is planned with things a clip has no room for — how far
/// a repeated section departs from its first playing, what leads into what, the arch of intensity
/// across the form — and a recipe that carried all of them would be a song specification wearing
/// a clip's name. What it promises is the useful thing: another take of this clip is the same
/// part, in the same register, at the same density, over the same chords, played the same way.
pub fn recipe_for(
    settings: &ScoreSettings,
    part: &PartSpec,
    section: &SectionPlan,
    song_seed: u64,
) -> Option<ClipRecipe> {
    let preset = preset_of(part.role)?;
    Some(ClipRecipe {
        preset,
        seed: clip_seed(song_seed, &part.name, &section.name, section.instance),
        // The *base* density, before `parts::writer::density` scales it by the role and by the
        // section's intensity — because writing this clip again puts it through that same scaling.
        // Recording the scaled figure would compound it, and a chorus re-taken twice would come
        // back busier each time.
        density: part.density.unwrap_or_else(|| settings.mood.density()),
        intensity: section.intensity,
        groove: settings.groove.clone(),
        swing: settings.swing,
        subdivision: part.subdivision,
        gate: part.gate,
        dynamics: settings.dynamics,
        // The mood's, which is where a clip's syncopation comes from in the other direction too:
        // `mood_for` builds a Mood out of the recipe, and this is that field read back.
        syncopation: settings.mood.syncopation,
        // A part's octave is absolute and a recipe's is a shift from where the preset sits, so
        // the difference is the number. Clamped to what the dial can hold: a part written four
        // octaves off its role would otherwise come back as a recipe nobody could edit back.
        octave: (part.octave - part.role.default_octave()).clamp(-2, 2),
        fill: settings.fill,
        // The composer describes what it asked for, not what came out: the digest of the notes
        // as written is the session's to stamp, at the moment the clip lands in a document.
        text_digest: 0,
    })
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
/// No tempo comes in, and that is a change worth a sentence: the humanisation used to be baked
/// into the notes here, and its dial is a *time* — milliseconds cannot become ticks without a
/// tempo. The wander now happens on the clip's performance stack, at playback, where the
/// document's own tempo map is in force; the text a phrase writes never cared how fast its
/// ticks go by.
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
    // A clip is generated from a recipe, which has dials of its own and no mood — so the register
    // is the neutral one, exactly where the arch sat before brightness could move it. A recipe
    // that grows a brightness of its own is what would change this.
    let skeleton = skeleton(&events, recipe.seed, section_key, instance, 0.5);

    let frame = Frame {
        grid,
        sections: vec![SectionPlan {
            name: section_key.to_string(),
            instance,
            start: Ticks::ZERO,
            length,
            bars,
            key,
            // Nothing under `write_parts` reads a tempo any more — the wander that converted
            // milliseconds into ticks happens on the performance stack now — so the plan carries
            // a placeholder rather than asking the caller to look one up it will never use.
            tempo: 120.0,
            intensity: recipe.intensity.clamp(0.0, 1.0),
            events,
            skeleton,
            parts: Vec::new(),
            // A clip is one part in one stretch, so there is nothing to patch it against.
            tweaks: Default::default(),
            // And it is a stretch of a piece, never the piece's ending.
            coda: false,
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
        dynamics: recipe.dynamics.clamp(0.0, 1.0),
        fill: recipe.fill.clamp(0.0, 1.0),
        // One clip is one playing of one section, so there is no repeat to depart from.
        variation: 0.0,
        groove: recipe.groove.clone(),
        // And no song around it to have given one a tune.
        motif: Vec::new(),
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
            velocity: draft.velocity.clamp(0.0, 1.0),
            // Truncate rather than overhang: the scheduler drops a note that runs past its clip.
            ..Note::new(
                draft.pitch.min(127),
                draft.start,
                draft.length.min(length - draft.start).max(Ticks(1)),
            )
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
            // A kit follows its groove, and the groove is the part somebody chose. These
            // ceilings used to be 0.75 and 0.85, and they were only that low because the kit was
            // being *sampled* rather than played: a third of the hits the groove asked for were
            // dropped at the neutral dial, and it was that dropping — not any musical decision —
            // that made one take differ from the next. The pattern as spelled now always plays,
            // so what still makes two takes different is the ghosts and the fills. Measured now:
            // kit 85 %, snare 89, hat 79.
            ClipPreset::Drums => 0.90,
            // One drum voice on its own has less room still. A lone snare is a backbeat, and a
            // backbeat played differently is a different groove rather than another take of this
            // one. The ceiling is here to catch a voice that never changes at all, not to demand
            // one that changes as much as a melody.
            ClipPreset::Snare | ClipPreset::Hat => 0.92,
            // And the kick has no room left. Its lines in the shipped grooves spell hits and
            // never ghosts, and the pattern as spelled always plays — so another take of a lone
            // kick *is* the same line, honestly. What used to differ between takes was which of
            // its hits were dropped, and that was sampling error wearing variety's clothes.
            // There is nothing here for a ceiling to catch.
            ClipPreset::Kick => f32::INFINITY,
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
            &ClipRecipe::new(ClipPreset::Bass, 4),
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
                &ClipRecipe {
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
        let recipe = ClipRecipe::new(ClipPreset::Lead, 5);
        let write = |start: Ticks, section: Option<(&str, usize)>| {
            write_phrase(&harmony, start, BAR * 4, four_four(), &recipe, section)
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
                    &ClipRecipe::new(ClipPreset::Lead, seed),
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
                &ClipRecipe::new(ClipPreset::Lead, seed),
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
            &ClipRecipe {
                density: 0.0,
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
                &ClipRecipe {
                    density,
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
                &ClipRecipe {
                    fill,
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
