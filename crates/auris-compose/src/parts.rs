//! Writing the parts.
//!
//! Every part is a pure function of the frozen [`Frame`](crate::frame::Frame) and its own name,
//! so no part can depend on another's notes. What makes them sound like a band anyway is that
//! they all read the same harmony, and the rhythm section all reads the same groove.

use auris_core::time::Ticks;

use crate::frame::{Frame, SectionPlan};
use crate::rhythm::{Accent, DrumVoice, Grid, Pattern, swing_offset};
use crate::rng::{Key as RngKey, Rng};
use crate::spec::{PartSpec, Role, SongSpec};
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

/// Writes every part of a spec against its frame.
pub fn write_parts(spec: &SongSpec, frame: &Frame) -> Vec<PartDraft> {
    spec.parts
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
                    Role::Melody => melody(spec, frame, section, index, part),
                    Role::Chords | Role::Pad => comp(spec, frame, section, index, part),
                    Role::Arp => arp(spec, frame, section, index, part),
                    Role::Bass => bass(spec, frame, section, index, part),
                    Role::Kick | Role::Snare | Role::Hat => {
                        drums(spec, frame, section, index, part)
                    }
                };
                draft.notes.extend(notes);
            }
            humanise(spec, frame, part, &mut draft.notes);
            draft
                .notes
                .sort_by_key(|note| (note.start.raw(), note.pitch));
            draft
        })
        .collect()
}

/// How busy a part is, as a fraction of the available steps.
fn density(spec: &SongSpec, part: &PartSpec, section: &SectionPlan) -> f32 {
    let base = part.density.unwrap_or_else(|| spec.mood.density());
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
    spec: &SongSpec,
    frame: &Frame,
    section: &SectionPlan,
    index: usize,
    part: &PartSpec,
) -> Vec<Draft> {
    let grid = frame.grid;
    let (low, high) = part.range();
    let density = density(spec, part, section);
    let mut notes = Vec::new();
    let mut previous: Option<i32> = None;

    for bar in 0..section.bars {
        let mut rng = Rng::stream(
            frame.seed,
            &[
                RngKey::Word("part"),
                RngKey::Word(&part.name),
                RngKey::Word(&section.name),
                RngKey::Index(section.instance as u64),
                RngKey::Index(bar as u64),
            ],
        );
        let bar_start = grid.bar_ticks() * bar as i64;
        let onsets = bar_onsets(
            grid,
            part.rhythm.as_ref(),
            density,
            spec.mood.syncopation,
            &mut rng,
        );

        for window in 0..onsets.len() {
            let (step, accent) = onsets[window];
            let at = bar_start + grid.tick_of(step);
            let Some(event) = section.chord_at(at) else {
                continue;
            };
            let event_index = section.event_index_at(at);
            let weight = grid.weight(step);

            // The structural pitch anchors the first strong note of each chord; everything else
            // moves around it.
            let anchor = section
                .skeleton
                .get(event_index)
                .copied()
                .unwrap_or((low + high) / 2);
            let pitch = if weight >= 3 || previous.is_none() {
                anchor
            } else if weight >= 1 {
                // A chord tone near where the line already is.
                event.chord.nearest_tone(previous.unwrap_or(anchor))
            } else {
                // A weak beat may sit on a scale tone between the chord tones, which is where
                // passing notes live.
                scale_step(section, previous.unwrap_or(anchor), &mut rng)
            };
            let pitch = fold_into(pitch, low, high);

            // Hold until the next onset, or to the end of the bar.
            let next = onsets
                .get(window + 1)
                .map(|(next_step, _)| grid.tick_of(*next_step))
                .unwrap_or(grid.bar_ticks());
            let length = (bar_start + next - at).max(grid.step_ticks());

            notes.push(Draft {
                section: index,
                pitch: pitch.clamp(0, 127) as u8,
                velocity: velocity(weight, section.intensity) * accent.scale(),
                start: section.start + at,
                length,
            });
            previous = Some(pitch);
        }
    }
    notes
}

/// A neighbouring tone of the section's scale, one or two steps from `from`.
fn scale_step(section: &SectionPlan, from: i32, rng: &mut Rng) -> i32 {
    let scale = section.key.scale;
    let tonic = section.key.tonic;
    let semitones = tonic.distance_up_to(PitchClass::new(from));
    let octaves = (from - tonic.midi(0) - semitones) / OCTAVE;
    let degree = scale.nearest_degree(semitones) + octaves * scale.degree_count() as i32;
    let step = if rng.chance(0.5) { 1 } else { -1 };
    tonic.midi(0) + scale.semitone(degree + step)
}

/// Chords, either comped in rhythm or held as a pad.
fn comp(
    spec: &SongSpec,
    frame: &Frame,
    section: &SectionPlan,
    index: usize,
    part: &PartSpec,
) -> Vec<Draft> {
    let (low, high) = part.range();
    let mut notes = Vec::new();
    let held = part.role == Role::Pad;
    let mut previous: Vec<i32> = Vec::new();

    for event in &section.events {
        // Voiced as close to the last chord as possible, which is what voice leading is: the
        // notes that can stay, stay.
        let mut voicing: Vec<i32> = event
            .chord
            .classes()
            .iter()
            .map(|class| {
                let target = previous
                    .iter()
                    .copied()
                    .min_by_key(|pitch| (pitch - class.midi(4)).abs())
                    .unwrap_or((low + high) / 2);
                let mut pitch = class.midi(target.div_euclid(OCTAVE) - 1);
                pitch = fold_into(pitch, low, high);
                pitch
            })
            .collect();
        voicing.sort_unstable();
        voicing.dedup();
        previous.clone_from(&voicing);

        let onsets: Vec<usize> = if held {
            vec![0]
        } else {
            let step = frame.grid.steps_per_beat as usize;
            (0..frame.grid.step_of(event.length))
                .step_by(step.max(1))
                .collect()
        };

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
                    velocity: velocity(weight, section.intensity) * if held { 0.7 } else { 0.9 },
                    start: section.start + at,
                    length,
                });
            }
        }
    }
    let _ = spec;
    notes
}

/// A broken chord.
fn arp(
    spec: &SongSpec,
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
            RngKey::Index(section.instance as u64),
        ],
    );
    let descending = rng.chance(0.3);

    for event in &section.events {
        let mut voicing = event.chord.voiced_from(low);
        voicing.retain(|pitch| *pitch <= high);
        if voicing.is_empty() {
            continue;
        }
        if descending {
            voicing.reverse();
        }
        let count = (event.length.raw() / step_length.raw().max(1)) as usize;
        for position in 0..count {
            let at = event.start + step_length * position as i64;
            let pitch = voicing[position % voicing.len()];
            notes.push(Draft {
                section: index,
                pitch: pitch.clamp(0, 127) as u8,
                velocity: velocity(grid.weight(grid.step_of(at)), section.intensity) * 0.8,
                start: section.start + at,
                length: step_length,
            });
        }
    }
    let _ = spec;
    notes
}

/// The bass line.
///
/// Locked to the kick pattern rather than to the kick *part*: reading the groove keeps the two
/// together without making one part depend on another's notes.
fn bass(
    spec: &SongSpec,
    frame: &Frame,
    section: &SectionPlan,
    index: usize,
    part: &PartSpec,
) -> Vec<Draft> {
    let (low, high) = part.range();
    let grid = frame.grid;
    let kick = crate::frame::groove_pattern(spec, DrumVoice::Kick);
    let mut notes = Vec::new();

    for event in &section.events {
        let root = event.chord.bass_class().midi(part.octave);
        let root = fold_into(root, low, high);
        let fifth = fold_into(root + 7, low, high);

        let first = grid.step_of(event.start);
        let steps = grid.step_of(event.length).max(1);
        // Follow the kick inside this chord, and always sound its start so a change is heard.
        let mut onsets: Vec<usize> = (0..steps)
            .filter(|offset| kick.at(first + offset).is_some())
            .collect();
        if !onsets.contains(&0) {
            onsets.insert(0, 0);
        }

        for (position, offset) in onsets.iter().enumerate() {
            let at = event.start + grid.tick_of(*offset);
            let next = onsets
                .get(position + 1)
                .map(|next| grid.tick_of(*next))
                .unwrap_or(event.length);
            let length = (next - grid.tick_of(*offset)).max(grid.step_ticks());
            // Root on the strong hits, fifth on the weak ones: the oldest bass line there is.
            let pitch = if position == 0 || grid.weight(grid.step_of(at)) >= 2 {
                root
            } else {
                fifth
            };
            notes.push(Draft {
                section: index,
                pitch: pitch.clamp(0, 127) as u8,
                velocity: velocity(grid.weight(grid.step_of(at)), section.intensity),
                start: section.start + at,
                length: length.min(event.end() - at).max(grid.step_ticks()),
            });
        }
    }
    notes
}

/// One drum voice.
fn drums(
    spec: &SongSpec,
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
    let pattern = part
        .rhythm
        .clone()
        .unwrap_or_else(|| crate::frame::groove_pattern(spec, voice));
    let grid = frame.grid;
    let mut notes = Vec::new();

    for bar in 0..section.bars {
        let mut rng = Rng::stream(
            frame.seed,
            &[
                RngKey::Word("part"),
                RngKey::Word(&part.name),
                RngKey::Word("drums"),
                RngKey::Index(section.instance as u64),
                RngKey::Index(bar as u64),
            ],
        );
        let bar_start = grid.bar_ticks() * bar as i64;
        for step in 0..grid.steps_per_bar() {
            let Some(accent) = pattern.at(step) else {
                continue;
            };
            let weight = grid.weight(step);
            // A quiet section thins the pattern out rather than playing it softly, which is what
            // a drummer does. The downbeat is never thinned, or the bar loses its footing.
            if weight < 4 {
                let survives =
                    (0.45 + 0.14 * f32::from(weight)) * (0.45 + 0.55 * section.intensity);
                if !rng.chance(survives.clamp(0.0, 1.0)) {
                    continue;
                }
            }
            notes.push(Draft {
                section: index,
                pitch: voice.pitch(),
                velocity: (velocity(weight, section.intensity) * accent.scale()).clamp(0.08, 1.0),
                start: section.start + bar_start + grid.tick_of(step),
                // A one-shot drum ignores its note-off, so the length is only there to make the
                // piano roll readable.
                length: Ticks(120),
            });
        }
    }
    notes
}

/// Swings, nudges and softens the timing so the part does not sound quantised.
///
/// `humanize: 0` is exactly the identity apart from swing, which is what lets every timing test
/// assert on an exact tick rather than on a tolerance.
fn humanise(spec: &SongSpec, frame: &Frame, part: &PartSpec, notes: &mut [Draft]) {
    let grid = frame.grid;
    let mut rng = Rng::stream(
        frame.seed,
        &[
            RngKey::Word("part"),
            RngKey::Word(&part.name),
            RngKey::Word("humanize"),
        ],
    );
    // Where a player sits against the beat: a hat pushes, a bass drags.
    let push = match part.role {
        Role::Hat => -8.0,
        Role::Melody | Role::Arp => -4.0,
        Role::Bass => 6.0,
        Role::Snare => 10.0,
        _ => 0.0,
    } * spec.humanize;

    for note in notes.iter_mut() {
        let bar_position = note.start.raw().rem_euclid(grid.bar_ticks().raw().max(1));
        let step = grid.step_of(Ticks(bar_position));
        let mut start = note.start + swing_offset(grid, step, spec.swing);
        if spec.humanize > 0.0 {
            let jitter = rng.jitter(6.0 + 19.0 * spec.humanize) + push;
            start += Ticks(jitter.round() as i64);
            let scale = 1.0 + rng.jitter(0.06 * spec.humanize);
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
        let parts = write_parts(&spec, &frame);
        (spec, frame, parts)
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
