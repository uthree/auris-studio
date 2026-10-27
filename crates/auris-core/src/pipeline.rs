//! Typed, format-independent processing topology.
//!
//! Resolved port descriptors and pure validation, independent of plugin loading, audio
//! buffers, file IO and UI. This foundation is not yet stored on [`crate::Project`] or used
//! by the renderer. Processor owners must resolve their capabilities into these descriptors;
//! validation cannot prove a plugin implements them. Native ownership, migration and compiler
//! integration follow the activation gate in `docs/render-pipeline-plan.md`.
//!
//! Event, audio, sidechain and control connections contribute to the same dependency order.
//! Disconnected nodes are valid. No implicit mixer route or conversion is added.

mod validation;

/// Native source and processor ownership, staged independently of project-file activation.
pub mod content;

/// A standalone native document envelope used by pre-activation conversion and snapshots.
pub mod document;

use crate::ParamUnit;
use serde::{Deserialize, Serialize};

/// Stable identifier for a processing node within a project.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct NodeId(pub u64);

/// Stable connection identifier, including parallel connections.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct EdgeId(pub u64);

/// Stable processor-defined port key, unique across both directions of one node.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PortId(pub String);

impl PortId {
    /// Creates a port identifier; validation rejects empty keys.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

/// Explicit channel layout; equality is required, with no automatic conversion.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AudioLayout {
    /// One channel.
    Mono,
    /// Left and right channels.
    Stereo,
    /// An ordered discrete-channel layout of at least three channels.
    Discrete(u16),
}

/// Control value representation. Direct connections require identical units and finite ranges.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ControlDomain {
    /// A normalized range contained within 0..=1, distinct from native units.
    Normalized {
        /// Inclusive lower bound.
        min: f32,
        /// Inclusive upper bound, strictly greater than `min`.
        max: f32,
    },
    /// Values in a parameter's native unit.
    Native {
        /// Value unit, never a translated display label.
        unit: ParamUnit,
        /// Inclusive finite lower bound.
        min: f32,
        /// Inclusive finite upper bound, strictly greater than `min`.
        max: f32,
    },
}

/// Data carried by a resolved port.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum PortType {
    /// Notes and associated events/provenance.
    Events,
    /// Audio with a declared channel layout.
    Audio(AudioLayout),
    /// Control values with a declared representation.
    Control(ControlDomain),
}

/// Connection semantics at the receiving port.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeRole {
    /// Primary audio signal.
    Main,
    /// Audio used by a processor's sidechain input.
    Sidechain,
    /// A control stream or absolute parameter driver.
    Control,
    /// A note/event stream.
    Events,
}

/// When a processor can consume parameter changes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AutomationPolicy {
    /// Continuous control with a realtime processing contract.
    Continuous,
    /// Values sampled at events with an explicit pairing contract.
    EventSampled,
    /// Static settings used while preparing a graph.
    PreparationOnly,
    /// Settings requiring an explicit regeneration command.
    RegenerationRequired,
    /// Settings that alter topology or processor configuration.
    Structural,
}

/// A port; direction comes from its owner's input or output list.
#[derive(Clone, Debug, PartialEq)]
pub struct Port {
    /// Stable key within the node.
    pub id: PortId,
    /// Data accepted or produced.
    pub ty: PortType,
    /// Input semantics. Audio outputs use `Main`, even when feeding a sidechain.
    pub role: EdgeRole,
    /// Explicit audio summing contract; valid only on audio inputs.
    pub accepts_multiple: bool,
}

impl Port {
    /// Creates a single-driver port with the role implied by its type.
    pub fn new(id: impl Into<String>, ty: PortType) -> Self {
        Self {
            id: PortId::new(id),
            ty,
            role: match ty {
                PortType::Events => EdgeRole::Events,
                PortType::Audio(_) => EdgeRole::Main,
                PortType::Control(_) => EdgeRole::Control,
            },
            accepts_multiple: false,
        }
    }

    /// Creates an audio input that explicitly sums incoming signals.
    pub fn sum_input(id: impl Into<String>, layout: AudioLayout) -> Self {
        Self {
            accepts_multiple: true,
            ..Self::new(id, PortType::Audio(layout))
        }
    }

    /// Creates a single-driver audio sidechain input.
    pub fn sidechain_input(id: impl Into<String>, layout: AudioLayout) -> Self {
        Self {
            role: EdgeRole::Sidechain,
            ..Self::new(id, PortType::Audio(layout))
        }
    }
}

/// Stable parameter address and its processor-supplied automation contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parameter {
    /// Stable saved key, never a runtime parameter index or translated label.
    pub key: String,
    /// Absolute driver input, or no port for a metadata-only setting.
    pub port: Option<PortId>,
    /// When the parameter can consume changes.
    pub policy: AutomationPolicy,
    /// Whether the processor reports an output that cannot be edited.
    pub read_only: bool,
}

/// Resolved interface; processor payloads and authored sources have separate owners.
#[derive(Clone, Debug, PartialEq)]
pub struct PipelineNode {
    /// Stable instance identity.
    pub id: NodeId,
    /// Incoming ports; unconnected ports are allowed by this topology contract.
    pub inputs: Vec<Port>,
    /// Outgoing ports; fan-out does not duplicate processor state.
    pub outputs: Vec<Port>,
    /// Parameters; validation rejects duplicate keys or driver-port aliases.
    pub parameters: Vec<Parameter>,
}

impl PipelineNode {
    /// Creates an empty node interface.
    pub fn new(id: NodeId) -> Self {
        Self {
            id,
            inputs: Vec::new(),
            outputs: Vec::new(),
            parameters: Vec::new(),
        }
    }
    /// Adds an input, preserving duplicates for validation to reject.
    pub fn input(mut self, port: Port) -> Self {
        self.inputs.push(port);
        self
    }
    /// Adds an output, preserving duplicates for validation to reject.
    pub fn output(mut self, port: Port) -> Self {
        self.outputs.push(port);
        self
    }
    /// Describes a parameter without creating a control input.
    pub fn parameter(mut self, key: impl Into<String>, policy: AutomationPolicy) -> Self {
        self.parameters.push(Parameter {
            key: key.into(),
            port: None,
            policy,
            read_only: false,
        });
        self
    }
    /// Adds an absolute parameter input using the stable parameter key as its port key.
    ///
    /// Static/read-only metadata remains present; validation rejects incoming drivers for it.
    pub fn parameter_input(
        mut self,
        key: impl Into<String>,
        domain: ControlDomain,
        policy: AutomationPolicy,
        read_only: bool,
    ) -> Self {
        let key = key.into();
        self.inputs
            .push(Port::new(key.clone(), PortType::Control(domain)));
        self.parameters.push(Parameter {
            port: Some(PortId::new(key.clone())),
            key,
            policy,
            read_only,
        });
        self
    }
}

/// Directed dependency between an output port and an input port.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineEdge {
    /// Stable identity, distinct even for parallel audio paths.
    pub id: EdgeId,
    /// Producing node and output key.
    pub from: (NodeId, PortId),
    /// Consuming node and input key.
    pub to: (NodeId, PortId),
    /// Must match the receiving port's role.
    pub role: EdgeRole,
}

/// Candidate resolved topology, independent of persisted project ownership.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pipeline {
    /// All interfaces, including disconnected nodes.
    pub nodes: Vec<PipelineNode>,
    /// All event, audio, control and sidechain dependencies.
    pub edges: Vec<PipelineEdge>,
}

/// Caller-selected budgets checked before allocating validation indexes.
///
/// These bound validator work, not rendered event/sample buffers or project decoding. Callers
/// must separately bound parsing and processor resources. No product capacity default is implied.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PipelineLimits {
    /// Maximum number of nodes.
    pub nodes: usize,
    /// Maximum number of edges.
    pub edges: usize,
    /// Maximum total input/output ports.
    pub ports: usize,
    /// Maximum total parameter descriptors.
    pub parameter_keys: usize,
    /// Maximum total UTF-8 bytes in port keys, parameter keys and port references.
    pub text_bytes: usize,
}

/// Validation of one snapshot; editing the candidate invalidates this result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Validation {
    /// Dependency order, selecting the smallest node ID among ready nodes.
    /// This is not a realtime execution plan.
    pub topological_order: Vec<NodeId>,
}

/// Validation failure with stable affected identities.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PipelineError {
    /// Internal dependency checks disagreed; the candidate must not be installed.
    #[error("pipeline dependency validation could not establish a complete order")]
    InconsistentDependencies,
    /// A supplied resource budget was exceeded.
    #[error("pipeline {resource} count {found} exceeds limit {limit}")]
    LimitExceeded {
        /// Resource exceeding its budget.
        resource: &'static str,
        /// Count, saturating at `usize::MAX` on overflow.
        found: usize,
        /// Supplied budget.
        limit: usize,
    },
    /// Repeated node identity.
    #[error("duplicate pipeline node {0:?}")]
    DuplicateNode(NodeId),
    /// Repeated connection identity.
    #[error("duplicate pipeline edge {0:?}")]
    DuplicateEdge(EdgeId),
    /// Repeated port key, including across opposite directions.
    #[error("duplicate port {port:?} on {node:?}")]
    DuplicatePort {
        /// Owner.
        node: NodeId,
        /// Repeated key.
        port: PortId,
    },
    /// Repeated parameter key within a node.
    #[error("duplicate parameter {key} on {node:?}")]
    DuplicateParameter {
        /// Owner.
        node: NodeId,
        /// Repeated stable key.
        key: String,
    },
    /// Invalid port range or channel layout.
    #[error("invalid range/layout for {port:?} on {node:?}")]
    InvalidPortRange {
        /// Owner.
        node: NodeId,
        /// Invalid port.
        port: PortId,
    },
    /// Invalid key, role or cardinality.
    #[error("invalid port {port:?} on {node:?}: {reason}")]
    InvalidPort {
        /// Owner.
        node: NodeId,
        /// Invalid port.
        port: PortId,
        /// Developer-facing explanation.
        reason: &'static str,
    },
    /// Invalid parameter key or driver-port reference, including aliases.
    #[error("invalid parameter {key} on {node:?}: {reason}")]
    InvalidParameter {
        /// Owner.
        node: NodeId,
        /// Stable key.
        key: String,
        /// Developer-facing explanation.
        reason: &'static str,
    },
    /// Node absent from the candidate.
    #[error("edge {edge:?} names missing node {node:?}")]
    MissingNode {
        /// Invalid connection.
        edge: EdgeId,
        /// Absent node.
        node: NodeId,
    },
    /// Port absent from both directions of its node.
    #[error("edge {edge:?} names missing port {port:?} on {node:?}")]
    MissingPort {
        /// Invalid connection.
        edge: EdgeId,
        /// Referenced node.
        node: NodeId,
        /// Absent key.
        port: PortId,
    },
    /// Existing port used in the wrong direction.
    #[error("edge {edge:?} uses {port:?} on {node:?} in the wrong direction")]
    WrongDirection {
        /// Invalid connection.
        edge: EdgeId,
        /// Referenced node.
        node: NodeId,
        /// Port used backwards.
        port: PortId,
    },
    /// Endpoint data types, layouts or domains disagree.
    #[error("incompatible endpoint types on edge {edge:?}")]
    TypeMismatch {
        /// Incompatible connection.
        edge: EdgeId,
    },
    /// Edge role disagrees with the receiving port.
    #[error("incompatible role on edge {edge:?}")]
    RoleMismatch {
        /// Incompatible connection.
        edge: EdgeId,
    },
    /// A non-summing input has two absolute drivers.
    #[error("conflicting edges {first:?} and {second:?} at {node:?}/{port:?}")]
    ConflictingDrivers {
        /// Consuming node.
        node: NodeId,
        /// Input with conflicting drivers.
        port: PortId,
        /// First observed connection.
        first: EdgeId,
        /// Conflicting connection.
        second: EdgeId,
    },
    /// A real closed dependency path, across any edge domains.
    #[error("pipeline cycle: nodes {nodes:?}, edges {edges:?}")]
    Cycle {
        /// Path with identical first and last node.
        nodes: Vec<NodeId>,
        /// Each edge connects the corresponding consecutive nodes.
        edges: Vec<EdgeId>,
    },
    /// No resolved parameter with this key.
    #[error("unknown parameter {key} on {node:?}")]
    UnknownParameter {
        /// Requested node.
        node: NodeId,
        /// Requested key.
        key: String,
    },
    /// A processor output, not an editable parameter.
    #[error("read-only parameter {key} on {node:?}")]
    ReadOnlyParameter {
        /// Owner.
        node: NodeId,
        /// Read-only key.
        key: String,
    },
    /// Parameter cannot consume an automation stream.
    #[error("unsupported automation for {node:?}/{key}: {policy:?}")]
    UnsupportedAutomation {
        /// Owner.
        node: NodeId,
        /// Stable key.
        key: String,
        /// Policy requiring a static edit or explicit command.
        policy: AutomationPolicy,
    },
}

impl Pipeline {
    /// Queries policy on an already validated descriptor snapshot.
    ///
    /// This is a policy query, not graph validation. Unknown, read-only, preparation-only,
    /// regeneration and structural parameters return an error.
    pub fn validate_automation(
        &self,
        node: NodeId,
        key: &str,
    ) -> Result<AutomationPolicy, PipelineError> {
        let parameter = self
            .nodes
            .iter()
            .find(|owner| owner.id == node)
            .and_then(|owner| {
                owner
                    .parameters
                    .iter()
                    .find(|parameter| parameter.key == key)
            })
            .ok_or_else(|| PipelineError::UnknownParameter {
                node,
                key: key.to_owned(),
            })?;
        parameter.automation_policy(node)
    }

    /// Checks budgets, references, port/parameter contracts, drivers and all dependencies.
    ///
    /// Returns the first structured error without changing the candidate. Budgets are checked
    /// before building indexes. Success includes all nodes in deterministic dependency order.
    pub fn validate(&self, limits: PipelineLimits) -> Result<Validation, PipelineError> {
        validation::validate(self, limits)
    }
}

impl Parameter {
    fn automation_policy(&self, node: NodeId) -> Result<AutomationPolicy, PipelineError> {
        if self.read_only {
            return Err(PipelineError::ReadOnlyParameter {
                node,
                key: self.key.clone(),
            });
        }
        match self.policy {
            AutomationPolicy::Continuous | AutomationPolicy::EventSampled => Ok(self.policy),
            policy => Err(PipelineError::UnsupportedAutomation {
                node,
                key: self.key.clone(),
                policy,
            }),
        }
    }
}
