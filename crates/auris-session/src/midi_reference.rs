//! Bounded, independent scores used as references in an agent conversation.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use auris_core::time::{SignatureMap, TICKS_PER_QUARTER, TempoMap};
use auris_core::{MidiClip, Note};
use serde::{Deserialize, Serialize};

/// Maximum size of a reference file read from disk, and of the encoded reference sent to the
/// model, including lyrics and timeline metadata.
///
/// The bounds exist only to refuse files no song needs — a "black MIDI" with millions of notes,
/// or something that is not a score at all — before it is held in memory several times over.
/// A finished, fully orchestrated song is a few tens of thousands of notes and well inside them;
/// whether a long score also fits a particular model's context is the provider's to say.
pub const MAX_REFERENCE_BYTES: usize = 16 * 1024 * 1024;
/// Maximum notes in one conversational score, summed over its tracks.
pub const MAX_REFERENCE_NOTES: usize = 100_000;
/// Maximum parts in one conversational score.
pub const MAX_REFERENCE_TRACKS: usize = 256;

fn too_many_notes() -> String {
    format!(
        "MIDI reference needs 1–{MAX_REFERENCE_TRACKS} tracks and at most {MAX_REFERENCE_NOTES} notes."
    )
}

fn too_large() -> String {
    format!(
        "MIDI reference exceeds {} MiB. Attach a shorter excerpt.",
        MAX_REFERENCE_BYTES / (1024 * 1024)
    )
}

/// A score snapshot, independent of the source song and of later edits.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MidiReference {
    /// Reference format version, independent of the project format.
    pub version: u32,
    /// Display name of the source clip or file.
    pub name: String,
    /// All note positions and lengths use this resolution.
    pub ticks_per_quarter: i64,
    /// Source-timeline offset of note tick zero. Tempo and meter maps use source ticks.
    #[serde(default)]
    pub source_origin_ticks: i64,
    /// Tempo changes from the source.
    pub tempo_map: TempoMap,
    /// Meter changes from the source.
    pub signatures: SignatureMap,
    /// Independent instrumental or vocal parts.
    pub tracks: Vec<ReferenceTrack>,
}

/// One part in a conversational score.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ReferenceTrack {
    /// Name displayed in the editor and sent to the model.
    pub name: String,
    /// Zero-based MIDI channel (9 conventionally denotes percussion).
    pub channel: u8,
    /// Exact notes and per-note lyrics; positions are relative to the reference origin.
    pub notes: Vec<Note>,
}

impl MidiReference {
    /// An empty melody ready for manual entry.
    pub fn new(name: String, tempo_map: TempoMap, signatures: SignatureMap) -> Self {
        Self {
            version: 1,
            name: name.clone(),
            ticks_per_quarter: TICKS_PER_QUARTER,
            source_origin_ticks: 0,
            tempo_map,
            signatures,
            tracks: vec![ReferenceTrack {
                name,
                channel: 0,
                notes: vec![],
            }],
        }
    }

    /// Copy a clip's written notes without regenerating or modifying the source.
    pub fn from_clip(
        clip: &MidiClip,
        tempo_map: TempoMap,
        signatures: SignatureMap,
    ) -> Result<Self, String> {
        let mut reference = Self::new(clip.name.clone(), tempo_map, signatures);
        reference.source_origin_ticks = clip.start.0;
        if clip.notes.len() > MAX_REFERENCE_NOTES {
            return Err(format!(
                "Select a shorter clip (at most {MAX_REFERENCE_NOTES} notes)."
            ));
        }
        reference.tracks[0].notes = clip.notes.clone();
        reference.encode()?;
        Ok(reference)
    }

    /// Decode an SMF or an Auris conversational score; neither path imports into the song.
    pub fn decode(path: &Path, bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_REFERENCE_BYTES {
            return Err(too_large());
        }
        let reference = if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("auris-midi"))
        {
            serde_json::from_slice(bytes).map_err(|e| format!("Invalid MIDI reference: {e}"))?
        } else {
            let midi = auris_io::midi::read_midi_bytes(bytes).map_err(|e| e.to_string())?;
            Self {
                version: 1,
                name: path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                ticks_per_quarter: TICKS_PER_QUARTER,
                source_origin_ticks: 0,
                tempo_map: midi.tempo_map,
                signatures: midi.signatures,
                tracks: midi
                    .tracks
                    .into_iter()
                    .filter(|t| !t.notes.is_empty())
                    .map(|t| ReferenceTrack {
                        name: t.name,
                        channel: t.channel,
                        notes: t.notes,
                    })
                    .collect(),
            }
        };
        reference.encode()?;
        Ok(reference)
    }

    /// Read with a hard byte bound, including files that grow during the read.
    pub fn read(path: &Path) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        if !file.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("MIDI reference must be a regular file.".into());
        }
        let mut bytes = Vec::new();
        file.take(MAX_REFERENCE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        Self::decode(path, &bytes)
    }

    /// Validate a score and encode the exact reference sent to the model.
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        if self.version != 1 || self.ticks_per_quarter != TICKS_PER_QUARTER {
            return Err("Unsupported MIDI reference version or tick resolution.".into());
        }
        if self.tracks.is_empty()
            || self.tracks.len() > MAX_REFERENCE_TRACKS
            || self.tracks.iter().map(|t| t.notes.len()).sum::<usize>() > MAX_REFERENCE_NOTES
        {
            return Err(too_many_notes());
        }
        for track in &self.tracks {
            if track.channel > 15
                || track.notes.iter().any(|n| {
                    n.pitch > 127
                        || !n.velocity.is_finite()
                        || !(0.0..=1.0).contains(&n.velocity)
                        || n.start.0 < 0
                        || n.length.0 <= 0
                        || n.start.0.checked_add(n.length.0).is_none()
                        || n.start.0.saturating_add(n.length.0) > TICKS_PER_QUARTER * 1_000_000
                })
            {
                return Err("MIDI reference contains an invalid note or channel.".into());
            }
        }
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_REFERENCE_BYTES {
            return Err(too_large());
        }
        Ok(bytes)
    }

    /// Save a new immutable snapshot. Earlier turns and source files are never overwritten.
    pub fn save_snapshot(&self, directory: &Path) -> Result<PathBuf, String> {
        let bytes = self.encode()?;
        std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
        let label: String = self
            .name
            .chars()
            .filter(|c| !c.is_control() && !"<>:\"/\\|?*".contains(*c))
            .take(32)
            .collect();
        let prefix = format!("melody-{}-", label.trim_matches([' ', '.']));
        let mut file = tempfile::Builder::new()
            .prefix(&prefix)
            .suffix(".auris-midi")
            .tempfile_in(directory)
            .map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        file.keep().map(|(_, path)| path).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::time::Ticks;

    #[test]
    fn standard_midi_preserves_timing_channels_and_lyric_meta_events() {
        let mut bytes = b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk".to_vec();
        let track = b"\0\xff\x05\x02la\0\x92\x40\x64\x83\x60\x82\x40\0\0\xff\x2f\0";
        bytes.extend_from_slice(&(track.len() as u32).to_be_bytes());
        bytes.extend_from_slice(track);
        let score = MidiReference::decode(Path::new("phrase.MID"), &bytes).unwrap();
        let track = &score.tracks[0];
        assert_eq!(track.channel, 2);
        assert_eq!(track.notes[0].pitch, 64);
        assert_eq!(track.notes[0].length, Ticks(960));
        assert_eq!(track.notes[0].lyric, "la");
    }

    #[test]
    fn snapshots_preserve_lyrics_and_do_not_overwrite_previous_turns() {
        let dir = tempfile::tempdir().unwrap();
        let mut score = MidiReference::new(
            "Melody".into(),
            TempoMap::constant(120.0),
            SignatureMap::default(),
        );
        let mut note = Note::new(60, Ticks(240), Ticks(480));
        note.lyric = "きみ".into();
        score.tracks[0].notes.push(note);
        let first = score.save_snapshot(dir.path()).unwrap();
        score.tracks[0].notes[0].pitch = 67;
        let second = score.save_snapshot(dir.path()).unwrap();
        assert_ne!(first, second);
        let restored = MidiReference::read(&first).unwrap();
        assert_eq!(restored.tracks[0].notes[0].pitch, 60);
        assert_eq!(restored.tracks[0].notes[0].lyric, "きみ");
        assert_eq!(restored.tracks[0].notes[0].start, Ticks(240));
    }

    /// A type-1 file of `tracks` tracks, each `notes_per_track` sixteenth notes long.
    fn smf(tracks: usize, notes_per_track: usize) -> Vec<u8> {
        let mut bytes = b"MThd\0\0\0\x06\0\x01".to_vec();
        bytes.extend_from_slice(&(tracks as u16).to_be_bytes());
        bytes.extend_from_slice(&480u16.to_be_bytes());
        for t in 0..tracks {
            let channel = (t % 16) as u8;
            let mut track = Vec::new();
            for n in 0..notes_per_track {
                let pitch = 36 + (n % 48) as u8;
                // Delta 0, note on; delta 120 (one byte), note off.
                track.extend_from_slice(&[0, 0x90 | channel, pitch, 100]);
                track.extend_from_slice(&[120, 0x80 | channel, pitch, 0]);
            }
            track.extend_from_slice(&[0, 0xff, 0x2f, 0]);
            bytes.extend_from_slice(b"MTrk");
            bytes.extend_from_slice(&(track.len() as u32).to_be_bytes());
            bytes.extend_from_slice(&track);
        }
        bytes
    }

    #[test]
    fn a_finished_song_is_accepted_whole() {
        // Issue #88: 24 parts and 30,000 notes, far beyond the old 64-track / 2048-note bound.
        let bytes = smf(24, 1250);
        let score = MidiReference::decode(Path::new("song.mid"), &bytes).unwrap();
        assert_eq!(score.tracks.len(), 24);
        let notes: usize = score.tracks.iter().map(|t| t.notes.len()).sum();
        assert_eq!(notes, 30_000);
        assert!(score.encode().unwrap().len() <= MAX_REFERENCE_BYTES);
    }

    #[test]
    fn only_absurd_note_counts_are_refused() {
        let bytes = smf(1, MAX_REFERENCE_NOTES + 1);
        let error = MidiReference::decode(Path::new("black.mid"), &bytes).unwrap_err();
        assert!(error.contains("at most 100000 notes"), "{error}");
    }

    #[test]
    fn invalid_or_oversized_scores_are_rejected() {
        let mut score = MidiReference::new(
            "Melody".into(),
            TempoMap::constant(120.0),
            SignatureMap::default(),
        );
        score.tracks[0]
            .notes
            .push(Note::new(60, Ticks::ZERO, Ticks(-1)));
        assert!(score.encode().is_err());
        assert!(MidiReference::decode(Path::new("bad.mid"), b"not midi").is_err());
        assert!(
            MidiReference::decode(Path::new("large.mid"), &vec![0; MAX_REFERENCE_BYTES + 1])
                .is_err()
        );
    }
}
