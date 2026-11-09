//! Applying, keeping and forgetting performance presets.
//!
//! The presets themselves are [`crate::performance_presets`]; these are the commands every
//! frontend reaches them through. Applying is an ordinary undoable edit of one clip's stack.
//! Keeping and forgetting change the installation's preset file, not the document, so they are
//! not on the undo stack — the same as keeping a sound or a chord progression.

use auris_core::{ClipId, NoteTransform, PerformancePreset, PresetFamily};

use crate::error::SessionError;
use crate::history::Edit;
use crate::performance_presets::{KeepRefusal, ListedPreset, kept_stack};

use super::Session;

/// Whether a whole-clip stage is applied after the drum-writer scopes: the ones the agent's and
/// the panel's rank place after `Lean`.
fn after_scopes(stage: &NoteTransform) -> bool {
    matches!(
        stage,
        NoteTransform::Humanize { .. }
            | NoteTransform::Expression { .. }
            | NoteTransform::Sustain { .. }
            | NoteTransform::Pitch { .. }
            | NoteTransform::Octaves { .. }
    )
}

impl Session {
    /// Every preset on offer: the built-ins first, then the kept ones in the order they were kept.
    pub fn performance_presets(&self) -> Vec<ListedPreset> {
        self.performance_presets.listed()
    }

    /// The preset called `name`, built in or kept.
    pub fn performance_preset(&self, name: &str) -> Option<ListedPreset> {
        self.performance_presets.find(name)
    }

    /// Reads the preset file again, for presets kept by another window or process.
    pub fn reload_performance_presets(&mut self) {
        self.performance_presets.reload();
    }

    /// Whether a clip's instrument has a pitch that the pitch gestures and octave copies mean
    /// something on.
    fn clip_is_pitched(&self, clip: ClipId) -> Result<bool, SessionError> {
        let (track, _) = self
            .project
            .midi_clip(clip)
            .ok_or(SessionError::UnknownClip(clip.0))?;
        Ok(self
            .project
            .track(track)
            .is_some_and(|track| track.kind.as_instrument().is_some() && !track.kind.is_drum()))
    }

    /// The stack `preset` gives `clip` as a whole: the preset's stages with the clip's own take,
    /// keeping the clip's drum-writer scopes when the preset brings none of its own. Returns the
    /// stage names left out because the clip has no pitch to bend.
    pub fn performance_preset_stack(
        &self,
        clip: ClipId,
        preset: &PerformancePreset,
    ) -> Result<(Vec<NoteTransform>, Vec<&'static str>), SessionError> {
        let pitched = self.clip_is_pitched(clip)?;
        let (mut stack, skipped) = preset.stack_for(clip.0, pitched);
        let scoped = |stage: &NoteTransform| matches!(stage, NoteTransform::ForDrumVoice { .. });
        if !stack.iter().any(scoped) {
            let at = stack.iter().position(after_scopes).unwrap_or(stack.len());
            let held: Vec<_> = self
                .clip_transforms(clip)?
                .iter()
                .filter(|stage| scoped(stage))
                .cloned()
                .collect();
            stack.splice(at..at, held);
        }
        Ok((stack, skipped))
    }

    /// Replaces a clip's performance with the preset called `name`, as one undo step.
    ///
    /// The stages are copied into the clip: nothing in the document names the preset, so a
    /// preset edited or deleted later changes no song that used it.
    pub fn apply_performance_preset(
        &mut self,
        clip: ClipId,
        name: &str,
    ) -> Result<Vec<&'static str>, SessionError> {
        let listed = self
            .performance_preset(name)
            .ok_or_else(|| SessionError::UnknownPerformancePreset(name.trim().to_owned()))?;
        let (stack, skipped) = self.performance_preset_stack(clip, &listed.preset)?;
        self.begin_transaction(Edit::SetClipTransforms(clip));
        let result = self.set_clip_transforms(clip, stack);
        self.end_transaction();
        result.map(|()| skipped)
    }

    /// Keeps `preset` in the installation's preset file. `replace` allows overwriting a kept
    /// preset of the same name; built-ins can never be overwritten.
    pub fn keep_performance_preset(
        &mut self,
        mut preset: PerformancePreset,
        replace: bool,
    ) -> Result<PerformancePreset, SessionError> {
        preset.transforms = kept_stack(&preset.transforms);
        let kept = preset.clone();
        self.performance_presets
            .update(|book| book.keep(preset, replace))?
            .map_err(SessionError::PerformancePreset)?;
        Ok(self
            .performance_preset(&kept.name)
            .map_or(kept, |listed| listed.preset))
    }

    /// Keeps a clip's current performance as a preset.
    pub fn keep_clip_performance(
        &mut self,
        clip: ClipId,
        name: &str,
        family: PresetFamily,
        description: &str,
        replace: bool,
    ) -> Result<PerformancePreset, SessionError> {
        let transforms = self.clip_transforms(clip)?.to_vec();
        self.keep_performance_preset(
            PerformancePreset {
                name: name.to_owned(),
                family,
                description: description.to_owned(),
                transforms,
            },
            replace,
        )
    }

    /// Forgets the kept preset called `name`. A built-in cannot be forgotten.
    pub fn forget_performance_preset(&mut self, name: &str) -> Result<(), SessionError> {
        if auris_core::builtin_performance_preset(name.trim()).is_some() {
            return Err(SessionError::PerformancePreset(KeepRefusal::Builtin));
        }
        if !self.performance_presets.update(|book| book.forget(name))? {
            return Err(SessionError::UnknownPerformancePreset(
                name.trim().to_owned(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionOptions;
    use auris_core::time::Ticks;

    #[test]
    fn applying_copies_the_stages_with_the_clips_own_take_and_undoes_in_one_step() {
        let mut session = Session::new(SessionOptions::headless().with_balance(false)).unwrap();
        let track = session.add_default_instrument_track("Piano").unwrap();
        let clip = session
            .add_midi_clip(track, "Verse", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        session
            .set_clip_transforms(clip, vec![NoteTransform::Gate { amount: 0.3 }])
            .unwrap();
        let skipped = session
            .apply_performance_preset(clip, "lead_guitar")
            .unwrap();
        assert!(skipped.is_empty());
        let stack = session.clip_transforms(clip).unwrap().to_vec();
        assert!(
            stack
                .iter()
                .any(|stage| matches!(stage, NoteTransform::Pitch { .. }))
        );
        assert!(stack.iter().any(
            |stage| matches!(stage, NoteTransform::Expression { settings } if settings.seed == clip.0)
        ));
        assert!(
            !stack
                .iter()
                .any(|stage| matches!(stage, NoteTransform::Gate { .. }))
        );
        assert!(session.undo().is_some());
        assert_eq!(
            session.clip_transforms(clip).unwrap(),
            [NoteTransform::Gate { amount: 0.3 }]
        );
        assert!(matches!(
            session.apply_performance_preset(clip, "no such feel"),
            Err(SessionError::UnknownPerformancePreset(_))
        ));
    }

    #[test]
    fn a_drum_kit_keeps_its_writer_scopes_and_skips_pitch_gestures() {
        let mut session = Session::new(SessionOptions::headless().with_balance(false)).unwrap();
        let track = session
            .add_drum_track("Drums", auris_synth::DrumKit::ID)
            .unwrap();
        let clip = session
            .add_midi_clip(track, "Beat", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        let scope = NoteTransform::ForDrumVoice {
            voice: "hat".into(),
            transforms: vec![NoteTransform::Lean { ticks: -8 }],
        };
        session
            .set_clip_transforms(clip, vec![scope.clone()])
            .unwrap();
        let skipped = session.apply_performance_preset(clip, "chiptune").unwrap();
        assert_eq!(skipped, vec!["pitch"]);
        let stack = session.clip_transforms(clip).unwrap();
        assert_eq!(stack[1], scope, "the scope sits before the expression");
        assert!(
            !stack
                .iter()
                .any(|stage| matches!(stage, NoteTransform::Pitch { .. }))
        );
    }

    #[test]
    fn a_kept_preset_comes_back_in_the_list_and_can_be_forgotten() {
        let mut session = Session::new(SessionOptions::headless().with_balance(false)).unwrap();
        let track = session.add_default_instrument_track("Keys").unwrap();
        let clip = session
            .add_midi_clip(track, "Verse", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        session
            .set_clip_transforms(
                clip,
                vec![NoteTransform::Humanize {
                    amount: 0.4,
                    seed: 99,
                }],
            )
            .unwrap();
        let kept = session
            .keep_clip_performance(clip, "My feel", PresetFamily::Keys, "loose", false)
            .unwrap();
        assert_eq!(
            kept.transforms,
            [NoteTransform::Humanize {
                amount: 0.4,
                seed: 0
            }]
        );
        assert!(
            session
                .performance_presets()
                .iter()
                .any(|listed| listed.preset.name == "My feel" && !listed.builtin)
        );
        assert!(
            session
                .keep_clip_performance(clip, "My feel", PresetFamily::Keys, "", false)
                .is_err()
        );
        assert!(session.forget_performance_preset("piano_ballad").is_err());
        session.forget_performance_preset("My feel").unwrap();
        assert!(session.performance_preset("My feel").is_none());
    }
}
