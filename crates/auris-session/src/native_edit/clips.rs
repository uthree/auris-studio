//! Source-addressed native clip commands.

use auris_core::pipeline::content::{ContentError, SourceContent, SourceDocumentId};
use auris_core::pipeline::document::{NativeDocument, NativeTrackKind};
use auris_core::project::{ClipId, MidiClip, StageId};
use auris_core::time::Ticks;

use super::{NativeCreated, NativeEditError};

/// A bounded edit to clips owned by one shared native source.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub enum NativeClipEdit {
    /// Adds an empty MIDI clip to a note source.
    AddMidi {
        /// Shared source identity.
        source: SourceDocumentId,
        /// Clip label.
        name: String,
        /// Timeline start.
        start: Ticks,
        /// Explicit timeline length.
        length: Ticks,
    },
    /// Removes one clip from its source.
    Remove {
        /// Shared source identity.
        source: SourceDocumentId,
        /// Clip identity.
        clip: ClipId,
    },
    /// Duplicates one clip with fresh clip and stage identities.
    Duplicate {
        /// Shared source identity.
        source: SourceDocumentId,
        /// Clip identity.
        clip: ClipId,
    },
    /// Moves one clip without changing its source or identity.
    Move {
        /// Shared source identity.
        source: SourceDocumentId,
        /// Clip identity.
        clip: ClipId,
        /// New timeline start.
        start: Ticks,
    },
    /// Renames every matching clip in a shared source.
    Rename {
        /// Shared source identity.
        source: SourceDocumentId,
        /// Clip identity.
        clip: ClipId,
        /// New label.
        name: String,
    },
    /// Mutes or unmutes every matching clip in a shared source.
    SetMuted {
        /// Shared source identity.
        source: SourceDocumentId,
        /// Clip identity.
        clip: ClipId,
        /// New mute state.
        muted: bool,
    },
    /// Changes a note clip's explicit timeline length.
    Resize {
        /// Shared source identity.
        source: SourceDocumentId,
        /// Clip identity.
        clip: ClipId,
        /// New explicit timeline length.
        length: Ticks,
    },
}

/// Applies one clip edit to a cloned native candidate.
pub(super) fn apply(
    document: &mut NativeDocument,
    edit: &NativeClipEdit,
    limits: auris_core::pipeline::content::ContentLimits,
) -> Result<Vec<NativeCreated>, NativeEditError> {
    match edit {
        NativeClipEdit::AddMidi {
            name,
            start,
            length,
            ..
        } => {
            if name.trim().is_empty()
                || length.raw() <= 0
                || start.raw() < 0
                || start.raw().checked_add(length.raw()).is_none()
            {
                return Err(NativeEditError::Invalid(
                    "MIDI clip bounds or name are invalid".into(),
                ));
            }
            let id = ClipId(super::allocate(document)?);
            let source = source_mut(document, edit.source())?;
            let SourceContent::Notes(clips) = &mut source.content else {
                return Err(NativeEditError::Invalid(
                    "MIDI clips require a note source".into(),
                ));
            };
            clips.push(
                MidiClip::new(id, name, *start, *length)
                    .with_stage_ids(Vec::new())
                    .map_err(|e| NativeEditError::Invalid(e.to_string()))?,
            );
            Ok(vec![NativeCreated::Clip(id)])
        }
        NativeClipEdit::Remove { clip, .. } => {
            remove_clip(document, edit.source(), *clip)?;
            Ok(Vec::new())
        }
        NativeClipEdit::Duplicate { clip, .. } => duplicate(document, edit.source(), *clip, limits),
        NativeClipEdit::Move { clip, start, .. } => {
            if start.raw() < 0 {
                return Err(NativeEditError::Invalid(
                    "clip start cannot be negative".into(),
                ));
            }
            let source = source_mut(document, edit.source())?;
            set_start(&mut source.content, *clip, *start)?;
            Ok(Vec::new())
        }
        NativeClipEdit::Rename { clip, name, .. } => {
            if name.trim().is_empty() || singer_source(document, edit.source()) {
                return Err(NativeEditError::Invalid(
                    "clip rename is not valid for this source".into(),
                ));
            }
            let source = source_mut(document, edit.source())?;
            rename_clip(&mut source.content, *clip, name)
        }
        NativeClipEdit::SetMuted { clip, muted, .. } => {
            let source = source_mut(document, edit.source())?;
            set_muted(&mut source.content, *clip, *muted)?;
            Ok(Vec::new())
        }
        NativeClipEdit::Resize { clip, length, .. } => {
            if length.raw() <= 0 {
                return Err(NativeEditError::Invalid(
                    "clip length must be positive".into(),
                ));
            }
            let source = source_mut(document, edit.source())?;
            let clip = find_clip_mut(&mut source.content, *clip)?;
            if clip.start.raw().checked_add(length.raw()).is_none() {
                return Err(NativeEditError::Invalid(
                    "clip end exceeds the timeline".into(),
                ));
            }
            clip.length = *length;
            clip.length_is_explicit = true;
            Ok(Vec::new())
        }
    }
}

impl NativeClipEdit {
    fn source(&self) -> SourceDocumentId {
        match self {
            Self::AddMidi { source, .. }
            | Self::Remove { source, .. }
            | Self::Duplicate { source, .. }
            | Self::Move { source, .. }
            | Self::Rename { source, .. }
            | Self::SetMuted { source, .. }
            | Self::Resize { source, .. } => *source,
        }
    }
}

fn source_mut(
    document: &mut NativeDocument,
    id: SourceDocumentId,
) -> Result<&mut auris_core::pipeline::content::SourceDocument, NativeEditError> {
    document
        .content
        .sources
        .iter_mut()
        .find(|source| source.id == id)
        .ok_or({
            ContentError::Missing(auris_core::pipeline::content::ContentRef::Source(id)).into()
        })
}

fn find_clip_mut(
    content: &mut SourceContent,
    id: ClipId,
) -> Result<&mut auris_core::project::StableMidiClip, NativeEditError> {
    match content {
        SourceContent::Notes(clips) => clips.iter_mut().find(|clip| clip.id == id).ok_or({
            NativeEditError::Content(ContentError::Missing(
                auris_core::pipeline::content::ContentRef::Clip(id),
            ))
        }),
        SourceContent::Audio(_) => Err(NativeEditError::Invalid(
            "operation requires a MIDI clip".into(),
        )),
    }
}

fn set_start(content: &mut SourceContent, id: ClipId, start: Ticks) -> Result<(), NativeEditError> {
    if let SourceContent::Notes(clips) = content
        && let Some(clip) = clips.iter().find(|clip| clip.id == id)
        && start.raw().checked_add(clip.length.raw()).is_none()
    {
        return Err(NativeEditError::Invalid(
            "clip end exceeds the timeline".into(),
        ));
    }
    match content {
        SourceContent::Notes(clips) => clips
            .iter_mut()
            .find(|clip| clip.id == id)
            .map(|clip| clip.start = start),
        SourceContent::Audio(clips) => clips
            .iter_mut()
            .find(|clip| clip.id == id)
            .map(|clip| clip.start = start),
    }
    .ok_or(ContentError::Missing(auris_core::pipeline::content::ContentRef::Clip(id)).into())
}

fn set_muted(content: &mut SourceContent, id: ClipId, muted: bool) -> Result<(), NativeEditError> {
    match content {
        SourceContent::Notes(clips) => clips
            .iter_mut()
            .find(|clip| clip.id == id)
            .map(|clip| clip.muted = muted),
        SourceContent::Audio(clips) => clips
            .iter_mut()
            .find(|clip| clip.id == id)
            .map(|clip| clip.muted = muted),
    }
    .ok_or(ContentError::Missing(auris_core::pipeline::content::ContentRef::Clip(id)).into())
}

fn remove_clip(
    document: &mut NativeDocument,
    source_id: SourceDocumentId,
    id: ClipId,
) -> Result<(), NativeEditError> {
    let content = &mut source_mut(document, source_id)?.content;
    let found = match content {
        SourceContent::Notes(clips) => clips.iter().position(|clip| clip.id == id).map(|index| {
            clips.remove(index);
        }),
        SourceContent::Audio(clips) => clips.iter().position(|clip| clip.id == id).map(|index| {
            clips.remove(index);
        }),
    };
    if found.is_none() {
        return Err(
            ContentError::Missing(auris_core::pipeline::content::ContentRef::Clip(id)).into(),
        );
    }
    document.context.transcription_provenance.remove(&id);
    Ok(())
}

fn rename_clip(
    content: &mut SourceContent,
    id: ClipId,
    name: &str,
) -> Result<Vec<NativeCreated>, NativeEditError> {
    match content {
        SourceContent::Notes(clips) => clips
            .iter_mut()
            .find(|clip| clip.id == id)
            .map(|clip| clip.name = name.into()),
        SourceContent::Audio(clips) => clips
            .iter_mut()
            .find(|clip| clip.id == id)
            .map(|clip| clip.name = name.into()),
    }
    .ok_or(ContentError::Missing(
        auris_core::pipeline::content::ContentRef::Clip(id),
    ))?;
    Ok(Vec::new())
}

fn duplicate(
    document: &mut NativeDocument,
    source_id: SourceDocumentId,
    id: ClipId,
    limits: auris_core::pipeline::content::ContentLimits,
) -> Result<Vec<NativeCreated>, NativeEditError> {
    let current_clips = document
        .content
        .sources
        .iter()
        .map(|source| match &source.content {
            SourceContent::Notes(clips) => clips.len(),
            SourceContent::Audio(clips) => clips.len(),
        })
        .sum::<usize>();
    if current_clips >= limits.clips {
        return Err(ContentError::Limit("clips").into());
    }
    let original = {
        let source = source_mut(document, source_id)?;
        source.content.clone()
    };
    if let SourceContent::Notes(clips) = &original {
        let stages = clips
            .iter()
            .find(|clip| clip.id == id)
            .map_or(0, |clip| clip.transforms.len());
        let current_stages = document
            .content
            .sources
            .iter()
            .map(|source| match &source.content {
                SourceContent::Notes(clips) => clips
                    .iter()
                    .map(|clip| clip.transforms.len())
                    .sum::<usize>(),
                SourceContent::Audio(_) => 0,
            })
            .sum::<usize>();
        if current_stages.saturating_add(stages) > limits.stages {
            return Err(ContentError::Limit("stages").into());
        }
    }
    let stage_count = match &original {
        SourceContent::Notes(clips) => clips
            .iter()
            .find(|clip| clip.id == id)
            .map_or(0, |clip| clip.transforms.len()),
        SourceContent::Audio(_) => 0,
    };
    let new_id = ClipId(super::allocate(document)?);
    let mut stage_ids = Vec::with_capacity(stage_count);
    let mut created = vec![NativeCreated::Clip(new_id)];
    for _ in 0..stage_count {
        let stage = StageId(super::allocate(document)?);
        stage_ids.push(stage);
        created.push(NativeCreated::Stage(stage));
    }
    let source = source_mut(document, source_id)?;
    match (&mut source.content, original) {
        (SourceContent::Notes(clips), SourceContent::Notes(mut originals)) => {
            let mut copy = originals
                .drain(..)
                .find(|clip| clip.id == id)
                .ok_or({
                    ContentError::Missing(auris_core::pipeline::content::ContentRef::Clip(id))
                })?
                .map_transforms(|stage| stage.transform);
            copy.id = new_id;
            copy.name = format!("{} copy", copy.name);
            clips.push(
                copy.with_stage_ids(stage_ids)
                    .map_err(|e| NativeEditError::Invalid(e.to_string()))?,
            );
        }
        (SourceContent::Audio(clips), SourceContent::Audio(mut originals)) => {
            let mut copy = originals.drain(..).find(|clip| clip.id == id).ok_or({
                ContentError::Missing(auris_core::pipeline::content::ContentRef::Clip(id))
            })?;
            copy.id = new_id;
            copy.name = format!("{} copy", copy.name);
            clips.push(copy);
        }
        _ => {
            return Err(NativeEditError::Invalid(
                "source content changed during duplicate".into(),
            ));
        }
    }
    if let Some(provenance) = document.context.transcription_provenance.get(&id).cloned() {
        document
            .context
            .transcription_provenance
            .insert(new_id, provenance);
    }
    Ok(created)
}

fn singer_source(document: &NativeDocument, source: SourceDocumentId) -> bool {
    document.content.tracks.iter().any(|binding| {
        binding.source == Some(source)
            && document
                .track_presentation
                .iter()
                .any(|track| track.id == binding.track && track.kind == NativeTrackKind::Singer)
    })
}
