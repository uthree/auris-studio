//! Shared MIDI material preparation for conventional and native document commands.
//!
//! Parsing selects instrument roles and rebases channel performance into clip-local time.
//! It does not allocate document identities or mutate a session. Both document authorities
//! consume this same material, so pedal tails and state preceding the first note agree.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use auris_core::pipeline::content::PluginInstance;
use auris_core::project::{CurvePoint, MidiClip};
use auris_core::time::{SignatureMap, TempoMap, Ticks};
use auris_core::{ClipId, Note, Project};

use crate::SessionError;
use crate::session::MidiReport;

/// One MIDI channel's material, with no destination document identities.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct PreparedMidiTrack {
    /// Track and clip label.
    pub name: String,
    /// Instrument selected by the caller's registry policy.
    pub instrument: PluginInstance,
    /// Whether this channel is General MIDI percussion (channel 10).
    pub drum: bool,
    /// Absolute start of the first note in the imported score.
    pub start: Ticks,
    /// Explicit clip duration, including performance events after the last note release.
    pub length: Ticks,
    /// Authored notes in clip-local time.
    pub notes: Vec<Note>,
    /// Pitch bend in clip-local time, including the state at the first note.
    pub bend: Vec<CurvePoint>,
    /// MIDI performance controllers in clip-local time.
    pub controllers: BTreeMap<u8, Vec<CurvePoint>>,
}

impl PreparedMidiTrack {
    fn into_clip(self, id: ClipId) -> MidiClip {
        let mut clip = MidiClip::new(id, self.name, self.start, self.length);
        clip.notes = self.notes;
        clip.bend = self.bend;
        clip.controllers = self.controllers;
        clip.length_is_explicit = true;
        clip
    }
}

/// Parsed score material shared by replacement and append import workflows.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedMidiImport {
    /// Document name derived from the selected file.
    pub name: String,
    /// Imported tempo map; append commands retain the destination's existing map.
    pub tempo_map: TempoMap,
    /// Imported meter map; append commands retain the destination's existing map.
    pub signatures: SignatureMap,
    /// Nonempty note channels, in their file order.
    pub tracks: Vec<PreparedMidiTrack>,
}

impl PreparedMidiImport {
    /// Reads a bounded Standard MIDI File and prepares material without creating a document.
    ///
    /// Instrument identifiers come from the caller's registry policy. Cancellation is checked
    /// before and after parsing and between channels. No plugin is loaded by this operation.
    pub fn read(
        path: &Path,
        fallback: &str,
        drums: &str,
        cancelled: &AtomicBool,
    ) -> Result<Option<Self>, SessionError> {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let imported = auris_io::read_midi_file(path)?;
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());
        let mut tracks = Vec::with_capacity(imported.tracks.len());
        for track in imported.tracks {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let Some(first) = track.notes.iter().map(|note| note.start).min() else {
                continue;
            };
            let last = track.end();
            let drum = track.channel == 9;
            let name = if track.name.trim().is_empty() {
                format!("Channel {}", track.channel + 1)
            } else {
                track.name
            };
            tracks.push(PreparedMidiTrack {
                name,
                instrument: PluginInstance {
                    plugin_id: if drum { drums } else { fallback }.to_owned(),
                    state: Default::default(),
                    file: None,
                },
                drum,
                start: first,
                length: Ticks((last - first).raw().max(1)),
                notes: track
                    .notes
                    .into_iter()
                    .map(|note| Note {
                        start: note.start - first,
                        ..note
                    })
                    .collect(),
                bend: rebase_curve(&track.bend, first, last),
                controllers: track
                    .controllers
                    .into_iter()
                    .filter_map(|(number, points)| {
                        let points = rebase_curve(&points, first, last);
                        (!points.is_empty()).then_some((number, points))
                    })
                    .collect(),
            });
        }
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        Ok(Some(Self {
            name,
            tempo_map: imported.tempo_map,
            signatures: imported.signatures,
            tracks,
        }))
    }

    /// Reports the imported material before any destination timeline offset is applied.
    pub fn report(&self) -> MidiReport {
        MidiReport {
            tracks: self.tracks.len(),
            notes: self.tracks.iter().map(|track| track.notes.len()).sum(),
            length: self
                .tracks
                .iter()
                .map(|track| track.start + track.length)
                .max()
                .unwrap_or(Ticks::ZERO),
        }
    }

    pub(crate) fn into_project(
        self,
        sample_rate: f64,
        cancelled: &AtomicBool,
    ) -> Result<Option<(Project, MidiReport)>, SessionError> {
        let report = self.report();
        let mut project = Project::new(self.name, sample_rate);
        project.tempo_map = self.tempo_map;
        project.signatures = self.signatures;
        for material in self.tracks {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let track = if material.drum {
                project.add_drum_track(&material.name, &material.instrument.plugin_id)
            } else {
                project.add_instrument_track(&material.name, &material.instrument.plugin_id)
            };
            let id = project
                .add_midi_clip(track, &material.name, material.start, material.length)
                .ok_or(SessionError::UnknownTrack(track.0))?;
            let clip = project
                .midi_clip_mut(id)
                .ok_or(SessionError::UnknownClip(id.0))?;
            *clip = material.into_clip(id);
        }
        project.validate_loop_expansion()?;
        Ok(Some((project, report)))
    }
}

pub(crate) fn rebase_curve(points: &[CurvePoint], first: Ticks, last: Ticks) -> Vec<CurvePoint> {
    let mut rebased: Vec<_> = points
        .iter()
        .filter(|point| point.at >= first && point.at <= last)
        .map(|point| CurvePoint {
            at: point.at - first,
            ..*point
        })
        .collect();
    // Curves are channel state. Carry the latest value before the first note into the clip when
    // no point exists exactly at that note.
    if !rebased.iter().any(|point| point.at == Ticks::ZERO)
        && let Some(point) = points
            .iter()
            .filter(|point| point.at < first)
            .max_by_key(|point| point.at)
    {
        rebased.insert(
            0,
            CurvePoint {
                at: Ticks::ZERO,
                value: point.value,
            },
        );
    }
    rebased
}

#[cfg(test)]
mod tests;
