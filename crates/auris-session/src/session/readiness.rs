//! Playback prerequisites, queried without changing the document.

use super::{Session, SingerTakeState};
use auris_core::TrackId;
use auris_sampler::SAMPLER_ID;

/// What will play for a track, independently of its mute/solo state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackState {
    /// The instrument or rendered singer take is available.
    Ready,
    /// The sampler has no loaded font and valid preset.
    MissingSoundfont,
    /// The requested instrument could not be loaded.
    MissingInstrument,
    /// Singer notes play through the temporary guide synthesizer.
    GuideVoice,
    /// The rendered singer take predates the current score or voice selection.
    StaleVocal,
}

/// The current playback readiness of a note track.
#[derive(Clone, Debug, serde::Serialize)]
pub struct PlaybackReadiness {
    /// Stable track identifier.
    pub track: TrackId,
    /// Track display name.
    pub name: String,
    /// Current instrument/take state.
    pub state: PlaybackState,
}

impl Session {
    /// Whether the standard General MIDI library is loaded and can be selected by program.
    pub fn general_midi_available(&self) -> bool {
        crate::library::shipped(crate::library::GENERAL_MIDI)
            .and_then(crate::library::installed)
            .and_then(|path| self.project.soundfont_at(self.project_folder(), &path))
            .is_some_and(|id| self.soundfont_is_loaded(id))
    }

    /// Reports unavailable instruments and guide/stale vocals before inspection or export.
    pub fn playback_readiness(&self) -> Vec<PlaybackReadiness> {
        self.project
            .tracks
            .iter()
            .filter_map(|track| {
                let state = if track.kind.is_singer() {
                    match self.singer_take_state(track.id).ok()? {
                        SingerTakeState::Absent => PlaybackState::GuideVoice,
                        SingerTakeState::Behind => PlaybackState::StaleVocal,
                        SingerTakeState::Current => PlaybackState::Ready,
                    }
                } else {
                    let instrument = track.kind.as_instrument()?;
                    if instrument.instrument_id == SAMPLER_ID {
                        let ready = self.track_preset(track.id).is_some_and(|preset| {
                            self.soundfont_presets(preset.font)
                                .iter()
                                .any(|p| p.bank == preset.bank && p.patch == preset.patch)
                        });
                        if ready {
                            PlaybackState::Ready
                        } else {
                            PlaybackState::MissingSoundfont
                        }
                    } else if self.registry.has_instrument(&instrument.instrument_id)
                        || self.hosted_instrument_name(track.id).is_some()
                    {
                        PlaybackState::Ready
                    } else {
                        PlaybackState::MissingInstrument
                    }
                };
                Some(PlaybackReadiness {
                    track: track.id,
                    name: track.name.clone(),
                    state,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sampler_registration_does_not_promise_loaded_samples() {
        let mut session = Session::new(super::super::SessionOptions::headless()).unwrap();
        session
            .add_instrument_track("Empty sampler", SAMPLER_ID)
            .unwrap();
        session.add_singer_track("Unsung");
        let report = session.playback_readiness();
        assert_eq!(report[0].state, PlaybackState::MissingSoundfont);
        assert_eq!(report[1].state, PlaybackState::GuideVoice);
        assert!(!session.general_midi_available());
    }
}
