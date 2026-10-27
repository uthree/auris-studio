//! Ownership and runtime metadata resolved into one validated dependency graph.

use std::collections::BTreeMap;

use super::{
    CapabilityError, CapabilityProvider, ContentError, ContentLimits, ParameterAddress,
    PipelineContent, ProcessorKind, resolve_node,
};
use crate::Ticks;
use crate::pipeline::{
    EdgeId, EdgeRole, NodeId, Pipeline, PipelineEdge, PipelineError, PipelineLimits, PortId,
    Validation,
};

/// Failure before a native candidate can become a prepared execution plan.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ResolutionError {
    /// Invalid saved ownership or curve data.
    #[error(transparent)]
    Content(#[from] ContentError),
    /// Missing or invalid processor metadata.
    #[error(transparent)]
    Capability(#[from] CapabilityError),
    /// Invalid resolved topology or automation policy.
    #[error(transparent)]
    Topology(#[from] PipelineError),
    /// A control connection has incompatible source and target value domains.
    #[error("automation binding {0:?} requires an explicit value-domain mapping")]
    BindingDomain(EdgeId),
    /// A parameter has metadata but no usable control input.
    #[error("automation binding {0:?} has no resolved control input")]
    BindingPort(EdgeId),
    /// Resolution exceeded a caller-supplied budget before cloning unbounded data.
    #[error("native resolution exceeds its {0} budget")]
    Limit(&'static str),
}

/// A resolved graph borrowing the immutable ownership snapshot it describes.
///
/// This is preparation data, not a realtime execution plan. Disabled bindings are validated
/// but contribute no dependency, driver or effective automation value.
#[derive(Debug)]
pub struct ResolvedContent<'a> {
    content: &'a PipelineContent,
    pipeline: Pipeline,
    validation: Validation,
}

impl ResolvedContent<'_> {
    /// Resolved interfaces and active connections from this snapshot.
    pub fn pipeline(&self) -> &Pipeline {
        &self.pipeline
    }

    /// Stable dependency order across every active edge domain.
    pub fn topological_order(&self) -> &[NodeId] {
        &self.validation.topological_order
    }

    /// Reads project-time automation during preparation or inspection.
    ///
    /// `None` means the caller must use the processor's stored static value. This lookup does
    /// not infer clip-local time or follow a moved clip; all persisted points use project ticks.
    pub fn automation_value(&self, target: &ParameterAddress, tick: Ticks) -> Option<f32> {
        let binding = self
            .content
            .automation_bindings
            .iter()
            .find(|binding| binding.enabled && binding.target == *target)?;
        let node = self
            .content
            .processors
            .iter()
            .find(|node| node.id == binding.source)?;
        let ProcessorKind::Automation(id) = node.kind else {
            return None;
        };
        self.content
            .automation
            .iter()
            .find(|curve| curve.id == id)
            .map(|curve| curve.value_at(tick))
    }
}

impl PipelineContent {
    /// Validates ownership, resolves processor-owned capabilities, then validates the full DAG.
    ///
    /// Both enabled and disabled bindings must name valid, automatable, domain-compatible
    /// parameters. Only enabled bindings compete for the target's single absolute driver.
    /// Missing plugin metadata fails explicitly without changing the saved state or loading IO.
    pub fn resolve_pipeline<P: CapabilityProvider>(
        &self,
        external_ids: &[u64],
        content_limits: ContentLimits,
        graph_limits: PipelineLimits,
        provider: &P,
    ) -> Result<ResolvedContent<'_>, ResolutionError> {
        self.validate_ownership(external_ids, content_limits)?;
        if self.processors.len() > graph_limits.nodes {
            return Err(ResolutionError::Limit("nodes"));
        }
        if self
            .edges
            .len()
            .saturating_add(self.automation_bindings.len())
            > graph_limits.edges
        {
            return Err(ResolutionError::Limit("edges"));
        }
        let mut remaining = graph_limits;
        // Disabled bindings still spend validation work and text budget.
        let edge_text = self.edges.iter().fold(0usize, |sum, edge| {
            sum.saturating_add(edge.from.1.0.len())
                .saturating_add(edge.to.1.0.len())
        });
        let binding_text = self
            .automation_bindings
            .iter()
            .fold(0usize, |sum, binding| {
                sum.saturating_add(binding.target.key.len())
                    .saturating_add(17)
            });
        remaining.text_bytes = remaining
            .text_bytes
            .checked_sub(edge_text.saturating_add(binding_text))
            .ok_or(ResolutionError::Limit("text_bytes"))?;
        let mut pipeline = Pipeline {
            nodes: Vec::with_capacity(self.processors.len()),
            edges: Vec::new(),
        };
        for processor in &self.processors {
            let node = resolve_node(self, processor, provider, remaining)?;
            let ports = node.inputs.len().saturating_add(node.outputs.len());
            let text = node
                .inputs
                .iter()
                .chain(&node.outputs)
                .fold(0usize, |sum, port| sum.saturating_add(port.id.0.len()))
                .saturating_add(node.parameters.iter().fold(0usize, |sum, parameter| {
                    sum.saturating_add(parameter.key.len())
                        .saturating_add(parameter.port.as_ref().map_or(0, |port| port.0.len()))
                }));
            remaining.nodes = remaining
                .nodes
                .checked_sub(1)
                .ok_or(ResolutionError::Limit("nodes"))?;
            remaining.ports = remaining
                .ports
                .checked_sub(ports)
                .ok_or(ResolutionError::Limit("ports"))?;
            remaining.parameter_keys = remaining
                .parameter_keys
                .checked_sub(node.parameters.len())
                .ok_or(ResolutionError::Limit("parameter_keys"))?;
            remaining.text_bytes = remaining
                .text_bytes
                .checked_sub(text)
                .ok_or(ResolutionError::Limit("text_bytes"))?;
            pipeline.nodes.push(node);
        }
        pipeline.edges = self.edges.clone();
        // Validate descriptors before resolving parameter references, including disconnected nodes.
        pipeline.validate(graph_limits)?;
        let indexes: BTreeMap<_, _> = pipeline
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.id, index))
            .collect();
        for binding in &self.automation_bindings {
            pipeline.validate_automation(binding.target.node, &binding.target.key)?;
            let target = indexes
                .get(&binding.target.node)
                .and_then(|index| pipeline.nodes.get(*index))
                .ok_or(ResolutionError::BindingPort(binding.id))?;
            let parameter = target
                .parameters
                .iter()
                .find(|parameter| parameter.key == binding.target.key)
                .ok_or(ResolutionError::BindingPort(binding.id))?;
            let target_port = parameter
                .port
                .as_ref()
                .and_then(|port| target.inputs.iter().find(|input| input.id == *port))
                .ok_or(ResolutionError::BindingPort(binding.id))?;
            let source = indexes
                .get(&binding.source)
                .and_then(|index| pipeline.nodes.get(*index))
                .and_then(|node| node.outputs.iter().find(|port| port.id.0 == "control_out"))
                .ok_or(ResolutionError::BindingPort(binding.id))?;
            if source.ty != target_port.ty {
                return Err(ResolutionError::BindingDomain(binding.id));
            }
            if binding.enabled {
                pipeline.edges.push(PipelineEdge {
                    id: binding.id,
                    from: (binding.source, PortId::new("control_out")),
                    to: (binding.target.node, target_port.id.clone()),
                    role: EdgeRole::Control,
                });
            }
        }
        let validation = pipeline.validate(graph_limits)?;
        Ok(ResolvedContent {
            content: self,
            pipeline,
            validation,
        })
    }
}
