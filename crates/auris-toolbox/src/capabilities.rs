//! Asset availability, distinguished from registered plugin names.
use super::*;

pub(super) fn playback_warnings(session: &Session) -> String {
    session
        .playback_readiness()
        .into_iter()
        .filter_map(|r| {
            let reason = match r.state {
                PlaybackState::Ready => return None,
                PlaybackState::MissingSoundfont => {
                    "sampler has no playable SoundFont preset; this track is silent"
                }
                PlaybackState::MissingInstrument => {
                    "instrument is unavailable; this track is silent"
                }
                PlaybackState::GuideVoice => {
                    "temporary synthesized guide voice; run sing with a voice to produce vocals"
                }
                PlaybackState::StaleVocal => {
                    "vocal take is stale; run sing again to match the current notes and voice"
                }
            };
            Some(format!("Note: {}: {reason}.\n", r.name))
        })
        .collect()
}

/// Discovers installed sound and voice resources and project playback readiness.
pub mod capabilities {
    use super::*;
    /// The wire name.
    pub const NAME: &str = "capabilities";
    /// The model-facing description.
    pub const DESCRIPTION: &str = "Reports General MIDI availability, installed voice paths from the desktop's library settings, and optional project playback readiness and selected voice metadata. Voice discovery does not load or validate models. Guide vocals are temporary synthesis; stale takes need sing again. Audio preview creates a playable file; it does not imply that the connected language model can hear audio.";
    /// Optional project whose instruments and vocals should be inspected.
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    pub struct Args {
        /// Absolute project path; omit for the machine's available resources.
        pub project: Option<String>,
    }
    /// Reads availability without modifying a project or downloading assets.
    pub fn run(args: &Args) -> Result<String, String> {
        let session = match &args.project {
            Some(path) => opened(path)?,
            None => headless()?,
        };
        let voices: Vec<_> = auris_session::library::voices_with_settings(&auris_session::Settings::load()).into_iter()
            .map(|(name, path)| serde_json::json!({"name":name,"backend":auris_session::library::voice_source_kind(&path).map(|kind| format!("{kind:?}")),"path":path,"validated":false})).collect();
        let selected: Vec<_> = session.project().tracks.iter().filter(|t| t.kind.is_singer()).map(|track| {
            let voice = session.singer_voice_info(track.id).ok().flatten().map(|info| serde_json::json!({
                "name":info.name,"path":info.path,"backend":format!("{:?}",info.backend),
                "speakers":info.speakers,"speaker":info.speaker,"corrections":{
                    "manual_phonemes":info.capabilities.manual_phonemes,
                    "phoneme_timing":info.capabilities.phoneme_timing
                }
            }));
            serde_json::json!({"track":format!("id:{}",track.id.0),"voice":voice})
        }).collect();
        Ok(serde_json::json!({"general_midi_available":session.general_midi_available(),"voices":voices,
            "playback":session.playback_readiness(),"selected_voices":selected,
            "preview":"playable audio file; model audio input support depends on the provider and model"}).to_string())
    }
}
