//! The mechanical-performance rule: a part that will play exactly as it was typed.
//!
//! Notes written by hand — by a person in the piano roll or by an agent through `edit_notes` —
//! land on the grid at whatever velocity was typed, and with an empty performance stack they are
//! played exactly so: every attack on the tick, every bar the same. That is the one thing that
//! makes a part sound programmed regardless of how good its notes are, and it has an answer that
//! changes no note: a performance preset for the instrument, copied into the clip's stack.
//!
//! The question has one right answer, which is what makes it a lint: *will this clip play with
//! machine-exact timing?* Whether that is wanted is the author's call — a chiptune or a drum
//! machine is exact on purpose — and the project's suppressions are how that call is recorded.
//!
//! A clip a writer generated is left alone. The composer installs its own performance as it
//! writes, and where it leaves a stack empty — the kick that keeps the time — that is the
//! writer's decision rather than an omission.

use auris_core::NoteTransform;
use auris_core::project::{Project, TrackKind};
use auris_core::time::{TICKS_PER_QUARTER, Ticks};

use super::super::{Diagnostic, Fix, NoteRef, Rule, Severity};

/// Fewer notes than this are a hit or a pickup, too few to sound programmed.
pub const MECHANICAL_MIN_NOTES: usize = 8;
/// The straight grid an exact attack sits on: a thirty-second note.
const STRAIGHT_GRID: i64 = TICKS_PER_QUARTER / 8;
/// The triplet grid an exact attack sits on: a sixteenth-note triplet.
const TRIPLET_GRID: i64 = TICKS_PER_QUARTER / 6;

/// Whether a stack moves timing or dynamics, or adds a gesture, rather than only reshaping or
/// relabelling the written notes.
///
/// Swing is not a performance in this sense: it moves the offbeats by exactly the same amount
/// every time, which is still the machine. Nor are transposition, octave doubling, gate and
/// sustain, which change what is held and not how it is played.
pub(in crate::lint) fn performs(stack: &[NoteTransform]) -> bool {
    stack.iter().any(|stage| match stage {
        NoteTransform::Humanize { amount, .. } => *amount > 0.0,
        NoteTransform::Lean { ticks } => *ticks != 0,
        NoteTransform::Expression { settings } => {
            settings.timing > 0.0
                || settings.velocity > 0.0
                || settings.swell > 0.0
                || settings.accent != 0.0
                || settings.shared > 0.0
        }
        NoteTransform::ForDrumVoice { transforms, .. } => performs(transforms),
        NoteTransform::Groove { .. }
        | NoteTransform::Strum { .. }
        | NoteTransform::Stroke { .. }
        | NoteTransform::Ghost { .. }
        | NoteTransform::Brush { .. }
        | NoteTransform::Slide { .. }
        | NoteTransform::Mute { .. }
        | NoteTransform::Pitch { .. } => true,
        NoteTransform::Swing { .. }
        | NoteTransform::Transpose { .. }
        | NoteTransform::Octaves { .. }
        | NoteTransform::Gate { .. }
        | NoteTransform::Sustain { .. } => false,
    })
}

/// Whether an attack lies exactly on the straight or the triplet grid.
fn on_grid(tick: Ticks) -> bool {
    let tick = tick.raw();
    tick.rem_euclid(STRAIGHT_GRID) == 0 || tick.rem_euclid(TRIPLET_GRID) == 0
}

/// One finding per hand-written instrument or drum clip that will play exactly as typed.
///
/// Singers are left out: a voice has its own timing and ornament pipeline, and its clip's stack
/// is not what makes it sound sung.
pub(super) fn mechanical_clips(project: &Project) -> Vec<Diagnostic> {
    let mut found = Vec::new();
    for track in project.tracks.iter().filter(|t| !t.mixer.mute) {
        let clips = match &track.kind {
            TrackKind::Instrument(instrument) | TrackKind::Drum(instrument) => &instrument.clips,
            TrackKind::Singer(_) | TrackKind::Audio(_) | TrackKind::Bus => continue,
        };
        for clip in clips.iter().filter(|clip| !clip.muted) {
            if clip.recipe.is_some() || performs(&clip.transforms) {
                continue;
            }
            let notes: Vec<_> = clip.playable_notes_with_indices().collect();
            if notes.len() < MECHANICAL_MIN_NOTES
                || !notes
                    .iter()
                    .all(|(_, note)| on_grid(clip.start + note.start))
            {
                continue;
            }
            let mut diagnostic = Diagnostic::new(
                Rule::MechanicalPerformance,
                Severity::Warning,
                clip.start,
                clip.start + clip.length,
            );
            diagnostic.track = Some(track.id);
            // One note names the clip, for a suppression scoped to it and for the panel's hover.
            diagnostic.notes = vec![NoteRef {
                clip: clip.id,
                index: notes[0].0,
            }];
            diagnostic.value = Some(notes.len() as f32);
            diagnostic.fix =
                crate::performance_presets::suggested_preset(track, clip).map(|preset| {
                    Fix::ApplyPerformancePreset {
                        clip: clip.id,
                        preset: preset.to_owned(),
                    }
                });
            found.push(diagnostic);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lint::lint_score;
    use auris_core::project::{ClipId, Note};

    /// A piano track holding one clip of `count` eighth notes on the grid, `nudge` ticks late.
    fn piano_line(count: i64, nudge: i64) -> (Project, ClipId) {
        let mut project = Project::new("Lint", 48_000.0);
        let track = project.add_instrument_track("Piano", "auris.physical.piano");
        let clip = project
            .add_midi_clip(track, "Verse", Ticks::ZERO, Ticks(3840 * 4))
            .unwrap();
        project.midi_clip_mut(clip).unwrap().notes = (0..count)
            .map(|i| Note::new(60 + (i % 5) as u8, Ticks(i * 480 + nudge), Ticks(480)))
            .collect();
        (project, clip)
    }

    fn mechanical(project: &Project) -> Vec<Diagnostic> {
        lint_score(project)
            .into_iter()
            .filter(|d| d.rule == Rule::MechanicalPerformance)
            .collect()
    }

    #[test]
    fn a_typed_line_with_no_performance_is_reported_with_its_instruments_preset() {
        let (project, clip) = piano_line(8, 0);
        let found = mechanical(&project);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].severity, Severity::Warning);
        assert_eq!(found[0].value, Some(8.0));
        assert_eq!(
            found[0].fix,
            Some(Fix::ApplyPerformancePreset {
                clip,
                preset: "piano_pop_comp".into()
            })
        );
        assert!(found[0].describe().contains("piano_pop_comp"));
    }

    #[test]
    fn short_played_generated_and_performed_clips_are_left_alone() {
        assert!(mechanical(&piano_line(7, 0).0).is_empty(), "a pickup");
        let (mut played, clip) = piano_line(8, 0);
        played.midi_clip_mut(clip).unwrap().notes[3].start = Ticks(1450);
        assert!(mechanical(&played).is_empty(), "one attack off the grid");

        let (mut performed, clip) = piano_line(8, 0);
        performed.midi_clip_mut(clip).unwrap().transforms = vec![NoteTransform::Humanize {
            amount: 0.2,
            seed: 1,
        }];
        assert!(mechanical(&performed).is_empty());

        // Swing moves every offbeat by the same amount, which is still the machine.
        let (mut swung, clip) = piano_line(8, 0);
        swung.midi_clip_mut(clip).unwrap().transforms = vec![
            NoteTransform::Swing {
                percent: 60,
                subdivision: auris_core::Subdivision::Eighth,
            },
            NoteTransform::Gate { amount: 0.8 },
        ];
        assert_eq!(mechanical(&swung).len(), 1);

        // A triplet grid is still a grid.
        let mut triplets = Project::new("Lint", 48_000.0);
        let track = triplets.add_instrument_track("Piano", "auris.physical.piano");
        let clip = triplets
            .add_midi_clip(track, "Verse", Ticks::ZERO, Ticks(3840))
            .unwrap();
        triplets.midi_clip_mut(clip).unwrap().notes = (0..9)
            .map(|i| Note::new(60, Ticks(i * 320), Ticks(320)))
            .collect();
        assert_eq!(mechanical(&triplets).len(), 1);
    }

    #[test]
    fn the_fix_installs_the_preset_as_the_clips_own_take_in_one_step() {
        let mut session =
            crate::Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap();
        let track = session.add_default_instrument_track("Piano").unwrap();
        let clip = session
            .add_midi_clip(track, "Verse", Ticks::ZERO, Ticks(3840 * 2))
            .unwrap();
        let notes: Vec<_> = (0..8)
            .map(|i| Note::new(60, Ticks(i * 960), Ticks(960)))
            .collect();
        session.replace_notes(clip, notes.clone()).unwrap();
        let report = session.lint_score();
        let found = report
            .diagnostics
            .iter()
            .find(|d| d.rule == Rule::MechanicalPerformance)
            .expect("a typed clip is reported");
        let Some(Fix::ApplyPerformancePreset { preset, .. }) = &found.fix else {
            panic!("the default instrument has a preset: {:?}", found.fix);
        };
        let expected = session.performance_preset(preset).unwrap().preset;
        let (stack, _) = session.performance_preset_stack(clip, &expected).unwrap();
        session.apply_lint_fix(found, report.revision).unwrap();
        assert_eq!(session.clip_transforms(clip).unwrap(), stack.as_slice());
        let (_, written) = session.project().midi_clip(clip).unwrap();
        assert_eq!(written.notes, notes, "the text is untouched");
        assert!(
            session
                .lint_score()
                .diagnostics
                .iter()
                .all(|d| d.rule != Rule::MechanicalPerformance)
        );
        assert!(session.undo().is_some());
        assert!(session.clip_transforms(clip).unwrap().is_empty());
    }
}
