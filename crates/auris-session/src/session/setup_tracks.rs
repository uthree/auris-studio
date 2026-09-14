//! Bounded, all-or-nothing initial arrangement setup.
use super::*;

/// Type of a newly created track.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SetupTrackKind {
    /// Melodic instrument.
    Instrument,
    /// Drum editor track.
    Drum,
    /// Vocal note track.
    Singer,
    /// Audio recording track.
    Audio,
    /// Mixer bus.
    Bus,
}

/// Optional empty clip, addressed in the project's meter.
#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetupClip {
    /// Nonempty clip name, at most 160 characters.
    #[schemars(length(min = 1, max = 160))]
    pub name: String,
    /// One-based first bar.
    #[schemars(range(min = 1))]
    pub start_bar: u32,
    /// Number of bars, 1..1024.
    #[schemars(range(min = 1, max = 1024))]
    pub bars: u32,
}
/// One track and optionally its sound and first empty clip.
#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetupTrack {
    /// Unique nonempty track name, at most 160 characters.
    #[schemars(length(min = 1, max = 160))]
    pub name: String,
    /// Explicit track kind.
    pub kind: SetupTrackKind,
    /// Exact search result ID; omit for the default sound. Instrument/drum only.
    pub sound_id: Option<String>,
    /// Empty note clip; only instrument, drum, or singer tracks.
    pub clip: Option<SetupClip>,
}

impl Session {
    /// Creates up to 16 tracks as one undo step. Any failure restores the document.
    /// Existing names are rejected, so replay cannot silently duplicate tracks.
    pub fn setup_tracks(
        &mut self,
        tracks: &[SetupTrack],
        plugin_paths: &[std::path::PathBuf],
    ) -> Result<String, String> {
        if self.transaction.is_some() {
            return Err("Finish the current edit before setup_tracks".into());
        }
        if !(1..=16).contains(&tracks.len()) {
            return Err("tracks must contain 1..16 items".into());
        }
        let mut names = std::collections::HashSet::new();
        for track in tracks {
            if track.name.trim().eq_ignore_ascii_case("master")
                || track.name.trim().starts_with("id:")
            {
                return Err("Track name must not be master or an id: selector".into());
            }
            if track.name.trim().is_empty() || track.name.chars().count() > 160 {
                return Err("Track name must contain 1..160 characters".into());
            }
            if !names.insert(track.name.trim().to_lowercase())
                || self
                    .project
                    .tracks
                    .iter()
                    .any(|t| t.name.trim().eq_ignore_ascii_case(track.name.trim()))
            {
                return Err(format!(
                    "Track '{}' already exists or occurs twice; inspect before retrying",
                    track.name
                ));
            }
            if track.sound_id.is_some()
                && !matches!(
                    track.kind,
                    SetupTrackKind::Instrument | SetupTrackKind::Drum
                )
            {
                return Err("sound_id requires an instrument or drum track".into());
            }
            if let Some(clip) = &track.clip {
                if matches!(track.kind, SetupTrackKind::Audio | SetupTrackKind::Bus) {
                    return Err("An audio track or bus cannot hold a note clip".into());
                }
                if clip.name.trim().is_empty()
                    || clip.name.chars().count() > 160
                    || clip.start_bar == 0
                    || !(1..=1024).contains(&clip.bars)
                    || clip.start_bar.checked_add(clip.bars).is_none()
                {
                    return Err("Clip requires a name, start_bar >= 1, bars 1..1024, and a representable end bar".into());
                }
            }
        }
        self.begin_transaction(Edit::AddInstrumentTrack);
        let result = (|| {
            let mut output = Vec::new();
            for track in tracks {
                let id = match track.kind {
                    SetupTrackKind::Instrument => self
                        .add_default_instrument_track(&track.name)
                        .map_err(|e| e.to_string())?,
                    SetupTrackKind::Drum => self
                        .add_default_drum_track(&track.name)
                        .map_err(|e| e.to_string())?,
                    SetupTrackKind::Singer => self.add_singer_track(&track.name),
                    SetupTrackKind::Audio => self.add_audio_track(&track.name),
                    SetupTrackKind::Bus => self.add_bus_track(&track.name),
                };
                if let Some(sound) = &track.sound_id {
                    self.use_library_sound(id, sound, plugin_paths)?;
                }
                let clip = if let Some(clip) = &track.clip {
                    let start = self.project.signatures.bar_start(clip.start_bar);
                    let length = self
                        .project
                        .signatures
                        .bar_start(clip.start_bar + clip.bars)
                        - start;
                    Some(
                        self.add_midi_clip(id, &clip.name, start, length)
                            .map_err(|e| e.to_string())?
                            .0,
                    )
                } else {
                    None
                };
                output.push(serde_json::json!({"track":format!("id:{}",id.0),"name":track.name,"clip_id":clip,"clip":clip.map(|_|1)}));
            }
            Ok(serde_json::json!({"tracks":output}).to_string())
        })();
        if result.is_ok() {
            self.end_transaction();
        } else {
            self.revert_transaction();
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::session;
    fn tracks() -> Vec<SetupTrack> {
        serde_json::from_value(serde_json::json!([
            {"name":"Lead","kind":"instrument","clip":{"name":"Verse","start_bar":2,"bars":4}},
            {"name":"Kit","kind":"drum"}
        ]))
        .unwrap()
    }
    #[test]
    fn setup_is_one_undo_step_and_a_failed_later_sound_restores_everything() {
        let mut session = session();
        session.forget_history();
        let before = session.project().clone();
        let mut items = tracks();
        items[1].sound_id = Some("s:expired:1".into());
        assert!(session.setup_tracks(&items, &[]).is_err());
        assert_eq!(session.project(), &before);
        assert!(!session.can_undo());
        items[1].sound_id = None;
        session.setup_tracks(&items, &[]).unwrap();
        let after = session.project().clone();
        assert!(session.setup_tracks(&items, &[]).is_err());
        assert_eq!(session.project(), &after);
        assert!(session.undo().is_some());
        assert_eq!(session.project(), &before);
        assert!(!session.can_undo());
    }
}
