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
    /// The consonant widths this model measured from its own training data, where the export
    /// carried the table. Newer exports do; older ones simply fall back to the host's fixed
    /// width.
    #[serde(default)]
    pub phoneme_durations: Option<PhonemeDurations>,
    /// How loud this model's training data sang each consonant against the vowel after it,
    /// where the export carried the table. Newer exports do; older ones fall back to the
    /// note's full level on every phoneme.
    #[serde(default)]
    pub phoneme_levels: Option<PhonemeLevels>,
    /// The presentational voice card, where the export carried one.
    #[serde(default)]
    pub voice: Option<VoiceCard>,
}

/// The consonant-width table an export measures from its training labels.
///
/// The application rule is the exporter's own: a phoneme takes `seconds[phoneme]` where the
/// table has it and `default` where it does not. The export also records how many labels each
/// number was measured from and what corpus they came from; that is provenance for a person,
/// not an input, and is deliberately not read here.
#[derive(Debug, Clone, Deserialize)]
pub struct PhonemeDurations {
    /// What the numbers are measured in. This build reads `seconds` and refuses anything
    /// else, because a table of frames read as seconds would be wrong by two orders.
    #[serde(default = "seconds")]
    pub unit: String,
    /// Seconds for a phoneme the table has no entry for.
    pub default: f64,
    /// Seconds per phoneme, keyed by the model's own symbols.
    #[serde(default)]
    pub seconds: BTreeMap<String, f64>,
}

fn seconds() -> String {
    "seconds".to_string()
}

/// The consonant-level table an export measures from its training data.
///
/// Decibels against the vowel that follows: a phoneme takes `db[phoneme]` where the table
/// has it and `default` where it does not. Counts and provenance ride along for a person and
/// are not read here, as with the widths.
#[derive(Debug, Clone, Deserialize)]
pub struct PhonemeLevels {
    /// What the numbers are measured in. This build reads `db` and refuses anything else —
    /// a table of linear gains read as decibels would be a whisper.
    #[serde(default = "db")]
    pub unit: String,
    /// Decibels for a consonant the table has no entry for.
    pub default: f64,
    /// Decibels per phoneme, keyed by the model's own symbols.
    #[serde(default)]
    pub db: BTreeMap<String, f64>,
}

fn db() -> String {
    "db".to_string()
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
        if let Some(durations) = &info.phoneme_durations {
            if durations.unit != "seconds" {
                return Err(SingError::Metadata(format!(
                    "phoneme durations in `{}` — this build reads seconds",
                    durations.unit
                )));
            }
            let broken = |seconds: &f64| !seconds.is_finite() || *seconds <= 0.0;
            if broken(&durations.default) || durations.seconds.values().any(broken) {
                return Err(SingError::Metadata(
                    "a phoneme duration that is not a positive number".into(),
                ));
            }
        }
        if let Some(levels) = &info.phoneme_levels {
            if levels.unit != "db" {
                return Err(SingError::Metadata(format!(
                    "phoneme levels in `{}` — this build reads db",
                    levels.unit
                )));
            }
            let broken = |db: &f64| !db.is_finite();
            if broken(&levels.default) || levels.db.values().any(broken) {
                return Err(SingError::Metadata(
                    "a phoneme level that is not a number".into(),
                ));
            }
        }
        Ok(info)
    }

    /// The model's consonant widths in the shape the document stores, where it measured any.
    ///
    /// The conversion lives here so a host never touches the raw table: what travels into a
    /// project is [`auris_core::ConsonantWidths`], the same struct the frame layout reads.
    pub fn consonant_widths(&self) -> Option<auris_core::ConsonantWidths> {
        self.phoneme_durations
            .as_ref()
            .map(|durations| auris_core::ConsonantWidths {
                default: durations.default,
                seconds: durations.seconds.clone(),
            })
    }

    /// The model's consonant levels in the shape the document stores, where it measured any.
    pub fn consonant_levels(&self) -> Option<auris_core::ConsonantLevels> {
        self.phoneme_levels
            .as_ref()
            .map(|levels| auris_core::ConsonantLevels {
                default: levels.default,
                db: levels.db.clone(),
            })
    }

    /// Seconds per feature frame — what a track's `frame_hop` must equal to be sung.
    pub fn hop_seconds(&self) -> f64 {
        f64::from(self.hop_length) / f64::from(self.sample_rate)
    }

    /// The speakers the model can sing as, in id order — one name per id.
    ///
    /// A single-speaker export carries one; a model trained on several corpora carries each
    /// source's name. An id the table does not name — a `speaker_to_id` shorter than
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

    /// The id behind a speaker's name, or `None` for a name the model never heard of.
    pub fn speaker_id(&self, name: &str) -> Option<u32> {
        self.speakers()
            .iter()
            .position(|known| known == name)
            .map(|at| at as u32)
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
        assert_eq!(info.speakers(), ["namine_ritsu"]);
        assert_eq!(info.speaker_id("namine_ritsu"), Some(0));
        assert_eq!(info.speaker_id("nobody"), None);
        // Fields this build does not know — f0_min, audio — pass through without complaint.
    }

    /// The new-spec export: the same voice with its measured consonant table aboard.
    const WITH_DURATIONS: &str = r#"{
        "format_version": 1,
        "sample_rate": 48000,
        "hop_length": 480,
        "inter_channels": 192,
        "symbols": ["<pad>", "<unk>", "<sil>", "a", "ts", "k"],
        "phoneme_durations": {
            "unit": "seconds",
            "default": 0.060,
            "seconds": {"ts": 0.119, "k": 0.091},
            "counts": {"ts": 679, "k": 4859},
            "measured_from": "Namine Ritsu singing DB Ver2.0.2, mono labels, 110 songs"
        }
    }"#;

    #[test]
    fn a_measured_consonant_table_rides_in_and_out_as_the_document_s_own_type() {
        let info = VoiceInfo::parse(WITH_DURATIONS).unwrap();
        let widths = info.consonant_widths().expect("the table was aboard");
        assert_eq!(widths.default, 0.060);
        assert_eq!(widths.width("ts"), 0.119, "measured");
        assert_eq!(widths.width("m"), 0.060, "unmeasured takes the default");
        // The provenance fields — counts, measured_from — pass through unread.

        // An old export simply has no table, and says so rather than inventing one.
        assert!(
            VoiceInfo::parse(RITSU)
                .unwrap()
                .consonant_widths()
                .is_none()
        );
    }

    #[test]
    fn a_duration_table_this_build_cannot_read_is_refused() {
        // A unit of frames read as seconds would be wrong by two orders of magnitude.
        let raw = WITH_DURATIONS.replace("\"unit\": \"seconds\"", "\"unit\": \"frames\"");
        let error = VoiceInfo::parse(&raw).unwrap_err();
        assert!(error.to_string().contains("frames"), "{error}");

        // A zero-length consonant is a table nobody can mean.
        let raw = WITH_DURATIONS.replace("0.119", "0.0");
        assert!(VoiceInfo::parse(&raw).is_err());
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

    #[test]
    fn the_level_table_rides_in_beside_the_widths_and_is_refused_in_the_wrong_unit() {
        let raw = r#"{"format_version": 1, "sample_rate": 48000, "hop_length": 480,
            "inter_channels": 192, "symbols": ["<pad>", "<unk>", "<sil>", "k", "a"],
            "phoneme_levels": {"unit": "db", "default": -12.0, "db": {"k": -22.6}}}"#;
        let info = VoiceInfo::parse(raw).expect("a level table in decibels loads");
        let levels = info.consonant_levels().expect("the table was aboard");
        assert_eq!(levels.db("k"), -22.6, "measured");
        assert_eq!(levels.db("s"), -12.0, "unmeasured takes the default");
        assert!(levels.measured("k") && !levels.measured("s"));

        let linear = raw.replace("\"unit\": \"db\"", "\"unit\": \"gain\"");
        let error = VoiceInfo::parse(&linear).expect_err("a table of gains is not decibels");
        assert!(error.to_string().contains("gain"), "{error}");
        let nan = raw.replace("-22.6", "null");
        assert!(
            VoiceInfo::parse(&nan).is_err(),
            "a level that is not a number is refused"
        );
    }
}

#[cfg(test)]
mod speaker_tests {
    use super::*;

    #[test]
    fn every_id_gets_a_name_and_the_names_keep_id_order() {
        let info: VoiceInfo = serde_json::from_value(serde_json::json!({
            "format_version": 1, "sample_rate": 48000, "hop_length": 480, "inter_channels": 192,
            "n_speakers": 3, "symbols": ["<pad>", "<unk>", "<sil>", "a"],
            "speaker_to_id": {"zoe": 1, "abe": 0}
        }))
        .expect("a three-speaker model with one id unnamed");
        assert_eq!(
            info.speakers(),
            ["abe", "zoe", "2"],
            "id order, and a number for the unnamed"
        );
        assert_eq!(info.speaker_id("zoe"), Some(1));
        assert_eq!(info.speaker_id("2"), Some(2));
    }
}
