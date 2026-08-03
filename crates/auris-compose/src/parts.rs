//! Writing the parts.
//!
//! Every part is a pure function of the frozen [`Frame`] and its own name,
//! so no part can depend on another's notes. What makes them sound like a band anyway is that
//! they all read the same harmony, and the rhythm section all reads the same groove.

use auris_core::time::Ticks;

use crate::frame::{Frame, SectionPlan};
use crate::rhythm::{Accent, DrumVoice, Grid, Pattern, swing_offset};
use crate::rng::{Key as RngKey, Rng};
use crate::spec::{Mood, PartSpec, Role, SongSpec};
use crate::theory::pitch::{OCTAVE, PitchClass, fold_into};

/// A note as the composer writes it, before it becomes a clip.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Draft {
    /// Which section it belongs to.
    pub section: usize,
    /// MIDI pitch.
    pub pitch: u8,
    /// How hard it is struck, from 0 to 1.
    pub velocity: f32,
    /// Where it starts, from the beginning of the song.
    pub start: Ticks,
    /// How long it sounds.
    pub length: Ticks,
}

/// Everything one part plays.
#[derive(Clone, Debug)]
pub struct PartDraft {
    /// The part's name, which becomes its track name.
    pub name: String,
    /// The plugin that plays it.
    pub instrument: String,
    /// Level trim.
    pub gain_db: f32,
    /// Stereo position.
    pub pan: f32,
    /// The notes, in time order.
    pub notes: Vec<Draft>,
}

/// How a part is played, as opposed to what it plays.
///
/// The five dials the writers read that are neither the harmony, the form, nor the part itself.
/// They arrive separately from a [`SongSpec`] so that a caller who has no specification — one
/// regenerating a single clip against the harmony already in a document — can still ask for a
/// part without inventing a whole song around it.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreSettings {
    /// How the music should feel, which sets density and syncopation.
    pub mood: Mood,
    /// How far the offbeats are delayed, as a percentage where 50 is straight.
    pub swing: u8,
    /// How far timing and velocity wander, from 0 for a machine to 1 for a sloppy band.
    pub humanize: f32,
    /// How much a repeat departs from what the section played the first time.
    pub variation: f32,
    /// Which drum groove the rhythm section plays.
    pub groove: String,
}

impl From<&SongSpec> for ScoreSettings {
    fn from(spec: &SongSpec) -> Self {
        Self {
            mood: spec.mood,
            swing: spec.swing,
            humanize: spec.humanize,
            variation: spec.variation,
            groove: spec.groove.clone(),
        }
    }
}

/// Writes every part of a roster against a frame.
pub fn write_parts(settings: &ScoreSettings, roster: &[PartSpec], frame: &Frame) -> Vec<PartDraft> {
    roster
        .iter()
        .map(|part| {
            let mut draft = PartDraft {
                name: part.name.clone(),
                instrument: part.instrument.clone(),
                gain_db: part.gain_db,
                pan: part.pan,
                notes: Vec::new(),
            };
            for (index, section) in frame.sections.iter().enumerate() {
                if !section.parts.is_empty() && !section.parts.contains(&part.name) {
                    continue;
                }
                let notes = match part.role {
                    Role::Melody => melody(settings, frame, section, index, part),
                    Role::Chords | Role::Pad => comp(settings, frame, section, index, part),
                    Role::Arp => arp(settings, frame, section, index, part),
                    Role::Bass => bass(settings, frame, section, index, part),
                    Role::Kick | Role::Snare | Role::Hat => {
                        drums(settings, frame, section, index, part)
                    }
                };
                draft.notes.extend(notes);
            }
            humanise(settings, frame, part, &mut draft.notes);
            draft
                .notes
                .sort_by_key(|note| (note.start.raw(), note.pitch));
            draft
        })
        .collect()
}

/// How a chord is struck through a bar.
///
/// A part that only ever played the chord on every beat wrote the same bar for every seed, so
/// asking it for another take gave back what it had already given. These are the four ways a
/// keyboard player actually comps, and one of them is chosen per bar.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum CompFigure {
    /// Held for as long as the chord lasts.
    Held,
    /// Once on every beat.
    Beats,
    /// On the second half of each beat, which pushes the music forward.
    Offbeats,
    /// Beat one and the half-beat after beat two: the Charleston, and half of pop music.
    Charleston,
}

/// The shape of a bass line through a bar.
///
/// Same reason as [`CompFigure`]: the bass followed the kick and alternated root and fifth, which
/// is one bass line and not a choice of them.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum BassFigure {
    /// The root, and nothing else. Solid, and what most of rock does.
    Root,
    /// Root on the strong hits, fifth on the weak ones: the oldest bass line there is.
    Fifth,
    /// The root, jumping an octave on the weak hits.
    Octave,
    /// Root and fifth, stepping into the next chord on the last hit before it.
    Approach,
}

/// How busy a part is, as a fraction of the available steps.
fn density(settings: &ScoreSettings, part: &PartSpec, section: &SectionPlan) -> f32 {
    let base = part.density.unwrap_or_else(|| settings.mood.density());
    let role = match part.role {
        Role::Melody => 1.0,
        Role::Arp => 1.2,
        Role::Chords => 0.8,
        Role::Pad => 0.4,
        Role::Bass => 0.9,
        _ => 1.0,
    };
    (base * role * (0.55 + 0.45 * section.intensity)).clamp(0.05, 1.0)
}

/// A velocity for a note at grid weight `weight` in a section of `intensity`.
fn velocity(weight: u8, intensity: f32) -> f32 {
    let base = 0.45 + f32::from(weight) * 0.11;
    (base * (0.7 + 0.35 * intensity)).clamp(0.08, 1.0)
}

/// How hard a moment of a section is played, as a multiplier on its notes' velocity.
///
/// A section whose every bar sat at one level sounded like a loop rather than like a passage
/// going somewhere: the only dynamic in the piece was the step between one section's intensity
/// and the next's. This lifts the playing gently across each four-bar phrase and again across the
/// section as a whole, which is what a player does to a repeated figure without being asked to.
///
/// Every part reads it, so the band leans together rather than one instrument at a time.
fn phrase_shape(grid: Grid, section: &SectionPlan, at: Ticks) -> f32 {
    let bar = (at.raw().max(0) / grid.bar_ticks().raw().max(1)) as usize;
    let within = (bar % 4) as f32 / 3.0;
    let through = if section.bars <= 1 {
        0.0
    } else {
        (bar.min(section.bars - 1)) as f32 / (section.bars - 1) as f32
    };
    0.88 + 0.10 * within + 0.08 * through
}

/// The stream one bar of one pass draws its material from.
///
/// Keyed by the section's *name* and not by which playing of it this is, so the second chorus
/// reaches for the same numbers as the first and comes out the same chorus. That is the whole
/// point of a chorus: the composer used to put the instance in every stream, which made every
/// repeat a new piece of music and left the piece with nothing in it to recognise.
///
/// [`SongSpec::variation`] buys the departures back. A bar it selects mixes the instance into the
/// name, so that one bar — and only that one — draws different numbers and plays something else.
fn bar_stream(
    settings: &ScoreSettings,
    frame: &Frame,
    part: &PartSpec,
    section: &SectionPlan,
    pass: &str,
    bar: usize,
) -> Rng {
    let mut path = vec![
        RngKey::Word("part"),
        RngKey::Word(&part.name),
        RngKey::Word(pass),
        RngKey::Word(&section.name),
        RngKey::Index(bar as u64),
    ];
    // The first playing is the one the others are repeats of, so it never departs from itself.
    if section.instance > 1 && settings.variation > 0.0 {
        let mut choose = Rng::stream(
            frame.seed,
            &[
                RngKey::Word("vary"),
                RngKey::Word(&part.name),
                RngKey::Word(pass),
                RngKey::Word(&section.name),
                RngKey::Index(section.instance as u64),
                RngKey::Index(bar as u64),
            ],
        );
        if choose.chance(settings.variation) {
            path.push(RngKey::Index(section.instance as u64));
        }
    }
    Rng::stream(frame.seed, &path)
}

/// Fewest notes a generated figure is allowed to have.
///
/// Three is the smallest number that can carry a shape: two notes are an interval, and one is a
/// note. It is also the smallest [`vary_motif`] has anything to work with.
const MOTIF_MINIMUM: usize = 3;

/// A short figure the melody is built out of.
///
/// Written in scale steps from whatever pitch the frame's skeleton puts under it rather than in
/// absolute notes, so restating it over a different chord keeps its shape while still belonging
/// to the harmony.
#[derive(Clone, Debug)]
struct Motif {
    cells: Vec<Cell>,
}

/// One note of a [`Motif`].
#[derive(Copy, Clone, Debug)]
struct Cell {
    /// Step of the bar it starts on.
    step: usize,
    accent: Accent,
    /// Steps it sounds for. Fewer than the gap to the next cell leaves a rest.
    length: usize,
    /// Scale steps above or below the bar's anchor pitch.
    degree: i32,
}

/// Invents the figure a section is built from.
///
/// Drawn once per part and section and then restated, which is what gives a section something an
/// ear can hold on to. A part with a written rhythm gets that rhythm; only the shape is invented.
fn motif(
    grid: Grid,
    pattern: Option<&Pattern>,
    density: f32,
    syncopation: f32,
    rng: &mut Rng,
) -> Motif {
    let steps = grid.steps_per_bar();
    let mut onsets = bar_onsets(grid, pattern, density, syncopation, rng);
    // A figure needs a few notes to be one, and one note cannot be varied at all. A thin roll
    // used to average out because every bar rolled again; now a single roll decides the whole
    // section, so a thin one would leave the section with nothing rather than with a quiet bar.
    // The strongest free steps are filled first, which is where a note would have gone anyway.
    if pattern.is_none() && onsets.len() < MOTIF_MINIMUM {
        let mut spare: Vec<usize> = (0..steps)
            .filter(|step| !onsets.iter().any(|(taken, _)| taken == step))
            .collect();
        spare.sort_by_key(|step| std::cmp::Reverse(grid.weight(*step)));
        for step in spare.into_iter().take(MOTIF_MINIMUM - onsets.len()) {
            onsets.push((step, Accent::Normal));
        }
        onsets.sort_by_key(|(step, _)| *step);
    }

    let mut cells = Vec::with_capacity(onsets.len());
    let mut degree = 0i32;

    for (position, (step, accent)) in onsets.iter().enumerate() {
        // Mostly steps with the occasional leap, and bounded either side of the anchor: a figure
        // that wandered off would not be recognisable when it came back.
        if position > 0 {
            let move_by = *rng.pick(&[-2, -1, -1, 1, 1, 2, 3, -3]).unwrap_or(&1);
            degree = (degree + move_by).clamp(-6, 6);
        }
        let next = onsets
            .get(position + 1)
            .map(|(next, _)| *next)
            .unwrap_or(steps);
        let gap = next.saturating_sub(*step).max(1);
        // The figure's last note stops short of the next bar, which is where the rest that lets a
        // phrase breathe comes from. Inside the figure a note is occasionally detached too.
        let length = if position + 1 == onsets.len() || rng.chance(0.25) {
            1 + rng.below(gap)
        } else {
            gap
        };
        cells.push(Cell {
            step: *step,
            accent: *accent,
            length: length.clamp(1, gap),
            degree,
        });
    }
    Motif { cells }
}

/// The figure with one thing about it changed.
///
/// Enough to stop four bars of the same bar, not so much that it stops being the same figure —
/// which is the difference between a variation and a different tune.
fn vary_motif(figure: &Motif, rng: &mut Rng) -> Motif {
    let mut cells = figure.cells.clone();
    if cells.len() < 2 {
        return Motif { cells };
    }
    match rng.below(3) {
        // Move the last note somewhere else, which is what turns a statement into a question.
        0 => {
            let last = cells.len() - 1;
            cells[last].degree += if rng.chance(0.5) { 2 } else { -2 };
        }
        // Take a note out, leaving a hole where the ear expects one.
        1 if cells.len() > 2 => {
            let doomed = 1 + rng.below(cells.len() - 1);
            cells.remove(doomed);
        }
        // Turn the figure over from its second note on.
        _ => {
            for cell in cells.iter_mut().skip(1) {
                cell.degree = -cell.degree;
            }
        }
    }
    Motif { cells }
}

/// Picks the onsets of one bar, either from a written rhythm or by rolling one.
///
/// The roll leans on the metric hierarchy: a strong step is far likelier to carry a note than a
/// weak one, which is what makes a generated rhythm feel like it is in the bar rather than
/// scattered across it.
fn bar_onsets(
    grid: Grid,
    pattern: Option<&Pattern>,
    density: f32,
    syncopation: f32,
    rng: &mut Rng,
) -> Vec<(usize, Accent)> {
    let steps = grid.steps_per_bar();
    if let Some(pattern) = pattern {
        return (0..steps)
            .filter_map(|step| pattern.at(step).map(|accent| (step, accent)))
            .collect();
    }
    let mut onsets = Vec::new();
    for step in 0..steps {
        let weight = f32::from(grid.weight(step));
        // Syncopation lifts the weak steps toward the strong ones rather than adding notes.
        let pull = (weight / 4.0) * (1.0 - syncopation) + syncopation * 0.55;
        if rng.chance((density * (0.35 + 1.1 * pull)).clamp(0.0, 0.95)) {
            let accent = if grid.weight(step) >= 3 {
                Accent::Strong
            } else {
                Accent::Normal
            };
            onsets.push((step, accent));
        }
    }
    // A bar with nothing in it reads as a mistake rather than as a rest, so keep the downbeat.
    if onsets.is_empty() {
        onsets.push((0, Accent::Normal));
    }
    onsets
}

/// The tune.
fn melody(
    settings: &ScoreSettings,
    frame: &Frame,
    section: &SectionPlan,
    index: usize,
    part: &PartSpec,
) -> Vec<Draft> {
    let grid = frame.grid;
    let (low, high) = part.range();
    let density = density(settings, part, section);

    // One figure per part and section, restated bar after bar. Keyed by neither the bar nor the
    // instance, so every bar of every playing reaches for the same one.
    let mut invent = Rng::stream(
        frame.seed,
        &[
            RngKey::Word("part"),
            RngKey::Word(&part.name),
            RngKey::Word("motif"),
            RngKey::Word(&section.name),
        ],
    );
    let figure = motif(
        grid,
        part.rhythm.as_ref(),
        density,
        settings.mood.syncopation,
        &mut invent,
    );

    let mut notes = Vec::new();
    for bar in 0..section.bars {
        let mut rng = bar_stream(settings, frame, part, section, "melody", bar);
        // Four bars is the phrase almost everything is built in: state the figure, restate it,
        // and then answer it. The fourth bar is where a tune stops repeating and goes somewhere.
        let closing = bar % 4 == 3;
        let cells = if closing || rng.chance(0.15) {
            vary_motif(&figure, &mut rng)
        } else {
            figure.clone()
        };
        let bar_start = grid.bar_ticks() * bar as i64;

        for cell in &cells.cells {
            let at = bar_start + grid.tick_of(cell.step);
            let Some(event) = section.chord_at(at) else {
                continue;
            };
            let event_index = section.event_index_at(at);
            let weight = grid.weight(cell.step);
            let anchor = section
                .skeleton
                .get(event_index)
                .copied()
                .unwrap_or((low + high) / 2);

            // The figure is written in scale steps from the chord's structural pitch, so it keeps
            // its shape while the harmony moves under it.
            let mut pitch = shift_within(section, anchor, cell.degree, low, high);
            // A note on a strong step has to agree with the chord, or the figure's shape wins an
            // argument with the harmony that it should not be having.
            if weight >= 3 {
                pitch = fold_into(event.chord.nearest_tone(pitch), low, high);
            }

            notes.push(Draft {
                section: index,
                pitch: pitch.clamp(0, 127) as u8,
                velocity: (velocity(weight, section.intensity)
                    * cell.accent.scale()
                    * phrase_shape(grid, section, at))
                .clamp(0.05, 1.0),
                start: section.start + at,
                length: grid.step_ticks() * cell.length.max(1) as i64,
            });
        }
    }
    notes
}

/// The pitch `steps` scale degrees from `from`, in the section's scale.
fn scale_shift(section: &SectionPlan, from: i32, steps: i32) -> i32 {
    let scale = section.key.scale;
    let tonic = section.key.tonic;
    let semitones = tonic.distance_up_to(PitchClass::new(from));
    let octaves = (from - tonic.midi(0) - semitones) / OCTAVE;
    let degree = scale.nearest_degree(semitones) + octaves * scale.degree_count() as i32;
    tonic.midi(0) + scale.semitone(degree + steps)
}

/// `anchor` shifted by `degree` scale steps, kept inside `low..=high` by shrinking the interval.
///
/// Folding an out-of-range note back by octaves moves it twelve semitones, which is a wider leap
/// than any the figure asked for — so a shape chosen to be smooth arrived with a jump in it that
/// nothing had priced. Pulling the interval in instead keeps the direction the figure was going,
/// which is what an ear follows. Folding is kept only for the case where even the anchor is out
/// of range, where there is nothing left to shrink.
fn shift_within(section: &SectionPlan, anchor: i32, degree: i32, low: i32, high: i32) -> i32 {
    let mut steps = degree;
    loop {
        let pitch = scale_shift(section, anchor, steps);
        if (low..=high).contains(&pitch) {
            return pitch;
        }
        if steps == 0 {
            return fold_into(pitch, low, high);
        }
        steps -= steps.signum();
    }
}

/// Chords, either comped in rhythm or held as a pad.
fn comp(
    settings: &ScoreSettings,
    frame: &Frame,
    section: &SectionPlan,
    index: usize,
    part: &PartSpec,
) -> Vec<Draft> {
    let (low, high) = part.range();
    let mut notes = Vec::new();
    let held = part.role == Role::Pad;
    let mut previous: Vec<i32> = Vec::new();

    // How the part sits, decided once for the section. A pad has no rhythm to vary, so this is
    // the whole of what makes one take of it differ from another: which octave it sits in, and
    // which notes of the chord it chooses to sound.
    let mut choose = bar_stream(settings, frame, part, section, "register", 0);
    let register = (choose.below(3) as i32 - 1) * OCTAVE;
    let voicing_variant = choose.below(3);

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
            // Drop the fifth: the note the bass is most likely to be covering anyway.
            1 if voicing.len() > 3 => {
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
        let bar = frame.grid.step_of(event.start) / frame.grid.steps_per_bar().max(1);
        let figure = if held {
            CompFigure::Held
        } else {
            let mut rng = bar_stream(settings, frame, part, section, "comp", bar);
            *rng.pick(&[
                CompFigure::Beats,
                CompFigure::Beats,
                CompFigure::Offbeats,
                CompFigure::Charleston,
                CompFigure::Held,
            ])
            .unwrap_or(&CompFigure::Beats)
        };

        let onsets: Vec<usize> = if figure == CompFigure::Held {
            vec![0]
        } else {
            let beat = (frame.grid.steps_per_beat as usize).max(1);
            let half = (beat / 2).max(1);
            let per_bar = frame.grid.steps_per_bar().max(1);
            let from = frame.grid.step_of(event.start);
            // Measured against the bar rather than against the chord, so a figure stays in step
            // with the beat when two chords share a bar.
            let mut chosen: Vec<usize> = (0..frame.grid.step_of(event.length))
                .filter(|offset| {
                    let at = (from + offset) % per_bar;
                    match figure {
                        CompFigure::Beats => at.is_multiple_of(beat),
                        CompFigure::Offbeats => at % beat == half,
                        CompFigure::Charleston => at == 0 || at == beat + half,
                        CompFigure::Held => false,
                    }
                })
                .collect();
            // A chord nobody strikes is a chord nobody hears change, so its own start always
            // sounds whatever the figure says.
            if !chosen.contains(&0) {
                chosen.insert(0, 0);
            }
            chosen
        };
        let held = held || figure == CompFigure::Held;

        for onset in &onsets {
            let at = event.start + frame.grid.tick_of(*onset);
            if at >= event.end() {
                continue;
            }
            let length = if held {
                event.length
            } else {
                frame.grid.step_ticks() * frame.grid.steps_per_beat as i64
            };
            let length = length.min(event.end() - at);
            let weight = frame.grid.weight(frame.grid.step_of(at));
            for pitch in &voicing {
                notes.push(Draft {
                    section: index,
                    pitch: (*pitch).clamp(0, 127) as u8,
                    velocity: (velocity(weight, section.intensity)
                        * if held { 0.7 } else { 0.9 }
                        * phrase_shape(frame.grid, section, at))
                    .clamp(0.05, 1.0),
                    start: section.start + at,
                    length,
                });
            }
        }
    }
    let _ = settings;
    notes
}

/// A broken chord.
fn arp(
    settings: &ScoreSettings,
    frame: &Frame,
    section: &SectionPlan,
    index: usize,
    part: &PartSpec,
) -> Vec<Draft> {
    let (low, high) = part.range();
    let grid = frame.grid;
    let step_length = grid.step_ticks() * 2;
    let mut notes = Vec::new();
    let mut rng = Rng::stream(
        frame.seed,
        &[
            RngKey::Word("part"),
            RngKey::Word(&part.name),
            RngKey::Word("arp"),
            // Not the instance: a repeat of a section runs its arpeggio the same way round.
            RngKey::Word(&section.name),
        ],
    );
    // One binary choice was the whole of this part's variety, so two seeds wrote the same
    // arpeggio five times out of six. A shape and a span are two more.
    let descending = rng.chance(0.3);
    let turns = rng.chance(0.45);
    let span = 1 + rng.below(2) as i32;

    for event in &section.events {
        let mut voicing: Vec<i32> = Vec::new();
        for octave in 0..span {
            for pitch in event.chord.voiced_from(low + octave * OCTAVE) {
                if pitch <= high {
                    voicing.push(pitch);
                }
            }
        }
        voicing.sort_unstable();
        voicing.dedup();
        if voicing.is_empty() {
            continue;
        }
        if descending {
            voicing.reverse();
        }
        // Up and back down again, without repeating the note it turns on.
        if turns && voicing.len() > 2 {
            let back: Vec<i32> = voicing[1..voicing.len() - 1]
                .iter()
                .rev()
                .copied()
                .collect();
            voicing.extend(back);
        }
        let count = (event.length.raw() / step_length.raw().max(1)) as usize;
        for position in 0..count {
            let at = event.start + step_length * position as i64;
            let pitch = voicing[position % voicing.len()];
            notes.push(Draft {
                section: index,
                pitch: pitch.clamp(0, 127) as u8,
                velocity: (velocity(grid.weight(grid.step_of(at)), section.intensity)
                    * 0.8
                    * phrase_shape(grid, section, at))
                .clamp(0.05, 1.0),
                start: section.start + at,
                length: step_length,
            });
        }
    }
    let _ = settings;
    notes
}

/// The bass line.
///
/// Locked to the kick pattern rather than to the kick *part*: reading the groove keeps the two
/// together without making one part depend on another's notes.
fn bass(
    settings: &ScoreSettings,
    frame: &Frame,
    section: &SectionPlan,
    index: usize,
    part: &PartSpec,
) -> Vec<Draft> {
    let (low, high) = part.range();
    let grid = frame.grid;
    let kick = crate::frame::groove_pattern(&settings.groove, DrumVoice::Kick);
    let mut notes = Vec::new();

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
        // Which line to play over this chord, drawn from the section's own stream so a repeat
        // plays the same line and a different seed plays a different one.
        let bar = grid.step_of(event.start) / grid.steps_per_bar().max(1);
        let mut choose = bar_stream(settings, frame, part, section, "figure", bar);
        let figure = *choose
            .pick(&[
                BassFigure::Root,
                BassFigure::Fifth,
                BassFigure::Fifth,
                BassFigure::Octave,
                BassFigure::Approach,
            ])
            .unwrap_or(&BassFigure::Fifth);

        // The figure decides how busy the line is as well as what it plays. Two lines that hit
        // the same beats and differ only on the weak ones are the same line to a listener.
        let mut onsets: Vec<usize> = match figure {
            // One note under the chord, held: the sound of a bass player staying out of the way.
            BassFigure::Root => Vec::new(),
            // Follow the kick, which is what locks a rhythm section together.
            BassFigure::Fifth | BassFigure::Approach => (0..steps)
                .filter(|offset| kick.at((first + offset) % per_bar).is_some())
                .collect(),
            // The kick, and the half-beats between it: a busier, walking feel.
            BassFigure::Octave => (0..steps)
                .filter(|offset| {
                    let at = (first + offset) % per_bar;
                    kick.at(at).is_some()
                        || at.is_multiple_of((grid.steps_per_beat as usize).max(1))
                })
                .collect(),
        };
        // Always sound the chord's start, so a change of chord is heard whatever the figure.
        if !onsets.contains(&0) {
            onsets.insert(0, 0);
        }

        // Where the next chord's root is, for a line that steps into it.
        let target = section
            .events
            .get(position_in_section + 1)
            .map(|next| fold_into(next.chord.bass_class().midi(part.octave), low, high));

        let last = onsets.len().saturating_sub(1);
        for (position, offset) in onsets.iter().enumerate() {
            let at = event.start + grid.tick_of(*offset);
            let next = onsets
                .get(position + 1)
                .map(|next| grid.tick_of(*next))
                .unwrap_or(event.length);
            let length = (next - grid.tick_of(*offset)).max(grid.step_ticks());
            let strong = position == 0 || grid.weight(grid.step_of(at)) >= 2;
            let pitch = match figure {
                BassFigure::Root => root,
                BassFigure::Fifth => {
                    if strong {
                        root
                    } else {
                        fifth
                    }
                }
                BassFigure::Octave => {
                    if strong {
                        root
                    } else {
                        fold_into(root + OCTAVE, low, high)
                    }
                }
                // A semitone below whatever comes next, on the last hit before it. A bass player
                // reaching for the next chord is the sound of a line going somewhere, and it is
                // the one figure here that needs to know what the next chord is.
                BassFigure::Approach if position == last && last > 0 => {
                    target.map_or(fifth, |next| fold_into(next - 1, low, high))
                }
                BassFigure::Approach => {
                    if strong {
                        root
                    } else {
                        fifth
                    }
                }
            };
            notes.push(Draft {
                section: index,
                pitch: pitch.clamp(0, 127) as u8,
                velocity: (velocity(grid.weight(grid.step_of(at)), section.intensity)
                    * phrase_shape(grid, section, at))
                .clamp(0.05, 1.0),
                start: section.start + at,
                length: length.min(event.end() - at).max(grid.step_ticks()),
            });
        }
    }
    notes
}

/// One drum voice.
fn drums(
    settings: &ScoreSettings,
    frame: &Frame,
    section: &SectionPlan,
    index: usize,
    part: &PartSpec,
) -> Vec<Draft> {
    let voice = match part.role {
        Role::Kick => DrumVoice::Kick,
        Role::Snare => DrumVoice::Snare,
        _ => DrumVoice::ClosedHat,
    };
    // A rhythm the user wrote is played as written. Only the groove's own pattern is thinned,
    // because that is the composer's suggestion rather than an instruction.
    let written = part.rhythm.is_some();
    let pattern = part
        .rhythm
        .clone()
        .unwrap_or_else(|| crate::frame::groove_pattern(&settings.groove, voice));
    let grid = frame.grid;
    let mut notes = Vec::new();

    for bar in 0..section.bars {
        let mut rng = bar_stream(settings, frame, part, section, "drums", bar);
        let bar_start = grid.bar_ticks() * bar as i64;
        // Which steps ended up carrying a hit, so a fill can go round them rather than double
        // them: the pattern says where a hit belongs and thinning may already have taken it away.
        let mut played = vec![false; grid.steps_per_bar()];
        for (step, sounded) in played.iter_mut().enumerate() {
            let Some(accent) = pattern.at(step) else {
                continue;
            };
            let weight = grid.weight(step);
            // A quiet section thins the pattern out rather than playing it softly, which is what
            // a drummer does. The downbeat is never thinned, or the bar loses its footing.
            if !written && weight < 4 {
                let survives =
                    (0.45 + 0.14 * f32::from(weight)) * (0.45 + 0.55 * section.intensity);
                if !rng.chance(survives.clamp(0.0, 1.0)) {
                    continue;
                }
            }
            let at = bar_start + grid.tick_of(step);
            *sounded = true;
            notes.push(Draft {
                section: index,
                pitch: voice.pitch(),
                velocity: (velocity(weight, section.intensity)
                    * accent.scale()
                    * phrase_shape(grid, section, at))
                .clamp(0.08, 1.0),
                start: section.start + at,
                // A one-shot drum ignores its note-off, so the length is only there to make the
                // piano roll readable.
                length: Ticks(120),
            });
        }
        fill(frame, section, index, part, voice, bar, &played, &mut notes);
    }
    notes
}

/// Runs the snare into the section that follows.
///
/// A section that simply stops and is replaced sounds like an edit rather than like an arrival:
/// the join is the one moment a listener is certain to notice, and nothing marked it. Only the
/// last bar of a section that something follows gets one, and only the snare plays it — the other
/// voices keep the groove underneath so the fill has something to be a departure from.
///
/// A part with a written rhythm is left alone, on the same principle as thinning: an instruction
/// is not a suggestion.
#[allow(clippy::too_many_arguments)]
fn fill(
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
    let something_follows = index + 1 < frame.sections.len();
    if part.rhythm.is_some() || voice != DrumVoice::Snare || !last_bar || !something_follows {
        return;
    }

    let grid = frame.grid;
    let steps = grid.steps_per_bar();
    let per_beat = grid.steps_per_beat as usize;
    // One beat of fill, or two when the section is playing hard enough to want the longer run.
    let beats = if section.intensity >= 0.6 { 2 } else { 1 };
    let from = steps.saturating_sub(beats * per_beat).max(1);
    let bar_start = grid.bar_ticks() * bar as i64;

    for step in from..steps {
        if played.get(step).copied().unwrap_or(false) {
            continue;
        }
        // Rising into the downbeat that follows, which is what makes it lead somewhere.
        let through = (step - from) as f32 / (steps - from).max(1) as f32;
        notes.push(Draft {
            section: index,
            pitch: voice.pitch(),
            velocity: (0.45 + 0.5 * through).clamp(0.08, 1.0),
            start: section.start + bar_start + grid.tick_of(step),
            length: Ticks(120),
        });
    }
}

/// Swings, nudges and softens the timing so the part does not sound quantised.
///
/// `humanize: 0` is exactly the identity apart from swing, which is what lets every timing test
/// assert on an exact tick rather than on a tolerance.
fn humanise(settings: &ScoreSettings, frame: &Frame, part: &PartSpec, notes: &mut [Draft]) {
    let grid = frame.grid;
    // Where a player sits against the beat: a hat pushes, a bass drags.
    let push = match part.role {
        Role::Hat => -8.0,
        Role::Melody | Role::Arp => -4.0,
        Role::Bass => 6.0,
        Role::Snare => 10.0,
        _ => 0.0,
    } * settings.humanize;

    for note in notes.iter_mut() {
        let bar_position = note.start.raw().rem_euclid(grid.bar_ticks().raw().max(1));
        let step = grid.step_of(Ticks(bar_position));
        let mut start = note.start + swing_offset(grid, step, settings.swing);
        if settings.humanize > 0.0 {
            // Named by *where the note is* rather than by how many notes came before it, so
            // adding a note to bar one does not re-time the whole song.
            let mut rng = Rng::stream(
                frame.seed,
                &[
                    RngKey::Word("part"),
                    RngKey::Word(&part.name),
                    RngKey::Word("humanize"),
                    RngKey::Index(note.start.raw().max(0) as u64),
                    RngKey::Index(u64::from(note.pitch)),
                ],
            );
            let jitter = rng.jitter(6.0 + 19.0 * settings.humanize) + push;
            start += Ticks(jitter.round() as i64);
            let scale = 1.0 + rng.jitter(0.06 * settings.humanize);
            note.velocity = (note.velocity * scale).clamp(0.05, 1.0);
        }
        note.start = start.max_zero();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::plan;

    fn draft(text: &str) -> (SongSpec, Frame, Vec<PartDraft>) {
        let spec = SongSpec::parse(text).expect("the fixture parses");
        let frame = plan(&spec);
        let parts = write_parts(&ScoreSettings::from(&spec), &spec.parts, &frame);
        (spec, frame, parts)
    }

    /// The steps of `bar` that `draft` starts a note on, without repeats.
    fn bar_steps(frame: &Frame, draft: &PartDraft, bar: usize) -> Vec<usize> {
        let bar_ticks = frame.grid.bar_ticks();
        let start = bar_ticks * bar as i64;
        let mut steps: Vec<usize> = draft
            .notes
            .iter()
            .filter(|note| note.start >= start && note.start < start + bar_ticks)
            .map(|note| frame.grid.step_of(note.start - start))
            .collect();
        steps.sort_unstable();
        steps.dedup();
        steps
    }

    /// Everything `draft` plays in one section, positioned from that section's own start.
    ///
    /// Rebased so two playings of the same section can be compared directly; the velocity travels
    /// as bits because two performances of the same music are equal or they are not.
    fn section_notes(frame: &Frame, draft: &PartDraft, section: usize) -> Vec<(i64, u8, i64, u32)> {
        let start = frame.sections[section].start;
        let mut notes: Vec<(i64, u8, i64, u32)> = draft
            .notes
            .iter()
            .filter(|note| note.section == section)
            .map(|note| {
                (
                    (note.start - start).raw(),
                    note.pitch,
                    note.length.raw(),
                    note.velocity.to_bits(),
                )
            })
            .collect();
        notes.sort_unstable();
        notes
    }

    /// Everything `draft` plays in one section except its final bar.
    ///
    /// The last bar carries the fill into whatever comes next, which is a property of where the
    /// section sits in the form rather than of the section itself — the last section of a piece
    /// has nothing to lead into and so plays the groove to the end.
    fn section_body(frame: &Frame, draft: &PartDraft, section: usize) -> Vec<(i64, u8, i64, u32)> {
        let body = frame.sections[section].length - frame.grid.bar_ticks();
        section_notes(frame, draft, section)
            .into_iter()
            .filter(|(start, ..)| *start < body.raw())
            .collect()
    }

    fn part<'a>(parts: &'a [PartDraft], name: &str) -> &'a PartDraft {
        parts
            .iter()
            .find(|part| part.name == name)
            .unwrap_or_else(|| panic!("no part called {name}"))
    }

    const BASE: &str = "
        form: verse
        chords: @axis
        humanize: 0
        swing: 50
        [section verse]
        bars: 4
    ";

    #[test]
    fn every_default_part_writes_notes() {
        let (_, _, parts) = draft(BASE);
        assert_eq!(parts.len(), 6);
        for part in &parts {
            assert!(!part.notes.is_empty(), "`{}` wrote nothing", part.name);
        }
    }

    #[test]
    fn notes_stay_inside_their_parts_range() {
        let (spec, _, parts) = draft(BASE);
        for (draft, declared) in parts.iter().zip(&spec.parts) {
            if declared.role.is_drum() {
                continue;
            }
            let (low, high) = declared.range();
            for note in &draft.notes {
                assert!(
                    (low..=high).contains(&i32::from(note.pitch)),
                    "`{}` played {} outside {low}..{high}",
                    draft.name,
                    note.pitch
                );
            }
        }
    }

    #[test]
    fn every_pitched_note_belongs_to_the_key() {
        // Not every note has to be a chord tone, but a note outside the scale is a wrong note.
        let (spec, frame, parts) = draft(BASE);
        let section = &frame.sections[0];
        for (draft, declared) in parts.iter().zip(&spec.parts) {
            if declared.role.is_drum() {
                continue;
            }
            for note in &draft.notes {
                let class = PitchClass::new(i32::from(note.pitch));
                let in_scale = section.key.scale.contains(section.key.tonic, class);
                let in_chord = section
                    .chord_at(note.start - section.start)
                    .is_some_and(|event| event.chord.contains(class));
                assert!(
                    in_scale || in_chord,
                    "`{}` played {class} which is in neither the scale nor the chord",
                    draft.name
                );
            }
        }
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
    fn no_note_starts_before_the_song_or_runs_past_it() {
        let (_, frame, parts) = draft(BASE);
        for draft in &parts {
            for note in &draft.notes {
                assert!(note.start >= Ticks::ZERO, "`{}` started early", draft.name);
                assert!(
                    note.start < frame.length,
                    "`{}` started past the end",
                    draft.name
                );
                assert!(
                    note.length > Ticks::ZERO,
                    "`{}` wrote a zero-length note",
                    draft.name
                );
            }
        }
    }

    #[test]
    fn without_humanising_every_note_lands_exactly_on_the_grid() {
        let (_, frame, parts) = draft(BASE);
        let step = frame.grid.step_ticks().raw();
        for draft in &parts {
            for note in &draft.notes {
                assert_eq!(
                    note.start.raw() % step,
                    0,
                    "`{}` placed a note off the grid at {}",
                    draft.name,
                    note.start.raw()
                );
            }
        }
    }

    #[test]
    fn humanising_moves_notes_and_the_seed_decides_where() {
        let straight = draft(BASE).2;
        let loose = draft(&BASE.replace("humanize: 0", "humanize: 0.8")).2;
        let moved = straight
            .iter()
            .zip(&loose)
            .flat_map(|(a, b)| a.notes.iter().zip(&b.notes))
            .filter(|(a, b)| a.start != b.start)
            .count();
        assert!(moved > 0, "humanising did nothing");

        // And it is reproducible.
        let again = draft(&BASE.replace("humanize: 0", "humanize: 0.8")).2;
        for (a, b) in loose.iter().zip(&again) {
            assert_eq!(a.notes, b.notes, "`{}` was not reproducible", a.name);
        }
    }

    #[test]
    fn swing_delays_the_offbeats_of_a_busy_part() {
        let straight = draft(BASE).2;
        let swung = draft(&BASE.replace("swing: 50", "swing: 66")).2;
        let hat_straight = part(&straight, "hat");
        let hat_swung = part(&swung, "hat");
        let delayed = hat_straight
            .notes
            .iter()
            .zip(&hat_swung.notes)
            .filter(|(a, b)| b.start > a.start)
            .count();
        assert!(delayed > 0, "swing moved nothing");
        assert!(
            hat_straight
                .notes
                .iter()
                .zip(&hat_swung.notes)
                .all(|(a, b)| b.start >= a.start),
            "swing must never rush a note"
        );
    }

    #[test]
    fn a_written_rhythm_survives_a_quiet_section() {
        // Thinning is a suggestion about the groove, not licence to ignore an instruction.
        let (_, frame, parts) = draft(
            "
            form: verse
            humanize: 0
            [section verse]
            bars: 1
            intensity: 0.05
            [part kick]
            rhythm: x ~ x ~ x ~ x ~ x ~ x ~ x ~ x ~
            ",
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
            "
            form: verse
            humanize: 0
            [section verse]
            bars: 1
            [part kick]
            rhythm: x ~ ~ ~ x ~ ~ ~ x ~ ~ ~ x ~ ~ ~
            ",
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
    fn a_louder_section_plays_more_drum_hits() {
        let quiet = draft(&BASE.replace("bars: 4", "bars: 4\nintensity: 0.1")).2;
        let loud = draft(&BASE.replace("bars: 4", "bars: 4\nintensity: 1.0")).2;
        assert!(
            part(&loud, "hat").notes.len() > part(&quiet, "hat").notes.len(),
            "intensity did not change how much the drummer plays"
        );
    }

    #[test]
    fn the_bass_sounds_every_chord_change() {
        let (_, frame, parts) = draft(BASE);
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
        let (_, frame, parts) = draft(
            "
            form: verse
            chords: @koakuma
            humanize: 0
            [section verse]
            bars: 4
            ",
        );
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
    fn no_part_plays_a_note_outside_the_scale_or_the_chord() {
        // The fixture deliberately contains a diminished triad and a slash chord, which is where
        // a bass line that assumed a perfect fifth above the sounding bass went wrong.
        for chart in [
            "| I | vii | I | V |",
            "@koakuma",
            "@marusa",
            "@junjo",
            "@blues",
        ] {
            let text = format!(
                "key: C major\nform: verse\nchords: {chart}\nhumanize: 0\n\
                 [section verse]\nbars: 4"
            );
            let (spec, frame, parts) = draft(&text);
            let section = &frame.sections[0];
            for (part_draft, declared) in parts.iter().zip(&spec.parts) {
                if declared.role.is_drum() {
                    continue;
                }
                for note in &part_draft.notes {
                    let class = PitchClass::new(i32::from(note.pitch));
                    let chord = section.chord_at(note.start - section.start);
                    let in_chord = chord.is_some_and(|event| event.chord.contains(class));
                    assert!(
                        section.key.scale.contains(section.key.tonic, class) || in_chord,
                        "`{}` played {class} over {} in `{chart}`",
                        part_draft.name,
                        chord.map(|e| e.chord.to_string()).unwrap_or_default()
                    );
                }
            }
        }
    }

    #[test]
    fn adding_a_part_leaves_the_other_parts_alone() {
        // Every part hangs off the same skeleton, so taking that skeleton from whichever melody
        // part happened to be in the roster meant adding a part rewrote the whole arrangement.
        let base = "form: verse\nchords: @axis\nhumanize: 0\n[section verse]\nbars: 4\n\
                    [part bass]\n[part kick]";
        let before = draft(base).2;
        let after = draft(&format!("{base}\n[part extra]\nrole: pad")).2;
        for name in ["bass", "kick"] {
            assert_eq!(
                part(&before, name).notes,
                part(&after, name).notes,
                "adding a part rewrote `{name}`"
            );
        }
    }

    #[test]
    fn editing_one_section_leaves_the_others_alone() {
        // The humanise stream used to be one sequential draw per part, so a note added anywhere
        // re-timed every note after it.
        let base = "form: verse chorus\nchords: @axis\nhumanize: 0.6\nseed: 3\n\
                    [section verse]\nbars: 2\n[section chorus]\nbars: 2\nintensity: {}";
        let quiet = draft(&base.replace("{}", "0.9")).2;
        let loud = draft(&base.replace("{}", "0.4")).2;
        for (a, b) in quiet.iter().zip(&loud) {
            let verse_a: Vec<&Draft> = a.notes.iter().filter(|n| n.section == 0).collect();
            let verse_b: Vec<&Draft> = b.notes.iter().filter(|n| n.section == 0).collect();
            assert_eq!(
                verse_a, verse_b,
                "changing the chorus rewrote the verse of `{}`",
                a.name
            );
        }
    }

    #[test]
    fn the_bass_follows_the_kick_in_an_odd_meter() {
        // The groove is a bar long, so it has to be read modulo the bar rather than modulo its
        // own sixteen steps, or it drifts against the drums in anything but four four.
        let (_, frame, parts) =
            draft("form: verse\nmeter: 3/4\nchords: @axis\nhumanize: 0\n[section verse]\nbars: 4");
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

    #[test]
    fn a_section_can_leave_a_part_out() {
        let (_, _, parts) = draft(
            "
            form: intro chorus
            humanize: 0

            [section intro]
            parts: bass
            ",
        );
        let hat = part(&parts, "hat");
        // Nothing in the intro, which is section zero.
        assert!(
            hat.notes.iter().all(|note| note.section == 1),
            "the hat played in a section it was left out of"
        );
        assert!(!part(&parts, "bass").notes.is_empty());
    }

    #[test]
    fn the_melody_restates_its_figure_bar_after_bar() {
        // Every bar used to roll its own rhythm from its own stream, so no figure ever recurred
        // and a section had nothing in it to recognise.
        let (_, frame, parts) = draft(
            "form: verse\nchords: @axis\nhumanize: 0\nvariation: 0\n\
             [section verse]\nbars: 8\n[part lead]",
        );
        let lead = part(&parts, "lead");
        let figure = bar_steps(&frame, lead, 0);
        assert!(!figure.is_empty(), "the melody played nothing at all");

        let restated = (0..8)
            .filter(|bar| bar_steps(&frame, lead, *bar) == figure)
            .count();
        assert!(
            restated >= 4,
            "only {restated} of 8 bars restate the figure {figure:?}"
        );
    }

    #[test]
    fn the_melody_leaves_room_to_breathe() {
        // A note used to be held until the next onset or the bar line, so every bar was full of
        // sound from end to end and a phrase never finished — it only stopped.
        let (_, frame, parts) = draft(
            "form: verse\nchords: @axis\nhumanize: 0\n\
             [section verse]\nbars: 8\n[part lead]",
        );
        let lead = part(&parts, "lead");
        let mut longest_rest = Ticks::ZERO;
        let mut sounded_to = Ticks::ZERO;
        for note in &lead.notes {
            longest_rest = longest_rest.max(note.start - sounded_to);
            sounded_to = sounded_to.max(note.start + note.length);
        }
        let beat = frame.grid.signature.ticks_per_beat();
        assert!(
            longest_rest >= beat,
            "the longest rest in eight bars is {} ticks, under one beat of {}",
            longest_rest.raw(),
            beat.raw()
        );
    }

    #[test]
    fn a_repeated_section_plays_the_same_music() {
        // The section instance used to be part of every stream name, so a second chorus shared
        // nothing with the first and the piece had no chorus, only two sections with one name.
        let (_, frame, parts) = draft(
            "form: verse verse\nchords: @axis\nhumanize: 0\nvariation: 0\n\
             [section verse]\nbars: 4",
        );
        assert_eq!(frame.sections.len(), 2);
        for draft in &parts {
            assert_eq!(
                section_body(&frame, draft, 0),
                section_body(&frame, draft, 1),
                "`{}` played a different second verse",
                draft.name
            );
        }
    }

    #[test]
    fn a_section_runs_a_fill_into_the_one_that_follows() {
        // A section that stopped and was replaced sounded like an edit rather than an arrival.
        // The last section of a piece has nothing to lead into, so it keeps the groove instead.
        let (_, frame, parts) = draft(
            "form: verse verse\nchords: @axis\nhumanize: 0\nvariation: 0\n\
             [section verse]\nbars: 4\nintensity: 0.8",
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
    fn a_section_leans_as_it_goes() {
        // Every bar used to be played at one level, so the only dynamic anywhere in a piece was
        // the step from one section's intensity to the next's.
        let (_, frame, parts) = draft(
            "form: verse\nchords: @axis\nhumanize: 0\n\
             [section verse]\nbars: 8\n[part lead]",
        );
        let lead = part(&parts, "lead");
        let mean = |from: i64, to: i64| -> f32 {
            let levels: Vec<f32> = lead
                .notes
                .iter()
                .filter(|note| note.start.raw() >= from && note.start.raw() < to)
                .map(|note| note.velocity)
                .collect();
            levels.iter().sum::<f32>() / levels.len().max(1) as f32
        };
        let half = frame.length.raw() / 2;
        let (early, late) = (mean(0, half), mean(half, frame.length.raw()));
        assert!(
            late > early,
            "the second half of the section is no louder than the first: {late} against {early}"
        );
    }

    #[test]
    fn variation_lets_a_repeat_depart_from_the_first_playing() {
        let text = "form: verse verse\nchords: @axis\nhumanize: 0\nvariation: 1.0\n\
                    [section verse]\nbars: 4\n[part lead]";
        let (_, frame, parts) = draft(text);
        let lead = part(&parts, "lead");
        assert_ne!(
            section_notes(&frame, lead, 0),
            section_notes(&frame, lead, 1),
            "`variation: 1.0` left the repeat identical"
        );
    }

    #[test]
    fn a_figure_too_wide_for_the_range_shrinks_rather_than_folding() {
        // Folding moves a note a whole octave, which is a wider leap than any figure asks for and
        // usually in the opposite direction to the one it was going.
        let (_, frame, _) = draft(BASE);
        let section = &frame.sections[0];
        let (low, high) = Role::Melody.range();
        let anchor = high - 2;
        let pitch = shift_within(section, anchor, 6, low, high);
        assert!(
            (low..=high).contains(&pitch),
            "{pitch} is outside the range"
        );
        assert!(
            pitch > anchor - OCTAVE,
            "a figure reaching upward was folded an octave down: {pitch} from {anchor}"
        );
    }

    #[test]
    fn the_same_spec_writes_the_same_notes_every_time() {
        let first = draft(BASE).2;
        let second = draft(BASE).2;
        for (a, b) in first.iter().zip(&second) {
            assert_eq!(a.notes, b.notes, "`{}` is not deterministic", a.name);
        }
    }

    #[test]
    fn a_different_seed_writes_a_different_piece() {
        let a = draft(&format!("seed: 1\n{BASE}")).2;
        let b = draft(&format!("seed: 2\n{BASE}")).2;
        let melody_a = &part(&a, "lead").notes;
        let melody_b = &part(&b, "lead").notes;
        assert_ne!(melody_a, melody_b, "the seed did not reach the melody");
    }
}
