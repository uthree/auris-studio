//! Live sound selection: the model receives handles, never filesystem arguments.

use std::path::PathBuf;

use super::Session;
use crate::prelude::*;

#[derive(Clone)]
struct PluginSound {
    token: String,
    file: PathBuf,
    native_id: String,
    name: String,
    vendor: String,
    vst3: bool,
}

#[derive(Default)]
pub(super) struct PluginCatalog {
    paths: Option<Vec<PathBuf>>,
    next_id: u64,
    sounds: Vec<PluginSound>,
    errors: Vec<String>,
}

impl Session {
    fn scan_agent_instruments(&mut self, paths: &[PathBuf], refresh: bool) {
        if !refresh && self.agent_instruments.paths.as_deref() == Some(paths) {
            return;
        }
        self.agent_instruments.paths = Some(paths.to_vec());
        self.agent_instruments.sounds.clear();
        self.agent_instruments.errors.clear();
        let files = self
            .installed_clap_files(paths)
            .into_iter()
            .map(|p| (p, false))
            .chain(
                self.installed_vst3_files(paths)
                    .into_iter()
                    .map(|p| (p, true)),
            )
            .collect::<Vec<_>>();
        for (file, vst3) in files {
            let result = if vst3 {
                self.vst3_plugins_in(&file).map(|plugins| {
                    plugins
                        .into_iter()
                        .filter(|p| p.kind == PluginKind::Instrument)
                        .map(|p| (p.class_id, p.name, p.vendor))
                        .collect::<Vec<_>>()
                })
            } else {
                self.hosted_plugins_in(&file).map(|plugins| {
                    plugins
                        .into_iter()
                        .filter(|p| p.kind == PluginKind::Instrument)
                        .map(|p| (p.clap_id, p.name, p.vendor))
                        .collect::<Vec<_>>()
                })
            };
            match result {
                Ok(plugins) => {
                    for (native_id, name, vendor) in plugins {
                        // Handles are never reused: a rescan must not redirect an old selection.
                        self.agent_instruments.next_id += 1;
                        self.agent_instruments.sounds.push(PluginSound {
                            token: format!("plugin:{}", self.agent_instruments.next_id),
                            file: file.clone(),
                            native_id,
                            name,
                            vendor,
                            vst3,
                        });
                    }
                }
                Err(error) => self
                    .agent_instruments
                    .errors
                    .push(format!("{}: {error}", file.display())),
            }
        }
    }

    pub(crate) fn agent_instrument_list(
        &mut self,
        query: Option<&str>,
        offset: usize,
        refresh: bool,
        paths: &[PathBuf],
    ) -> Result<String, String> {
        if refresh && offset != 0 {
            return Err("Refresh only on the first page (offset=0)".into());
        }
        self.scan_agent_instruments(paths, refresh);
        let mut sounds = self
            .registry()
            .instruments()
            // The sampler needs a concrete font preset; its bare ID is not a usable sound.
            .filter(|p| p.id != SAMPLER_ID)
            .map(|p| serde_json::json!({"id":p.id,"name":p.name,"source":"builtin"}))
            .collect::<Vec<_>>();
        for font in self
            .soundfonts()
            .filter(|font| self.soundfont_is_loaded(font.id))
        {
            for preset in self.soundfont_presets(font.id) {
                sounds.push(serde_json::json!({
                    "id":format!("soundfont:{}:{}:{}",font.id.0,preset.bank,preset.patch),
                    "name":preset.name,"library":font.name,"source":"soundfont",
                    "bank":preset.bank,"program":preset.patch,
                }));
            }
        }
        for sound in &self.agent_instruments.sounds {
            sounds.push(serde_json::json!({"id":sound.token,"name":sound.name,
                "vendor":sound.vendor,"source":if sound.vst3 {"vst3"} else {"clap"}}));
        }
        let query = query.unwrap_or_default().trim().to_lowercase();
        sounds.retain(|sound| {
            ["id", "name", "library", "vendor", "source"]
                .iter()
                .any(|key| {
                    sound[key]
                        .as_str()
                        .is_some_and(|text| text.to_lowercase().contains(&query))
                })
        });
        sounds.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        let total = sounds.len();
        if offset > total {
            return Err("Offset exceeds the matching sound count".into());
        }
        let page = sounds.into_iter().skip(offset).take(50).collect::<Vec<_>>();
        let next = offset + page.len();
        Ok(serde_json::json!({"sounds":page,"total":total,
            "next_offset":(next<total).then_some(next),
            "scan_errors":self.agent_instruments.errors.iter().take(10).collect::<Vec<_>>(),
            "scan_error_count":self.agent_instruments.errors.len(),
            "usage":"Copy an id into set_instrument. SoundFonts must already be loaded. Plugin handles expire on rescan."}).to_string())
    }

    pub(crate) fn agent_set_instrument(&mut self, track: TrackId, id: &str) -> Result<(), String> {
        if !self
            .project()
            .track(track)
            .is_some_and(|t| t.kind.is_instrument())
        {
            return Err("Choose an existing instrument track from inspect_project".into());
        }
        if let Some(reference) = id.strip_prefix("soundfont:") {
            let fields = reference.split(':').collect::<Vec<_>>();
            if fields.len() != 3 {
                return Err("Use an exact soundfont id from list_instruments".into());
            }
            let preset = PresetRef {
                font: SoundFontId(fields[0].parse().map_err(|_| "Invalid font ID")?),
                bank: fields[1].parse().map_err(|_| "Invalid bank")?,
                patch: fields[2].parse().map_err(|_| "Invalid program")?,
            };
            if !self.soundfont_is_loaded(preset.font)
                || !self
                    .soundfont_presets(preset.font)
                    .iter()
                    .any(|p| p.bank == preset.bank && p.patch == preset.patch)
            {
                return Err("This sound is unavailable; call list_instruments again".into());
            }
            return self
                .set_track_preset(track, preset)
                .map_err(|e| e.to_string());
        }
        if id.starts_with("plugin:") {
            let sound = self
                .agent_instruments
                .sounds
                .iter()
                .find(|p| p.token == id)
                .cloned()
                .ok_or("Unknown or expired plugin ID; call list_instruments again")?;
            return if sound.vst3 {
                self.set_vst3_instrument(track, &sound.file, &sound.native_id)
            } else {
                self.set_hosted_instrument(track, &sound.file, &sound.native_id)
            }
            .map_err(|e| e.to_string());
        }
        if id == SAMPLER_ID {
            return Err("Choose a concrete SoundFont sound from list_instruments".into());
        }
        self.set_track_instrument(track, id)
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::{Scratch, session};

    #[test]
    fn live_font_selection_preserves_score_and_undo_and_rejects_missing_sounds() {
        let scratch = Scratch::new("agent-sounds");
        let mut session = session();
        let font = session
            .import_soundfont(&scratch.soundfont("live.sf2"))
            .unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session
            .add_midi_clip(track, "Keep", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        // Avoid scanning the test machine's external plugins.
        session.agent_instruments.paths = Some(vec![]);
        let before = session.project().clone();
        let listing: serde_json::Value = serde_json::from_str(
            &session
                .agent_instrument_list(Some("soundfont"), 0, false, &[])
                .unwrap(),
        )
        .unwrap();
        assert_eq!(session.project(), &before);
        let id = listing["sounds"][0]["id"].as_str().unwrap();
        session.agent_set_instrument(track, id).unwrap();
        assert_eq!(session.track_preset(track).unwrap().font, font);
        assert_eq!(
            session.project().track(track).unwrap().kind.note_clips(),
            before.track(track).unwrap().kind.note_clips()
        );
        session.undo().unwrap();
        assert_eq!(session.project(), &before);
        assert!(
            session
                .agent_set_instrument(track, "soundfont:9999:0:0")
                .is_err()
        );
        assert!(session.agent_set_instrument(track, "plugin:9999").is_err());
        assert!(
            session
                .agent_set_instrument(track, "/tmp/arbitrary.clap")
                .is_err()
        );
        assert_eq!(session.project(), &before);
        let empty: serde_json::Value = serde_json::from_str(
            &session
                .agent_instrument_list(Some("not-a-real-sound"), 0, false, &[])
                .unwrap(),
        )
        .unwrap();
        assert_eq!(empty["total"], 0);
        assert!(
            session
                .agent_instrument_list(None, usize::MAX, false, &[])
                .is_err()
        );
    }
    #[test]
    fn search_pages_cover_the_plugin_library_without_changing_the_document() {
        let mut session = session();
        session.agent_instruments.paths = Some(vec![]);
        session.agent_instruments.sounds = (1..=121)
            .map(|index| PluginSound {
                token: format!("plugin:{index}"),
                file: PathBuf::from("missing.vst3"),
                native_id: format!("class-{index}"),
                name: format!("Synth {index}"),
                vendor: "Test Vendor".into(),
                vst3: true,
            })
            .collect();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let before = session.project().clone();
        let mut ids = std::collections::HashSet::new();
        let mut offset = 0;
        loop {
            let page: serde_json::Value = serde_json::from_str(
                &session
                    .agent_instrument_list(Some("TEST VENDOR"), offset, false, &[])
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(page["total"], 121);
            let sounds = page["sounds"].as_array().unwrap();
            assert!(sounds.len() <= 50);
            for sound in sounds {
                assert!(ids.insert(sound["id"].as_str().unwrap().to_owned()));
            }
            let Some(next) = page["next_offset"].as_u64() else {
                break;
            };
            offset = next as usize;
        }
        assert_eq!(ids.len(), 121);
        // A previously listed bundle can disappear before selection.
        assert!(session.agent_set_instrument(track, "plugin:1").is_err());
        assert_eq!(session.project(), &before);
        assert!(session.agent_instrument_list(None, 50, true, &[]).is_err());
    }

    #[test]
    #[ignore = "loads installed native plugins; set AURIS_AGENT_TEST_INSTRUMENT to a name"]
    fn installed_plugin_can_be_selected_and_undone_through_live_commands() {
        let query = std::env::var("AURIS_AGENT_TEST_INSTRUMENT").unwrap();
        let mut session = session();
        let track = session
            .add_default_instrument_track("Keep this track")
            .unwrap();
        let before = session.project().clone();
        let listing: serde_json::Value = serde_json::from_str(
            &session
                .agent_command(crate::live_agent::Command::ListInstruments {
                    query: Some(query),
                    offset: 0,
                    refresh: false,
                })
                .unwrap(),
        )
        .unwrap();
        println!("{listing}");
        assert_eq!(session.project(), &before);
        let sounds = listing["sounds"].as_array().unwrap();
        let plugins = sounds
            .iter()
            .filter(|p| p["source"] == "vst3" || p["source"] == "clap")
            .collect::<Vec<_>>();
        assert!(!plugins.is_empty());
        for sound in plugins {
            session
                .agent_command(crate::live_agent::Command::SetInstrument {
                    track: track.0,
                    instrument: sound["id"].as_str().unwrap().into(),
                })
                .unwrap();
            let inner = session
                .project()
                .track(track)
                .unwrap()
                .kind
                .as_instrument()
                .unwrap();
            assert!(inner.file.is_some());
            session.undo().unwrap();
            assert_eq!(session.project(), &before);
        }
    }
}
