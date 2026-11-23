//! Freeze a discovered sound's identity before loading it away from the live document.

use super::*;
use auris_core::AssetPath;
use auris_core::project::InstrumentTrack;

#[derive(Clone)]
pub(crate) struct SoundPreparation {
    source: Source,
    /// The sound's files as they were when the plan was validated; empty for one in no file.
    stamp: String,
}

/// A prepared sound: the instrument a track will hold and, for a saved sound, the inserts that
/// replace the track's own.
pub(crate) struct PreparedSound {
    pub(crate) instrument: InstrumentTrack,
    pub(crate) effects: Option<Vec<crate::SoundPresetEffect>>,
}

impl Session {
    #[cfg(test)]
    pub(crate) fn arrangement_test_sound(&self, query: &str) -> String {
        let mut job = self.sound_library_job(&[]);
        job.scan_plugins = false;
        let text = job
            .run(
                SoundSearch {
                    name: query.into(),
                    limit: 50,
                    ..Default::default()
                },
                false,
            )
            .unwrap();
        let result: serde_json::Value = serde_json::from_str(&text).unwrap();
        result["sounds"][0]["id"].as_str().unwrap().to_owned()
    }

    pub(crate) fn prepare_arrangement_sound(
        &self,
        id: &str,
        paths: &[PathBuf],
    ) -> Result<SoundPreparation, String> {
        let sound = resolve_library_sound(&self.sound_library_scopes(paths), id)?;
        if let Source::Font(preset) = &sound.source
            && (!self.soundfont_is_loaded(preset.font)
                || !self
                    .soundfont_presets(preset.font)
                    .iter()
                    .any(|p| p.bank == preset.bank && p.patch == preset.patch))
        {
            return Err("The selected SoundFont or patch is unavailable; search again".into());
        }
        if let Source::Patch { sound: patch, .. } = &sound.source
            && patch.soundfont.is_some()
        {
            return Err(
                "A saved sound that plays a SoundFont cannot revoice a part; apply it to a track with use_library_sound instead"
                    .into(),
            );
        }
        Ok(SoundPreparation {
            stamp: selection_stamp(&sound.source),
            source: sound.source,
        })
    }
}

impl SoundPreparation {
    pub(crate) fn still_current(&self) -> Result<(), String> {
        if selection_stamp(&self.source) == self.stamp {
            Ok(())
        } else {
            Err("The discovered plugin/preset changed; search and validate a fresh plan".into())
        }
    }

    pub(crate) fn run(&self, prepare: &PrepareContext) -> Result<PreparedSound, String> {
        self.still_current()?;
        let mut state = PluginState::empty();
        let mut effects = None;
        let (instrument_id, file) = match &self.source {
            Source::Builtin(id) => (id.clone(), None),
            Source::Font(preset) => {
                auris_sampler::store_preset(&mut state, *preset);
                (SAMPLER_ID.to_owned(), None)
            }
            Source::Patch { sound, .. } => {
                state = sound.state.clone();
                effects = Some(sound.effects.clone());
                (
                    sound.instrument_id.clone(),
                    sound.file.as_deref().map(AssetPath::external),
                )
            }
            Source::Clap {
                file, id, preset, ..
            } => {
                // SAFETY: this exact plugin ID came from the configured, scoped sound catalog.
                // The main-thread object is created and dropped on this worker, never transferred.
                let library =
                    unsafe { auris_clap::ClapLibrary::load(file) }.map_err(|e| e.to_string())?;
                let mut plugin = library.instantiate(id).map_err(|e| e.to_string())?;
                if let Some(preset) = preset {
                    plugin.load_preset(preset)?;
                }
                let audio = plugin
                    .activate_instrument(prepare)
                    .map_err(|e| e.to_string())?;
                plugin.deactivate_instrument(audio);
                state.set_hosted_bytes(&plugin.save_state().map_err(|e| e.to_string())?);
                (format!("clap:{id}"), Some(AssetPath::external(file)))
            }
            Source::Vst3 {
                file, id, preset, ..
            } => {
                let plugin =
                    auris_vst3::Vst3Plugin::load(file, id, prepare).map_err(|e| e.to_string())?;
                if let Some(preset) = preset {
                    plugin.load_preset(preset).map_err(|e| e.to_string())?;
                }
                state.set_hosted_bytes(&plugin.save_state().map_err(|e| e.to_string())?);
                (format!("vst3:{id}"), Some(AssetPath::external(file)))
            }
        };
        Ok(PreparedSound {
            instrument: InstrumentTrack {
                instrument_id,
                instrument_state: state,
                clips: Vec::new(),
                file,
                follows: None,
            },
            effects,
        })
    }
}
