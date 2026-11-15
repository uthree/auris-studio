//! One drum voice, and the fill that runs a section into whatever follows it.
//!
//! Apart from the pitched writers because almost nothing they read applies here. A drum has no
//! range, no scale and no voicing; its density thins or thickens a groove somebody already wrote
//! instead of choosing notes; and the length it writes is only there to make the piano roll
//! readable, because a one-shot ignores its note-off. The fill is here because it is the snare
//! that plays it.

use auris_core::time::Ticks;

use crate::frame::{Frame, SectionPlan};
use crate::rhythm::{Accent, DrumVoice};
use crate::spec::PartSpec;

use super::writer::{bar_stream, dynamic, part_grid, phrase_shape, struck, width};
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
    // Above the middle, the steps the groove left empty start taking strokes of their own. That
    // is how a drummer gets busier without playing something else — and it is why they land on
    // the weak steps only, and why `added` gives each voice the stroke that voice has for it.
    let filling = (dialled - 0.5).max(0.0) * 2.0;
    // A groove is a bar and is mapped onto this bar; a rhythm somebody wrote is a cell and
    // repeats. See `Pattern::at_in_bar` for what a sixteen-step groove used to do to a bar that
    // is not 4/4.
    let own = crate::frame::groove_steps_per_beat(&settings.groove);
    let spelled = |step: usize| {
        if written {
            pattern.at(step)
        } else {
            pattern.at_in_bar(step, grid.steps_per_bar(), grid.steps_per_beat(), own)
        }
    };
    let stroke = |bar_start: Ticks, step: usize, accent: Accent| {
        let at = bar_start + grid.tick_of(step);
        Draft {
            section: index,
            pitch,
            velocity: (struck(grid.weight(step), section.intensity, settings.dynamics)
                * accent_level(accent, settings.dynamics)
                * phrase_shape(grid, section, at, settings.dynamics))
            .clamp(0.08, 1.0),
            start: section.start + at,
            // A one-shot drum ignores its note-off, so the length is only there to make the
            // piano roll readable.
            length: Ticks(120),
        }
    };
    // What this take's second bar says back to its first. A rhythm somebody wrote has no answer
    // for the reason it has no thinning: an instruction is not a suggestion.
    let answer = if written {
        None
    } else {
        let mut choose = bar_stream(frame, part, section, "answer", 0);
        answer(voice, grid, dialled, spelled, &mut choose)
    };
    // Where the section's last bar stops being the groove, so the answer does not land in the
    // middle of the snare's own run.
    let filled_from = (voice == DrumVoice::Snare
        && crate::rhythm::groove(&settings.groove).is_some())
    .then(|| fill_from(settings, section, grid))
    .flatten();

    for bar in 0..section.bars {
        // Every choice the kit makes for itself is made for a bar of the cell, not for a bar of
        // the section. Drawn bar by bar, a dial a notch off its middle wrote sixteen different
        // bars in sixteen: the thinning took another backbeat each time and the filling put its
        // strokes somewhere new, so what should have been a sparser or a busier groove was no
        // groove at all. A drummer who leaves a stroke out leaves it out every time round.
        let cell = bar % CELL_BARS;
        let mut rng = bar_stream(frame, part, section, "drums", cell);
        // The added strokes draw from a stream of their own. They exist only above the dial's
        // middle, and a draw that appears the moment the dial crosses it would shift every later
        // survival roll in the bar — nudging density from 0.50 to 0.51 rescrambled which hits
        // of the groove survive instead of only adding to them. One decision, one stream.
        let mut haunt = bar_stream(frame, part, section, "ghosts", cell);
        let bar_start = grid.bar_ticks() * bar as i64;
        // Which steps ended up carrying a hit, so a fill can go round them rather than double
        // them: the pattern says where a hit belongs and thinning may already have taken it away.
        let mut played = vec![false; grid.steps_per_bar()];
        for (step, sounded) in played.iter_mut().enumerate() {
            let weight = grid.weight(step);
            let accent = match spelled(step) {
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
                None if written || filling <= 0.0 => continue,
                None => match added(voice, weight) {
                    Some(stroke) if haunt.chance(filling * 0.45) => stroke,
                    _ => continue,
                },
            };
            *sounded = true;
            notes.push(stroke(bar_start, step, accent));
        }
        // The cell's last bar takes the answer, wherever the dial's own strokes left room for
        // it and the fill has not taken the bar over.
        if cell + 1 == CELL_BARS
            && let Some((step, accent)) = answer
            && !played[step]
            && !(bar + 1 == section.bars && filled_from.is_some_and(|from| step >= from))
        {
            played[step] = true;
            notes.push(stroke(bar_start, step, accent));
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

/// How many bars the kit's own choices repeat over.
///
/// Two, because that is the length a drummer's pattern has when it is not one bar: the second
/// bar answers the first and the pair comes round again. Measured on the kit tracks of a
/// pop and rock score corpus, a bar's kick is the bar before it six times in ten and the bar
/// *two* before it, but not the one before, two times in ten.
const CELL_BARS: usize = 2;

/// The stroke the dial's upper half puts on a step the groove left empty, or `None` where this
/// voice has nothing to add there.
///
/// Every voice used to get the same answer — a ghost on any step below the beat — and that is
/// a snare's answer. A kick has no ghost stroke, so its "ghosts" were kicks: at a density of
/// 0.8 the kit struck six of them a bar, four in ten on an odd sixteenth, where the corpus kits
/// strike three and put seven in a hundred there. A busier kick is an extra kick on an offbeat
/// eighth and nothing finer. A hat fills the strokes between its strokes in at their own grid
/// weight, which is eighths becoming sixteenths; only the snare ghosts.
fn added(voice: DrumVoice, weight: u8) -> Option<Accent> {
    match voice {
        DrumVoice::Kick => (weight == 1).then_some(Accent::Normal),
        DrumVoice::Snare => (weight <= 1).then_some(Accent::Ghost),
        _ => (weight <= 1).then_some(Accent::Normal),
    }
}

/// The one stroke a voice adds to the second bar of its cell, and the step it lands on.
///
/// What a seed chooses for a kit. It used to "choose" which of the groove's strokes went
/// missing, and with that gone — the pattern as spelled plays whole — two takes of a kit at the
/// neutral dial were the same notes: pressing "another take" did nothing at all. This is what a
/// drummer varies instead. The first bar states the groove and the second says it back with one
/// thing changed, on the way into the turn: a ghost on the snare, a kick pushed onto an offbeat
/// eighth. One stroke of the kind [`added`] gives the voice, somewhere in the back half of the
/// bar the groove left empty, or nothing — drawn once for the section, so every pair of bars
/// in it is the same pair.
///
/// A hat has none, because it is the pulse the answer is heard against. Nor has a voice the
/// groove gives a single stroke or no stroke: in `half-time` and `sparse` that voice is keeping
/// out of the way, and an answer would be it stepping back in.
///
/// It goes with the strokes of its own weight when the dial is turned down, as [`kept`] says.
/// Both draws are made whatever they decide, so which answer a take has does not move with the
/// dial.
fn answer(
    voice: DrumVoice,
    grid: crate::rhythm::Grid,
    dialled: f32,
    spelled: impl Fn(usize) -> Option<Accent>,
    rng: &mut crate::rng::Rng,
) -> Option<(usize, Accent)> {
    let steps = grid.steps_per_bar();
    let open: Vec<(usize, Accent)> = (steps / 2..steps)
        .filter(|step| spelled(*step).is_none())
        .filter_map(|step| added(voice, grid.weight(step)).map(|accent| (step, accent)))
        .collect();
    let pick = rng.below(open.len() + 1);
    let stays = rng.unit();
    if !matches!(voice, DrumVoice::Kick | DrumVoice::Snare)
        || (0..steps).filter(|step| spelled(*step).is_some()).count() < 2
    {
        return None;
    }
    let (step, accent) = *open.get(pick)?;
    (stays < kept(grid.weight(step), dialled)).then_some((step, accent))
}

/// What an accent multiplies a drum stroke by.
///
/// An accent is a written difference in strength like any other and is played at the width
/// the `dynamics` dial allows — all but the ghost. A quarter of a ghost's difference left it at
/// nine tenths of a full stroke, and the kit measured that way: a ghosted snare struck at 0.54
/// beside a backbeat at 0.68. That is not a ghost, it is a second backbeat in the wrong place.
/// A ghost is a different stroke rather than a softer one, so it keeps the whole of its
/// distance, and the dial still closes it: at zero every stroke is struck alike.
fn accent_level(accent: Accent, dynamics: f32) -> f32 {
    match accent {
        Accent::Ghost => 1.0 + (accent.scale() - 1.0) * dynamics.clamp(0.0, 1.0),
        Accent::Normal | Accent::Strong => dynamic(accent.scale(), dynamics),
    }
}

/// How much of a grid weight's strokes the lower half of the density dial keeps.
///
/// The dial takes the finest strokes first and the beats last, an eighth of its travel to each
/// weight: sixteenths go between 0.5 and 0.375, offbeat eighths by 0.25, the beats by 0.125 and
/// the middle of the bar at zero. Playing less is playing the simpler version of the same beat.
/// It used to take a fifth of everything at once, the backbeat included.
fn kept(weight: u8, dialled: f32) -> f32 {
    let gone = 0.375 - 0.125 * f32::from(weight.min(3));
    ((dialled - gone) / 0.125).clamp(0.0, 1.0)
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
/// the drummer plays less — [`kept`] says of what, and in which order; that is an instruction,
/// not a temperature — and above it `drums` fills empty weak steps in with strokes of its own.
/// Two things the lower half never takes: the downbeat, and a `Strong` hit, which is how a
/// groove spells its backbeat. The same sentence about a dropped backbeat holds for a dial as
/// for a quiet section, and at a density of 0.3 one bar in three used to be missing one.
fn survival(accent: Accent, weight: u8, intensity: f32, dialled: f32) -> f32 {
    // The downbeat is never thinned, or the bar loses its footing.
    if weight >= 4 {
        return 1.0;
    }
    let dialled = dialled.clamp(0.0, 1.0);
    match accent {
        Accent::Strong => 1.0,
        Accent::Normal => kept(weight, dialled),
        Accent::Ghost => {
            let strength = match weight {
                0 => 0.72,
                1 => 0.90,
                _ => 1.0,
            };
            let temperature = strength * (0.15 + 0.85 * intensity.clamp(0.0, 1.0));
            // The ornaments lean with the dial either side of its middle, and go with their
            // weight's strokes below it.
            (temperature * (0.5 + dialled) * kept(weight, dialled)).clamp(0.0, 1.0)
        }
    }
}

/// The shape a fill runs in.
///
/// One shape — every free sixteenth, rising — was every fill in every piece, so the one moment a
/// listener is certain to notice sounded the same at every join of everything the composer wrote.
/// These are the shapes a drummer actually reaches for, all on the snare's own note so a custom
/// kit is never sent pitches it did not map; which one plays is drawn from the section's stream,
/// so a clip in a stretch with the same label fills the way the last one did.
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

/// The step of a section's last bar a fill runs from, or `None` when the dial asks for none.
///
/// How much of the bar runs, from none to two beats. The section's intensity still leans on
/// it, so a quiet section fills shorter than a loud one at the same setting — the dial says
/// how much of a fill this piece wants, not how much this one bar gets.
fn fill_from(
    settings: &ScoreSettings,
    section: &SectionPlan,
    grid: crate::rhythm::Grid,
) -> Option<usize> {
    let wanted = settings.fill.clamp(0.0, 1.0) * (0.6 + 0.4 * section.intensity);
    let beats = (wanted * 2.0).round() as usize;
    (beats > 0).then(|| {
        grid.steps_per_bar()
            .saturating_sub(beats * grid.steps_per_beat())
    })
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
    // A clip always leads into something — another clip, or another playing of itself.
    if part.rhythm.is_some() || voice != DrumVoice::Snare || !last_bar {
        return;
    }

    let grid = part_grid(frame, part);
    let steps = grid.steps_per_bar();
    let per_beat = grid.steps_per_beat();
    let Some(from) = fill_from(settings, section, grid) else {
        return;
    };
    let bar_start = grid.bar_ticks() * bar as i64;

    // Drawn from the section's own stream, like every other choice a bar makes.
    const SHAPES: [FillShape; 4] = [
        FillShape::Run,
        FillShape::Tresillo,
        FillShape::Eighths,
        FillShape::Burst,
    ];
    let mut choose = bar_stream(frame, part, section, "fill", bar);
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
    use super::{CELL_BARS, FillShape, accent_level, added, fill_steps, kept, survival};
    use crate::fixture::{Song, bar_steps, notes_of, part};
    use crate::rhythm::{Accent, DrumVoice, Pattern};
    use crate::spec::Role;

    /// The whole kit for `bars` bars of `groove` with no fill, every voice at `density`.
    fn kit(groove: &str, density: f32, intensity: f32, seed: u64, bars: usize) -> Song {
        let mut song = Song {
            seed,
            bars,
            intensity,
            ..Song::with_parts(
                [Role::Kick, Role::Snare, Role::Hat]
                    .map(|role| Song::part(role, |voice| voice.density = Some(density))),
            )
        };
        song.settings.groove = groove.to_string();
        // The fill is the last bar's business and is asserted where it lives.
        song.settings.fill = 0.0;
        song
    }

    /// One kick alone for a bar, playing `rhythm`, at `intensity`.
    fn written_kick(rhythm: &str, intensity: f32) -> Song {
        Song {
            bars: 1,
            intensity,
            ..Song::with_parts([Song::part(Role::Kick, |kick| {
                kick.rhythm = Pattern::parse(rhythm);
            })])
        }
    }

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
        // less, not a temperature — but never the downbeat and never the backbeat.
        assert!(survival(Accent::Normal, 2, 1.0, 0.0) < 1.0);
        assert_eq!(survival(Accent::Ghost, 4, 0.0, 0.0), 1.0, "the downbeat");
        for weight in 0..=3 {
            assert_eq!(
                survival(Accent::Strong, weight, 0.0, 0.0),
                1.0,
                "the backbeat"
            );
        }
        // And the ornaments go no later than the strokes of their own weight.
        for dialled in [0.1, 0.3, 0.45] {
            for weight in 0..=3 {
                assert!(
                    survival(Accent::Ghost, weight, 1.0, dialled)
                        <= survival(Accent::Normal, weight, 1.0, dialled)
                );
            }
        }
    }

    #[test]
    fn the_lower_half_of_the_dial_takes_the_finest_strokes_first() {
        // Playing less is playing the simpler version of the same beat: the sixteenths are gone
        // before an offbeat eighth is touched, the eighths before a beat is.
        assert_eq!(kept(0, 0.5), 1.0, "the middle plays the groove as written");
        assert_eq!(kept(0, 0.375), 0.0, "the sixteenths are gone");
        assert_eq!(kept(1, 0.375), 1.0, "and every eighth is still there");
        assert_eq!(kept(1, 0.25), 0.0);
        assert_eq!(kept(2, 0.25), 1.0);
        assert_eq!(kept(2, 0.125), 0.0);
        assert_eq!(kept(3, 0.125), 1.0);
        assert_eq!(kept(3, 0.0), 0.0, "leaving the downbeat and the backbeat");
        for weight in 0..=3 {
            let mut last = 0.0;
            for tenth in 0..=10 {
                let now = kept(weight, tenth as f32 / 10.0);
                assert!(now >= last, "asking for more took a stroke away");
                last = now;
            }
        }
    }

    #[test]
    fn the_kit_plays_a_two_bar_cell_wherever_the_dial_stands() {
        // The complaint this came out of, as a number: off the dial's middle the kit wrote
        // sixteen different bars in sixteen. Thinning and filling were drawn bar by bar, so a
        // sparser groove was a different set of holes every bar and a busier one a different
        // scatter of strokes. Now the draw belongs to a bar of the cell and comes round again.
        for groove in ["basic-rock", "sixteen-beat", "breakbeat", "bossa-nova"] {
            for density in [0.0, 0.2, 0.3, 0.45, 0.5, 0.55, 0.8, 1.0] {
                for seed in 0..4u64 {
                    let (frame, parts) = kit(groove, density, 0.55, seed, 8).draft();
                    for name in ["kick", "snare", "hat"] {
                        let voice = part(&parts, name);
                        for bar in CELL_BARS..8 {
                            assert_eq!(
                                bar_steps(&frame, voice, bar),
                                bar_steps(&frame, voice, bar - CELL_BARS),
                                "{groove} at {density}, seed {seed}: the {name} of bar {bar} is \
                                 not the bar two before it"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn no_setting_of_the_dial_takes_a_backbeat() {
        // A listener hears a dropped backbeat as a mistake, and that holds for a dial as it does
        // for a quiet section. At a density of 0.3 one bar in three used to be missing one.
        for groove in [
            "basic-rock",
            "eight-beat",
            "sixteen-beat",
            "shuffle",
            "breakbeat",
        ] {
            for density in [0.0, 0.1, 0.3, 0.45] {
                for seed in 0..8u64 {
                    let (frame, parts) = kit(groove, density, 0.3, seed, 4).draft();
                    let snare = part(&parts, "snare");
                    for bar in 0..4 {
                        let steps = bar_steps(&frame, snare, bar);
                        assert!(
                            steps.contains(&4) && steps.contains(&12),
                            "{groove} at {density}, seed {seed}: bar {bar} struck {steps:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_hat_keeps_time_in_every_bar_of_a_quiet_section() {
        // The hat's in-between strokes were spelled as ghosts, and a ghost is something a quiet
        // section leaves out: a verse lost four in ten of basic-rock's offbeat eighths, another
        // four every bar. A timekeeper is the pulse and is spelled as one.
        for (groove, per_bar) in [("basic-rock", 8), ("shuffle", 8), ("sixteen-beat", 16)] {
            for seed in 0..4u64 {
                let (frame, parts) = kit(groove, 0.5, 0.05, seed, 4).draft();
                let hat = part(&parts, "hat");
                for bar in 0..4 {
                    assert_eq!(
                        bar_steps(&frame, hat, bar).len(),
                        per_bar,
                        "{groove}, seed {seed}: the hat of bar {bar} has holes in it"
                    );
                }
            }
        }
    }

    #[test]
    fn a_ghost_is_struck_well_under_a_backbeat_until_the_dial_closes_it() {
        // Played at a quarter of its written distance, a ghost came out at nine tenths of a
        // full stroke: a second backbeat in the wrong place.
        assert!(accent_level(Accent::Ghost, 1.0) < 0.6);
        assert_eq!(accent_level(Accent::Ghost, 0.0), 1.0, "a flat dial is flat");
        assert!(
            accent_level(Accent::Strong, 1.0) < 1.05,
            "the rest stays narrow"
        );

        // And in the kit: the sixteen-beat's ghosted snare beside its own backbeat.
        let (frame, parts) = kit("sixteen-beat", 0.5, 1.0, 0, 4).draft();
        let snare = part(&parts, "snare");
        let level = |ghost: bool| -> f32 {
            let levels: Vec<f32> = snare
                .notes
                .iter()
                .filter(|note| {
                    let step = frame.grid.step_of(note.start) % 16;
                    ghost != (step == 4 || step == 12)
                })
                .map(|note| note.velocity)
                .collect();
            assert!(!levels.is_empty(), "the hottest section plays its ghosts");
            levels.iter().sum::<f32>() / levels.len() as f32
        };
        assert!(
            level(true) < level(false) * 0.6,
            "a ghost at {} beside a backbeat at {}",
            level(true),
            level(false)
        );
    }

    #[test]
    fn the_upper_half_of_the_dial_gives_each_voice_its_own_stroke() {
        // A kick has no ghost: what the dial adds to it is a kick, so it adds few and none on
        // a sixteenth. The snare's additions are ghosts; the hat's are the strokes between its
        // strokes, at their own weight.
        assert_eq!(added(DrumVoice::Kick, 0), None);
        assert_eq!(added(DrumVoice::Kick, 1), Some(Accent::Normal));
        assert_eq!(added(DrumVoice::Snare, 0), Some(Accent::Ghost));
        assert_eq!(added(DrumVoice::ClosedHat, 0), Some(Accent::Normal));
        for voice in [DrumVoice::Kick, DrumVoice::Snare, DrumVoice::ClosedHat] {
            for weight in 2..=4 {
                assert_eq!(added(voice, weight), None, "nothing is added on a beat");
            }
        }

        for seed in 0..8u64 {
            let (frame, parts) = kit("basic-rock", 1.0, 0.7, seed, 4).draft();
            for note in &part(&parts, "kick").notes {
                let step = frame.grid.step_of(note.start);
                assert!(
                    step.is_multiple_of(2),
                    "seed {seed}: the dial put a kick on the sixteenth at step {step}"
                );
            }
        }
    }

    #[test]
    fn a_quiet_section_keeps_the_pattern_and_loses_the_ghosts() {
        // basic-rock spells no ghosts, so even this quiet it plays whole, bar for bar. The old
        // thinning read the intensity over the whole kit: at the default settings it lost one
        // backbeat in nine, and at this one it lost a third of the part.
        for seed in 0..8u64 {
            let (frame, parts) = kit("basic-rock", 0.5, 0.05, seed, 4).draft();
            for (name, steps) in [
                ("kick", vec![0, 6, 10]),
                ("snare", vec![4, 12]),
                ("hat", vec![0, 2, 4, 6, 8, 10, 12, 14]),
            ] {
                let drum = part(&parts, name);
                for bar in 0..4 {
                    let played = bar_steps(&frame, drum, bar);
                    // The cell's second bar may carry the take's answer on top; nothing of the
                    // groove is ever missing under it.
                    assert!(
                        steps.iter().all(|step| played.contains(step)),
                        "seed {seed}: {name} lost a stroke of bar {bar}: {played:?}"
                    );
                    if bar % CELL_BARS == 0 {
                        assert_eq!(played, steps, "seed {seed}: {name}, bar {bar}");
                    }
                }
            }
        }
        // The sixteen-beat's snare does spell them — two a bar — and this cold they are nearly
        // all gone, while its backbeat still lands in every bar. Counted in the cell's first
        // bar, which has no answer in it, and over several seeds, because one seed is one draw.
        let mut ghosts = 0;
        for seed in 0..16u64 {
            let (frame, parts) = kit("sixteen-beat", 0.5, 0.05, seed, 4).draft();
            let snare = part(&parts, "snare");
            for bar in 0..4 {
                let steps = bar_steps(&frame, snare, bar);
                assert!(
                    steps.contains(&4) && steps.contains(&12),
                    "seed {seed}, bar {bar}"
                );
                if bar % CELL_BARS == 0 {
                    ghosts += steps.len() - 2;
                }
            }
        }
        assert!(
            ghosts <= 20,
            "{ghosts} of 64 ghosts survived a nearly silent verse"
        );
    }

    #[test]
    fn the_second_bar_of_the_cell_answers_the_first() {
        // What a seed chooses for a kit, now that it no longer chooses which strokes go
        // missing: one stroke in the back half of the cell's second bar, or none.
        let mut snare_answers = std::collections::BTreeSet::new();
        let mut kick_answers = std::collections::BTreeSet::new();
        for seed in 0..32u64 {
            let (frame, parts) = kit("basic-rock", 0.5, 0.7, seed, 4).draft();
            for (name, answers) in [("kick", &mut kick_answers), ("snare", &mut snare_answers)] {
                let voice = part(&parts, name);
                let first = bar_steps(&frame, voice, 0);
                let second = bar_steps(&frame, voice, 1);
                let extra: Vec<usize> = second
                    .iter()
                    .copied()
                    .filter(|step| !first.contains(step))
                    .collect();
                assert!(
                    first.iter().all(|step| second.contains(step)),
                    "seed {seed}: the {name}'s answer took a stroke away"
                );
                assert!(
                    extra.len() <= 1,
                    "seed {seed}: {name} answered with {extra:?}"
                );
                for step in &extra {
                    assert!(*step >= 8, "seed {seed}: {name} answered early, at {step}");
                    assert!(
                        frame.grid.weight(*step) <= 1,
                        "seed {seed}: {name} answered on a beat, at {step}"
                    );
                }
                answers.insert(extra.first().copied());
            }
            // The pulse the answer is heard against does not answer.
            let hat = part(&parts, "hat");
            assert_eq!(bar_steps(&frame, hat, 0), bar_steps(&frame, hat, 1));

            // A snare's answer is a ghost, and is struck like one.
            let snare = part(&parts, "snare");
            for note in &snare.notes {
                let step = frame.grid.step_of(note.start) % 16;
                if step != 4 && step != 12 {
                    assert!(
                        note.velocity < 0.45,
                        "seed {seed}: the answer at step {step} is struck at {}",
                        note.velocity
                    );
                }
            }
        }
        assert!(
            snare_answers.len() >= 4,
            "thirty-two takes answer only {snare_answers:?} on the snare"
        );
        assert_eq!(
            kick_answers,
            [None, Some(14)].into_iter().collect(),
            "basic-rock's kick has one offbeat eighth free in the back half of its bar"
        );

        // A voice keeping out of the way has nothing to say back: half-time gives the kick and
        // the snare one stroke each, and sparse gives the snare none.
        for groove in ["half-time", "sparse"] {
            for seed in 0..8u64 {
                let (frame, parts) = kit(groove, 0.5, 0.7, seed, 4).draft();
                for name in ["kick", "snare", "hat"] {
                    let voice = part(&parts, name);
                    assert_eq!(
                        bar_steps(&frame, voice, 0),
                        bar_steps(&frame, voice, 1),
                        "{groove}, seed {seed}: the {name} answered"
                    );
                }
            }
        }

        // And the dial takes the answer with the strokes of its own weight.
        for seed in 0..8u64 {
            let (frame, parts) = kit("basic-rock", 0.25, 0.7, seed, 4).draft();
            let snare = part(&parts, "snare");
            assert_eq!(bar_steps(&frame, snare, 1), vec![4, 12], "seed {seed}");
        }
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
    fn two_takes_do_not_always_fill_the_same_way() {
        // The draw has to reach the music: one shape was every fill in every piece, so the one
        // moment a listener is certain to notice sounded the same at every join. The fill
        // window's struck steps now differ from seed to seed.
        let mut windows = std::collections::BTreeSet::new();
        for seed in 1..=8u64 {
            let mut song = Song {
                seed,
                intensity: 0.8,
                ..Song::default()
            };
            song.settings.fill = 1.0;
            let (frame, parts) = song.draft();
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
        let (_, parts) = Song::default().draft();
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
        let (frame, parts) = written_kick("x ~ x ~ x ~ x ~ x ~ x ~ x ~ x ~", 0.05).draft();
        let steps: Vec<usize> = part(&parts, "kick")
            .notes
            .iter()
            .map(|note| frame.grid.step_of(note.start))
            .collect();
        assert_eq!(steps, vec![0, 2, 4, 6, 8, 10, 12, 14]);
    }

    #[test]
    fn a_written_rhythm_is_played_as_written() {
        let (frame, parts) = written_kick("x ~ ~ ~ x ~ ~ ~ x ~ ~ ~ x ~ ~ ~", 0.55).draft();
        let kick = part(&parts, "kick");
        let steps: Vec<usize> = kick
            .notes
            .iter()
            .map(|note| frame.grid.step_of(note.start))
            .collect();
        assert_eq!(steps, vec![0, 4, 8, 12]);
    }

    #[test]
    fn a_valid_sparse_groove_can_fill_even_with_an_empty_snare_row() {
        let mut song = Song {
            intensity: 1.0,
            ..Song::default()
        };
        song.settings.groove = "sparse".to_string();
        song.settings.fill = 1.0;
        let (frame, parts) = song.draft();
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
    fn a_louder_section_plays_more_drum_hits() {
        // Through the ghosts and nothing else, so on a groove that spells some.
        let ghosted = |intensity: f32| -> usize {
            (0..8u64)
                .map(|seed| {
                    let (_, parts) = kit("sixteen-beat", 0.5, intensity, seed, 4).draft();
                    part(&parts, "snare").notes.len()
                })
                .sum()
        };
        let (quiet, loud) = (ghosted(0.1), ghosted(1.0));
        assert!(
            loud > quiet,
            "intensity did not change how much the drummer plays: {loud} against {quiet}"
        );
    }

    #[test]
    fn nudging_density_past_the_middle_only_adds_strokes() {
        // Crossing 0.5 used to insert a ghost draw before every later survival roll in the
        // bar, so the dial's smallest movement rescrambled which hits of the groove survive
        // instead of only thickening the playing. With the ghosts on a stream of their own,
        // everything the kit plays at the middle it still plays above it — the added strokes
        // arrive on top.
        for role in [Role::Kick, Role::Snare, Role::Hat] {
            let voice = |density: f32| {
                let song = Song::with_parts([Song::part(role, |voice| {
                    voice.density = Some(density);
                })]);
                let (_, parts) = song.draft();
                notes_of(part(&parts, role.name()))
                    .into_iter()
                    .map(|(start, pitch, ..)| (start, pitch))
                    .collect::<Vec<_>>()
            };
            let middle = voice(0.5);
            let above = voice(1.0);
            for note in &middle {
                assert!(
                    above.contains(note),
                    "a {} hit at {note:?} was lost by asking for *more*",
                    role.name()
                );
            }
            assert!(
                above.len() > middle.len(),
                "nothing was added to the {} above the middle",
                role.name()
            );
        }
    }
}
