//! Direct native placement of an imported or recorded audio source.

use std::collections::BTreeMap;

use auris_core::pipeline::content::{ContentError, ContentLimits, SourceContent};
use auris_core::pipeline::document::{NativeDocument, NativeTrackKind};
use auris_core::project::{AudioClip, AudioSource, ClipId, FadeCurve, SourceId, TrackId};
use auris_core::time::Ticks;

use super::{NativeCreated, NativeEditError};

/// Audio metadata and placement for one native recording/import edit.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct NativeAudioEdit {
    /// Audio track receiving the placement.
    pub track: TrackId,
    /// Display name for the source and clip.
    pub name: String,
    /// Stored asset location; disk I/O is performed by the caller before this edit.
    pub path: auris_core::AssetPath,
    /// Decoded frame count.
    pub frame_count: u64,
    /// Decoded sample rate.
    pub sample_rate: f64,
    /// Decoded channel count.
    pub channel_count: usize,
    /// Stored file size, or zero when unavailable.
    pub byte_size: u64,
    /// Provenance metadata copied into the project registry.
    #[serde(default)]
    pub provenance: BTreeMap<String, String>,
    /// Timeline placement.
    pub start: Ticks,
    /// First source frame to play.
    pub offset_frames: u64,
    /// Number of source frames to play.
    pub length_frames: u64,
    /// Optional clip gain in dB.
    pub gain_db: f32,
    /// Optional source tempo used for tempo-following playback.
    pub source_bpm: Option<f64>,
    /// Whether the clip follows tempo.
    pub follows_tempo: bool,
}

/// Applies one audio landing operation to a native document candidate.
pub(super) fn apply(
    document: &mut NativeDocument,
    edit: &NativeAudioEdit,
    limits: ContentLimits,
) -> Result<Vec<NativeCreated>, NativeEditError> {
    if edit.name.trim().is_empty() || edit.frame_count == 0 || edit.channel_count == 0 {
        return Err(NativeEditError::Invalid(
            "audio source metadata is empty".into(),
        ));
    }
    if !edit.sample_rate.is_finite() || edit.sample_rate <= 0.0 {
        return Err(NativeEditError::Invalid(
            "audio source sample rate must be finite and positive".into(),
        ));
    }
    if !edit.gain_db.is_finite()
        || edit.offset_frames > edit.frame_count
        || edit.length_frames == 0
        || edit.length_frames > edit.frame_count.saturating_sub(edit.offset_frames)
    {
        return Err(NativeEditError::Invalid(
            "audio clip source bounds are invalid".into(),
        ));
    }
    if let Some(bpm) = edit.source_bpm
        && (!bpm.is_finite() || bpm <= 0.0)
    {
        return Err(NativeEditError::Invalid(
            "audio source tempo must be finite and positive".into(),
        ));
    }
    let track = document
        .track_presentation
        .iter()
        .find(|track| track.id == edit.track)
        .ok_or({
            NativeEditError::Content(ContentError::Missing(
                auris_core::pipeline::content::ContentRef::Track(edit.track),
            ))
        })?;
    if track.kind != NativeTrackKind::Audio {
        return Err(NativeEditError::Invalid(
            "audio landing requires an audio track".into(),
        ));
    }
    let binding = document
        .content
        .tracks
        .iter()
        .find(|binding| binding.track == edit.track)
        .ok_or({
            NativeEditError::Content(ContentError::Missing(
                auris_core::pipeline::content::ContentRef::Track(edit.track),
            ))
        })?;
    let source_document = binding
        .source
        .ok_or_else(|| NativeEditError::Invalid("audio track has no source binding".into()))?;
    let source = document
        .content
        .sources
        .iter()
        .find(|source| source.id == source_document)
        .ok_or({
            NativeEditError::Content(ContentError::Missing(
                auris_core::pipeline::content::ContentRef::Source(source_document),
            ))
        })?;
    if !matches!(source.content, SourceContent::Audio(_)) {
        return Err(NativeEditError::Invalid(
            "audio track source is not an audio source".into(),
        ));
    }
    if document
        .context
        .audio_sources
        .len()
        .saturating_add(document.context.soundfonts.len())
        >= limits.external_ids
    {
        return Err(ContentError::Limit("audio_sources").into());
    }
    let clip_count: usize = document
        .content
        .sources
        .iter()
        .map(|source| match &source.content {
            SourceContent::Audio(clips) => clips.len(),
            SourceContent::Notes(clips) => clips.len(),
        })
        .sum();
    if clip_count >= limits.clips {
        return Err(ContentError::Limit("clips").into());
    }
    let source_id = SourceId(super::allocate(document)?);
    let clip_id = ClipId(super::allocate(document)?);
    document.context.audio_sources.insert(
        source_id,
        AudioSource {
            id: source_id,
            name: edit.name.clone(),
            path: edit.path.clone(),
            frame_count: edit.frame_count,
            sample_rate: edit.sample_rate,
            channel_count: edit.channel_count,
            byte_size: edit.byte_size,
            provenance: edit.provenance.clone(),
        },
    );
    let clip = AudioClip {
        id: clip_id,
        name: edit.name.clone(),
        start: edit.start.max_zero(),
        source: source_id,
        offset_frames: edit.offset_frames,
        length_frames: edit.length_frames,
        gain_db: edit.gain_db,
        fade_in_frames: 0,
        fade_out_frames: 0,
        fade_in_curve: FadeCurve::Linear,
        fade_out_curve: FadeCurve::Linear,
        muted: false,
        source_bpm: edit.source_bpm,
        follows_tempo: edit.follows_tempo,
        tempo_anchor: None,
        loop_end: Ticks::ZERO,
    };
    let Some(source) = document
        .content
        .sources
        .iter_mut()
        .find(|source| source.id == source_document)
    else {
        return Err(NativeEditError::Content(ContentError::Missing(
            auris_core::pipeline::content::ContentRef::Source(source_document),
        )));
    };
    let SourceContent::Audio(clips) = &mut source.content else {
        return Err(NativeEditError::Invalid(
            "audio track source is not an audio source".into(),
        ));
    };
    clips.push(clip);
    Ok(vec![
        NativeCreated::AudioSource(source_id),
        NativeCreated::Clip(clip_id),
    ])
}
