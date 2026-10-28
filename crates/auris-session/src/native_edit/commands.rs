//! Small, identity based edits for the native document controller.
//!
//! These operations deliberately do not record history or build an audio graph.  The controller
//! applies one operation to a cloned document, validates the result, and commits that candidate
//! as one history step.

use super::{NativeCreated, NativeEditError};
use auris_core::pipeline::NodeId;
use auris_core::pipeline::content::{ContentError, ProcessorKind, SourceContent};
use auris_core::pipeline::document::NativeDocument;
use auris_core::project::{ClipId, Note, NoteTransform, StageId, TrackId};

/// A bounded edit addressed by persistent native identities.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub enum NativeEdit {
    /// Changes plugin instances or their stored state without loading a host.
    Plugin(super::NativePluginEdit),
    /// Edits shared project-time automation and its stable parameter bindings.
    Automation(super::NativeAutomationEdit),
    /// Changes native main routes and auxiliary sends.
    Routing(super::NativeRoutingEdit),
    /// Edits clips in one shared authored source.
    Clip(super::NativeClipEdit),
    /// Creates, removes or reorders a track and its native ownership.
    Track(super::NativeTrackEdit),
    /// Places a recorded/imported asset directly into a canonical audio source.
    Audio(super::NativeAudioEdit),
    /// Inserts, removes, moves or sidechains a conventional effect.
    Effect(super::NativeEffectEdit),
    /// Changes persisted timeline/transport settings.
    Timeline(super::NativeTimelineEdit),
    /// Changes a track's display name.
    RenameTrack {
        /// Track identity.
        track: TrackId,
        /// New display name.
        name: String,
    },
    /// Changes a track's display colour.
    SetTrackColor {
        /// Track identity.
        track: TrackId,
        /// New display colour.
        color: auris_core::Color,
    },
    /// Changes a track lane height.
    SetTrackHeight {
        /// Track identity.
        track: TrackId,
        /// New lane height in pixels.
        height: f32,
    },
    /// Changes one console strip field.
    SetStrip {
        /// Strip processor identity.
        node: NodeId,
        /// Optional gain in dB, bounded to the mixer descriptor range.
        gain_db: Option<f32>,
        /// Optional stereo position in -1..=1.
        pan: Option<f32>,
        /// Optional mute state.
        mute: Option<bool>,
        /// Optional solo state.
        solo: Option<bool>,
    },
    /// Enables or bypasses an effect while retaining its slot identity.
    SetEffectEnabled {
        /// Effect processor identity.
        node: NodeId,
        /// Whether the effect processes audio.
        enabled: bool,
    },
    /// Moves one non-destructive stage by a signed number of positions.
    MoveStage {
        /// Clip identity.
        clip: ClipId,
        /// Stable stage identity.
        stage: StageId,
        /// Signed stack offset.
        offset: isize,
    },
    /// Removes one non-destructive stage.
    RemoveStage {
        /// Clip identity.
        clip: ClipId,
        /// Stable stage identity.
        stage: StageId,
    },
    /// Copies one shared source with fresh clip and stage identities.
    DuplicateSource {
        /// Source identity.
        source: auris_core::pipeline::content::SourceDocumentId,
    },
    /// Copies one processing branch, retaining its shared source nodes.
    DuplicateBranch {
        /// Processing group identity.
        group: auris_core::pipeline::content::GroupId,
    },
    /// Replaces the authored notes while retaining the clip's stages and metadata.
    ReplaceClipNotes {
        /// Clip identity.
        clip: ClipId,
        /// New authored note list.
        notes: Vec<Note>,
    },
    /// Appends a new non-destructive stage and consumes one native ID.
    AppendStage {
        /// Clip identity.
        clip: ClipId,
        /// New transform payload.
        transform: NoteTransform,
    },
}

/// Applies one identity based edit to a native candidate.
pub(super) fn apply(
    document: &mut NativeDocument,
    edit: &NativeEdit,
    limits: auris_core::pipeline::content::ContentLimits,
) -> Result<Vec<NativeCreated>, NativeEditError> {
    match edit {
        NativeEdit::Plugin(edit) => return super::plugins::apply(document, edit, limits),
        NativeEdit::Automation(edit) => return super::automation::apply(document, edit, limits),
        NativeEdit::Routing(edit) => return super::routing::apply(document, edit, limits),
        NativeEdit::Clip(edit) => return super::clips::apply(document, edit, limits),
        NativeEdit::Track(edit) => return super::tracks::apply(document, edit, limits),
        NativeEdit::Audio(edit) => return super::audio::apply(document, edit, limits),
        NativeEdit::Effect(edit) => return super::effects::apply(document, edit, limits),
        NativeEdit::Timeline(edit) => super::timeline::apply(document, edit)?,
        NativeEdit::RenameTrack { track, name } => {
            presentation(document, *track)?.name = name.clone()
        }
        NativeEdit::SetTrackColor { track, color } => {
            presentation(document, *track)?.color = *color
        }
        NativeEdit::SetTrackHeight { track, height } => {
            if !height.is_finite() || *height <= 0.0 {
                return Err(NativeEditError::Invalid(
                    "track height must be finite and positive".into(),
                ));
            }
            presentation(document, *track)?.height = *height;
        }
        NativeEdit::SetStrip {
            node,
            gain_db,
            pan,
            mute,
            solo,
        } => {
            let processor = processor_mut(document, *node)?;
            let ProcessorKind::Strip(strip) = &mut processor.kind else {
                return Err(NativeEditError::Invalid(
                    "target processor is not a strip".into(),
                ));
            };
            if let Some(value) = gain_db {
                validate_mixer_value(auris_core::param::MixerControl::Gain, *value)?;
                strip.gain_db = *value;
            }
            if let Some(value) = pan {
                validate_mixer_value(auris_core::param::MixerControl::Pan, *value)?;
                strip.pan = *value;
            }
            if let Some(value) = mute {
                strip.mute = *value;
            }
            if let Some(value) = solo {
                strip.solo = *value;
            }
        }
        NativeEdit::SetEffectEnabled { node, enabled } => {
            let processor = processor_mut(document, *node)?;
            let ProcessorKind::Effect {
                enabled: current, ..
            } = &mut processor.kind
            else {
                return Err(NativeEditError::Invalid(
                    "target processor is not an effect".into(),
                ));
            };
            *current = *enabled;
        }
        NativeEdit::MoveStage {
            clip,
            stage,
            offset,
        } => stage_edit(document, *clip, |clip| clip.move_stage(*stage, *offset))?,
        NativeEdit::RemoveStage { clip, stage } => stage_edit(document, *clip, |clip| {
            clip.remove_stage(*stage).map(|_| true)
        })?,
        NativeEdit::ReplaceClipNotes { clip, notes } => {
            let mut found = false;
            for source in &mut document.content.sources {
                let SourceContent::Notes(clips) = &mut source.content else {
                    continue;
                };
                if let Some(entry) = clips.iter_mut().find(|entry| entry.id == *clip) {
                    let replacement = crate::session::notes::replacement_clip(
                        &entry.performer_clip(),
                        notes.clone(),
                    )
                    .map_err(|error| NativeEditError::Invalid(error.to_string()))?;
                    entry.notes = replacement.notes;
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(NativeEditError::Content(ContentError::Missing(
                    auris_core::pipeline::content::ContentRef::Clip(*clip),
                )));
            }
        }
        NativeEdit::AppendStage { clip, transform } => {
            let id = StageId(document.content.next_id);
            if id.0 == 0 || document.content.next_id == u64::MAX {
                return Err(ContentError::IdExhausted.into());
            }
            // JSON maps non-finite floats to null, including optional fields. Verify the new
            // transform can be stored losslessly before accepting it into authored state.
            let encoded = serde_json::to_vec(transform)
                .map_err(|error| NativeEditError::Invalid(error.to_string()))?;
            let decoded: NoteTransform = serde_json::from_slice(&encoded)
                .map_err(|error| NativeEditError::Invalid(error.to_string()))?;
            if decoded != *transform {
                return Err(NativeEditError::Invalid(
                    "transform cannot round-trip without losing values".into(),
                ));
            }
            stage_edit(document, *clip, |clip| {
                clip.append_stage(id, transform.clone()).map(|_| true)
            })?;
            document.content.next_id += 1;
            return Ok(vec![NativeCreated::Stage(id)]);
        }
        NativeEdit::DuplicateSource { source } => {
            let external = document.external_ids();
            let copied = document
                .content
                .duplicate_source(*source, &external, limits)?;
            return Ok(std::iter::once(NativeCreated::Source(copied.source))
                .chain(copied.clips.into_values().map(NativeCreated::Clip))
                .chain(copied.stages.into_values().map(NativeCreated::Stage))
                .collect());
        }
        NativeEdit::DuplicateBranch { group } => {
            let external = document.external_ids();
            let copied = document
                .content
                .duplicate_branch(*group, &external, limits)?;
            return Ok(std::iter::once(NativeCreated::Group(copied.group))
                .chain(copied.nodes.into_values().map(NativeCreated::Node))
                .chain(copied.effects.into_values().map(NativeCreated::Effect))
                .chain(copied.sends.into_values().map(NativeCreated::Send))
                .chain(copied.edges.into_values().map(NativeCreated::Edge))
                .chain(copied.bindings.into_values().map(NativeCreated::Edge))
                .collect());
        }
    }
    Ok(Vec::new())
}

fn presentation(
    document: &mut NativeDocument,
    track: TrackId,
) -> Result<&mut auris_core::pipeline::document::TrackPresentation, NativeEditError> {
    document
        .track_presentation
        .iter_mut()
        .find(|entry| entry.id == track)
        .ok_or(NativeEditError::Content(ContentError::Missing(
            auris_core::pipeline::content::ContentRef::Track(track),
        )))
}

fn validate_mixer_value(
    control: auris_core::param::MixerControl,
    value: f32,
) -> Result<(), NativeEditError> {
    let descriptor = control.descriptor();
    if !value.is_finite() || !(descriptor.min..=descriptor.max).contains(&value) {
        return Err(NativeEditError::Invalid(format!(
            "{} must be finite and within {}..={}",
            descriptor.key, descriptor.min, descriptor.max
        )));
    }
    Ok(())
}

fn processor_mut(
    document: &mut NativeDocument,
    node: NodeId,
) -> Result<&mut auris_core::pipeline::content::Processor, NativeEditError> {
    document
        .content
        .processors
        .iter_mut()
        .find(|entry| entry.id == node)
        .ok_or(NativeEditError::Content(ContentError::Missing(
            auris_core::pipeline::content::ContentRef::Node(node),
        )))
}

fn stage_edit<F>(
    document: &mut NativeDocument,
    clip: ClipId,
    edit: F,
) -> Result<(), NativeEditError>
where
    F: FnOnce(
        &mut auris_core::project::StableMidiClip,
    ) -> Result<bool, auris_core::project::StageError>,
{
    for source in &mut document.content.sources {
        let SourceContent::Notes(clips) = &mut source.content else {
            continue;
        };
        if let Some(entry) = clips.iter_mut().find(|entry| entry.id == clip) {
            edit(entry).map_err(|error| NativeEditError::Invalid(error.to_string()))?;
            entry
                .performer_clip()
                .looped_note_instances()
                .map_err(|error| NativeEditError::Invalid(error.to_string()))?;
            return Ok(());
        }
    }
    Err(NativeEditError::Content(ContentError::Missing(
        auris_core::pipeline::content::ContentRef::Clip(clip),
    )))
}
