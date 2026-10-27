//! Resolution of retained project commands to native pipeline identities.
//!
//! The resolver deliberately contains no fallback based on vector order.  A validated content
//! snapshot is borrowed for the lifetime of the resolver, so command code can use the result
//! without copying or mutating the ownership fragment.

use serde::{Deserialize, Serialize};

use crate::param::ParamTarget;
use crate::project::{StableMidiClip, StageId};
use crate::{ClipId, EffectSlotId, SendId, TrackId};

use super::{ContentError, ContentRef, PipelineContent, ProcessorKind, SourceContent};
use super::{GroupId, SourceDocumentId};
use crate::pipeline::NodeId;

/// A stable address for a built-in or plugin parameter on a native processor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParameterAddress {
    /// Native processor owning the parameter.
    pub node: NodeId,
    /// Stable parameter key supplied by the plugin metadata or a built-in descriptor.
    pub key: String,
}

/// Resolves conventional project commands against a validated content snapshot.
pub struct CommandBindings<'a> {
    content: &'a PipelineContent,
}

impl<'a> CommandBindings<'a> {
    pub(super) fn new(content: &'a PipelineContent) -> Self {
        Self { content }
    }

    /// Returns the authored source used by a track.
    pub fn source(&self, track: TrackId) -> Result<SourceDocumentId, ContentError> {
        let binding = self.track(track)?;
        let source = binding.source.ok_or(ContentError::Role(
            ContentRef::Track(track),
            "track has no authored source",
        ))?;
        if self
            .content
            .sources
            .iter()
            .any(|candidate| candidate.id == source)
        {
            Ok(source)
        } else {
            Err(ContentError::Missing(ContentRef::Source(source)))
        }
    }

    /// Returns the strip node for a track, or the master strip when `track` is absent.
    pub fn strip(&self, track: Option<TrackId>) -> Result<NodeId, ContentError> {
        let group = self.group_for(track)?;
        let node = self
            .content
            .processors
            .iter()
            .find(|processor| processor.id == group.strip)
            .ok_or(ContentError::Missing(ContentRef::Node(group.strip)))?;
        if node.group != Some(group.id) || !matches!(node.kind, ProcessorKind::Strip(_)) {
            return Err(ContentError::Role(
                ContentRef::Node(node.id),
                "group strip is not an owned strip processor",
            ));
        }
        Ok(node.id)
    }

    /// Returns the explicitly designated primary instrument of a track's group.
    ///
    /// Membership or processor order is intentionally never used as a fallback.  A singer's
    /// offline processor is a separate role; its guide instrument is resolved like any other
    /// explicitly designated instrument node.
    pub fn instrument(&self, track: TrackId) -> Result<NodeId, ContentError> {
        let binding = self.track(track)?;
        let group = self.group(binding.group)?;
        let node_id = group.primary_instrument.ok_or(ContentError::Role(
            ContentRef::Track(track),
            "track group has no explicit primary instrument",
        ))?;
        let node = self
            .content
            .processors
            .iter()
            .find(|processor| processor.id == node_id)
            .ok_or(ContentError::Missing(ContentRef::Node(node_id)))?;
        if node.group != Some(group.id) || !matches!(node.kind, ProcessorKind::Instrument(_)) {
            return Err(ContentError::Role(
                ContentRef::Node(node.id),
                "primary instrument is not an owned instrument processor",
            ));
        }
        Ok(node.id)
    }

    /// Returns an effect node by its retained slot identity on a track or the master bus.
    pub fn effect(
        &self,
        track: Option<TrackId>,
        slot: EffectSlotId,
    ) -> Result<NodeId, ContentError> {
        let expected = self.group_for(track)?.id;
        let node = self
            .content
            .processors
            .iter()
            .find(|processor| matches!(processor.kind, ProcessorKind::Effect { slot: candidate, .. } if candidate == slot))
            .ok_or(ContentError::Missing(ContentRef::Effect(slot)))?;
        if node.group != Some(expected) {
            return Err(ContentError::Role(
                ContentRef::Effect(slot),
                "effect belongs to another processing group",
            ));
        }
        Ok(node.id)
    }

    /// Returns a send node by its retained send identity and owning track.
    pub fn send(&self, track: TrackId, send: SendId) -> Result<NodeId, ContentError> {
        let expected = self.track(track)?.group;
        let node = self
            .content
            .processors
            .iter()
            .find(|processor| matches!(processor.kind, ProcessorKind::Send { send: candidate, .. } if candidate == send))
            .ok_or(ContentError::Missing(ContentRef::Send(send)))?;
        if node.group != Some(expected) {
            return Err(ContentError::Role(
                ContentRef::Send(send),
                "send belongs to another processing group",
            ));
        }
        Ok(node.id)
    }

    /// Returns the source document containing a retained clip identity.
    pub fn clip(&self, clip: ClipId) -> Result<SourceDocumentId, ContentError> {
        self.find_clip(clip).map(|(source, _)| source)
    }

    /// Returns a clip's source document and its current local stage index.
    ///
    /// The index is intentionally only a snapshot convenience; the `StageId` remains the
    /// persistent address across reordering.
    pub fn stage(
        &self,
        clip: ClipId,
        stage: StageId,
    ) -> Result<(SourceDocumentId, usize), ContentError> {
        let (source, stable_clip) = self.find_clip(clip)?;
        let stable_clip = stable_clip.ok_or(ContentError::Role(
            ContentRef::Clip(clip),
            "audio clips do not contain performance stages",
        ))?;
        Ok((source, stable_clip.stage_index(stage)?))
    }

    /// Resolves a retained parameter target to a native node and stable key.
    ///
    /// Numeric plugin parameter IDs are compatibility metadata only.  Plugin targets must carry
    /// a nonempty stable key resolved from plugin metadata by the caller.
    pub fn parameter(
        &self,
        target: ParamTarget,
        stable_key: Option<&str>,
    ) -> Result<ParameterAddress, ContentError> {
        let (node, key) = match target {
            ParamTarget::TrackGain(track) => (self.strip(Some(track))?, "gain_db"),
            ParamTarget::TrackPan(track) => (self.strip(Some(track))?, "pan"),
            ParamTarget::MasterGain => (self.strip(None)?, "gain_db"),
            ParamTarget::MasterPan => (self.strip(None)?, "pan"),
            ParamTarget::Send { track, send } => (self.send(track, send)?, "level_db"),
            ParamTarget::Instrument { track, .. } => {
                (self.instrument(track)?, require_key(stable_key)?)
            }
            ParamTarget::Effect { track, slot, .. } => {
                (self.effect(track, slot)?, require_key(stable_key)?)
            }
        };
        Ok(ParameterAddress {
            node,
            key: key.to_owned(),
        })
    }

    fn track(&self, track: TrackId) -> Result<&super::TrackBinding, ContentError> {
        self.content
            .tracks
            .iter()
            .find(|binding| binding.track == track)
            .ok_or(ContentError::Missing(ContentRef::Track(track)))
    }

    fn group(&self, group: GroupId) -> Result<&super::ProcessingGroup, ContentError> {
        self.content
            .groups
            .iter()
            .find(|candidate| candidate.id == group)
            .ok_or(ContentError::Missing(ContentRef::Group(group)))
    }

    fn group_for(&self, track: Option<TrackId>) -> Result<&super::ProcessingGroup, ContentError> {
        let group = match track {
            Some(track) => self.track(track)?.group,
            None => self.content.master,
        };
        self.group(group)
    }

    fn find_clip(
        &self,
        clip: ClipId,
    ) -> Result<(SourceDocumentId, Option<&StableMidiClip>), ContentError> {
        for source in &self.content.sources {
            match &source.content {
                SourceContent::Notes(clips) => {
                    if let Some(clip) = clips.iter().find(|candidate| candidate.id == clip) {
                        return Ok((source.id, Some(clip)));
                    }
                }
                SourceContent::Audio(clips) => {
                    if clips.iter().any(|candidate| candidate.id == clip) {
                        return Ok((source.id, None));
                    }
                }
            }
        }
        Err(ContentError::Missing(ContentRef::Clip(clip)))
    }
}

fn require_key(key: Option<&str>) -> Result<&str, ContentError> {
    match key.filter(|key| !key.is_empty()) {
        Some(key) => Ok(key),
        None => Err(ContentError::ParameterKeyRequired),
    }
}
