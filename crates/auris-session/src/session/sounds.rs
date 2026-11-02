//! Capturing and applying reusable sounds through the common session boundary.

use std::path::Path;

use auris_core::{AssetPath, PluginKind, PluginState, TrackId};

use super::Session;
use crate::{Edit, SessionError, SoundPreset, SoundPresetEffect, SoundPresetFont};

impl Session {
    /// Whether a track still has exactly the settings and inserts of a reusable sound.
    pub fn sound_is_applied(&self, track: TrackId, sound: &SoundPreset) -> bool {
        let Some(entry) = self.project.track(track) else {
            return false;
        };
        let Some(instrument) = entry.kind.as_instrument() else {
            return false;
        };
        let path_matches = |asset: Option<&AssetPath>, saved: Option<&Path>| {
            asset
                .and_then(|asset| asset.resolve(self.project_folder()))
                .as_deref()
                == saved
        };
        if instrument.instrument_id != sound.instrument_id
            || !path_matches(instrument.file.as_ref(), sound.file.as_deref())
        {
            return false;
        }
        let mut state = std::borrow::Cow::Borrowed(&sound.state);
        if let Some(font) = &sound.soundfont {
            let Some(preset) = auris_sampler::stored_preset(&instrument.instrument_state) else {
                return false;
            };
            if preset.bank != font.bank
                || preset.patch != font.patch
                || !self
                    .project
                    .soundfonts
                    .get(&preset.font)
                    .is_some_and(|reference| {
                        reference.path.resolve(self.project_folder()).as_deref()
                            == Some(font.path.as_path())
                    })
            {
                return false;
            }
            auris_sampler::store_preset(state.to_mut(), preset);
        }
        instrument.instrument_state == *state
            && entry.mixer.effects.len() == sound.effects.len()
            && entry
                .mixer
                .effects
                .iter()
                .zip(&sound.effects)
                .all(|(slot, effect)| {
                    slot.effect_id == effect.effect_id
                        && slot.state == effect.state
                        && slot.enabled == effect.enabled
                        && slot.sidechain.is_none()
                        && path_matches(slot.file.as_ref(), effect.file.as_deref())
                })
    }

    /// Captures an instrument and its inserts, resolving all asset references outside the song.
    pub fn capture_sound(
        &mut self,
        track: TrackId,
        name: &str,
    ) -> Result<SoundPreset, SessionError> {
        self.require_sound_track(track)?;
        if name.trim().is_empty() {
            return Err(SessionError::SoundPreset("enter a sound name".into()));
        }
        self.collect_hosted_state()?;
        self.refresh_sound_snapshots(track)?;
        let entry = self
            .project
            .track(track)
            .ok_or(SessionError::UnknownTrack(track.0))?
            .clone();
        let instrument = entry
            .kind
            .as_instrument()
            .ok_or_else(|| SessionError::SoundPreset("select an instrument track".into()))?;
        // The audio sampler names its sample by a source id that only means something inside
        // this song; a library sound carrying it would play some other project's audio.
        if instrument.instrument_id == auris_sampler::AUDIO_SAMPLER_ID {
            return Err(SessionError::SoundPreset(
                "a sampler playing a song's own audio cannot be saved as a library sound".into(),
            ));
        }
        let folder = self.project_folder().map(Path::to_path_buf);
        let resolve = |asset: &AssetPath| {
            asset.resolve(folder.as_deref()).ok_or_else(|| {
                SessionError::SoundPreset("save the project before saving this sound".into())
            })
        };
        let soundfont = if instrument.instrument_id == auris_sampler::SAMPLER_ID {
            let preset =
                auris_sampler::stored_preset(&instrument.instrument_state).ok_or_else(|| {
                    SessionError::SoundPreset("choose a SoundFont sound first".into())
                })?;
            let font = self
                .project
                .soundfonts
                .get(&preset.font)
                .ok_or(SessionError::UnknownSoundFont(preset.font.0))?;
            Some(SoundPresetFont {
                path: resolve(&font.path)?,
                bank: preset.bank,
                patch: preset.patch,
            })
        } else {
            None
        };
        let effects = entry
            .mixer
            .effects
            .iter()
            .map(|slot| {
                Ok(SoundPresetEffect {
                    effect_id: slot.effect_id.clone(),
                    enabled: slot.enabled,
                    state: self.complete_sound_state(&slot.effect_id, &slot.state),
                    file: slot.file.as_ref().map(resolve).transpose()?,
                })
            })
            .collect::<Result<_, SessionError>>()?;
        let state =
            self.complete_sound_state(&instrument.instrument_id, &instrument.instrument_state);
        Ok(SoundPreset {
            name: name.trim().into(),
            instrument_id: instrument.instrument_id.clone(),
            state,
            file: instrument.file.as_ref().map(resolve).transpose()?,
            soundfont,
            effects,
        })
    }

    fn complete_sound_state(&mut self, id: &str, saved: &PluginState) -> PluginState {
        let mut state = saved.clone();
        for param in self.param_descriptors(id).iter() {
            state
                .params
                .entry(param.key.to_string())
                .or_insert(param.default);
        }
        state
    }

    fn refresh_sound_snapshots(&mut self, track: TrackId) -> Result<(), SessionError> {
        let entry = self
            .project
            .track(track)
            .ok_or(SessionError::UnknownTrack(track.0))?
            .clone();
        let instrument = entry
            .kind
            .as_instrument()
            .ok_or_else(|| SessionError::SoundPreset("select an instrument track".into()))?;
        let instrument_state = instrument
            .file
            .as_ref()
            .map(|file| {
                let path = file.resolve(self.project_folder()).ok_or_else(|| {
                    SessionError::SoundPreset("the plugin path is unavailable".into())
                })?;
                if path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("vst3"))
                {
                    self.vst3
                        .drum_probe_state(track, &instrument.instrument_state)
                } else {
                    self.hosted.sound_state(
                        super::PluginWindow::Instrument(track),
                        &instrument.instrument_state,
                    )
                }
            })
            .transpose()?;
        let mut effects = Vec::new();
        for slot in &entry.mixer.effects {
            if slot.file.is_some() && slot.effect_id.starts_with("clap:") {
                effects.push((
                    slot.id,
                    self.hosted
                        .sound_state(super::PluginWindow::Effect(slot.id), &slot.state)?,
                ));
            }
        }
        // Stage all snapshots first; a plugin refusing capture must not partially publish values.
        if let Some(entry) = self.project.track_mut(track) {
            if let Some(state) = instrument_state
                && let Some(instrument) = entry.kind.as_instrument_mut()
            {
                instrument.instrument_state = state;
            }
            for (id, state) in effects {
                if let Some(slot) = entry.mixer.effects.iter_mut().find(|slot| slot.id == id) {
                    slot.state = state;
                }
            }
        }
        Ok(())
    }

    fn require_sound_track(&self, track: TrackId) -> Result<(), SessionError> {
        let entry = self
            .project
            .track(track)
            .ok_or(SessionError::UnknownTrack(track.0))?;
        if entry.kind.as_instrument().is_none() {
            return Err(SessionError::SoundPreset(
                "select an instrument or drum track".into(),
            ));
        }
        Ok(())
    }

    fn validate_sound_plugin(
        &mut self,
        id: &str,
        state: &PluginState,
        file: Option<&Path>,
        kind: PluginKind,
    ) -> Result<(), SessionError> {
        if state.params.values().any(|value| !value.is_finite()) {
            return Err(SessionError::SoundPreset(
                "sound parameters must be finite".into(),
            ));
        }
        if let Some(file) = file {
            if !file.is_absolute() || !file.exists() {
                return Err(SessionError::SoundPreset(format!(
                    "plugin unavailable: {}",
                    file.display()
                )));
            }
            let available = if file
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("vst3"))
            {
                self.vst3_plugins_in(file)?
                    .iter()
                    .any(|info| info.auris_id() == id && info.kind == kind)
            } else {
                self.hosted_plugins_in(file)?
                    .iter()
                    .any(|info| info.auris_id() == id && info.kind == kind)
            };
            if !available {
                return Err(SessionError::UnknownPlugin(id.into()));
            }
        } else {
            if self
                .registry
                .descriptor(id)
                .map(|descriptor| descriptor.kind)
                != Some(kind)
            {
                return Err(SessionError::UnknownPlugin(id.into()));
            }
            for param in self.param_descriptors(id).iter() {
                if let Some(value) = state.params.get(param.key.as_ref())
                    && !(param.min..=param.max).contains(value)
                {
                    return Err(SessionError::SoundPreset(format!(
                        "{} is outside its range",
                        param.key
                    )));
                }
            }
        }
        Ok(())
    }

    /// Replaces an instrument and all its inserts in one undo step. Notes, mixer levels, sends
    /// and routing stay intact. Automation belonging to the replaced sound is removed.
    pub fn apply_sound(&mut self, track: TrackId, sound: &SoundPreset) -> Result<(), SessionError> {
        self.require_sound_track(track)?;
        if self.transaction.is_some() {
            return Err(SessionError::EditInProgress);
        }
        self.validate_sound_plugin(
            &sound.instrument_id,
            &sound.state,
            sound.file.as_deref(),
            PluginKind::Instrument,
        )?;
        for effect in &sound.effects {
            self.validate_sound_plugin(
                &effect.effect_id,
                &effect.state,
                effect.file.as_deref(),
                PluginKind::Effect,
            )?;
        }
        if (sound.instrument_id == auris_sampler::SAMPLER_ID) != sound.soundfont.is_some() {
            return Err(SessionError::SoundPreset(
                "the sampler sound needs a SoundFont reference".into(),
            ));
        }
        // Decode before recording history or changing the document: a missing library cannot
        // leave the target with half of a new chain.
        let font = sound
            .soundfont
            .as_ref()
            .map(|font| {
                if !font.path.is_absolute() {
                    return Err(SessionError::SoundPreset(
                        "SoundFont paths must be absolute".into(),
                    ));
                }
                super::read_soundfont(&font.path)
            })
            .transpose()?;
        self.collect_hosted_state()?;
        self.refresh_sound_snapshots(track)?;
        self.begin_transaction(Edit::ApplySoundPreset);
        self.record(Edit::ApplySoundPreset);
        let mut state = sound.state.clone();
        if let (Some(reference), Some(font)) = (&sound.soundfont, font) {
            match self.install_soundfont(&reference.path, font) {
                Ok(font) => auris_sampler::store_preset(
                    &mut state,
                    auris_core::PresetRef {
                        font,
                        bank: reference.bank,
                        patch: reference.patch,
                    },
                ),
                Err(error) => {
                    self.revert_transaction();
                    return Err(error);
                }
            }
        }
        let old: Vec<_> = self
            .project
            .track(track)
            .into_iter()
            .flat_map(|entry| &entry.mixer.effects)
            .map(|slot| slot.id)
            .collect();
        for slot in old {
            self.project.remove_effect(slot);
        }
        self.project.remove_instrument_automation(track);
        if let Some(instrument) = self
            .project
            .track_mut(track)
            .and_then(|entry| entry.kind.as_instrument_mut())
        {
            instrument.instrument_id.clone_from(&sound.instrument_id);
            instrument.instrument_state = state;
            instrument.file = sound.file.as_ref().map(AssetPath::external);
        }
        for effect in &sound.effects {
            if let Some(slot) = self.project.add_effect(Some(track), &effect.effect_id)
                && let Some(entry) = self.project.track_mut(track).and_then(|entry| {
                    entry
                        .mixer
                        .effects
                        .iter_mut()
                        .find(|entry| entry.id == slot)
                })
            {
                entry.state.clone_from(&effect.state);
                entry.enabled = effect.enabled;
                entry.file = effect.file.as_ref().map(AssetPath::external);
            }
        }
        // The same hosted id can hold a completely different opaque patch. Fresh instances
        // must read the saved patch instead of inheriting the outgoing live sound.
        self.hosted.forget_instrument(track);
        self.vst3.forget_instrument(track);
        self.invalidate_graph();
        self.end_transaction();
        Ok(())
    }

    /// Captures and durably adds a sound to a user library, returning the updated library.
    pub fn save_sound(
        &mut self,
        track: TrackId,
        name: &str,
        path: &Path,
    ) -> Result<crate::SoundBook, SessionError> {
        let sound = self.capture_sound(track, name)?;
        crate::SoundBook::save_sound(path, sound)
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{Scratch, session};
    use super::*;
    use crate::{SoundBook, factory_sounds};

    #[test]
    fn every_factory_family_has_audible_distinct_patches_that_can_be_saved_as_user_sounds() {
        use auris_core::{Note, time::Ticks};
        for family in crate::factory_sound_families() {
            let mut session = session();
            let track = session.add_default_instrument_track("Family").unwrap();
            let clip = session
                .add_midi_clip(track, "Note", Ticks::ZERO, Ticks::from_beats(2.0))
                .unwrap();
            let pitch = if family.instrument_id == auris_synth::DrumKit::ID {
                36
            } else {
                60
            };
            session
                .add_note(clip, Note::new(pitch, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
            let mut reference: Option<Vec<f32>> = None;
            for sound in &family.presets {
                session.apply_sound(track, sound).unwrap();
                let audio = session
                    .render_job()
                    .render(&Default::default(), &mut Default::default())
                    .unwrap();
                assert!(
                    audio.peak().is_finite() && audio.peak() > 0.0001,
                    "{}: peak {}",
                    sound.name,
                    audio.peak()
                );
                assert!(
                    audio
                        .channels()
                        .iter()
                        .flatten()
                        .all(|sample| sample.is_finite()),
                    "{}",
                    sound.name
                );
                if let Some(reference) = &reference {
                    let difference: f64 = reference
                        .iter()
                        .zip(audio.channel(0))
                        .map(|(a, b)| f64::from(a - b).powi(2))
                        .sum();
                    let energy: f64 = reference
                        .iter()
                        .map(|sample| f64::from(*sample).powi(2))
                        .sum();
                    assert!(
                        difference / energy > 0.0001,
                        "{}: no meaningful audio difference",
                        sound.name
                    );
                } else {
                    reference = Some(audio.channel(0).to_vec());
                }
                let saved = session.capture_sound(track, &sound.name).unwrap();
                let restored: SoundPreset =
                    serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
                assert_eq!(restored, *sound);
                assert!(session.sound_is_applied(track, &restored));
                if family.instrument_id == auris_synth::DrumKit::ID {
                    assert!(auris_core::DrumMap::load(&saved.state).is_some());
                }
            }
        }
    }

    #[test]
    fn factory_strings_produce_distinct_finite_audio_and_survive_project_reopening() {
        let mut session = session();
        let track = session.add_default_instrument_track("Strings").unwrap();
        let clip = session
            .add_midi_clip(
                track,
                "Phrase",
                auris_core::time::Ticks::ZERO,
                auris_core::time::Ticks::from_beats(2.0),
            )
            .unwrap();
        session
            .add_note(
                clip,
                auris_core::Note::new(
                    60,
                    auris_core::time::Ticks::ZERO,
                    auris_core::time::Ticks::QUARTER,
                ),
            )
            .unwrap();
        let notes = session
            .project
            .track(track)
            .unwrap()
            .kind
            .as_instrument()
            .unwrap()
            .clips
            .clone();
        let mut renders = Vec::new();
        for sound in &crate::factory_sound_families()[0].presets {
            session.apply_sound(track, sound).unwrap();
            let audio = session
                .render_job()
                .render(&Default::default(), &mut Default::default())
                .unwrap();
            assert!(audio.peak().is_finite() && audio.peak() > 0.001);
            for other in &renders {
                assert_ne!(audio.channels(), other);
            }
            renders.push(audio.channels().to_vec());
            assert_eq!(
                session
                    .project
                    .track(track)
                    .unwrap()
                    .kind
                    .as_instrument()
                    .unwrap()
                    .clips,
                notes
            );
        }
        let scratch = Scratch::new("string-sounds-render");
        let saved = session.save_as(&scratch.join("Strings.auris")).unwrap();
        let mut reopened = super::super::fixtures::session();
        reopened.open(&saved.document).unwrap();
        let audio = reopened
            .render_job()
            .render(&Default::default(), &mut Default::default())
            .unwrap();
        assert_eq!(audio.channels(), renders.last().unwrap());
    }
    use auris_core::{ParamId, ParamTarget};

    #[test]
    fn saved_sound_restores_parameters_chain_and_bypass_in_one_undo_step() {
        let mut session = session();
        let track = session.add_default_instrument_track("Source").unwrap();
        session.apply_sound(track, &factory_sounds()[2]).unwrap();
        let first = session.add_effect(Some(track), "auris.fx.reverb").unwrap();
        let second = session.add_effect(Some(track), "auris.fx.delay").unwrap();
        session.set_param(
            ParamTarget::Effect {
                track: Some(track),
                slot: first,
                param: ParamId(0),
            },
            0.7,
        );
        session.set_effect_enabled(Some(track), second, false);
        let scratch = Scratch::new("sound-library");
        let path = scratch.join("sounds.json");
        let source = session.project.clone();
        let book = session.save_sound(track, "My cello", &path).unwrap();
        assert_eq!(SoundBook::read(&path).unwrap(), book);
        assert_eq!(
            session.project, source,
            "saving a user sound does not edit the song"
        );
        assert!(session.save_sound(track, "My cello", &path).is_err());
        assert_eq!(SoundBook::read(&path).unwrap(), book);

        let target = session.add_default_instrument_track("Target").unwrap();
        session.set_param(ParamTarget::TrackGain(target), -7.0);
        session.add_effect(Some(target), "auris.fx.chorus").unwrap();
        let before = session.project.clone();
        session.forget_history();
        let sound = &book.entries()[0];
        session.apply_sound(target, sound).unwrap();
        assert!(session.sound_is_applied(target, sound));
        let entry = session.project.track(target).unwrap();
        assert_eq!(
            entry.mixer.gain_db,
            before.track(target).unwrap().mixer.gain_db
        );
        assert_ne!(entry.mixer.effects[0].id, first);
        assert_ne!(entry.mixer.effects[1].id, second);
        assert!(!entry.mixer.effects[1].enabled);
        let after = session.project.clone();
        assert_eq!(session.undo(), Some(Edit::ApplySoundPreset));
        assert_eq!(session.project, before);
        assert!(session.undo().is_none());
        assert_eq!(session.redo(), Some(Edit::ApplySoundPreset));
        assert_eq!(session.project, after);

        let saved = session.save_as(&scratch.join("Song.auris")).unwrap();
        let mut reopened = super::super::fixtures::session();
        reopened.open(&saved.document).unwrap();
        assert!(reopened.sound_is_applied(target, sound));
    }

    #[test]
    fn invalid_sound_leaves_the_document_and_history_intact() {
        let mut session = session();
        let track = session.add_default_instrument_track("Target").unwrap();
        session.forget_history();
        let before = session.project.clone();
        let mut sound = factory_sounds()[0].clone();
        sound.effects.push(SoundPresetEffect {
            effect_id: "missing.effect".into(),
            enabled: true,
            state: PluginState::empty(),
            file: None,
        });
        assert!(session.apply_sound(track, &sound).is_err());
        assert_eq!(session.project, before);
        assert!(!session.can_undo());
        sound.effects.clear();
        sound.state.params.insert("bow_pressure".into(), f32::NAN);
        assert!(session.apply_sound(track, &sound).is_err());
        assert_eq!(session.project, before);
        let audio = session.add_audio_track("Audio");
        assert!(session.apply_sound(audio, &factory_sounds()[0]).is_err());
    }

    #[test]
    fn sampler_sounds_remap_font_ids_between_projects() {
        let scratch = Scratch::new("sound-font");
        let file = scratch.soundfont("Tiny.sf2");
        let mut source = session();
        let track = source.add_default_instrument_track("Source").unwrap();
        let font = source.import_soundfont(&file).unwrap();
        source
            .set_track_preset(
                track,
                auris_core::PresetRef {
                    font,
                    bank: 0,
                    patch: 0,
                },
            )
            .unwrap();
        let sound = source.capture_sound(track, "Tiny piano").unwrap();
        let mut target = session();
        target.add_audio_track("Different IDs");
        let track = target.add_default_instrument_track("Target").unwrap();
        target.apply_sound(track, &sound).unwrap();
        let restored = target.track_preset(track).unwrap();
        assert_ne!(restored.font, font);
        assert_eq!((restored.bank, restored.patch), (0, 0));
        assert!(target.sound_is_applied(track, &sound));
    }

    #[test]
    fn replacing_a_sound_removes_only_its_automation() {
        let mut session = session();
        let track = session.add_default_instrument_track("Part").unwrap();
        let slot = session.add_effect(Some(track), "auris.fx.reverb").unwrap();
        let instrument = ParamTarget::Instrument {
            track,
            param: ParamId(0),
        };
        let effect = ParamTarget::Effect {
            track: Some(track),
            slot,
            param: ParamId(0),
        };
        let gain = ParamTarget::TrackGain(track);
        for target in [instrument, effect, gain] {
            session.set_automation_point(target, auris_core::time::Ticks::ZERO, 0.5);
        }
        let before = session.project.clone();
        session.apply_sound(track, &factory_sounds()[1]).unwrap();
        assert!(session.project.automation.lane(instrument).is_none());
        assert!(session.project.automation.lane(effect).is_none());
        assert!(session.project.automation.lane(gain).is_some());
        session.undo();
        assert_eq!(session.project, before);
    }
}
