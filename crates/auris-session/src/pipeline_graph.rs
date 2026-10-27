//! A typed, read-only document projection, independent of the currently audible graph.
//!
//! No plugins, assets or singer models are opened. Nodes and edges page independently under
//! one content revision. An edge's endpoints may be on different node pages. This is a read
//! model, not a second routing store; existing domain commands remain the editing boundary.

use crate::Session;
use auris_core::{AutomationCurve, ParamTarget, ParamUnit, Project, SourceId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet};

#[cfg(test)]
mod tests;

/// Maximum nodes per page.
pub const MAX_NODES: usize = 64;
/// Maximum edges per page, even for highly connected buses.
pub const MAX_EDGES: usize = 128;

/// Independent cursors bound to the same document revision and scope.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PipelineQuery {
    /// Focused object and its immediate neighbors; omitted means all.
    pub scope: PipelineScope,
    /// First node to return.
    pub offset: usize,
    /// Node page size, default 32, clamped to 1..64.
    pub limit: Option<usize>,
    /// First edge to return, independent of the node cursor.
    pub edge_offset: usize,
    /// Edge page size, default 64, clamped to 1..128.
    pub edge_limit: Option<usize>,
    /// Required when either cursor is nonzero; copy the first page's revision.
    pub expected_revision: Option<String>,
}

/// A focused read includes owned nodes and their immediate incoming/outgoing neighbors.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum PipelineScope {
    /// The complete document projection.
    #[default]
    All,
    /// One track, its clips, stages, inserts, sends and automation.
    Track {
        /// Stable track ID from inspect_project.
        track: u64,
    },
    /// One clip's source, placement, stages and immediate renderer.
    Clip {
        /// Stable clip ID from inspect_project.
        clip: u64,
    },
    /// One exact projection node and its neighbors.
    Node {
        /// Node ID returned by this reader.
        node: String,
    },
}

impl PipelineScope {
    /// Map the compact agent-facing address to a typed scope without a separate ID vocabulary.
    pub fn from_address(address: Option<&str>) -> Self {
        let Some(address) = address.filter(|address| *address != "all") else {
            return Self::All;
        };
        if let Some(track) = address
            .strip_prefix("track:")
            .and_then(|id| id.parse().ok())
        {
            Self::Track { track }
        } else if let Some(clip) = address.strip_prefix("clip:").and_then(|id| id.parse().ok()) {
            Self::Clip { clip }
        } else {
            Self::Node {
                node: address.into(),
            }
        }
    }
}

/// A bounded structural snapshot, without a runtime readiness guarantee.
#[derive(Clone, Debug, Serialize)]
pub struct PipelinePage {
    /// Content hash shared with note and automation readers.
    pub revision: String,
    /// Scope to keep unchanged while paging.
    pub scope: PipelineScope,
    /// Deterministic document order, not an execution schedule.
    pub nodes: Vec<PipelineNode>,
    /// Independently paged connections; endpoints belong to the scoped node stream.
    pub edges: Vec<PipelineEdge>,
    /// Number of scoped nodes, including boundary neighbors.
    pub total_nodes: usize,
    /// Number of scoped edges.
    pub total_edges: usize,
    /// Next node cursor, or null when exhausted.
    pub next_offset: Option<usize>,
    /// Next edge cursor, or null when exhausted.
    pub next_edge_offset: Option<usize>,
}

/// Failed reads never edit the document or advance history.
#[derive(Clone, Debug, Serialize, thiserror::Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum PipelineError {
    /// Continuation without a revision would permit mixed snapshots.
    #[error("Pass expected_revision from the first pipeline_read page when continuing")]
    RevisionRequired,
    /// The document changed after the first page.
    #[error("Document changed; restart pipeline_read at both offsets zero")]
    RevisionMismatch {
        /// Current revision for diagnostics; restart rather than reusing old cursors.
        current_revision: String,
    },
    /// The requested object does not exist.
    #[error("Pipeline scope does not exist; inspect_project or pipeline_read again")]
    UnknownScope,
    /// A cursor exceeds its scoped stream.
    #[error("Pipeline {stream} offset {offset} exceeds {total}")]
    InvalidOffset {
        /// Nodes or edges.
        stream: &'static str,
        /// Rejected cursor.
        offset: usize,
        /// Stream length.
        total: usize,
    },
    /// Content could not be serialized for the revision or a transform digest.
    #[error("Cannot read pipeline: {message}")]
    Serialization {
        /// Serialization failure.
        message: String,
    },
}

/// Data domain; preparation dependencies additionally carry a distinct edge role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineDomain {
    /// Notes, lyrics, bends and MIDI controllers.
    Event,
    /// Rendered audio.
    Audio,
    /// Parameter values along the timeline.
    Control,
}

/// The current performer evaluates notes and generated curves in separate passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineStageEvaluation {
    /// Note transforms run in stored order for each expanded loop pass.
    PerPassNotes,
    /// Derived bend/controller gestures read the final performed notes.
    AfterNoteStages,
    /// A drum-voice stack may contain both note stages and deferred curve gestures.
    Composite,
}

/// A projection node, retaining the document identities needed by existing commands.
#[derive(Clone, Debug, Serialize)]
pub struct PipelineNode {
    /// Projection ID. Only track:N/effect:N are existing mixer_graph addresses.
    pub id: String,
    /// True for stages and plugin bindings whose meaning can change with the revision.
    pub revision_scoped: bool,
    /// Track owner, when applicable; master and shared sources have no track owner.
    pub track_id: Option<u64>,
    /// Clip owner for authored content, placement and performance.
    pub clip_id: Option<u64>,
    /// Display label, bounded to 256 characters.
    pub label: String,
    /// Accepted input domains, without implying arbitrary wiring is supported.
    pub inputs: Vec<PipelineDomain>,
    /// Produced output domains.
    pub outputs: Vec<PipelineDomain>,
    /// Structural details, omitting full scores, curves and opaque plugin state.
    #[serde(flatten)]
    pub data: PipelineNodeData,
}

/// Bounded metadata read without constructing runtime plugins.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PipelineNodeData {
    /// Authored notes and clip-local MIDI curves before placement/performance.
    NoteSource {
        /// Authored note count before transforms.
        note_count: usize,
        /// Whether a recipe is retained; reading never regenerates it.
        has_recipe: bool,
    },
    /// An audio reference; physical file/buffer availability is not checked.
    AudioSource {
        /// Stable source ID.
        source: u64,
        /// Whether document metadata exists, independently of a loaded buffer.
        registered: bool,
    },
    /// Timeline placement of MIDI or audio content.
    Clip {
        /// Absolute start in project ticks.
        start_tick: i64,
        /// Repetition extent relative to clip start, in ticks.
        loop_end_tick: i64,
        /// Whether playback skips the clip.
        muted: bool,
    },
    /// One non-destructive operation; nested drum transforms remain composite.
    ClipStage {
        /// Ordinal, not a persistent StageId.
        stage_index: usize,
        /// Stored transform discriminant.
        transform_kind: String,
        /// Full transform digest for diagnostics, not persistent identity.
        fingerprint: String,
        /// Whether this contains nested drum-voice operations.
        composite: bool,
        /// Evaluation phase; graph edges retain stored order, not a literal runtime schedule.
        evaluation: PipelineStageEvaluation,
    },
    /// Current track-owned realtime instrument, including singer guide fallback.
    Instrument {
        /// Document plugin identity; availability is unchecked.
        plugin_id: String,
        /// Whether an external plugin file is referenced.
        hosted: bool,
        /// True for a singer's guide instrument, used for fallback playback or note audition.
        singer_guide: bool,
        /// True when a cached singer take owns timeline playback and this only auditions notes.
        audition_only: bool,
    },
    /// Cached playback with an offline score dependency, not realtime synthesis.
    SingerTake {
        /// Stored source selected by the renderer, even when the score has changed.
        source: u64,
        /// False here; inspect_singer performs the more expensive freshness check.
        freshness_checked: bool,
    },
    /// Summing input before inserts; preserves the mixer_graph track address.
    StripInput,
    /// One insert, retained while bypassed.
    Effect {
        /// Stable slot ID.
        slot: u64,
        /// Document plugin identity.
        plugin_id: String,
        /// Bypass state, independent of plugin availability.
        enabled: bool,
        /// Whether an external file is referenced.
        hosted: bool,
    },
    /// Gain/pan and mute/solo audibility after inserts and latency compensation.
    Fader {
        /// Static decibels; automation can override this.
        gain_db: f32,
        /// Static pan in -1..1.
        pan: f32,
        /// Authored mute switch, not the current runtime fade.
        mute: bool,
        /// Authored solo switch, not resolved runtime audibility.
        solo: bool,
    },
    /// Auxiliary send gain, independently targetable by automation.
    Send {
        /// Stable send ID for mixer_graph operations.
        send: u64,
        /// Static decibels.
        level_db: f32,
        /// Post-insert tap before strip gain/pan/mute instead of after them.
        pre_fader: bool,
    },
    /// Project-timeline control lane; full points remain in the automation tool.
    Automation {
        /// Stored runtime address; plugin persistence also requires the key.
        target: ParamTarget,
        /// Saved plugin parameter key, if present.
        key: Option<String>,
        /// Stored point count.
        point_count: usize,
        /// Stored interpolation; values hold beyond the first/last points.
        curve: AutomationCurve,
        /// Whether the target owner exists in the projection.
        owner_present: bool,
        /// True only for structural mixer controls; plugin descriptors remain unchecked.
        parameter_checked: bool,
        /// Mixer units, or null for unchecked plugin parameters.
        value_unit: Option<ParamUnit>,
    },
    /// Master musical output, excluding device monitoring and metronome.
    Output,
}

/// Connection or preparation dependency derived from the document.
#[derive(Clone, Debug, Serialize)]
pub struct PipelineEdge {
    /// Source in the scoped node stream.
    pub source: String,
    /// Destination in the scoped node stream.
    pub target: String,
    /// Carried data domain.
    pub domain: PipelineDomain,
    /// Routing or preparation semantics.
    pub kind: PipelineEdgeKind,
}

/// Roles distinguish offline preparation from realtime data flow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineEdgeKind {
    /// Source to placement.
    Content,
    /// Stored clip-stack order; each stage's evaluation field identifies its execution phase.
    Performance,
    /// Events entering a realtime instrument.
    Events,
    /// Offline score dependency; editing does not regenerate a take.
    SynthesisDependency,
    /// Audio entering its strip or cached take.
    Playback,
    /// Consecutive insert/strip stages.
    Chain,
    /// Post-fader main output.
    Output,
    /// Auxiliary tap through send gain.
    Send,
    /// Post-fader key input to an effect.
    Sidechain,
    /// Control of a parameter on the destination.
    Automation,
}

impl Session {
    /// Finds one projection node without applying the public page-size limit.
    pub(crate) fn pipeline_node(&self, node: &str) -> Result<Option<PipelineNode>, PipelineError> {
        let mut projection = Projection::from_project(self.project())?;
        projection.nodes.retain(|candidate| candidate.id == node);
        Ok(projection.nodes.into_iter().next())
    }

    /// Read a consistent page without opening assets or changing history.
    pub fn pipeline_graph(&self, query: &PipelineQuery) -> Result<PipelinePage, PipelineError> {
        if (query.offset > 0 || query.edge_offset > 0) && query.expected_revision.is_none() {
            return Err(PipelineError::RevisionRequired);
        }
        let revision = self
            .agent_document_revision()
            .map_err(|message| PipelineError::Serialization { message })?;
        if query
            .expected_revision
            .as_ref()
            .is_some_and(|expected| expected != &revision)
        {
            return Err(PipelineError::RevisionMismatch {
                current_revision: revision,
            });
        }
        let mut projection = Projection::from_project(self.project())?;
        projection.focus(&query.scope)?;
        let total_nodes = projection.nodes.len();
        let total_edges = projection.edges.len();
        validate_offset("nodes", query.offset, total_nodes)?;
        validate_offset("edges", query.edge_offset, total_edges)?;
        let nodes: Vec<_> = projection
            .nodes
            .into_iter()
            .skip(query.offset)
            .take(query.limit.unwrap_or(32).clamp(1, MAX_NODES))
            .collect();
        let edges: Vec<_> = projection
            .edges
            .into_iter()
            .skip(query.edge_offset)
            .take(query.edge_limit.unwrap_or(64).clamp(1, MAX_EDGES))
            .collect();
        let next = query.offset + nodes.len();
        let next_edge = query.edge_offset + edges.len();
        Ok(PipelinePage {
            revision,
            scope: query.scope.clone(),
            nodes,
            edges,
            total_nodes,
            total_edges,
            next_offset: (next < total_nodes).then_some(next),
            next_edge_offset: (next_edge < total_edges).then_some(next_edge),
        })
    }
}

fn validate_offset(stream: &'static str, offset: usize, total: usize) -> Result<(), PipelineError> {
    if offset > total {
        return Err(PipelineError::InvalidOffset {
            stream,
            offset,
            total,
        });
    }
    Ok(())
}

#[derive(Default)]
struct Projection {
    nodes: Vec<PipelineNode>,
    edges: Vec<PipelineEdge>,
}

impl Projection {
    fn node(
        &mut self,
        id: String,
        track: Option<TrackId>,
        clip: Option<u64>,
        label: &str,
        data: PipelineNodeData,
    ) {
        use PipelineDomain::{Audio, Control, Event};
        use PipelineNodeData as N;
        let (inputs, outputs) = match &data {
            N::NoteSource { .. } => (vec![], vec![Event]),
            N::AudioSource { .. } => (vec![], vec![Audio]),
            N::Clip { .. } => (vec![], vec![]),
            N::ClipStage { .. } => (vec![Event], vec![Event]),
            N::Instrument { .. } => (vec![Event, Control], vec![Audio]),
            N::SingerTake { .. } => (vec![Audio, Event], vec![Audio]),
            N::StripInput => (vec![Audio], vec![Audio]),
            N::Effect { .. } | N::Fader { .. } | N::Send { .. } => {
                (vec![Audio, Control], vec![Audio])
            }
            N::Automation { .. } => (vec![], vec![Control]),
            N::Output => (vec![Audio], vec![]),
        };
        let revision_scoped = matches!(
            data,
            N::ClipStage { .. }
                | N::Automation {
                    target: ParamTarget::Instrument { .. } | ParamTarget::Effect { .. },
                    ..
                }
        );
        self.nodes.push(PipelineNode {
            id,
            revision_scoped,
            track_id: track.map(|id| id.0),
            clip_id: clip,
            label: label.chars().take(256).collect(),
            inputs,
            outputs,
            data,
        });
    }

    fn edge(
        &mut self,
        source: String,
        target: String,
        domain: PipelineDomain,
        kind: PipelineEdgeKind,
    ) {
        self.edges.push(PipelineEdge {
            source,
            target,
            domain,
            kind,
        });
    }

    fn from_project(project: &Project) -> Result<Self, PipelineError> {
        use PipelineDomain::{Audio, Control, Event};
        use PipelineEdgeKind as E;
        use PipelineNodeData as N;
        let mut graph = Self::default();
        let mut sources: BTreeSet<_> = project.audio_sources.keys().copied().collect();
        for track in &project.tracks {
            if let Some(audio) = track.kind.as_audio() {
                sources.extend(audio.clips.iter().map(|clip| clip.source));
            }
            if let Some(singer) = track.kind.as_singer()
                && let Some(take) = &singer.take
            {
                sources.insert(take.source);
            }
        }
        for source in sources {
            let entry = project.audio_sources.get(&source);
            graph.node(
                audio_source(source),
                None,
                None,
                entry.map_or("Missing audio source", |source| &source.name),
                N::AudioSource {
                    source: source.0,
                    registered: entry.is_some(),
                },
            );
        }
        for track in &project.tracks {
            let owner = Some(track.id);
            let instrument_id = instrument(track.id);
            let mut renderer = None;
            if let Some(synth) = track.kind.as_instrument() {
                graph.node(
                    instrument_id.clone(),
                    owner,
                    None,
                    &synth.instrument_id,
                    N::Instrument {
                        plugin_id: synth.instrument_id.clone(),
                        hosted: synth.is_hosted(),
                        singer_guide: false,
                        audition_only: false,
                    },
                );
                renderer = Some((instrument_id.clone(), E::Events));
            } else if let Some(singer) = track.kind.as_singer() {
                if let Some(take) = singer
                    .take
                    .as_ref()
                    .filter(|take| project.audio_sources.contains_key(&take.source))
                {
                    graph.node(
                        instrument_id.clone(),
                        owner,
                        None,
                        &singer.instrument_id,
                        N::Instrument {
                            plugin_id: singer.instrument_id.clone(),
                            hosted: false,
                            singer_guide: true,
                            audition_only: true,
                        },
                    );
                    let id = format!("singer_take:track:{}", track.id.0);
                    graph.node(
                        id.clone(),
                        owner,
                        None,
                        &track.name,
                        N::SingerTake {
                            source: take.source.0,
                            freshness_checked: false,
                        },
                    );
                    graph.edge(audio_source(take.source), id.clone(), Audio, E::Playback);
                    renderer = Some((id, E::SynthesisDependency));
                } else {
                    graph.node(
                        instrument_id.clone(),
                        owner,
                        None,
                        &singer.instrument_id,
                        N::Instrument {
                            plugin_id: singer.instrument_id.clone(),
                            hosted: false,
                            singer_guide: true,
                            audition_only: false,
                        },
                    );
                    renderer = Some((instrument_id.clone(), E::Events));
                }
            }
            if let Some(clips) = track.kind.note_clips() {
                for clip in clips {
                    let id = format!("clip:{}", clip.id.0);
                    let source = format!("source:clip:{}", clip.id.0);
                    graph.node(
                        source.clone(),
                        owner,
                        Some(clip.id.0),
                        &clip.name,
                        N::NoteSource {
                            note_count: clip.notes.len(),
                            has_recipe: clip.recipe.is_some(),
                        },
                    );
                    graph.clip_node(
                        id.clone(),
                        track.id,
                        clip.id.0,
                        &clip.name,
                        clip.start.raw(),
                        clip.loop_end.raw(),
                        clip.muted,
                        Event,
                    );
                    graph.edge(source, id.clone(), Event, E::Content);
                    let mut previous = id;
                    for (index, transform) in clip.transforms.iter().enumerate() {
                        let value = serde_json::to_value(transform).map_err(|error| {
                            PipelineError::Serialization {
                                message: error.to_string(),
                            }
                        })?;
                        let kind = value["kind"].as_str().unwrap_or("transform").to_owned();
                        let fingerprint = hex::encode(Sha256::digest(value.to_string().as_bytes()));
                        let stage = format!("clip:{}/stage:{index}", clip.id.0);
                        let evaluation = match transform {
                            auris_core::NoteTransform::Pitch { .. } => {
                                PipelineStageEvaluation::AfterNoteStages
                            }
                            auris_core::NoteTransform::ForDrumVoice { .. } => {
                                PipelineStageEvaluation::Composite
                            }
                            _ => PipelineStageEvaluation::PerPassNotes,
                        };
                        graph.node(
                            stage.clone(),
                            owner,
                            Some(clip.id.0),
                            &kind,
                            N::ClipStage {
                                stage_index: index,
                                transform_kind: kind.clone(),
                                fingerprint,
                                composite: matches!(
                                    transform,
                                    auris_core::NoteTransform::ForDrumVoice { .. }
                                ),
                                evaluation,
                            },
                        );
                        graph.edge(previous, stage.clone(), Event, E::Performance);
                        previous = stage;
                    }
                    if let Some((renderer, kind)) = &renderer {
                        graph.edge(previous, renderer.clone(), Event, *kind);
                    }
                }
            }
            if let Some((renderer, _)) = renderer {
                graph.edge(renderer, strip(owner), Audio, E::Playback);
            }
            if let Some(audio) = track.kind.as_audio() {
                for clip in &audio.clips {
                    let id = format!("clip:{}", clip.id.0);
                    graph.clip_node(
                        id.clone(),
                        track.id,
                        clip.id.0,
                        &clip.name,
                        clip.start.raw(),
                        clip.loop_end.raw(),
                        clip.muted,
                        Audio,
                    );
                    graph.edge(audio_source(clip.source), id.clone(), Audio, E::Content);
                    graph.edge(id, strip(owner), Audio, E::Playback);
                }
            }
        }
        for (owner, mixer, name) in project
            .tracks
            .iter()
            .map(|track| (Some(track.id), &track.mixer, track.name.as_str()))
            .chain(std::iter::once((None, &project.master, "Master")))
        {
            graph.node(strip(owner), owner, None, name, N::StripInput);
            let mut previous = strip(owner);
            for effect in &mixer.effects {
                let id = format!("effect:{}", effect.id.0);
                graph.node(
                    id.clone(),
                    owner,
                    None,
                    &effect.effect_id,
                    N::Effect {
                        slot: effect.id.0,
                        plugin_id: effect.effect_id.clone(),
                        enabled: effect.enabled,
                        hosted: effect.is_hosted(),
                    },
                );
                graph.edge(previous, id.clone(), Audio, E::Chain);
                if let Some(source) = effect.sidechain {
                    graph.edge(fader(Some(source)), id.clone(), Audio, E::Sidechain);
                }
                previous = id;
            }
            graph.node(
                fader(owner),
                owner,
                None,
                name,
                N::Fader {
                    gain_db: mixer.gain_db,
                    pan: mixer.pan,
                    mute: mixer.mute,
                    solo: mixer.solo,
                },
            );
            graph.edge(previous.clone(), fader(owner), Audio, E::Chain);
            if let Some(track) = owner.and_then(|id| project.track(id)) {
                for output in &track.outputs {
                    graph.edge(fader(owner), strip(output.bus()), Audio, E::Output);
                }
                for send in &track.sends {
                    let id = format!("send:{}", send.id.0);
                    graph.node(
                        id.clone(),
                        owner,
                        None,
                        "Send",
                        N::Send {
                            send: send.id.0,
                            level_db: send.level_db,
                            pre_fader: send.pre_fader,
                        },
                    );
                    graph.edge(
                        if send.pre_fader {
                            previous.clone()
                        } else {
                            fader(owner)
                        },
                        id.clone(),
                        Audio,
                        E::Send,
                    );
                    graph.edge(id, strip(Some(send.target)), Audio, E::Send);
                }
            }
        }
        graph.node(
            "output:master".into(),
            None,
            None,
            "Master output",
            N::Output,
        );
        graph.edge(fader(None), "output:master".into(), Audio, E::Output);
        let owners: HashSet<_> = graph.nodes.iter().map(|node| node.id.clone()).collect();
        for lane in project.automation.lanes() {
            let (target, unit, plugin_param) = parameter_owner(lane.target);
            // Runtime indexes disambiguate duplicate saved keys; plugin binding IDs are explicitly
            // revision-scoped. Keep the saved key visible without claiming descriptor resolution.
            let id = automation_id(lane.target);
            let present = owners.contains(&target) && parameter_owner_exists(project, lane.target);
            graph.node(
                id.clone(),
                lane.target.track(),
                None,
                lane.key().unwrap_or("Automation"),
                N::Automation {
                    target: lane.target,
                    key: lane.key().map(str::to_owned),
                    point_count: lane.points().len(),
                    curve: lane.curve,
                    owner_present: present,
                    parameter_checked: present && !plugin_param,
                    value_unit: unit,
                },
            );
            if present {
                graph.edge(id, target, Control, E::Automation);
            }
        }
        Ok(graph)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "One constructor keeps MIDI/audio placement ports consistent"
    )]
    fn clip_node(
        &mut self,
        id: String,
        track: TrackId,
        clip: u64,
        label: &str,
        start_tick: i64,
        loop_end_tick: i64,
        muted: bool,
        domain: PipelineDomain,
    ) {
        self.node(
            id,
            Some(track),
            Some(clip),
            label,
            PipelineNodeData::Clip {
                start_tick,
                loop_end_tick,
                muted,
            },
        );
        if let Some(node) = self.nodes.last_mut() {
            node.inputs.push(domain);
            node.outputs.push(domain);
        }
    }

    fn focus(&mut self, scope: &PipelineScope) -> Result<(), PipelineError> {
        if matches!(scope, PipelineScope::All) {
            return Ok(());
        }
        let anchors: HashSet<_> = self
            .nodes
            .iter()
            .filter(|node| match scope {
                PipelineScope::All => true,
                PipelineScope::Track { track } => node.track_id == Some(*track),
                PipelineScope::Clip { clip } => node.clip_id == Some(*clip),
                PipelineScope::Node { node: id } => node.id == *id,
            })
            .map(|node| node.id.clone())
            .collect();
        if anchors.is_empty() {
            return Err(PipelineError::UnknownScope);
        }
        self.edges
            .retain(|edge| anchors.contains(&edge.source) || anchors.contains(&edge.target));
        let mut included = anchors;
        for edge in &self.edges {
            included.insert(edge.source.clone());
            included.insert(edge.target.clone());
        }
        self.nodes.retain(|node| included.contains(&node.id));
        Ok(())
    }
}

fn audio_source(source: SourceId) -> String {
    format!("source:audio:{}", source.0)
}
fn instrument(track: TrackId) -> String {
    format!("instrument:track:{}", track.0)
}
fn strip(owner: Option<TrackId>) -> String {
    owner.map_or_else(|| "track:master".into(), |id| format!("track:{}", id.0))
}
fn fader(owner: Option<TrackId>) -> String {
    owner.map_or_else(
        || "fader:master".into(),
        |id| format!("fader:track:{}", id.0),
    )
}

fn parameter_owner(target: ParamTarget) -> (String, Option<ParamUnit>, bool) {
    match target {
        ParamTarget::TrackGain(track) => (fader(Some(track)), Some(ParamUnit::Decibels), false),
        ParamTarget::TrackPan(track) => (fader(Some(track)), Some(ParamUnit::Pan), false),
        ParamTarget::MasterGain => (fader(None), Some(ParamUnit::Decibels), false),
        ParamTarget::MasterPan => (fader(None), Some(ParamUnit::Pan), false),
        ParamTarget::Send { send, .. } => {
            (format!("send:{}", send.0), Some(ParamUnit::Decibels), false)
        }
        ParamTarget::Instrument { track, .. } => (instrument(track), None, true),
        ParamTarget::Effect { slot, .. } => (format!("effect:{}", slot.0), None, true),
    }
}

fn automation_id(target: ParamTarget) -> String {
    match target {
        ParamTarget::TrackGain(track) => format!("automation:track:{}/gain", track.0),
        ParamTarget::TrackPan(track) => format!("automation:track:{}/pan", track.0),
        ParamTarget::MasterGain => "automation:master/gain".into(),
        ParamTarget::MasterPan => "automation:master/pan".into(),
        ParamTarget::Send { track, send } => {
            format!("automation:track:{}/send:{}/level", track.0, send.0)
        }
        ParamTarget::Instrument { track, param } => {
            format!("automation:instrument:{}/param:{}", track.0, param.raw())
        }
        ParamTarget::Effect { track, slot, param } => format!(
            "automation:{}/effect:{}/param:{}",
            strip(track),
            slot.0,
            param.raw()
        ),
    }
}

fn parameter_owner_exists(project: &Project, target: ParamTarget) -> bool {
    match target {
        ParamTarget::TrackGain(track) | ParamTarget::TrackPan(track) => {
            project.track(track).is_some()
        }
        ParamTarget::MasterGain | ParamTarget::MasterPan => true,
        ParamTarget::Instrument { track, .. } => project.track(track).is_some_and(|track| {
            track.kind.as_instrument().is_some() || track.kind.as_singer().is_some()
        }),
        ParamTarget::Send { track, send } => project
            .track(track)
            .is_some_and(|track| track.sends.iter().any(|entry| entry.id == send)),
        ParamTarget::Effect { track, slot, .. } => {
            let mixer = match track {
                Some(track) => project.track(track).map(|track| &track.mixer),
                None => Some(&project.master),
            };
            mixer.is_some_and(|mixer| mixer.effects.iter().any(|entry| entry.id == slot))
        }
    }
}
