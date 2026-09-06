//! Clips that write themselves.
//!
//! A [`MidiClip`](auris_core::MidiClip) may carry a [`ClipRecipe`]: a preset, a seed and a few
//! dials saying how the notes in it were written from the harmony underneath. It can then be
//! written again — after the chords move, or with a different feel, or simply as another take —
//! and [`Session::freeze_clip`] drops the recipe when one of the takes turns out to be the keeper.
//!
//! Nothing here is a third kind of track, and none of it is visible downstream: the notes are
//! stored like anybody else's, so the engine, the exporter and the piano roll never learn that a
//! composer was involved. That is why these commands are a file of their own rather than a
//! [`TrackKind`](auris_core::TrackKind) — see [`crate::guide::harmony`] for what the alternative
//! would have cost.
//!
//! [`Session::phrase`] is the one thing here that the rest of the module reaches for: `clips`
//! calls it when a drag makes a generated clip longer or trims it from the front, because
//! stretching a recipe means writing it again rather than repeating what was there.

use auris_core::time::Ticks;
use auris_core::{ClipId, ClipRecipe, Note, TrackId};

use crate::error::SessionError;
use crate::history::Edit;

use super::Session;

impl Session {
    /// Writes a clip on `track` from the harmony underneath it.
    ///
    /// The clip keeps its recipe, so it can be written again after the chords change or with a
    /// different feel. Its notes are stored like anybody else's: the engine, the exporter and the
    /// piano roll never learn that a composer was involved.
    ///
    /// A range with no chords under it produces an empty clip rather than an error. That is the
    /// honest answer — there was nothing to play — and it leaves something on the timeline to
    /// aim the next progression at.
    pub fn generate_clip(
        &mut self,
        track: TrackId,
        start: Ticks,
        length: Ticks,
        recipe: ClipRecipe,
    ) -> Result<ClipId, SessionError> {
        let index = self.require_track(track)?;
        if self.project.tracks[index].kind.as_instrument().is_none() {
            return Err(SessionError::WrongTrackKind {
                id: track.0,
                actual: self.project.tracks[index].kind.label(),
                expected: "an instrument track",
            });
        }
        let start = self.snap(start);
        let length = Ticks(length.raw().max(1));
        let mut recipe = recipe;
        if let Some(map) = self.project.tracks[index]
            .kind
            .as_instrument()
            .and_then(|instrument| auris_core::project::DrumMap::load(&instrument.instrument_state))
        {
            auris_compose::apply_drum_map(&mut recipe, &map);
        }
        let notes = self.phrase(start, length, &recipe);
        recipe.text_digest = auris_core::notes_digest(&notes);

        self.record(Edit::GenerateClip);
        let id = self
            .project
            .add_midi_clip(track, recipe.preset.name(), start, length)
            .ok_or(SessionError::UnknownTrack(track.0))?;
        if let Some(clip) = self.project.midi_clip_mut(id) {
            clip.notes = notes;
            // The feel the preset starts with — the lean and the wander a recipe used to bake
            // into the notes arrive as the performance stack instead, where the panel edits
            // them and writing the text again leaves them alone.
            clip.transforms = auris_compose::clip_performance(recipe.preset, recipe.seed);
            clip.recipe = Some(recipe);
        }
        self.invalidate_graph();
        Ok(id)
    }

    /// Writes a generated clip's notes again from its own recipe, and returns how many there are.
    ///
    /// Within one build, unchanged harmony writes the same notes back, which is what makes it
    /// safe to press; what it is for is the other case, where the chords underneath moved and the
    /// part should follow them. Across a composer update it is instead a redraw in the current
    /// style — the old take was only ever the stored notes, and cannot be re-derived once they
    /// are replaced. "Keep this one" is [`Session::freeze_clip`], not a seed written down.
    pub fn regenerate_clip(&mut self, clip: ClipId) -> Result<usize, SessionError> {
        let recipe = self.recipe_of(clip)?;
        self.rewrite(clip, recipe)
    }

    /// Writes another take of a generated clip, and returns how many notes it has.
    ///
    /// The next seed rather than a random one, so pressing it twice from the same starting point
    /// lands in the same place and a take somebody liked can be got back to.
    pub fn reroll_clip(&mut self, clip: ClipId) -> Result<usize, SessionError> {
        let recipe = self.recipe_of(clip)?;
        let next = recipe.seed.wrapping_add(1);
        self.rewrite(clip, recipe.with_seed(next))
    }

    /// Replaces a generated clip's recipe and writes its notes again.
    pub fn set_clip_recipe(
        &mut self,
        clip: ClipId,
        mut recipe: ClipRecipe,
    ) -> Result<usize, SessionError> {
        let previous = self.recipe_of(clip)?;
        update_kit_dials(&previous, &mut recipe);
        self.rewrite(clip, recipe)
    }

    /// Regenerates one independent drum writer, preserving every other voice's stored notes.
    pub fn regenerate_drum_voice(
        &mut self,
        clip: ClipId,
        voice: &str,
    ) -> Result<usize, SessionError> {
        let recipe = self.drum_voice_recipe(clip, voice)?;
        self.set_drum_voice_recipe(clip, voice, recipe)
    }

    /// Draws another take of one drum writer while leaving the rest of its kit unchanged.
    pub fn reroll_drum_voice(&mut self, clip: ClipId, voice: &str) -> Result<usize, SessionError> {
        let recipe = self.drum_voice_recipe(clip, voice)?;
        self.set_drum_voice_recipe(clip, voice, recipe.with_seed(recipe.seed.wrapping_add(1)))
    }

    /// Changes one drum writer's settings and rewrites only its notes, as one undoable edit.
    ///
    /// The role and destination sound stay with the voice. Form-dependent fixed accents cannot
    /// be replaced by a generic rhythmic writer through this command.
    pub fn set_drum_voice_recipe(
        &mut self,
        clip: ClipId,
        voice: &str,
        mut writer: ClipRecipe,
    ) -> Result<usize, SessionError> {
        let previous = self.drum_voice_recipe(clip, voice)?;
        if writer.preset != previous.preset || !writer.drum_voices.is_empty() {
            return Err(SessionError::InvalidDrumRecipe(
                "a voice must keep its drum writer".into(),
            ));
        }
        let mut recipe = self.recipe_of(clip)?;
        let map = recipe.drum_map.clone();
        let component = recipe
            .drum_voices
            .iter_mut()
            .find(|part| part.name == voice)
            .ok_or_else(|| {
                SessionError::InvalidDrumRecipe(format!("unknown drum voice `{voice}`"))
            })?;
        writer.drum_map = None;
        writer.drum_note = Some(match &map {
            Some(map) => *map.voices.get(&component.role).ok_or_else(|| {
                SessionError::InvalidDrumRecipe(format!("`{voice}` has no assigned sound"))
            })?,
            None => component.note,
        });
        let (_, midi) = self
            .project
            .midi_clip(clip)
            .ok_or(SessionError::UnknownClip(clip.0))?;
        let mut written = self.phrase(midi.start, midi.length, &writer);
        for note in &mut written {
            note.drum_voice = voice.to_string();
        }
        let count = written.len();
        writer.text_digest = auris_core::notes_digest(&written);
        component.recipe = Some(Box::new(writer));
        let mut notes: Vec<Note> = midi
            .notes
            .iter()
            .filter(|note| note.drum_voice != voice)
            .cloned()
            .collect();
        notes.extend(written);
        // Preserve the original writer order for simultaneous hits on the same key. A plugin
        // may retrigger that key, making the last velocity significant even at the same tick.
        notes.sort_by_key(|note| {
            let voice = recipe
                .drum_voices
                .iter()
                .position(|voice| voice.name == note.drum_voice)
                .unwrap_or(usize::MAX);
            (note.start.raw(), note.pitch, voice)
        });
        let digest = auris_core::notes_digest(&notes);
        // An edit on another voice survives this command and must keep the full-kit warning.
        recipe.text_digest = if self.clip_hand_edited(clip) {
            if digest == 1 { 2 } else { digest ^ 1 }
        } else {
            digest
        };
        if midi.notes == notes && midi.recipe.as_ref() == Some(&recipe) {
            return Ok(count);
        }
        self.record(Edit::GenerateClip);
        if let Some(midi) = self.project.midi_clip_mut(clip) {
            midi.notes = notes;
            midi.recipe = Some(recipe);
        }
        self.invalidate_graph();
        Ok(count)
    }

    fn drum_voice_recipe(&self, clip: ClipId, voice: &str) -> Result<ClipRecipe, SessionError> {
        self.recipe_of(clip)?
            .drum_voices
            .iter()
            .find(|part| part.name == voice)
            .and_then(|part| part.recipe.as_deref())
            .cloned()
            .ok_or_else(|| {
                SessionError::InvalidDrumRecipe(format!("`{voice}` is not a generated drum voice"))
            })
    }

    /// Drops a clip's recipe, leaving its notes exactly where they are.
    ///
    /// What "keep this one" means. The notes stop being derived from anything, so nothing can
    /// rewrite them afterwards — which is the point.
    pub fn freeze_clip(&mut self, clip: ClipId) -> Result<(), SessionError> {
        self.recipe_of(clip)?;
        self.record(Edit::FreezeClip);
        if let Some(clip) = self.project.midi_clip_mut(clip) {
            clip.recipe = None;
        }
        Ok(())
    }

    /// Drops every recipe on a track, and returns how many clips stopped being generated.
    pub fn freeze_track(&mut self, track: TrackId) -> Result<usize, SessionError> {
        let index = self.require_track(track)?;
        let Some(instrument) = self.project.tracks[index].kind.as_instrument() else {
            return Ok(0);
        };
        let generated = instrument
            .clips
            .iter()
            .filter(|clip| clip.is_generated())
            .count();
        if generated == 0 {
            return Ok(0);
        }
        self.record(Edit::FreezeClip);
        if let Some(instrument) = self.project.tracks[index].kind.as_instrument_mut() {
            for clip in &mut instrument.clips {
                clip.recipe = None;
            }
        }
        Ok(generated)
    }

    /// The recipe a clip was written from.
    pub fn clip_recipe(&self, clip: ClipId) -> Option<&ClipRecipe> {
        self.project.midi_clip(clip)?.1.recipe.as_ref()
    }

    /// Whether a generated clip's notes have been edited by hand since the composer wrote them.
    ///
    /// Read against the digest every write stamps into the recipe
    /// ([`ClipRecipe::text_digest`]), so a note moved, struck softer, repitched or deleted all
    /// answer `true` — and an edit undone answers `false` again, because the digest is exact.
    /// The interface shows this beside the recipe's own controls: writing the clip again is
    /// still every bit as allowed, but it replaces the edits, and that is worth a sentence on
    /// screen *before* the button rather than a surprise after it.
    ///
    /// `false` for a clip with no recipe (nothing can rewrite it), and for a recipe carrying no
    /// digest — a file from before the field — because a warning that cannot be trusted teaches
    /// people to ignore the one that can.
    pub fn clip_hand_edited(&self, clip: ClipId) -> bool {
        let Some((_, midi)) = self.project.midi_clip(clip) else {
            return false;
        };
        let Some(recipe) = &midi.recipe else {
            return false;
        };
        recipe.text_digest != 0 && auris_core::notes_digest(&midi.notes) != recipe.text_digest
    }

    /// The recipe of a clip that has one, or the reason it has not.
    fn recipe_of(&self, clip: ClipId) -> Result<ClipRecipe, SessionError> {
        let Some((_, midi)) = self.project.midi_clip(clip) else {
            return Err(SessionError::UnknownClip(clip.0));
        };
        midi.recipe
            .clone()
            .ok_or(SessionError::NotGenerated(clip.0))
    }

    /// Writes `recipe` onto `clip` and replaces its notes with what that recipe says.
    fn rewrite(&mut self, clip: ClipId, mut recipe: ClipRecipe) -> Result<usize, SessionError> {
        let Some((_, midi)) = self.project.midi_clip(clip) else {
            return Err(SessionError::UnknownClip(clip.0));
        };
        let (start, length) = (midi.start, midi.length);
        let notes = self.phrase(start, length, &recipe);
        recipe.text_digest = auris_core::notes_digest(&notes);
        let written = notes.len();
        let mut transforms = if midi
            .recipe
            .as_ref()
            .is_some_and(|previous| previous.preset == recipe.preset)
        {
            midi.transforms.clone()
        } else {
            auris_compose::clip_performance(recipe.preset, recipe.seed)
        };
        if let Some(previous) = &midi.recipe {
            retake_performance(&mut transforms, previous, &recipe);
        }
        if midi.notes == notes
            && midi.recipe.as_ref() == Some(&recipe)
            && midi.transforms == transforms
        {
            return Ok(written);
        }

        self.record(Edit::GenerateClip);
        if let Some(midi) = self.project.midi_clip_mut(clip) {
            midi.notes = notes;
            midi.transforms = transforms;
            midi.recipe = Some(recipe);
        }
        self.invalidate_graph();
        Ok(written)
    }

    /// The notes a recipe writes over a stretch of this document's harmony.
    ///
    /// The section under the clip's start travels along as the composer's hint: two clips
    /// written into stretches with the same label draw the same figures, which is what makes
    /// the second サビ recognisably the first.
    pub(super) fn phrase(&self, start: Ticks, length: Ticks, recipe: &ClipRecipe) -> Vec<Note> {
        auris_compose::write_phrase(
            &self.project.harmony,
            start,
            length,
            // The meter the clip begins in. `write_phrase` builds every figure on one grid, so a
            // clip is written in one meter however many the timeline holds.
            self.project.signatures.signature_at(start),
            // No tempo goes along any more: the humanisation that needed one to turn its
            // milliseconds into ticks lives on the clip's transform stack now, where the
            // renderer hands it the tempo actually in force at playback.
            recipe,
            self.project.sections.section_at(start),
        )
    }
}

/// A generated wander follows its take's seed while retaining any edited amount or custom seed.
fn retake_performance(
    transforms: &mut [auris_core::NoteTransform],
    previous: &ClipRecipe,
    next: &ClipRecipe,
) {
    for transform in transforms {
        match transform {
            auris_core::NoteTransform::Humanize { seed, .. } if *seed == previous.seed => {
                *seed = next.seed
            }
            auris_core::NoteTransform::ForDrumVoice { voice, transforms } => {
                let old = previous
                    .drum_voices
                    .iter()
                    .find(|part| part.name == *voice)
                    .and_then(|part| part.recipe.as_deref());
                let new = next
                    .drum_voices
                    .iter()
                    .find(|part| part.name == *voice)
                    .and_then(|part| part.recipe.as_deref());
                if let (Some(old), Some(new)) = (old, new) {
                    retake_performance(transforms, old, new);
                }
            }
            _ => {}
        }
    }
}

/// Applies changed kit controls to its writers without flattening their independent settings.
fn update_kit_dials(previous: &ClipRecipe, next: &mut ClipRecipe) {
    for voice in &mut next.drum_voices {
        let Some(writer) = &mut voice.recipe else {
            continue;
        };
        if previous.density != next.density {
            writer.density = next.density;
        }
        if previous.intensity != next.intensity {
            writer.intensity = next.intensity;
        }
        if previous.groove != next.groove {
            writer.groove = next.groove.clone();
        }
        if previous.swing != next.swing {
            writer.swing = next.swing;
        }
        if previous.subdivision != next.subdivision {
            writer.subdivision = next.subdivision;
        }
        if previous.gate != next.gate {
            writer.gate = next.gate;
        }
        if previous.dynamics != next.dynamics {
            writer.dynamics = next.dynamics;
        }
        if previous.syncopation != next.syncopation {
            writer.syncopation = next.syncopation;
        }
        if previous.fill != next.fill {
            writer.fill = next.fill;
        }
        if previous.rhythm != next.rhythm {
            writer.rhythm = next.rhythm.clone();
        }
        if previous.seed != next.seed
            && previous
                .drum_voices
                .iter()
                .find(|old| old.name == voice.name)
                .and_then(|old| old.recipe.as_ref())
                .is_some_and(|old| old.seed == writer.seed)
        {
            writer.seed = writer
                .seed
                .wrapping_add(next.seed.wrapping_sub(previous.seed));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::{BAR, Scratch, session, with_a_progression};
    use auris_core::{ClipPreset, NoteTransform};

    #[test]
    fn future_generation_uses_the_tracks_accepted_map_and_pins_it_for_regeneration() {
        use auris_core::project::{DrumMap, DrumRole};
        let (mut session, track) = with_a_progression();
        let map = DrumMap {
            voices: [(DrumRole::Snare, 84)].into_iter().collect(),
        };
        map.store(
            &mut session
                .project
                .track_mut(track)
                .unwrap()
                .kind
                .as_instrument_mut()
                .unwrap()
                .instrument_state,
        );
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                BAR * 4,
                ClipRecipe::new(ClipPreset::Drums, 2),
            )
            .unwrap();
        let before = session.midi_clip(clip).unwrap().notes.clone();
        assert!(!before.is_empty());
        assert!(
            before
                .iter()
                .all(|note| note.pitch == 84 && note.drum_voice == "snare")
        );
        assert_eq!(session.clip_recipe(clip).unwrap().drum_map, Some(map));
        DrumMap::default().store(
            &mut session
                .project
                .track_mut(track)
                .unwrap()
                .kind
                .as_instrument_mut()
                .unwrap()
                .instrument_state,
        );
        session.regenerate_clip(clip).unwrap();
        assert_eq!(session.midi_clip(clip).unwrap().notes, before);
        let silent = session
            .generate_clip(
                track,
                BAR * 4,
                BAR * 4,
                ClipRecipe::new(ClipPreset::Drums, 2),
            )
            .unwrap();
        assert!(session.midi_clip(silent).unwrap().notes.is_empty());
    }

    #[test]
    fn rewriting_one_drum_preserves_other_voices_edits_and_performance() {
        let (mut session, track) = with_a_progression();
        let piece = auris_compose::compose(&auris_compose::SongSpec::default());
        let draft = &piece
            .tracks
            .iter()
            .find(|track| !track.drum_parts.is_empty())
            .unwrap()
            .clips[0];
        let clip = session
            .generate_clip(track, Ticks::ZERO, BAR * 4, draft.recipe.clone().unwrap())
            .unwrap();
        let midi = session.project.midi_clip_mut(clip).unwrap();
        midi.transforms = draft.performance.clone();
        let kick = midi
            .notes
            .iter_mut()
            .find(|note| note.drum_voice == "kick")
            .unwrap();
        kick.velocity = 0.123;
        let before = midi.clone();
        let untouched: Vec<_> = before
            .notes
            .iter()
            .filter(|note| note.drum_voice != "snare")
            .cloned()
            .collect();
        let mut writer = session.drum_voice_recipe(clip, "snare").unwrap();
        writer.intensity = 0.12;
        session.forget_history();
        session
            .set_drum_voice_recipe(clip, "snare", writer)
            .unwrap();
        let after = session.midi_clip(clip).unwrap();
        let retained: Vec<_> = after
            .notes
            .iter()
            .filter(|note| note.drum_voice != "snare")
            .cloned()
            .collect();
        assert_eq!(retained, untouched);
        assert_eq!(after.transforms, before.transforms);
        assert!(session.clip_hand_edited(clip));
        session.undo();
        assert_eq!(session.midi_clip(clip).unwrap(), &before);
    }

    #[test]
    fn rewriting_a_voice_keeps_the_order_of_simultaneous_hits_on_a_shared_key() {
        let (mut session, track) = with_a_progression();
        let mut recipe = ClipRecipe::new(ClipPreset::Drums, 3);
        for (name, role, preset, intensity) in [
            ("low", auris_core::DrumRole::Kick, ClipPreset::Kick, 0.3),
            (
                "backbeat",
                auris_core::DrumRole::Snare,
                ClipPreset::Snare,
                0.9,
            ),
        ] {
            let mut writer = ClipRecipe::new(preset, 3);
            writer.rhythm = Some("x...............".into());
            writer.fill = 0.0;
            writer.intensity = intensity;
            recipe.drum_voices.push(auris_core::DrumVoiceRecipe {
                name: name.into(),
                role,
                note: 73,
                recipe: Some(Box::new(writer)),
                fixed_notes: Vec::new(),
            });
        }
        let clip = session
            .generate_clip(track, Ticks::ZERO, BAR * 4, recipe)
            .unwrap();
        let before = session.midi_clip(clip).unwrap().notes.clone();
        assert!(
            before
                .windows(2)
                .any(|pair| pair[0].start == pair[1].start && pair[0].pitch == pair[1].pitch)
        );
        session.regenerate_drum_voice(clip, "low").unwrap();
        assert_eq!(session.midi_clip(clip).unwrap().notes, before);
    }

    #[test]
    fn kit_regeneration_keeps_fixed_accents_custom_notes_and_the_performance_stack() {
        let (mut session, track) = with_a_progression();
        let mut spec = auris_compose::SongSpec::default();
        spec.parts.push(auris_compose::PartSpec::of_role(
            "accent",
            auris_compose::Role::Crash,
        ));
        for part in &mut spec.parts {
            if part.role == auris_compose::Role::Snare {
                part.note = Some(73);
            }
        }
        let piece = auris_compose::compose(&spec);
        let kit = piece
            .tracks
            .iter()
            .find(|track| !track.drum_parts.is_empty())
            .unwrap();
        let draft = kit
            .clips
            .iter()
            .find(|clip| {
                clip.recipe
                    .as_ref()
                    .unwrap()
                    .drum_voices
                    .iter()
                    .any(|voice| voice.name == "accent")
            })
            .unwrap();
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                draft.length,
                draft.recipe.clone().unwrap(),
            )
            .unwrap();
        session.project.midi_clip_mut(clip).unwrap().transforms = draft.performance.clone();
        let before = session.midi_clip(clip).unwrap().clone();
        session.reroll_clip(clip).unwrap();
        let after = session.midi_clip(clip).unwrap();
        assert_eq!(after.transforms, before.transforms);
        for note in &after.notes {
            if note.drum_voice == "snare" {
                assert_eq!(note.pitch, 73);
            }
        }
        let accents = |notes: &[Note]| {
            notes
                .iter()
                .filter(|note| note.drum_voice == "accent")
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(accents(&after.notes), accents(&before.notes));
        assert!(!accents(&after.notes).is_empty());
    }

    #[test]
    fn trimming_a_kit_does_not_recreate_an_accent_at_the_new_start() {
        use auris_core::DrumVoiceRecipe;
        use auris_core::project::DrumRole;
        let (mut session, track) = with_a_progression();
        let mut recipe = ClipRecipe::new(ClipPreset::Drums, 7);
        recipe.drum_voices.push(DrumVoiceRecipe {
            name: "accent".into(),
            role: DrumRole::Crash,
            note: 49,
            recipe: None,
            fixed_notes: vec![
                Note::new(49, Ticks::ZERO, Ticks(120)),
                Note::new(49, BAR * 2, Ticks(120)),
            ],
        });
        let clip = session
            .generate_clip(track, Ticks::ZERO, BAR * 4, recipe)
            .unwrap();
        session.trim_clip_start(clip, BAR).unwrap();
        session.regenerate_clip(clip).unwrap();
        let notes = &session.midi_clip(clip).unwrap().notes;
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].start, BAR);
        let right = session.split_clip(clip, BAR * 2).unwrap();
        session.regenerate_clip(clip).unwrap();
        session.regenerate_clip(right).unwrap();
        assert!(session.midi_clip(clip).unwrap().notes.is_empty());
        assert_eq!(
            session.midi_clip(right).unwrap().notes[0].start,
            Ticks::ZERO
        );
    }

    #[test]
    fn a_generated_clip_carries_its_feel_instead_of_baking_it() {
        // The humanisation asks for a wander of so many *milliseconds*, so it used to make
        // writing a clip need a tempo — the notes came out shaken by an amount only true at one
        // speed. It rides the clip's transform stack now, where the renderer hands it the tempo
        // actually in force at playback. Two things follow, and both are the assertion: the
        // text no longer depends on the tempo at all, and the clip arrives already carrying
        // the preset's own feel for the stack to apply.
        let generated = |bpm: f64, preset: ClipPreset| {
            let mut session = session();
            session.set_bpm(bpm);
            let track = session.add_default_instrument_track("Lead").expect("track");
            session
                .stamp_named_progression("axis", Ticks::ZERO, 8)
                .expect("the catalogue knows axis");
            let clip = session
                .generate_clip(track, BAR * 4, BAR * 4, ClipRecipe::new(preset, 7))
                .expect("generated");
            session.midi_clip(clip).expect("clip").clone()
        };

        let slow = generated(60.0, ClipPreset::Lead);
        assert!(!slow.notes.is_empty(), "nothing was written to compare");
        assert_eq!(
            slow.notes,
            generated(120.0, ClipPreset::Lead).notes,
            "the text is the score, and a score does not change with the metronome"
        );
        // The feel the recipe used to bake: a lead leans and wanders, and the wander is seeded
        // by the take so the two are named by one number.
        assert!(
            slow.transforms
                .iter()
                .any(|transform| matches!(transform, NoteTransform::Humanize { seed: 7, .. })),
            "a lead arrived unperformed: {:?}",
            slow.transforms
        );
        // And the kit keeps the time: a kick starts with nothing on its stack at all.
        assert!(generated(120.0, ClipPreset::Kick).transforms.is_empty());
    }

    #[test]
    fn a_generated_clip_is_an_ordinary_clip_that_remembers_how_it_was_written() {
        let (mut session, track) = with_a_progression();
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                BAR * 4,
                ClipRecipe::new(ClipPreset::Bass, 1),
            )
            .unwrap();

        let (owner, midi) = session.project().midi_clip(clip).expect("a real clip");
        assert_eq!(owner, track);
        assert!(!midi.notes.is_empty(), "a clip with no notes in it");
        assert_eq!(midi.start, Ticks::ZERO);
        assert_eq!(midi.length, BAR * 4);
        assert!(midi.is_generated());
        assert_eq!(
            session.clip_recipe(clip).map(|recipe| recipe.preset),
            Some(ClipPreset::Bass)
        );
    }

    #[test]
    fn regenerating_writes_the_same_notes_until_the_chords_move() {
        let (mut session, track) = with_a_progression();
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                BAR * 4,
                ClipRecipe::new(ClipPreset::Lead, 5),
            )
            .unwrap();
        let first = session.project().midi_clip(clip).unwrap().1.notes.clone();

        // Nothing changed, so nothing should: this is what makes the button safe to press.
        session.forget_history();
        session.regenerate_clip(clip).unwrap();
        assert_eq!(session.project().midi_clip(clip).unwrap().1.notes, first);
        assert!(!session.can_undo(), "an identical rewrite recorded a step");

        // Now move the harmony underneath it. The part should follow.
        session
            .stamp_named_progression("marusa", Ticks::ZERO, 4)
            .unwrap();
        session.regenerate_clip(clip).unwrap();
        assert_ne!(
            session.project().midi_clip(clip).unwrap().1.notes,
            first,
            "the chords changed and the part did not"
        );
    }

    #[test]
    fn changing_a_generated_preset_replaces_its_performance_stack() {
        let (mut session, track) = with_a_progression();
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                BAR * 4,
                ClipRecipe::new(ClipPreset::Lead, 5),
            )
            .unwrap();
        assert!(!session.midi_clip(clip).unwrap().transforms.is_empty());

        session
            .set_clip_recipe(clip, ClipRecipe::new(ClipPreset::Kick, 5))
            .unwrap();
        assert!(session.midi_clip(clip).unwrap().transforms.is_empty());
    }

    #[test]
    fn the_recipe_knows_when_its_text_was_edited_by_hand() {
        let (mut session, track) = with_a_progression();
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                BAR * 4,
                ClipRecipe::new(ClipPreset::Lead, 1),
            )
            .unwrap();
        assert!(
            !session.clip_hand_edited(clip),
            "fresh from the composer and already accused"
        );

        // The machine's own arithmetic over the text is not a hand edit: a resize writes the
        // phrase again and the digest follows it.
        session.resize_clip(clip, BAR * 2).unwrap();
        assert!(!session.clip_hand_edited(clip), "a resize is not an edit");

        // A note nudged by hand is exactly what the flag is for — and undoing the nudge clears
        // it, because the digest is exact rather than approximate.
        let origin = session.midi_clip(clip).unwrap().notes[0].clone();
        session
            .move_notes(clip, &[(0, origin.start, origin.pitch)], Ticks(30), 0)
            .unwrap();
        assert!(session.clip_hand_edited(clip), "the nudge went unnoticed");
        session.undo();
        assert!(
            !session.clip_hand_edited(clip),
            "the undo did not acquit it"
        );

        // Writing the part again replaces the edits, and with them the accusation.
        session
            .move_notes(clip, &[(0, origin.start, origin.pitch)], Ticks(30), 0)
            .unwrap();
        session.regenerate_clip(clip).unwrap();
        assert!(!session.clip_hand_edited(clip));

        // A recipe carrying no digest — a file from before the field — never accuses anybody.
        if let Some(midi) = session.project.midi_clip_mut(clip)
            && let Some(recipe) = &mut midi.recipe
        {
            recipe.text_digest = 0;
        }
        session
            .move_notes(clip, &[(0, origin.start, origin.pitch)], Ticks(30), 0)
            .unwrap();
        assert!(
            !session.clip_hand_edited(clip),
            "an unknown text was treated as a known one"
        );
    }

    #[test]
    fn splitting_a_generated_clip_accuses_neither_half() {
        let (mut session, track) = with_a_progression();
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                BAR * 4,
                ClipRecipe::new(ClipPreset::Lead, 1),
            )
            .unwrap();
        let right = session.split_clip(clip, BAR * 2).unwrap();
        assert!(!session.clip_hand_edited(clip), "the left half");
        assert!(!session.clip_hand_edited(right), "the right half");
    }

    #[test]
    fn another_take_changes_the_notes_for_every_preset_from_the_seed_the_app_starts_at() {
        // The desktop application gives the first clip in a project seed 1, so the first press of
        // "another take" is always 1 to 2. If that one pair happened to write the same notes the
        // button would look broken however well every other seed behaved.
        //
        // The kick is the honest exception. Its text at the default dials is the groove spelled
        // out, with nothing left to the seed — the difference two takes of it used to show was
        // the baked wobble, which was noise wearing a take's name and lives on the performance
        // stack now. What its take still changes is the wander's seed, asserted below for the
        // presets that carry one.
        for preset in ClipPreset::ALL {
            let (mut session, track) = with_a_progression();
            let clip = session
                .generate_clip(track, Ticks::ZERO, BAR * 4, ClipRecipe::new(preset, 1))
                .unwrap();
            let first = session.project().midi_clip(clip).unwrap().1.notes.clone();
            assert!(!first.is_empty(), "{} wrote nothing", preset.name());

            session.reroll_clip(clip).unwrap();
            let after = session.project().midi_clip(clip).unwrap().1.clone();
            if preset == ClipPreset::Kick {
                assert_eq!(first, after.notes, "the kick's groove is not the seed's");
                continue;
            }
            assert_ne!(
                first,
                after.notes,
                "{} wrote the same notes for seed 1 and seed 2",
                preset.name()
            );
            // The wobble follows the take: one number names both.
            for transform in &after.transforms {
                if let NoteTransform::Humanize { seed, .. } = transform {
                    assert_eq!(
                        *seed,
                        2,
                        "{}'s wander kept the old take's seed",
                        preset.name()
                    );
                }
            }
        }
    }

    #[test]
    fn another_take_is_a_different_phrase_of_the_same_part() {
        let (mut session, track) = with_a_progression();
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                BAR * 4,
                ClipRecipe::new(ClipPreset::Lead, 5),
            )
            .unwrap();
        let first = session.project().midi_clip(clip).unwrap().1.notes.clone();

        session.reroll_clip(clip).unwrap();
        let second = session.project().midi_clip(clip).unwrap().1.notes.clone();
        assert_ne!(first, second);
        assert!(!second.is_empty());
        assert_eq!(
            session.clip_recipe(clip).unwrap().seed,
            6,
            "the next seed, not a random one, so a take can be got back to"
        );

        // And one undo step takes the take back, not one note.
        assert_eq!(session.undo(), Some(Edit::GenerateClip));
        assert_eq!(session.project().midi_clip(clip).unwrap().1.notes, first);
    }

    #[test]
    fn freezing_keeps_the_notes_and_forgets_how_they_got_there() {
        let (mut session, track) = with_a_progression();
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                BAR * 4,
                ClipRecipe::new(ClipPreset::Chords, 2),
            )
            .unwrap();
        let kept = session.project().midi_clip(clip).unwrap().1.notes.clone();

        session.freeze_clip(clip).unwrap();
        assert_eq!(
            session.project().midi_clip(clip).unwrap().1.notes,
            kept,
            "freezing must not touch a note"
        );
        assert!(!session.project().midi_clip(clip).unwrap().1.is_generated());

        // And now nothing can rewrite it, which is the whole point of having frozen it.
        let error = session.regenerate_clip(clip).unwrap_err();
        assert!(matches!(error, SessionError::NotGenerated(id) if id == clip.0));
    }

    #[test]
    fn a_clip_somebody_played_is_never_rewritten_by_accident() {
        let (mut session, track) = with_a_progression();
        let clip = session
            .add_midi_clip(track, "Played", Ticks::ZERO, BAR)
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, BAR))
            .unwrap();
        session.forget_history();

        for outcome in [
            session.regenerate_clip(clip),
            session.reroll_clip(clip),
            session.freeze_clip(clip).map(|()| 0),
        ] {
            assert!(matches!(
                outcome,
                Err(SessionError::NotGenerated(id)) if id == clip.0
            ));
        }
        assert_eq!(session.project().midi_clip(clip).unwrap().1.notes.len(), 1);
        assert!(!session.can_undo(), "a refusal must not cost an undo step");
    }

    #[test]
    fn a_generated_clip_survives_a_save_and_writes_itself_again_after() {
        let scratch = Scratch::new("clip-recipe");
        let (mut session, track) = with_a_progression();
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                BAR * 4,
                ClipRecipe::new(ClipPreset::Bass, 3),
            )
            .unwrap();
        let written = session.project().midi_clip(clip).unwrap().1.notes.clone();

        let document = session
            .save_as(&scratch.join("Song.auris"))
            .unwrap()
            .document;
        let mut reopened = self::tests::session();
        reopened.open(&document).unwrap();

        let (_, midi) = reopened
            .project()
            .midi_clip(clip)
            .expect("the clip came back");
        assert_eq!(midi.notes, written, "the notes are stored, not recomputed");
        assert_eq!(reopened.clip_recipe(clip).unwrap().seed, 3);
        assert_eq!(reopened.regenerate_clip(clip).unwrap(), written.len());
    }

    #[test]
    fn a_range_with_no_chords_under_it_makes_an_empty_clip_rather_than_an_error() {
        let mut session = self::tests::session();
        let track = session.add_default_instrument_track("Keys").unwrap();
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                BAR * 4,
                ClipRecipe::new(ClipPreset::Lead, 1),
            )
            .expect("nothing to play is not a failure");
        assert!(
            session
                .project()
                .midi_clip(clip)
                .unwrap()
                .1
                .notes
                .is_empty()
        );
        assert!(
            session.project().midi_clip(clip).unwrap().1.is_generated(),
            "so that writing a progression and pressing regenerate fills it in"
        );
    }

    #[test]
    fn generating_needs_a_track_that_can_hold_notes() {
        let (mut session, _) = with_a_progression();
        let bus = session.add_bus_track("Bus");
        let error = session
            .generate_clip(bus, Ticks::ZERO, BAR, ClipRecipe::new(ClipPreset::Lead, 1))
            .unwrap_err();
        assert!(matches!(
            error,
            SessionError::WrongTrackKind { actual: "Bus", .. }
        ));
    }

    #[test]
    fn freezing_a_track_stops_every_generated_clip_on_it_and_says_how_many() {
        let mut session = self::tests::session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        for bar in 0..3 {
            session
                .generate_clip(
                    track,
                    Ticks::from_beats(bar as f64 * 4.0),
                    Ticks::from_beats(4.0),
                    ClipRecipe::new(ClipPreset::Lead, bar),
                )
                .unwrap();
        }
        // One clip written by hand, which has no recipe to drop.
        session
            .add_midi_clip(
                track,
                "By hand",
                Ticks::from_beats(12.0),
                Ticks::from_beats(4.0),
            )
            .unwrap();

        assert_eq!(session.freeze_track(track).unwrap(), 3);
        let generated = session
            .project()
            .track(track)
            .unwrap()
            .kind
            .as_instrument()
            .unwrap()
            .clips
            .iter()
            .filter(|clip| clip.is_generated())
            .count();
        assert_eq!(generated, 0);
        // Nothing left to freeze, so nothing happens and nothing is recorded.
        session.forget_history();
        assert_eq!(session.freeze_track(track).unwrap(), 0);
        assert!(!session.can_undo());
    }
}
