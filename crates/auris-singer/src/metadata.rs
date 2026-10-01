//! Voice information supplied by singing backends.

use std::collections::BTreeMap;

use serde::Deserialize;

/// Audio parameters, phoneme vocabulary, speakers and presentation for a loaded voice.
#[derive(Debug, Clone, Deserialize)]
pub struct VoiceInfo {
    /// Samples per second.
    pub sample_rate: u32,
    /// Samples per feature frame.
    pub hop_length: u32,
    /// Number of available speakers.
    pub n_speakers: u32,
    /// Backend phoneme vocabulary, indexed by id.
    pub symbols: Vec<String>,
    /// Speaker names mapped to backend ids.
    pub speaker_to_id: BTreeMap<String, u32>,
    /// Optional presentation card.
    pub voice: Option<VoiceCard>,
}

/// Presentation information for a voice.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct VoiceCard {
    /// The voice's display name — the name shown in the library.
    #[serde(default)]
    pub name: String,
    /// A sentence about the voice: range, character, training data.
    #[serde(default)]
    pub description: String,
    /// Version label of this voice.
    #[serde(default)]
    pub version: String,
    /// The terms the voice is distributed under.
    #[serde(default)]
    pub license: String,
    /// Who the voice is: singer, character, database authors.
    #[serde(default)]
    pub credits: Vec<String>,
    /// Where to read more.
    #[serde(default)]
    pub url: String,
}

impl VoiceInfo {
    /// Seconds per feature frame — what a track's `frame_hop` must equal to be sung.
    pub fn hop_seconds(&self) -> f64 {
        f64::from(self.hop_length) / f64::from(self.sample_rate)
    }

    /// The speakers the model can sing as, in id order — one name per id.
    ///
    /// An id the table does not name — a `speaker_to_id` shorter than
    /// `n_speakers` — is listed as its number, so every id has a name to be chosen by.
    pub fn speakers(&self) -> Vec<String> {
        (0..self.n_speakers)
            .map(|id| {
                self.speaker_to_id
                    .iter()
                    .find(|(_, at)| **at == id)
                    .map(|(name, _)| name.clone())
                    .unwrap_or_else(|| id.to_string())
            })
            .collect()
    }

    /// The id behind a speaker's name, or `None` for an unknown name.
    pub fn speaker_id(&self, name: &str) -> Option<u32> {
        self.speakers()
            .iter()
            .position(|known| known == name)
            .map(|at| at as u32)
    }

    /// The voice's display name, or empty when there is no card.
    pub fn display_name(&self) -> &str {
        self.voice
            .as_ref()
            .map(|card| card.name.as_str())
            .unwrap_or("")
    }
}
