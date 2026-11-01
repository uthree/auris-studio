//! Freeze a discovered sound's identity before loading it away from the live document.

use super::*;
use auris_core::AssetPath;
use auris_core::project::InstrumentTrack;

#[derive(Clone)]
pub(crate) struct SoundPreparation {
    source: Source,
    stamp: Option<String>,
}

impl Session {
    #[cfg(test)]
    pub(crate) fn arrangement_test_sound(&self, query: &str) -> String {
        let mut job = self.sound_library_job(&[]);
        job.scan_plugins = false;
        let text = job
            .run(
                SoundSearch::Text {
                    query: query.into(),
                    filter: SoundFilter::default(),
                    offset: 0,
                    limit: 50,
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
        let sound = resolve_library_sound(&self.sound_library_scope(paths), id)?;
        if let Source::Font(preset) = &sound.source
            && (!self.soundfont_is_loaded(preset.font)
                || !self
                    .soundfont_presets(preset.font)
                    .iter()
                    .any(|p| p.bank == preset.bank && p.patch == preset.patch))
        {
            return Err("The selected SoundFont or patch is unavailable; search again".into());
        }
        let stamp = match &sound.source {
            Source::Clap { file, preset, .. } => Some(clap_selection_stamp(file, preset.as_ref())),
            Source::Vst3 { file, preset, .. } => Some(vst3_selection_stamp(file, preset.as_ref())),
            _ => None,
        };
        Ok(SoundPreparation {
            source: sound.source,
            stamp,
        })
    }
}

impl SoundPreparation {
    pub(crate) fn still_current(&self) -> Result<(), String> {
        let unchanged = match &self.source {
            Source::Clap {
                file,
                preset,
                discovery_stamp,
                isolated_metadata,
                ..
            } => {
                let current = clap_selection_stamp(file, preset.as_ref());
                (*isolated_metadata || current == *discovery_stamp)
                    && self.stamp.as_ref() == Some(&current)
            }
            Source::Vst3 {
                file,
                preset,
                discovery_stamp,
                isolated_metadata,
                ..
            } => {
                let current = vst3_selection_stamp(file, preset.as_ref());
                (*isolated_metadata || current == *discovery_stamp)
                    && self.stamp.as_ref() == Some(&current)
            }
            _ => true,
        };
        if unchanged {
            Ok(())
        } else {
            Err("The discovered plugin/preset changed; search and validate a fresh plan".into())
        }
    }

    pub(crate) fn run(&self, prepare: &PrepareContext) -> Result<InstrumentTrack, String> {
        self.still_current()?;
        let mut state = PluginState::empty();
        let (instrument_id, file) = match &self.source {
            Source::Builtin(id) => (id.clone(), None),
            Source::Font(preset) => {
                auris_sampler::store_preset(&mut state, *preset);
                (SAMPLER_ID.to_owned(), None)
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
        Ok(InstrumentTrack {
            instrument_id,
            instrument_state: state,
            clips: Vec::new(),
            file,
            follows: None,
        })
    }
}
