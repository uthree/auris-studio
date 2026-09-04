//! One drum voice, and the fill that runs a section into whatever follows it.
//!
//! Apart from the pitched writers because almost nothing they read applies here. A drum has no
//! range, no scale and no voicing; its density thins or thickens a groove somebody already wrote
//! instead of choosing notes; and the length it writes is only there to make the piano roll
//! readable, because a one-shot ignores its note-off. The fill is here rather than with the form
//! because it is the snare that plays it.

use auris_core::time::Ticks;

use crate::frame::{Frame, SectionPlan};
use crate::rhythm::{Accent, DrumVoice};
use crate::spec::PartSpec;

use super::writer::{bar_stream, dynamic, part_grid, phrase_shape, velocity, width};
use super::{Draft, ScoreSettings};

/// One drum voice.
pub(super) fn drums(
    settings: &ScoreSettings,
    frame: &Frame,
    section: &SectionPlan,
    index: usize,
    part: &PartSpec,
) -> Vec<Draft> {
    let voice = part.role.drum_voice().unwrap_or(DrumVoice::ClosedHat);
    // What the part strikes, which is General MIDI unless it says otherwise. A SoundFont kit that
    // does not follow GM comes out silent or playing a cowbell without this.
    let pitch = part.drum_note().unwrap_or_else(|| voice.pitch());
    // A rhythm the user wrote is played as written. Only the groove's own pattern is thinned,
    // because that is the composer's suggestion rather than an instruction.
    let written = part.rhythm.is_some();
    let pattern = part
        .rhythm
        .clone()
        .unwrap_or_else(|| crate::frame::groove_pattern(&settings.groove, voice));
    let grid = part_grid(frame, part);
    let mut notes = Vec::new();
    // How hard the drummer is leaning on the groove. The middle of the dial plays it as written
    // — everything below thins it, everything above fills it in — so that a kit nobody has
    // touched plays the pattern somebody wrote rather than a version of it. *Which* groove is
    // still the groove: this is not a second way to spell that.
    //
    // Read straight off the dial rather than through `density`, which folds the section's
    // intensity in. The survival roll below already weighs the intensity, and counting it twice
    // would thin a quiet section twice as fast as its own number says — and would put the
    // neutral position somewhere nobody could find.
    let dialled = part.density.unwrap_or(0.5).clamp(0.0, 1.0);
    // Above the middle, the steps the groove left empty start taking ghost notes. That is how a
    // drummer gets busier without playing something else — and it is why they are ghosts and why
    // they land on the weak steps only. A filled-in step arriving at full weight would not be a
    // busier groove, it would be a different one.
    let ghosting = (dialled - 0.5).max(0.0) * 2.0;

    for bar in 0..section.bars {
        let mut rng = bar_stream(settings, frame, part, section, "drums", bar);
        // The ghosts draw from a stream of their own. They exist only above the dial's middle,
        // and a draw that appears the moment the dial crosses it would shift every later
        // survival roll in the bar — nudging density from 0.50 to 0.51 rescrambled which hits
        // of the groove survive instead of only adding ghosts. One decision, one stream.
        let mut haunt = bar_stream(settings, frame, part, section, "ghosts", bar);
        let bar_start = grid.bar_ticks() * bar as i64;
        // Which steps ended up carrying a hit, so a fill can go round them rather than double
        // them: the pattern says where a hit belongs and thinning may already have taken it away.
        let mut played = vec![false; grid.steps_per_bar()];
        for (step, sounded) in played.iter_mut().enumerate() {
            let weight = grid.weight(step);
            // A groove is a bar and is mapped onto this bar; a rhythm somebody wrote is a cell and
            // repeats. See `Pattern::at_in_bar` for what a sixteen-step groove used to do to a
            // bar that is not 4/4.
            let accent = match if written {
                pattern.at(step)
            } else {
                pattern.at_in_bar(
                    step,
                    grid.steps_per_bar(),
                    grid.steps_per_beat(),
                    crate::frame::groove_steps_per_beat(&settings.groove),
                )
            } {
                Some(accent) => {
                    // Rolled for every hit whether the chance can fail or not, so which numbers
                    // the ghosts and every later bar see does not move with the dials — the same
                    // roll-anyway rule the humanisation follows.
                    if !written && !rng.chance(survival(accent, weight, section.intensity, dialled))
                    {
                        continue;
                    }
                    accent
                }
                // A rhythm somebody wrote is played as written, so nothing is added to one
                // either: thinning and filling are both what to do with a suggestion.
                None if written || weight > 1 || ghosting <= 0.0 => continue,
                None if !haunt.chance(ghosting * 0.45) => continue,
                None => Accent::Ghost,
            };
            let at = bar_start + grid.tick_of(step);
            *sounded = true;
            notes.push(Draft {
                section: index,
                pitch,
                velocity: (velocity(weight, section.intensity, settings.dynamics)
                    * dynamic(accent.scale(), settings.dynamics)
                    * phrase_shape(grid, section, at, settings.dynamics))
                .clamp(0.08, 1.0),
                start: section.start + at,
                // A one-shot drum ignores its note-off, so the length is only there to make the
                // piano roll readable.
                length: Ticks(120),
            });
        }
        // A fill is a departure from a groove, so there has to be a groove to depart from. A
        // name nobody recognises leaves every voice a bar of rests, and running a fill over that
        // would be the kit inventing a part out of a typo.
        if crate::rhythm::groove(&settings.groove).is_some() {
            fill(
                settings, frame, section, index, part, voice, bar, &played, &mut notes,
            );
        }
    }
    notes
}

/// The chance a groove hit is played, before the seed says which ones are.
///
/// The pattern as spelled is the part. A `Normal` or `Strong` hit plays at every intensity,
/// because a drummer playing quietly plays the same beat more softly rather than losing pieces
/// of it — the lost pieces were exactly what the old arithmetic wrote: it thinned everything
/// below the downbeat by how quiet the section was, so at the default settings one backbeat in
/// nine and one four-on-the-floor kick in nine simply vanished, a different bar of holes every
/// bar. A listener hears a dropped backbeat as a mistake, never as dynamics — and a syncopation
/// the groove spells out, the sixteen-beat's kick or the bossa's clave, *is* the groove, so
/// losing one of those was losing the pattern's identity, not its volume.
///
/// What breathes with the section is the **ghosts**: the ornaments a drummer adds as the music
/// heats up and leaves out when it cools, the finest steps first. At the default intensities
/// that is roughly half of them in a verse and nearly all of them in a chorus, which is also
/// what makes a chorus feel busier without a single skeleton hit moving.
///
/// The density dial keeps its contract on top: the middle plays the groove as written, below it
/// the drummer plays less of everything but the downbeat — that is an instruction, not a
/// temperature — and above it `drums` fills empty weak steps in with ghosts of its own.
fn survival(accent: Accent, weight: u8, intensity: f32, dialled: f32) -> f32 {
    // The downbeat is never thinned, or the bar loses its footing.
    if weight >= 4 {
        return 1.0;
    }
    let base = match accent {
        Accent::Ghost => {
            let strength = match weight {
                0 => 0.72,
                1 => 0.90,
                _ => 1.0,
            };
            strength * (0.15 + 0.85 * intensity.clamp(0.0, 1.0))
        }
        Accent::Normal | Accent::Strong => 1.0,
    };
    let leaning = 0.5 + dialled.clamp(0.0, 1.0);
    (base * leaning).clamp(0.0, 1.0)
}

/// The shape a fill runs in.
///
/// One shape — every free sixteenth, rising — was every fill in every piece, so the one moment a
/// listener is certain to notice sounded the same at every join of everything the composer wrote.
/// These are the shapes a drummer actually reaches for, all on the snare's own note so a custom
/// kit is never sent pitches it did not map; which one plays is drawn from the section's stream,
/// so a repeat of a section fills the way it filled before and the `variation` dial can differ.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum FillShape {
    /// Every free step, rising: the classic run up to the downbeat.
    Run,
    /// Three against the window's four, the way `CompFigure::Cross` puts them: a fill that
    /// breathes instead of rattling.
    Tresillo,
    /// Every half-beat: a heavier, slower fill, the run's big brother.
    Eighths,
    /// Nothing, then everything: the window's first half stays with the groove and its second
    /// is struck solid.
    Burst,
}

/// The steps a fill strikes, before the groove's own hits are taken back out.
///
/// A free function of the window so each shape is a fact that can be checked: `from..steps` is
/// the window the dial bought, `per_beat` the bar's own beat. Every shape scales with the
/// window, which is what keeps a longer dial a longer fill whichever shape was drawn.
fn fill_steps(shape: FillShape, from: usize, steps: usize, per_beat: usize) -> Vec<usize> {
    let window = steps.saturating_sub(from);
    match shape {
        FillShape::Run => (from..steps).collect(),
        FillShape::Tresillo => {
            let hits = (window * 3).div_ceil(8).max(2).min(window);
            let pattern = crate::rhythm::euclid(hits, window, 0);
            (from..steps)
                .filter(|step| pattern.at(step - from).is_some())
                .collect()
        }
        FillShape::Eighths => (from..steps)
            .filter(|step| (step - from).is_multiple_of((per_beat / 2).max(1)))
            .collect(),
        FillShape::Burst => (from + window / 2..steps).collect(),
    }
}

/// Runs the snare into whatever follows the section.
///
/// A section that simply stops and is replaced sounds like an edit rather than like an arrival:
/// the join is the one moment a listener is certain to notice, and nothing marked it. Only the
/// last bar of a section gets one, and only the snare plays it — the other voices keep the groove
/// underneath so the fill has something to be a departure from. Which [`FillShape`] it runs in
/// is the section's own draw.
///
/// A part with a written rhythm is left alone, on the same principle as thinning: an instruction
/// is not a suggestion.
#[allow(clippy::too_many_arguments)]
fn fill(
    settings: &ScoreSettings,
    frame: &Frame,
    section: &SectionPlan,
    index: usize,
    part: &PartSpec,
    voice: DrumVoice,
    bar: usize,
    played: &[bool],
    notes: &mut Vec<Draft>,
) {
    let last_bar = bar + 1 == section.bars;
    // The last section of a piece has nothing to lead into and plays the groove to the end.
    let leads_somewhere = index + 1 < frame.sections.len() || frame.joins_on;
    if part.rhythm.is_some() || voice != DrumVoice::Snare || !last_bar || !leads_somewhere {
        return;
    }

    let grid = part_grid(frame, part);
    let steps = grid.steps_per_bar();
    let per_beat = grid.steps_per_beat();
    // How much of the bar runs, from none to two beats. The section's intensity still leans on
    // it, so a quiet section fills shorter than a loud one at the same setting — the dial says
    // how much of a fill this piece wants, not how much this one bar gets.
    let wanted = settings.fill.clamp(0.0, 1.0) * (0.6 + 0.4 * section.intensity);
    let beats = (wanted * 2.0).round() as usize;
    if beats == 0 {
        return;
    }
    let from = steps.saturating_sub(beats * per_beat).max(1);
    let bar_start = grid.bar_ticks() * bar as i64;

    // Drawn from the section's own stream, like every other choice a bar makes: a repeat of the
    // section fills the way it filled the first time, and `variation` can buy a departure back.
    const SHAPES: [FillShape; 4] = [
        FillShape::Run,
        FillShape::Tresillo,
        FillShape::Eighths,
        FillShape::Burst,
    ];
    let mut choose = bar_stream(settings, frame, part, section, "fill", bar);
    let shape = SHAPES[choose.weighted(&[1.4, 1.0, 0.9, 0.8]).min(SHAPES.len() - 1)];

    for step in fill_steps(shape, from, steps, per_beat) {
        if played.get(step).copied().unwrap_or(false) {
            continue;
        }
        // Rising into the downbeat that follows, which is what makes it lead somewhere — and the
        // rise is a dynamic like any other, so it flattens with the rest of them rather than
        // being the one crescendo left standing in a part played at one level on purpose.
        let through = (step - from) as f32 / (steps - from).max(1) as f32;
        let mean = 0.70;
        let rise = mean + (0.45 + 0.5 * through - mean) * width(settings.dynamics);
        notes.push(Draft {
            section: index,
            // The same note the groove is being played on, or the fill would run on the snare a
            // General MIDI kit has rather than the one this instrument actually carries.
            pitch: part.drum_note().unwrap_or_else(|| voice.pitch()),
            velocity: rise.clamp(0.08, 1.0),
            start: section.start + bar_start + grid.tick_of(step),
            length: Ticks(120),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{FillShape, fill_steps, survival};
    use crate::parts::fixture::{BASE, bar_steps, draft, part, section_notes};
    use crate::rhythm::Accent;

    #[test]
    fn survival_is_certain_for_the_pattern_and_a_temperature_for_the_ghosts() {
        // The spelled pattern is certain at the dial's middle, at every intensity and weight:
        // quiet is the velocity's business, and a dropped backbeat reads as a mistake.
        for weight in 0..=4 {
            for intensity in [0.0, 0.5, 1.0] {
                for accent in [Accent::Normal, Accent::Strong] {
                    assert_eq!(survival(accent, weight, intensity, 0.5), 1.0);
                }
            }
        }
        // Ghosts breathe with the section, finest steps first, and never outlive the pattern.
        let ghost = |weight, intensity| survival(Accent::Ghost, weight, intensity, 0.5);
        assert!(ghost(0, 0.2) < ghost(0, 0.9), "hotter is busier");
        assert!(ghost(0, 0.5) < ghost(1, 0.5), "the finest go first");
        assert!(ghost(1, 1.0) <= 1.0);
        assert!(ghost(0, 0.0) > 0.0, "a cold section still breathes");
        // Below the middle the dial thins even the pattern — that is an instruction to play
        // less, not a temperature — but never the downbeat.
        assert!(survival(Accent::Normal, 2, 1.0, 0.0) < 1.0);
        assert_eq!(survival(Accent::Ghost, 4, 0.0, 0.0), 1.0, "the downbeat");
    }

    #[test]
    fn a_quiet_section_keeps_the_pattern_and_loses_the_ghosts() {
        let (_, frame, parts) = draft(
            r#"
            form = "verse"
            chords = "@axis"
            humanize = 0
            ending = "none"
            [section.verse]
            bars = 4
            intensity = 0.05
            "#,
        );
        // basic-rock's kick and snare spell no ghosts, so even this quiet they play whole,
        // bar for bar. The old thinning read the intensity over the whole kit: at the default
        // settings it lost one backbeat in nine, and at this one it lost a third of the part.
        for (name, steps) in [("kick", vec![0, 6, 10]), ("snare", vec![4, 12])] {
            let drum = part(&parts, name);
            for bar in 0..4 {
                assert_eq!(bar_steps(&frame, drum, bar), steps, "{name}, bar {bar}");
            }
        }
        // The hat's offbeat ghosts are the ornaments, and this cold they are nearly all gone —
        // while its own spelled hits, the beats, still land in every bar.
        let hat = part(&parts, "hat");
        let mut ghosts = 0;
        for bar in 0..4 {
            let steps = bar_steps(&frame, hat, bar);
            for beat in [0, 4, 8, 12] {
                assert!(
                    steps.contains(&beat),
                    "the hat lost beat {beat} of bar {bar}"
                );
            }
            ghosts += steps.iter().filter(|step| !step.is_multiple_of(4)).count();
        }
        assert!(
            ghosts <= 6,
            "{ghosts} of 16 ghosts survived a nearly silent verse"
        );
    }

    #[test]
    fn every_fill_shape_scales_with_its_window_and_stays_inside_it() {
        // A two-beat window of a sixteen-step bar, which is what the dial at 1.0 buys.
        assert_eq!(
            fill_steps(FillShape::Run, 8, 16, 4),
            vec![8, 9, 10, 11, 12, 13, 14, 15]
        );
        assert_eq!(
            fill_steps(FillShape::Eighths, 8, 16, 4),
            vec![8, 10, 12, 14]
        );
        assert_eq!(
            fill_steps(FillShape::Burst, 8, 16, 4),
            vec![12, 13, 14, 15],
            "nothing, then everything"
        );
        assert_eq!(
            fill_steps(FillShape::Tresillo, 8, 16, 4),
            vec![8, 11, 14],
            "three against the window's four"
        );

        // Every shape scales with the window — a longer dial is a longer fill whichever shape
        // was drawn, which is what `a_drum_clip_ends_on_a_fill_and_the_dial_says_how_long`
        // holds for the finished kit — and never reaches outside it.
        for shape in [
            FillShape::Run,
            FillShape::Tresillo,
            FillShape::Eighths,
            FillShape::Burst,
        ] {
            let long = fill_steps(shape, 8, 16, 4);
            let short = fill_steps(shape, 12, 16, 4);
            assert!(short.len() < long.len(), "{shape:?} did not scale");
            assert!(!short.is_empty(), "{shape:?} vanished at one beat");
            assert!(
                short.iter().all(|step| (12..16).contains(step)),
                "{shape:?} reached outside its window"
            );
        }
    }

    #[test]
    fn two_songs_do_not_always_fill_the_same_way() {
        // The draw has to reach the music: one shape was every fill in every piece, so the one
        // moment a listener is certain to notice sounded the same at every join the composer
        // ever wrote. The fill window's struck steps now differ from seed to seed.
        let mut windows = std::collections::BTreeSet::new();
        for seed in 1..=8u64 {
            let (_, frame, parts) = draft(&format!(
                r#"
                    form = "verse verse"
                    chords = "@axis"
                    humanize = 0
                    variation = 0
                    fill = 1.0
                    seed = {seed}
                    [section.verse]
                    bars = 4
                    intensity = 0.8
                    "#
            ));
            let snare = part(&parts, "snare");
            let plan = &frame.sections[0];
            let bar = frame.grid.bar_ticks();
            let last_bar = plan.start + plan.length - bar;
            let mut struck: Vec<usize> = snare
                .notes
                .iter()
                .filter(|note| note.section == 0 && note.start >= last_bar)
                .map(|note| frame.grid.step_of(note.start - last_bar))
                .filter(|step| *step >= 8)
                .collect();
            struck.sort_unstable();
            windows.insert(struck);
        }
        assert!(
            windows.len() > 1,
            "eight seeds filled the same way: {windows:?}"
        );
    }

    #[test]
    fn drums_play_their_general_midi_pitches() {
        let (_, _, parts) = draft(BASE);
        for (name, pitch) in [("kick", 36), ("snare", 38), ("hat", 42)] {
            let drum = part(&parts, name);
            assert!(
                drum.notes.iter().all(|note| note.pitch == pitch),
                "`{name}` played something other than {pitch}"
            );
        }
    }

    #[test]
    fn a_written_rhythm_survives_a_quiet_section() {
        // Thinning is a suggestion about the groove, not licence to ignore an instruction.
        // The ending is off because it lands a kick of its own past the written bar, and this
        // test reads the written rhythm back step for step.
        let (_, frame, parts) = draft(
            r#"
            form = "verse"
            humanize = 0
            ending = "none"
            [section.verse]
            bars = 1
            intensity = 0.05
            [[part]]
            name = "kick"
            rhythm = "x ~ x ~ x ~ x ~ x ~ x ~ x ~ x ~"
            "#,
        );
        let steps: Vec<usize> = part(&parts, "kick")
            .notes
            .iter()
            .map(|note| frame.grid.step_of(note.start))
            .collect();
        assert_eq!(steps, vec![0, 2, 4, 6, 8, 10, 12, 14]);
    }

    #[test]
    fn a_written_rhythm_is_played_as_written() {
        let (_, frame, parts) = draft(
            r#"
            form = "verse"
            humanize = 0
            ending = "none"
            [section.verse]
            bars = 1
            [[part]]
            name = "kick"
            rhythm = "x ~ ~ ~ x ~ ~ ~ x ~ ~ ~ x ~ ~ ~"
            "#,
        );
        let kick = part(&parts, "kick");
        let steps: Vec<usize> = kick
            .notes
            .iter()
            .map(|note| frame.grid.step_of(note.start))
            .collect();
        assert_eq!(steps, vec![0, 4, 8, 12]);
    }

    #[test]
    fn the_last_section_fills_into_the_ending() {
        // The held final bar is somewhere to land, so the section before it runs its fill — the
        // fill into the final hit, which is how every live set ends. Without the ending that
        // section used to play the groove flat to the stop.
        let spec = |ending: &str| {
            format!(
                r#"
                form = "verse"
                chords = "@axis"
                humanize = 0
                fill = 1.0
                ending = "{ending}"
                [section.verse]
                bars = 4
                intensity = 0.8
                "#
            )
        };
        let hits_in_last_verse_bar = |text: &str| {
            let (_, frame, parts) = draft(text);
            let snare = part(&parts, "snare");
            let verse = &frame.sections[0];
            let bar = frame.grid.bar_ticks();
            snare
                .notes
                .iter()
                .filter(|note| note.section == 0 && note.start >= verse.start + verse.length - bar)
                .count()
        };
        assert!(
            hits_in_last_verse_bar(&spec("held")) > hits_in_last_verse_bar(&spec("none")),
            "the verse did not fill into the ending"
        );
    }

    #[test]
    fn a_valid_sparse_groove_can_fill_even_with_an_empty_snare_row() {
        let (_, frame, parts) = draft(
            r#"
            form = "verse"
            chords = "@axis"
            groove = "sparse"
            humanize = 0
            fill = 1.0
            ending = "held"
            [section.verse]
            bars = 4
            intensity = 1.0
            "#,
        );
        let verse = &frame.sections[0];
        let last_bar = verse.start + verse.length - frame.grid.bar_ticks();
        assert!(
            part(&parts, "snare")
                .notes
                .iter()
                .any(|note| note.section == 0 && note.start >= last_bar),
            "the recognised sparse groove lost its ending fill"
        );
    }

    #[test]
    fn the_ending_is_one_kick_and_silence() {
        // The kick lands once with the chord; the snare and the hat have nothing to keep time
        // for. The cymbal is the joins writer's business and is asserted where it lives.
        let (_, frame, parts) = draft(
            r#"
            form = "verse"
            chords = "@axis"
            humanize = 0
            [section.verse]
            bars = 4
            "#,
        );
        let ending = frame.sections.last().expect("an ending");
        assert!(ending.coda, "the fixture grew no ending");
        let in_ending = |name: &str| {
            part(&parts, name)
                .notes
                .iter()
                .filter(|note| note.start >= ending.start)
                .count()
        };
        assert_eq!(in_ending("kick"), 1, "one kick on the landing");
        assert_eq!(in_ending("snare"), 0);
        assert_eq!(in_ending("hat"), 0);
        let landing = part(&parts, "kick").notes.last().expect("the kick plays");
        assert_eq!(landing.start, ending.start, "on the downbeat, exactly");
    }

    #[test]
    fn a_louder_section_plays_more_drum_hits() {
        let quiet = draft(&BASE.replace("bars = 4", "bars = 4\nintensity = 0.1")).2;
        let loud = draft(&BASE.replace("bars = 4", "bars = 4\nintensity = 1.0")).2;
        assert!(
            part(&loud, "hat").notes.len() > part(&quiet, "hat").notes.len(),
            "intensity did not change how much the drummer plays"
        );
    }

    #[test]
    fn a_section_runs_a_fill_into_the_one_that_follows() {
        // A section that stopped and was replaced sounded like an edit rather than an arrival.
        // With no ending written, the last section of a piece has nothing to lead into and keeps
        // the groove instead — the ending is off here so that contrast is what gets measured.
        let (_, frame, parts) = draft(
            r#"
                form = "verse verse"
                chords = "@axis"
                humanize = 0
                variation = 0
                ending = "none"
                [section.verse]
                bars = 4
                intensity = 0.8
                "#,
        );
        let snare = part(&parts, "snare");
        let bar = frame.grid.bar_ticks();
        let last_bar_hits = |section: usize| -> usize {
            let plan = &frame.sections[section];
            snare
                .notes
                .iter()
                .filter(|note| {
                    note.section == section && note.start >= plan.start + plan.length - bar
                })
                .count()
        };
        assert!(
            last_bar_hits(0) > last_bar_hits(1),
            "the first verse ran {} hits into the second's {}",
            last_bar_hits(0),
            last_bar_hits(1)
        );
    }

    #[test]
    fn nudging_density_past_the_middle_only_adds_ghosts() {
        // Crossing 0.5 used to insert a ghost draw before every later survival roll in the
        // bar, so the dial's smallest movement rescrambled which hits of the groove survive
        // instead of only thickening the playing. With the ghosts on a stream of their own,
        // everything the kit plays at the middle it still plays above it — the ghosts arrive
        // on top.
        let kit = |density: f32| {
            let text = format!(
                r#"
                    form = "verse"
                    chords = "@axis"
                    humanize = 0
                    swing = 50
                    [section.verse]
                    bars = 4
                    [[part]]
                    name = "kick"
                    density = {density}
                    "#
            );
            let (_, frame, parts) = draft(&text);
            section_notes(&frame, part(&parts, "kick"), 0)
                .into_iter()
                .map(|(start, pitch, ..)| (start, pitch))
                .collect::<Vec<_>>()
        };
        let middle = kit(0.5);
        let above = kit(0.9);
        for note in &middle {
            assert!(
                above.contains(note),
                "a groove hit at {note:?} was lost by asking for *more*"
            );
        }
        assert!(
            above.len() > middle.len(),
            "nothing was added above the middle"
        );
    }
}
