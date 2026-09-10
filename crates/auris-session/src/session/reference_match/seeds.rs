//! Detached retakes use the same writer and inherited performance seeds as Another Take.

use auris_core::{ClipId, Project, rng::Rng};

#[derive(Clone)]
pub(super) struct Dial {
    clip: ClipId,
    search_seed: u64,
    original_seed: u64,
}

pub(super) fn dials(project: &Project, search_seed: u64) -> Vec<Dial> {
    project
        .tracks
        .iter()
        .filter(|track| !track.mixer.mute)
        .filter_map(|track| track.kind.as_instrument())
        .flat_map(|instrument| &instrument.clips)
        .filter(|clip| !clip.muted && clip.length > auris_core::Ticks::ZERO)
        .filter_map(|clip| {
            clip.recipe.as_ref().map(|recipe| Dial {
                clip: clip.id,
                search_seed,
                original_seed: recipe.seed,
            })
        })
        .collect()
}

impl Dial {
    pub(super) fn adjust(&self, project: &mut Project, occurrence: usize) {
        let Some((_, clip)) = project.midi_clip(self.clip) else {
            return;
        };
        let Some(previous) = clip.recipe.clone() else {
            return;
        };
        let mut rng = Rng::stream(
            self.search_seed,
            &[
                "render_search_take".into(),
                self.clip.0.into(),
                occurrence.into(),
            ],
        );
        let mut seed = rng.next_u64();
        while seed == previous.seed || seed == self.original_seed {
            seed = rng.next_u64();
        }
        let mut recipe = previous.with_seed(seed);
        let notes = auris_compose::write_phrase(
            &project.harmony,
            clip.start,
            clip.length,
            project.signatures.signature_at(clip.start),
            &recipe,
            project.sections.section_at(clip.start),
        );
        recipe.text_digest = auris_core::notes_digest(&notes);
        let clip = project.midi_clip_mut(self.clip).expect("captured clip");
        super::super::generated::retake_performance(&mut clip.transforms, &previous, &recipe);
        clip.notes = notes;
        clip.recipe = Some(recipe);
    }
}

pub(super) fn describe_changes(original: &Project, best: &Project) -> Vec<String> {
    let mut changes = Vec::new();
    for track in &original.tracks {
        let Some(instrument) = track.kind.as_instrument() else {
            continue;
        };
        for clip in &instrument.clips {
            let Some(before) = clip.recipe.as_ref() else {
                continue;
            };
            let Some((_, after)) = best.midi_clip(clip.id) else {
                continue;
            };
            let Some(recipe) = after.recipe.as_ref() else {
                continue;
            };
            if before.seed != recipe.seed {
                changes.push(format!(
                    "{} / {}: take seed {} → {} ({} → {} notes)",
                    track.name,
                    clip.name,
                    before.seed,
                    recipe.seed,
                    clip.notes.len(),
                    after.notes.len()
                ));
            }
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Session, SessionOptions};
    use auris_core::{ClipPreset, ClipRecipe, NoteTransform, Ticks};

    #[test]
    fn retakes_match_the_explicit_writer_and_keep_clip_boundaries_and_custom_seeds() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let track = session
            .add_drum_track("Kit", "auris.synth.noisedrum")
            .unwrap();
        let mut recipe = ClipRecipe::new(ClipPreset::Snare, 42);
        recipe.drum_note = Some(38);
        let clip = session
            .generate_clip(track, Ticks::ZERO, Ticks::from_beats(8.0), recipe)
            .unwrap();
        session
            .project
            .midi_clip_mut(clip)
            .unwrap()
            .transforms
            .push(NoteTransform::Humanize {
                amount: 0.2,
                seed: 991,
            });
        let original = session.project.clone();
        let dial = dials(&original, 77).remove(0);
        let mut candidate = original.clone();
        dial.adjust(&mut candidate, 0);
        let next = candidate.midi_clip(clip).unwrap().1;
        let next_recipe = next.recipe.clone().unwrap();
        assert_ne!(next_recipe.seed, 42);
        session.set_clip_recipe(clip, next_recipe).unwrap();
        assert_eq!(&candidate, session.project());
        assert!(
            next.transforms
                .iter()
                .any(|stage| matches!(stage, NoteTransform::Humanize { seed: 991, .. }))
        );
        assert_eq!(next.start, original.midi_clip(clip).unwrap().1.start);
        assert_eq!(next.length, original.midi_clip(clip).unwrap().1.length);
        let mut repeated = original.clone();
        dial.adjust(&mut repeated, 0);
        assert_eq!(candidate, repeated);
        dial.adjust(&mut repeated, 1);
        assert_ne!(
            candidate.midi_clip(clip).unwrap().1.recipe,
            repeated.midi_clip(clip).unwrap().1.recipe
        );
        assert_eq!(describe_changes(&original, &candidate).len(), 1);
        let mut frozen = original.clone();
        frozen.midi_clip_mut(clip).unwrap().recipe = None;
        assert!(dials(&frozen, 77).is_empty());
    }
}
