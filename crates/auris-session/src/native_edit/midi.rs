//! Atomic append-only MIDI import for the native document.

use auris_core::Ticks;
use auris_core::pipeline::content::{ContentError, ContentLimits, SourceContent};
use auris_core::pipeline::document::NativeDocument;
use auris_core::project::Note;

use crate::midi_import::PreparedMidiTrack;

use super::{NativeClipEdit, NativeCreated, NativeEditError, NativeTrackEdit, NativeTrackKindSpec};

const MAX_IMPORT_TRACKS: usize = 256;
const MAX_IMPORT_NOTES: usize = 200_000;

/// Applies a prepared MIDI import to a cloned native document.
pub(super) fn apply(
    document: &mut NativeDocument,
    tracks: &[PreparedMidiTrack],
    at: Ticks,
    limits: ContentLimits,
) -> Result<Vec<NativeCreated>, NativeEditError> {
    preflight(tracks, at, document, limits)?;
    let mut created = Vec::new();
    for prepared in tracks {
        let kind = if prepared.drum {
            NativeTrackKindSpec::Drum(prepared.instrument.clone())
        } else {
            NativeTrackKindSpec::Instrument(prepared.instrument.clone())
        };
        let track_created = super::tracks::apply(
            document,
            &NativeTrackEdit::Add {
                name: prepared.name.clone(),
                kind,
            },
            limits,
        )?;
        let source = track_created
            .iter()
            .find_map(|entry| match entry {
                NativeCreated::Source(source) => Some(*source),
                _ => None,
            })
            .ok_or_else(|| NativeEditError::Invalid("MIDI track has no note source".into()))?;
        created.extend(track_created);
        let start = at
            .raw()
            .checked_add(prepared.start.raw())
            .map(Ticks)
            .ok_or_else(|| NativeEditError::Invalid("MIDI timeline overflow".into()))?;
        let clip_created = super::clips::apply(
            document,
            &NativeClipEdit::AddMidi {
                source,
                name: prepared.name.clone(),
                start,
                length: prepared.length,
            },
            limits,
        )?;
        let clip = clip_created
            .iter()
            .find_map(|entry| match entry {
                NativeCreated::Clip(clip) => Some(*clip),
                _ => None,
            })
            .ok_or_else(|| NativeEditError::Invalid("MIDI track did not create a clip".into()))?;
        let source = document
            .content
            .sources
            .iter_mut()
            .find(|candidate| candidate.id == source)
            .ok_or(ContentError::Missing(
                auris_core::pipeline::content::ContentRef::Source(source),
            ))?;
        let SourceContent::Notes(clips) = &mut source.content else {
            return Err(NativeEditError::Invalid(
                "MIDI source is not a note source".into(),
            ));
        };
        let target = clips
            .iter_mut()
            .find(|candidate| candidate.id == clip)
            .ok_or(ContentError::Missing(
                auris_core::pipeline::content::ContentRef::Clip(clip),
            ))?;
        target.notes = prepared.notes.clone();
        target.bend = prepared.bend.clone();
        target.controllers = prepared.controllers.clone();
        target.length_is_explicit = true;
        created.extend(clip_created);
    }
    Ok(created)
}

fn preflight(
    tracks: &[PreparedMidiTrack],
    at: Ticks,
    document: &NativeDocument,
    limits: ContentLimits,
) -> Result<(), NativeEditError> {
    if at.raw() < 0 {
        return Err(NativeEditError::Invalid(
            "MIDI import offset is negative".into(),
        ));
    }
    if tracks.is_empty() || tracks.len() > MAX_IMPORT_TRACKS {
        return Err(NativeEditError::Invalid(
            "MIDI import requires 1..256 non-empty tracks".into(),
        ));
    }
    if document.content.tracks.len().saturating_add(tracks.len()) > limits.tracks
        || document.content.sources.len().saturating_add(tracks.len()) > limits.sources
    {
        return Err(ContentError::Limit("tracks or sources").into());
    }
    let mut note_count = 0usize;
    for track in tracks {
        if track.name.trim().is_empty() || track.notes.is_empty() {
            return Err(NativeEditError::Invalid(
                "MIDI import tracks must have a name and notes".into(),
            ));
        }
        super::plugins::validate_plugin(&track.instrument)?;
        if track.start.raw() < 0
            || track.length.raw() <= 0
            || track.start.raw().checked_add(track.length.raw()).is_none()
            || at.raw().checked_add(track.start.raw()).is_none()
            || at
                .raw()
                .checked_add(track.start.raw())
                .and_then(|start| start.checked_add(track.length.raw()))
                .is_none()
        {
            return Err(NativeEditError::Invalid(
                "MIDI clip bounds are invalid".into(),
            ));
        }
        note_count = note_count.saturating_add(track.notes.len());
        for note in &track.notes {
            validate_note(note, track.length)?;
        }
        // Notes also carry optional vocal fields. JSON would silently turn their NaN/Inf
        // into null; reject any payload that cannot be saved without changing its values.
        let encoded = serde_json::to_vec(&track.notes)
            .map_err(|error| NativeEditError::Invalid(error.to_string()))?;
        let decoded: Vec<Note> = serde_json::from_slice(&encoded)
            .map_err(|error| NativeEditError::Invalid(error.to_string()))?;
        if decoded != track.notes {
            return Err(NativeEditError::Invalid(
                "MIDI notes cannot be stored without losing values".into(),
            ));
        }
        // An imported RPN bend range can exceed the editor's drawing range. Preserve the
        // decoded semitones, including multiple channel events at the same tick.
        validate_curve(&track.bend, track.length, None)?;
        for (controller, curve) in &track.controllers {
            if *controller > 127 {
                return Err(NativeEditError::Invalid(
                    "MIDI controller is out of range".into(),
                ));
            }
            validate_curve(curve, track.length, Some((0.0, 1.0)))?;
        }
    }
    if !(1..=MAX_IMPORT_NOTES).contains(&note_count) {
        return Err(NativeEditError::Invalid(
            "MIDI import requires 1..200000 notes".into(),
        ));
    }
    let existing_clips = document
        .content
        .sources
        .iter()
        .map(|source| match &source.content {
            SourceContent::Notes(clips) => clips.len(),
            SourceContent::Audio(clips) => clips.len(),
        })
        .sum::<usize>();
    if existing_clips.saturating_add(tracks.len()) > limits.clips {
        return Err(ContentError::Limit("clips").into());
    }
    Ok(())
}

fn validate_note(note: &Note, length: Ticks) -> Result<(), NativeEditError> {
    if note.pitch > 127
        || !note.velocity.is_finite()
        || !(0.0..=1.0).contains(&note.velocity)
        || note.start.raw() < 0
        || note.length.raw() <= 0
        || note.start.raw().checked_add(note.length.raw()).is_none()
        || note.start.raw() + note.length.raw() > length.raw()
    {
        return Err(NativeEditError::Invalid(
            "MIDI note is out of bounds".into(),
        ));
    }
    Ok(())
}

fn validate_curve(
    curve: &[auris_core::project::CurvePoint],
    length: Ticks,
    range: Option<(f32, f32)>,
) -> Result<(), NativeEditError> {
    let mut previous = None;
    for point in curve {
        if point.at.raw() < 0
            || point.at.raw() > length.raw()
            || !point.value.is_finite()
            || range.is_some_and(|(min, max)| point.value < min || point.value > max)
            || previous.is_some_and(|tick| point.at.raw() < tick)
        {
            return Err(NativeEditError::Invalid(
                "MIDI performance curve is invalid".into(),
            ));
        }
        previous = Some(point.at.raw());
    }
    Ok(())
}
