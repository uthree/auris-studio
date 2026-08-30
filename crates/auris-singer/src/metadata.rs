//! What an exported voice model says about itself.
//!
//! An auris-singer `.onnx` file is self-contained: the phoneme table, the audio parameters and
//! a presentational *voice card* ride inside the file's `metadata_props` as JSON under the
//! `auris_singer` key (a `.json` sidecar carries the same object for tools that would rather
//! not parse protobuf). [`VoiceInfo`] is that object read and checked, so everything after
//! loading can trust the numbers.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::SingError;

/// The `metadata_props` key an auris-singer export stores its JSON under.
pub const METADATA_KEY: &str = "auris_singer";

/// The one metadata format this build reads.
///
/// The exporter stamps `format_version: 1`; a bigger number means the file was written by a
/// newer auris-singer whose fields this build could misread, and is refused rather than
/// half-understood.
pub const FORMAT_VERSION: u32 = 1;

/// A voice model's own account of itself: audio parameters, phoneme table, speakers, card.
#[derive(Debug, Clone, Deserialize)]
pub struct VoiceInfo {
    /// Metadata format version — see [`FORMAT_VERSION`].
    pub format_version: u32,
    /// Samples per second the model sings at.
    pub sample_rate: u32,
    /// Samples per feature frame; `hop_length / sample_rate` is the frame hop in seconds.
    pub hop_length: u32,
    /// Channels of the prior's latent — the shape of the noise the caller must supply.
    pub inter_channels: u32,
    /// How many speakers the model was trained on.
    #[serde(default = "one")]
    pub n_speakers: u32,
    /// The phoneme table: index in this list is the id the model's `phonemes` input wants.
    pub symbols: Vec<String>,
    /// Speaker names to ids, for models trained on more than one voice.
    #[serde(default)]
    pub speaker_to_id: BTreeMap<String, u32>,
    /// The presentational voice card, where the export carried one.
    #[serde(default)]
    pub voice: Option<VoiceCard>,
}

fn one() -> u32 {
    1
}

/// What a host shows a person browsing voices, as opposed to what it feeds the model.
///
/// Every field is free-form prose from whoever exported the model; the portrait image the
/// format can also carry is deliberately not read here — it is base64 the size of a picture,
/// and nothing below the frontends wants it in memory.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct VoiceCard {
    /// The voice's display name — 波音リツ, not `auris_singer_ritsu_40k.onnx`.
    #[serde(default)]
    pub name: String,
    /// A sentence about the voice: range, character, training data.
    #[serde(default)]
    pub description: String,
    /// Version label of this export.
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
    /// Reads the metadata JSON and refuses anything a later `sing` would trip over.
    pub(crate) fn parse(raw: &str) -> Result<VoiceInfo, SingError> {
        let info: VoiceInfo = serde_json::from_str(raw)
            .map_err(|error| SingError::Metadata(format!("unreadable metadata: {error}")))?;
        if info.format_version > FORMAT_VERSION {
            return Err(SingError::Metadata(format!(
                "metadata format {} is newer than the {} this build reads — update Auris Studio",
                info.format_version, FORMAT_VERSION
            )));
        }
        if info.sample_rate == 0 || info.hop_length == 0 || info.inter_channels == 0 {
            return Err(SingError::Metadata(
                "a zero among sample_rate, hop_length and inter_channels".into(),
            ));
        }
        for required in [crate::score::MODEL_SILENCE, crate::score::MODEL_UNKNOWN] {
            if !info.symbols.iter().any(|symbol| symbol == required) {
                return Err(SingError::Metadata(format!(
                    "the phoneme table is missing {required}"
                )));
            }
        }
        Ok(info)
    }

    /// Seconds per feature frame — what a track's `frame_hop` must equal to be sung.
    pub fn hop_seconds(&self) -> f64 {
        f64::from(self.hop_length) / f64::from(self.sample_rate)
    }

    /// The voice's display name: the card's, or empty where no card was embedded.
    pub fn display_name(&self) -> &str {
        self.voice
            .as_ref()
            .map(|card| card.name.as_str())
            .unwrap_or("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped ritsu-40k sidecar, trimmed to the fields that matter and one of each list.
    const RITSU: &str = r#"{
        "format_version": 1,
        "sample_rate": 48000,
        "hop_length": 480,
        "inter_channels": 192,
        "n_speakers": 1,
        "f0_min": 40.0,
        "symbols": ["<pad>", "<unk>", "<sil>", "<pau>", "a", "k", "tɕ"],
        "speaker_to_id": {"namine_ritsu": 0},
        "audio": {"sample_rate": 48000, "n_fft": 2048},
        "voice": {"name": "波音リツ", "description": "Strong low-range female voice.",
                  "credits": ["波音リツ", "カノン"]}
    }"#;

    #[test]
    fn the_shipped_metadata_parses_and_answers_its_hop() {
        let info = VoiceInfo::parse(RITSU).unwrap();
        assert_eq!(info.sample_rate, 48_000);
        assert_eq!(info.hop_seconds(), 0.010);
        assert_eq!(info.display_name(), "波音リツ");
        assert_eq!(info.speaker_to_id.get("namine_ritsu"), Some(&0));
        // Fields this build does not know — f0_min, audio — pass through without complaint.
    }

    #[test]
    fn a_newer_format_is_refused_rather_than_misread() {
        let raw = RITSU.replace("\"format_version\": 1", "\"format_version\": 2");
        let error = VoiceInfo::parse(&raw).unwrap_err();
        assert!(error.to_string().contains("newer"), "{error}");
    }

    #[test]
    fn a_table_without_the_specials_is_refused() {
        let raw = RITSU.replace("\"<sil>\", ", "");
        let error = VoiceInfo::parse(&raw).unwrap_err();
        assert!(error.to_string().contains("<sil>"), "{error}");
    }
}
