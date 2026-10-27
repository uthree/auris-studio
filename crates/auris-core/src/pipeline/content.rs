//! Native ownership fragment for sources, processors and conventional command identities.
//!
//! This serializable data is not embedded in [`crate::Project`], written by the project IO
//! layer, or installed in playback. It keeps authored data in one source, processor state in
//! one node, and routing in explicit edges. Topology/parameter capabilities are resolved and
//! validated separately with [`crate::pipeline::Pipeline`]; this module validates ownership only.

mod automation;
mod bindings;
mod capabilities;
mod operations;
mod ownership;
mod resolution;

pub use automation::{AutomationBinding, AutomationDocument, AutomationError, AutomationId};
pub use bindings::{CommandBindings, ParameterAddress};
pub use capabilities::{
    CapabilityError, CapabilityProvider, PluginCapabilities, PluginParameter, resolve_node,
};
pub use operations::{BranchCopy, SourceCopy};
pub use resolution::{ResolutionError, ResolvedContent};

use serde::{Deserialize, Serialize};

use super::{EdgeId, NodeId, PipelineEdge};
use crate::project::{StableMidiClip, StageError, StageId};
use crate::{
    AssetPath, AudioClip, ClipId, EffectSlotId, NoteTransform, PluginState, SendId, SingerTake,
    SingerVoice, TrackId,
};

/// Authored musical source identity, distinct from the audio-asset `SourceId`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceDocumentId(pub u64);

/// Processing ownership and console-group identity.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GroupId(pub u64);

/// One authoritative collection of authored clips.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SourceContent {
    /// Notes, lyrics, recipes, clip-local curves and stable performance stages.
    Notes(Vec<StableMidiClip>),
    /// Audio placements referencing the project's existing audio-asset registry.
    Audio(Vec<AudioClip>),
}

/// Content shared by source nodes and timeline tracks through an ID, never copied implicitly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceDocument {
    /// Unique source identity.
    pub id: SourceDocumentId,
    /// Sole owner of the source's authored data.
    pub content: SourceContent,
}

/// Stored plugin instance state, independent of its source and track presentation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginInstance {
    /// Registered plugin identifier.
    pub plugin_id: String,
    /// Native parameters and opaque state, preserved without loading the plugin.
    pub state: PluginState,
    /// Hosted plugin location, if it is not a built-in.
    pub file: Option<AssetPath>,
}

/// Offline singer state. A guide instrument connects through the separate `guide_in` audio port.
///
/// Playback uses the cached take whenever one is declared, including a stale take. A missing
/// take asset is silence, not guide fallback; `guide_in` is heard only when no take is declared.
/// The event input supplies offline synthesis material without implicitly regenerating a take.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SingerInstance {
    /// Seconds per feature frame.
    pub frame_hop: f64,
    /// Voice/model reference and settings.
    pub voice: Option<SingerVoice>,
    /// Existing cached take and provenance; conversion never regenerates it.
    pub take: Option<SingerTake>,
}

/// Console state without a second effect chain or routing list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StripState {
    /// Static fader value in decibels.
    pub gain_db: f32,
    /// Stereo position from -1 to 1.
    pub pan: f32,
    /// Mute state.
    pub mute: bool,
    /// Solo state.
    pub solo: bool,
    /// Authored target loudness, if present.
    pub target_lufs: Option<f32>,
}

impl Default for StripState {
    fn default() -> Self {
        Self {
            gain_db: 0.,
            pan: 0.,
            mute: false,
            solo: false,
            target_lufs: None,
        }
    }
}

/// Sole owner of a processing node's settings. Connections live in `PipelineContent::edges`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ProcessorKind {
    /// Shared authored source, independent of processing-group lifetimes.
    Source(SourceDocumentId),
    /// Shared project-time automation curve, independent of processing-group lifetimes.
    Automation(AutomationId),
    /// An independent instrument instance, including a singer guide instrument.
    Instrument(PluginInstance),
    /// Offline singer and its existing take.
    Singer(Box<SingerInstance>),
    /// Non-destructive branch-local performance, evaluated in preparation.
    Transform(NoteTransform),
    /// Effect identity and payload; sidechains and insert order are graph edges.
    Effect {
        /// Existing stable slot identity retained for command compatibility.
        slot: EffectSlotId,
        /// Independent plugin state.
        plugin: PluginInstance,
        /// Bypass state.
        enabled: bool,
    },
    /// Strip with explicit upstream inserts and downstream routes.
    Strip(StripState),
    /// Addressable send gain; tap position and destination are expressed by edges.
    Send {
        /// Existing stable send identity.
        send: SendId,
        /// Static gain in decibels.
        level_db: f32,
    },
    /// Explicit audio summing point.
    Sum,
    /// Device/export output boundary.
    Output,
}

/// One processor instance; sharing a source does not share its plugin state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Processor {
    /// Stable processing identity.
    pub id: NodeId,
    /// Owning group; musical and automation source nodes have no group owner.
    pub group: Option<GroupId>,
    /// Sole stored settings for the node.
    pub kind: ProcessorKind,
}

/// Organization and exposed controls, with no implicit chain or output route.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessingGroup {
    /// Stable ownership identity.
    pub id: GroupId,
    /// Owned strip exposed in the console.
    pub strip: NodeId,
    /// Explicit instrument for track-scoped commands; never guessed from membership/order.
    pub primary_instrument: Option<NodeId>,
}

/// Source and processing roles of a timeline track, separate from its presentation metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackBinding {
    /// Existing track identity.
    pub track: TrackId,
    /// Source for score/clip commands; a bus has no authored source.
    pub source: Option<SourceDocumentId>,
    /// Home processing group for console and instrument commands.
    pub group: GroupId,
}

/// Pre-activation native ownership fragment, suitable for snapshots and explicit round trips.
///
/// Assets, tempo and track presentation remain outside this fragment. Their allocated IDs must
/// be passed to ownership validation/counter repair. There is one numeric ID namespace, shared
/// with retained track, clip, effect, send and asset identities.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PipelineContent {
    /// Authoritative shared clips.
    pub sources: Vec<SourceDocument>,
    /// Curves stored once, shared through automation source nodes.
    pub automation: Vec<AutomationDocument>,
    /// Authoritative processor settings.
    pub processors: Vec<Processor>,
    /// Group identities and exposed controls.
    pub groups: Vec<ProcessingGroup>,
    /// Conventional track command roles.
    pub tracks: Vec<TrackBinding>,
    /// Signal connections. Control connections live only in `automation_bindings`.
    pub edges: Vec<PipelineEdge>,
    /// Canonical control connections, including retained disabled bindings.
    pub automation_bindings: Vec<AutomationBinding>,
    /// Master group used by commands whose track argument is absent.
    pub master: GroupId,
    /// Monotonic shared ID high-water mark. Deletion does not reduce it.
    pub next_id: u64,
}

/// Budgets for identity validation and native ownership edits, not audio/event expansion.
#[derive(Copy, Clone, Debug)]
pub struct ContentLimits {
    /// Maximum authored sources.
    pub sources: usize,
    /// Maximum total clips across all sources.
    pub clips: usize,
    /// Maximum total clip-local stages.
    pub stages: usize,
    /// Maximum processors.
    pub processors: usize,
    /// Maximum groups.
    pub groups: usize,
    /// Maximum timeline bindings.
    pub tracks: usize,
    /// Maximum connections.
    pub edges: usize,
    /// Maximum asset/other external owner IDs supplied by the caller.
    pub external_ids: usize,
    /// Maximum independently owned automation curves.
    pub automations: usize,
    /// Maximum total automation points, checked before inspecting point values.
    pub automation_points: usize,
    /// Maximum control bindings, including disabled ones.
    pub bindings: usize,
}

/// A typed object reference used in diagnostics and legacy command resolution.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ContentRef {
    /// Authored source.
    Source(SourceDocumentId),
    /// Shared automation curve.
    Automation(AutomationId),
    /// Clip.
    Clip(ClipId),
    /// Clip-local stage.
    Stage(StageId),
    /// Processing node.
    Node(NodeId),
    /// Group.
    Group(GroupId),
    /// Timeline track.
    Track(TrackId),
    /// Legacy effect address.
    Effect(EffectSlotId),
    /// Legacy send address.
    Send(SendId),
    /// Connection.
    Edge(EdgeId),
}

/// Native ownership or command-resolution failure; edits leave the original candidate intact.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ContentError {
    /// Caller-supplied count budget was exceeded.
    #[error("native {0} count exceeds its budget")]
    Limit(&'static str),
    /// Two owners spend the same numeric identity, even across typed collections.
    #[error("duplicate native object ID {0}")]
    DuplicateId(u64),
    /// A required referenced owner is absent.
    #[error("missing native object {0:?}")]
    Missing(ContentRef),
    /// An owner exists but has the wrong role, group or source kind.
    #[error("invalid native role for {0:?}: {1}")]
    Role(ContentRef, &'static str),
    /// Counter must be explicitly repaired before allocating new IDs.
    #[error("native ID counter must be greater than {0}")]
    StaleCounter(u64),
    /// Allocation would exhaust the shared identity space.
    #[error("native document ID space is exhausted")]
    IdExhausted,
    /// Stable clip-stage identity validation failed.
    #[error(transparent)]
    Stage(#[from] StageError),
    /// Curve shape or value-domain validation failed.
    #[error(transparent)]
    Automation(#[from] AutomationError),
    /// A plugin parameter index cannot become a stable key without resolved metadata.
    #[error("a nonempty stable plugin parameter key is required")]
    ParameterKeyRequired,
    /// Source removal requires explicitly detaching all references first.
    #[error("source {0:?} still has consumers: {1:?}")]
    SourceInUse(SourceDocumentId, Vec<ContentRef>),
    /// Curve removal requires detaching its source processors first.
    #[error("automation {0:?} still has consumers: {1:?}")]
    AutomationInUse(AutomationId, Vec<ContentRef>),
    /// Removing the master group would invalidate conventional command resolution.
    #[error("the master processing group cannot be removed")]
    MasterRemoval,
}

impl PipelineContent {
    /// Creates an empty authored document with one explicitly owned master strip.
    pub fn new() -> Self {
        Self {
            sources: Vec::new(),
            automation: Vec::new(),
            processors: vec![Processor {
                id: NodeId(2),
                group: Some(GroupId(1)),
                kind: ProcessorKind::Strip(StripState::default()),
            }],
            groups: vec![ProcessingGroup {
                id: GroupId(1),
                strip: NodeId(2),
                primary_instrument: None,
            }],
            tracks: Vec::new(),
            edges: Vec::new(),
            automation_bindings: Vec::new(),
            master: GroupId(1),
            next_id: 3,
        }
    }

    /// Checks ownership identities/references. Port capabilities and DAG validation are separate.
    pub fn validate_ownership(
        &self,
        external_ids: &[u64],
        limits: ContentLimits,
    ) -> Result<(), ContentError> {
        ownership::validate(self, external_ids, limits)
    }

    /// Repairs the high-water mark using every native owner and caller-supplied asset ID.
    ///
    /// Existing high-water marks never decrease, including after deletion. Failure is atomic.
    pub fn repair_id_counter(
        &mut self,
        external_ids: &[u64],
        limits: ContentLimits,
    ) -> Result<(), ContentError> {
        ownership::repair(self, external_ids, limits)
    }

    /// Borrows a validated snapshot for conventional command identity resolution.
    pub fn command_bindings(
        &self,
        external_ids: &[u64],
        limits: ContentLimits,
    ) -> Result<CommandBindings<'_>, ContentError> {
        self.validate_ownership(external_ids, limits)?;
        Ok(CommandBindings::new(self))
    }

    /// Lists direct source references in saved track/node order, without topology traversal.
    pub fn source_consumers(
        &self,
        source: SourceDocumentId,
    ) -> Result<Vec<ContentRef>, ContentError> {
        if !self.sources.iter().any(|entry| entry.id == source) {
            return Err(ContentError::Missing(ContentRef::Source(source)));
        }
        let mut consumers: Vec<_> = self
            .tracks
            .iter()
            .filter(|track| track.source == Some(source))
            .map(|track| ContentRef::Track(track.track))
            .collect();
        consumers.extend(
            self.processors
                .iter()
                .filter(|node| matches!(node.kind, ProcessorKind::Source(id) if id == source))
                .map(|node| ContentRef::Node(node.id)),
        );
        Ok(consumers)
    }
}

impl Default for PipelineContent {
    fn default() -> Self {
        Self::new()
    }
}
