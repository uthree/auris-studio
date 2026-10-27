//! Pure capability resolution for native processors.
//!
//! Persisted processor payloads contain ownership and state, not an executable interface. This
//! module converts that state into a [`PipelineNode`] using metadata supplied by the owning
//! plugin host. The provider is deliberately a runtime boundary and is never serialized.

use crate::param::{MixerControl, ParamDescriptor};

use super::{PipelineContent, Processor, ProcessorKind, SourceContent};
use crate::PluginKind;
use crate::pipeline::{
    AudioLayout, AutomationPolicy, ControlDomain, Parameter, PipelineLimits, PipelineNode, Port,
    PortType,
};

/// One plugin parameter and its resolved automation contract.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginParameter {
    /// Stable descriptor, including its persistent key and native range.
    pub descriptor: ParamDescriptor,
    /// Domain exposed by the plugin host for graph control.
    pub domain: ControlDomain,
    /// Policy governing whether a graph driver may change the parameter.
    pub policy: AutomationPolicy,
}

/// Runtime plugin metadata borrowed by the capability resolver.
#[derive(Clone, Copy, Debug)]
pub struct PluginCapabilities<'a> {
    /// Identifier of the instance this metadata belongs to.
    pub plugin_id: &'a str,
    /// Whether the instance is an instrument or an effect.
    pub kind: PluginKind,
    /// Stable parameter descriptors owned by the host.
    pub parameters: &'a [PluginParameter],
    /// Whether the effect exposes an audio sidechain input.
    pub sidechain: bool,
}

/// Runtime owner of plugin capability metadata.
pub trait CapabilityProvider {
    /// Returns metadata for a processor, or `None` when it is unavailable.
    fn plugin<'a>(&'a self, processor: &Processor) -> Option<PluginCapabilities<'a>>;
}

/// Capability-resolution failures. Persisted payloads remain untouched on every error.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CapabilityError {
    /// A source node refers to a missing authored source.
    #[error("source node {node:?} refers to missing source {source_id:?}")]
    MissingSource {
        /// Source node whose authored source was unavailable.
        node: crate::pipeline::NodeId,
        /// Referenced authored source identity.
        source_id: super::SourceDocumentId,
    },
    /// A plugin instance has no host metadata.
    #[error("plugin metadata is unavailable for node {node:?}")]
    MissingPlugin {
        /// Processor whose metadata was unavailable.
        node: crate::pipeline::NodeId,
    },
    /// Provider metadata belongs to another plugin instance.
    #[error("plugin metadata {provided:?} does not match {expected:?} on node {node:?}")]
    PluginIdMismatch {
        /// Processor whose metadata was returned.
        node: crate::pipeline::NodeId,
        /// ID stored in the processor payload.
        expected: String,
        /// ID returned by the provider.
        provided: String,
    },
    /// Provider metadata has the wrong processor role.
    #[error("plugin kind does not match node {node:?}")]
    PluginKindMismatch {
        /// Processor whose role did not match provider metadata.
        node: crate::pipeline::NodeId,
    },
    /// A parameter descriptor cannot be used as a graph control.
    #[error("invalid parameter {key:?} on node {node:?}: {reason}")]
    InvalidParameter {
        /// Processor owning the invalid descriptor.
        node: crate::pipeline::NodeId,
        /// Descriptor key, if available.
        key: String,
        /// Validation reason.
        reason: &'static str,
    },
    /// The caller-supplied per-node validation budget was exceeded.
    #[error("capability {0} exceeds its validation budget")]
    Limit(&'static str),
    /// Preserved legacy automation has no authoritative value domain yet.
    #[error("automation curve {0:?} has an unresolved value domain")]
    UnresolvedAutomation(super::AutomationId),
    /// An automation processor refers to a missing curve.
    #[error("automation node {node:?} refers to missing curve {automation:?}")]
    MissingAutomation {
        /// Processor whose curve was unavailable.
        node: crate::pipeline::NodeId,
        /// Referenced curve identity.
        automation: super::AutomationId,
    },
}

/// Resolves one persisted processor into its pure topology interface.
pub fn resolve_node<P: CapabilityProvider>(
    content: &PipelineContent,
    processor: &Processor,
    provider: &P,
    limits: PipelineLimits,
) -> Result<PipelineNode, CapabilityError> {
    let audio = || Port::new("audio_in", PortType::Audio(AudioLayout::Stereo));
    let audio_out = || Port::new("audio_out", PortType::Audio(AudioLayout::Stereo));
    let events_in = || Port::new("events_in", PortType::Events);
    let events_out = || Port::new("events_out", PortType::Events);
    let mut node = PipelineNode::new(processor.id);

    if limits.nodes < 1 {
        return Err(CapabilityError::Limit("nodes"));
    }
    match &processor.kind {
        ProcessorKind::Source(source) => {
            let source = content
                .sources
                .iter()
                .find(|candidate| candidate.id == *source)
                .ok_or(CapabilityError::MissingSource {
                    node: processor.id,
                    source_id: *source,
                })?;
            match &source.content {
                SourceContent::Notes(_) => node.outputs.push(events_out()),
                SourceContent::Audio(_) => node.outputs.push(audio_out()),
            }
        }
        ProcessorKind::Transform(_) => {
            node.inputs.push(events_in());
            node.outputs.push(events_out());
        }
        ProcessorKind::Instrument(instance) => {
            let capabilities = plugin_for(
                processor,
                &instance.plugin_id,
                PluginKind::Instrument,
                provider,
                &limits,
            )?;
            check_plugin_limits(capabilities.parameters, false, 18, &limits)?;
            node.inputs.push(events_in());
            node.outputs.push(audio_out());
            add_parameters(&mut node, capabilities.parameters, processor.id)?;
        }
        ProcessorKind::Singer(_) => {
            node.inputs.push(events_in());
            node.inputs
                .push(Port::new("guide_in", PortType::Audio(AudioLayout::Stereo)));
            node.outputs.push(audio_out());
        }
        ProcessorKind::Effect { plugin, .. } => {
            let capabilities = plugin_for(
                processor,
                &plugin.plugin_id,
                PluginKind::Effect,
                provider,
                &limits,
            )?;
            check_plugin_limits(capabilities.parameters, capabilities.sidechain, 17, &limits)?;
            node.inputs.push(audio());
            if capabilities.sidechain {
                node.inputs
                    .push(Port::sidechain_input("sidechain_in", AudioLayout::Stereo));
            }
            node.outputs.push(audio_out());
            add_parameters(&mut node, capabilities.parameters, processor.id)?;
        }
        ProcessorKind::Strip(_) => {
            node.inputs.push(audio());
            let gain = MixerControl::Gain.descriptor();
            let pan = MixerControl::Pan.descriptor();
            node.inputs.push(Port::new(
                format!("param:{}", gain.key),
                PortType::Control(domain_from_descriptor(&gain)),
            ));
            node.inputs.push(Port::new(
                format!("param:{}", pan.key),
                PortType::Control(domain_from_descriptor(&pan)),
            ));
            node.outputs
                .push(Port::new("pre_fader", PortType::Audio(AudioLayout::Stereo)));
            node.outputs.push(audio_out());
            node.parameters.push(parameter(
                &gain.key,
                AutomationPolicy::Continuous,
                false,
                true,
            ));
            node.parameters.push(parameter(
                &pan.key,
                AutomationPolicy::Continuous,
                false,
                true,
            ));
        }
        ProcessorKind::Send { .. } => {
            node.inputs.push(audio());
            let send = MixerControl::Send.descriptor();
            node.inputs.push(Port::new(
                format!("param:{}", send.key),
                PortType::Control(domain_from_descriptor(&send)),
            ));
            node.outputs.push(audio_out());
            node.parameters.push(parameter(
                &send.key,
                AutomationPolicy::Continuous,
                false,
                true,
            ));
        }
        ProcessorKind::Sum => {
            node.inputs
                .push(Port::sum_input("audio_in", AudioLayout::Stereo));
            node.outputs.push(audio_out());
        }
        ProcessorKind::Output => node.inputs.push(audio()),
        ProcessorKind::Automation(automation) => {
            let curve = content
                .automation
                .iter()
                .find(|curve| curve.id == *automation)
                .ok_or(CapabilityError::MissingAutomation {
                    node: processor.id,
                    automation: *automation,
                })?;
            let domain = curve
                .domain
                .ok_or(CapabilityError::UnresolvedAutomation(*automation))?;
            node.outputs
                .push(Port::new("control_out", PortType::Control(domain)));
        }
    }
    check_node_limits(&node, &limits)?;
    Ok(node)
}

fn parameter(
    key: &str,
    policy: AutomationPolicy,
    read_only: bool,
    expose_control: bool,
) -> Parameter {
    Parameter {
        key: key.to_owned(),
        port: expose_control.then(|| crate::pipeline::PortId::new(format!("param:{key}"))),
        policy,
        read_only,
    }
}

fn domain_from_descriptor(descriptor: &ParamDescriptor) -> ControlDomain {
    ControlDomain::Native {
        unit: descriptor.unit,
        min: descriptor.min,
        max: descriptor.max,
    }
}

fn check_plugin_limits(
    parameters: &[PluginParameter],
    sidechain: bool,
    signal_port_bytes: usize,
    limits: &PipelineLimits,
) -> Result<(), CapabilityError> {
    if parameters.len() > limits.parameter_keys {
        return Err(CapabilityError::Limit("parameter_keys"));
    }
    let exposed = parameters
        .iter()
        .filter(|parameter| {
            !parameter.descriptor.read_only
                && matches!(
                    parameter.policy,
                    AutomationPolicy::Continuous | AutomationPolicy::EventSampled
                )
                && !(parameter.descriptor.min == parameter.descriptor.max
                    && parameter.descriptor.default == parameter.descriptor.min)
        })
        .count();
    let ports = exposed + usize::from(sidechain) + 2;
    if ports > limits.ports {
        return Err(CapabilityError::Limit("ports"));
    }
    let fixed_bytes = signal_port_bytes.saturating_add(if sidechain { 12 } else { 0 });
    let text_bytes = parameters.iter().fold(fixed_bytes, |sum, parameter| {
        let controls = if parameter.descriptor.read_only
            || !matches!(
                parameter.policy,
                AutomationPolicy::Continuous | AutomationPolicy::EventSampled
            )
            || (parameter.descriptor.min == parameter.descriptor.max
                && parameter.descriptor.default == parameter.descriptor.min)
        {
            1
        } else {
            3
        };
        sum.saturating_add(
            parameter
                .descriptor
                .key
                .len()
                .saturating_mul(controls)
                .saturating_add(if controls == 3 { 12 } else { 0 }),
        )
    });
    if text_bytes > limits.text_bytes {
        return Err(CapabilityError::Limit("text_bytes"));
    }
    Ok(())
}

fn check_node_limits(node: &PipelineNode, limits: &PipelineLimits) -> Result<(), CapabilityError> {
    let ports = node.inputs.len().saturating_add(node.outputs.len());
    if ports > limits.ports {
        return Err(CapabilityError::Limit("ports"));
    }
    if node.parameters.len() > limits.parameter_keys {
        return Err(CapabilityError::Limit("parameter_keys"));
    }
    let text_bytes = node
        .inputs
        .iter()
        .chain(&node.outputs)
        .fold(0usize, |sum, port| sum.saturating_add(port.id.0.len()))
        .saturating_add(node.parameters.iter().fold(0usize, |sum, parameter| {
            sum.saturating_add(parameter.key.len())
                .saturating_add(parameter.port.as_ref().map_or(0, |port| port.0.len()))
        }));
    if text_bytes > limits.text_bytes {
        return Err(CapabilityError::Limit("text_bytes"));
    }
    Ok(())
}

fn plugin_for<'a, P: CapabilityProvider>(
    processor: &Processor,
    expected_id: &str,
    expected_kind: PluginKind,
    provider: &'a P,
    limits: &PipelineLimits,
) -> Result<PluginCapabilities<'a>, CapabilityError> {
    let capabilities = provider
        .plugin(processor)
        .ok_or(CapabilityError::MissingPlugin { node: processor.id })?;
    if expected_id
        .len()
        .saturating_add(capabilities.plugin_id.len())
        > limits.text_bytes
    {
        return Err(CapabilityError::Limit("text_bytes"));
    }
    if capabilities.plugin_id != expected_id {
        return Err(CapabilityError::PluginIdMismatch {
            node: processor.id,
            expected: expected_id.to_owned(),
            provided: capabilities.plugin_id.to_owned(),
        });
    }
    if capabilities.kind != expected_kind {
        return Err(CapabilityError::PluginKindMismatch { node: processor.id });
    }
    Ok(capabilities)
}

fn add_parameters(
    node: &mut PipelineNode,
    parameters: &[PluginParameter],
    node_id: crate::pipeline::NodeId,
) -> Result<(), CapabilityError> {
    for plugin_parameter in parameters {
        let descriptor = &plugin_parameter.descriptor;
        if descriptor.key.is_empty() {
            return Err(CapabilityError::InvalidParameter {
                node: node_id,
                key: String::new(),
                reason: "key is empty",
            });
        }
        if !descriptor.min.is_finite()
            || !descriptor.max.is_finite()
            || !descriptor.default.is_finite()
        {
            return Err(CapabilityError::InvalidParameter {
                node: node_id,
                key: descriptor.key.to_string(),
                reason: "range/default is not finite",
            });
        }
        let static_range = descriptor.min == descriptor.max && descriptor.default == descriptor.min;
        if (!static_range && descriptor.min >= descriptor.max)
            || descriptor.default < descriptor.min
            || descriptor.default > descriptor.max
        {
            return Err(CapabilityError::InvalidParameter {
                node: node_id,
                key: descriptor.key.to_string(),
                reason: "range/default is invalid",
            });
        }
        match plugin_parameter.domain {
            ControlDomain::Native { unit, min, max } => {
                if unit != descriptor.unit || min != descriptor.min || max != descriptor.max {
                    return Err(CapabilityError::InvalidParameter {
                        node: node_id,
                        key: descriptor.key.to_string(),
                        reason: "native domain does not match descriptor",
                    });
                }
            }
            ControlDomain::Normalized { min, max } => {
                if min != 0.0 || max != 1.0 || descriptor.min != 0.0 || descriptor.max != 1.0 {
                    return Err(CapabilityError::InvalidParameter {
                        node: node_id,
                        key: descriptor.key.to_string(),
                        reason: "normalized domain must be exactly 0..1",
                    });
                }
            }
        }
        let expose_control = !descriptor.read_only
            && matches!(
                plugin_parameter.policy,
                AutomationPolicy::Continuous | AutomationPolicy::EventSampled
            )
            && !static_range;
        if expose_control {
            node.inputs.push(Port::new(
                format!("param:{}", descriptor.key),
                PortType::Control(plugin_parameter.domain),
            ));
        }
        node.parameters.push(parameter(
            &descriptor.key,
            plugin_parameter.policy,
            descriptor.read_only,
            expose_control,
        ));
    }
    Ok(())
}
