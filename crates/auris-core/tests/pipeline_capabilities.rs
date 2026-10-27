use auris_core::PluginKind;
use auris_core::automation::{AutomationCurve, AutomationPoint};
use auris_core::param::{ParamDescriptor, ParamUnit};
use auris_core::pipeline::content::{
    AutomationDocument, AutomationId, CapabilityError, CapabilityProvider, GroupId,
    PipelineContent, PluginCapabilities, PluginInstance, PluginParameter, Processor, ProcessorKind,
    SourceContent, SourceDocument, SourceDocumentId, resolve_node,
};
use auris_core::pipeline::{
    AutomationPolicy, ControlDomain, NodeId, Pipeline, PipelineNode, PortType,
};
use auris_core::project::{MidiClip, NoteTransform, StageId};
use auris_core::{ClipId, PluginState, Ticks};

fn limits() -> auris_core::pipeline::PipelineLimits {
    auris_core::pipeline::PipelineLimits {
        nodes: 1,
        edges: 0,
        ports: 16,
        parameter_keys: 16,
        text_bytes: 4096,
    }
}

struct Provider {
    plugin_id: String,
    kind: PluginKind,
    parameters: Vec<PluginParameter>,
    sidechain: bool,
}

impl CapabilityProvider for Provider {
    fn plugin<'a>(&'a self, _processor: &Processor) -> Option<PluginCapabilities<'a>> {
        Some(PluginCapabilities {
            plugin_id: &self.plugin_id,
            kind: self.kind,
            parameters: &self.parameters,
            sidechain: self.sidechain,
        })
    }
}

fn plugin(id: &str, kind: PluginKind, parameters: Vec<PluginParameter>) -> (Provider, Processor) {
    (
        Provider {
            plugin_id: id.into(),
            kind,
            parameters,
            sidechain: true,
        },
        Processor {
            id: NodeId(10),
            group: None,
            kind: match kind {
                PluginKind::Instrument => ProcessorKind::Instrument(PluginInstance {
                    plugin_id: id.into(),
                    state: PluginState::default(),
                    file: None,
                }),
                PluginKind::Effect => ProcessorKind::Effect {
                    slot: auris_core::EffectSlotId(11),
                    plugin: PluginInstance {
                        plugin_id: id.into(),
                        state: PluginState::default(),
                        file: None,
                    },
                    enabled: true,
                },
            },
        },
    )
}

fn parameter(key: &'static str, min: f32, max: f32, unit: ParamUnit) -> PluginParameter {
    PluginParameter {
        descriptor: ParamDescriptor::new(0u32, key, key, min, max, min).with_unit(unit),
        domain: ControlDomain::Native { unit, min, max },
        policy: AutomationPolicy::Continuous,
    }
}

fn empty_content() -> PipelineContent {
    PipelineContent::new()
}

#[test]
fn source_kind_controls_event_or_audio_output() {
    let mut content = empty_content();
    let mut clip = MidiClip::new(ClipId(20), "notes", Ticks::ZERO, Ticks(480));
    clip.transforms.push(NoteTransform::Lean { ticks: 0 });
    content.sources.push(SourceDocument {
        id: SourceDocumentId(21),
        content: SourceContent::Notes(vec![clip.with_stage_ids(vec![StageId(22)]).unwrap()]),
    });
    let processor = Processor {
        id: NodeId(23),
        group: None,
        kind: ProcessorKind::Source(SourceDocumentId(21)),
    };
    let node = resolve_node(
        &content,
        &processor,
        &Provider {
            plugin_id: String::new(),
            kind: PluginKind::Instrument,
            parameters: vec![],
            sidechain: false,
        },
        limits(),
    )
    .unwrap();
    assert!(matches!(node.outputs[0].ty, PortType::Events));
}

#[test]
fn effect_ports_include_sidechain_and_namespaced_parameter() {
    let (mut provider, processor) = plugin(
        "delay",
        PluginKind::Effect,
        vec![parameter("mix", 0.0, 1.0, ParamUnit::Plain)],
    );
    provider.sidechain = true;
    let node = resolve_node(&empty_content(), &processor, &provider, limits()).unwrap();
    assert_eq!(
        node.inputs
            .iter()
            .map(|port| port.id.0.as_str())
            .collect::<Vec<_>>(),
        vec!["audio_in", "sidechain_in", "param:mix"]
    );
    assert_eq!(node.parameters[0].port.as_ref().unwrap().0, "param:mix");
}

#[test]
fn plugin_metadata_must_match_instance_and_kind() {
    let (mut provider, processor) = plugin("synth", PluginKind::Instrument, vec![]);
    provider.plugin_id = "other".into();
    assert!(matches!(
        resolve_node(&empty_content(), &processor, &provider, limits()),
        Err(CapabilityError::PluginIdMismatch { .. })
    ));
    provider.plugin_id = "synth".into();
    provider.kind = PluginKind::Effect;
    assert!(matches!(
        resolve_node(&empty_content(), &processor, &provider, limits()),
        Err(CapabilityError::PluginKindMismatch { .. })
    ));
}

#[test]
fn invalid_read_only_and_static_metadata_is_preserved_for_pipeline_validation() {
    let mut parameter = parameter("meter", 0.0, 1.0, ParamUnit::Plain);
    parameter.descriptor.read_only = true;
    parameter.policy = AutomationPolicy::PreparationOnly;
    let (provider, processor) = plugin("synth", PluginKind::Instrument, vec![parameter]);
    let node = resolve_node(&empty_content(), &processor, &provider, limits()).unwrap();
    assert!(node.parameters[0].read_only);
    assert_eq!(node.parameters[0].policy, AutomationPolicy::PreparationOnly);
}

#[test]
fn degenerate_native_descriptor_is_metadata_only() {
    let parameter = PluginParameter {
        descriptor: ParamDescriptor::new(0u32, "mode", "Mode", 1.0, 1.0, 1.0),
        domain: ControlDomain::Native {
            unit: ParamUnit::Plain,
            min: 1.0,
            max: 1.0,
        },
        policy: AutomationPolicy::PreparationOnly,
    };
    let (provider, processor) = plugin("synth", PluginKind::Instrument, vec![parameter]);
    let node = resolve_node(&empty_content(), &processor, &provider, limits()).unwrap();
    assert_eq!(node.parameters[0].key, "mode");
    assert!(node.parameters[0].port.is_none());
    assert!(node.inputs.iter().all(|port| port.id.0 != "param:mode"));
}

#[test]
fn normalized_domain_is_explicitly_zero_to_one() {
    let parameter = PluginParameter {
        descriptor: ParamDescriptor::new(0u32, "cutoff", "Cutoff", 0.0, 1.0, 0.5),
        domain: ControlDomain::Normalized { min: 0.0, max: 1.0 },
        policy: AutomationPolicy::Continuous,
    };
    let (provider, processor) = plugin("synth", PluginKind::Instrument, vec![parameter]);
    let node = resolve_node(&empty_content(), &processor, &provider, limits()).unwrap();
    assert!(matches!(
        node.inputs
            .iter()
            .find(|port| port.id.0 == "param:cutoff")
            .unwrap()
            .ty,
        PortType::Control(ControlDomain::Normalized { .. })
    ));
}

#[test]
fn malformed_domain_is_rejected_before_capability_is_published() {
    let mut parameter = parameter("cutoff", 20.0, 20_000.0, ParamUnit::Hertz);
    parameter.domain = ControlDomain::Normalized { min: 0.0, max: 1.0 };
    let (provider, processor) = plugin("synth", PluginKind::Instrument, vec![parameter]);
    assert!(matches!(
        resolve_node(&empty_content(), &processor, &provider, limits()),
        Err(CapabilityError::InvalidParameter { .. })
    ));
}

#[test]
fn builtin_nodes_have_stable_audio_and_control_keys() {
    let content = empty_content();
    let strip = Processor {
        id: NodeId(2),
        group: Some(GroupId(1)),
        kind: ProcessorKind::Strip(Default::default()),
    };
    let node: PipelineNode = resolve_node(
        &content,
        &strip,
        &Provider {
            plugin_id: String::new(),
            kind: PluginKind::Instrument,
            parameters: vec![],
            sidechain: false,
        },
        limits(),
    )
    .unwrap();
    assert!(node.inputs.iter().any(|port| port.id.0 == "param:gain"));
    assert!(node.inputs.iter().any(|port| port.id.0 == "param:pan"));
    assert!(node.outputs.iter().any(|port| port.id.0 == "pre_fader"));
}

#[test]
fn every_builtin_capability_is_accepted_by_topology_validation() {
    let mut content = empty_content();
    let mut clip = MidiClip::new(ClipId(20), "notes", Ticks::ZERO, Ticks(480));
    clip.transforms.push(NoteTransform::Lean { ticks: 0 });
    content.sources.push(SourceDocument {
        id: SourceDocumentId(21),
        content: SourceContent::Notes(vec![clip.with_stage_ids(vec![StageId(22)]).unwrap()]),
    });
    content.automation.push(
        AutomationDocument::new(
            AutomationId(30),
            ControlDomain::Native {
                unit: ParamUnit::Plain,
                min: 0.0,
                max: 1.0,
            },
            AutomationCurve::Linear,
            vec![AutomationPoint::new(Ticks::ZERO, 0.5)],
        )
        .unwrap(),
    );
    let processors = vec![
        Processor {
            id: NodeId(31),
            group: None,
            kind: ProcessorKind::Source(SourceDocumentId(21)),
        },
        Processor {
            id: NodeId(32),
            group: None,
            kind: ProcessorKind::Transform(NoteTransform::Lean { ticks: 0 }),
        },
        Processor {
            id: NodeId(33),
            group: None,
            kind: ProcessorKind::Singer(Box::new(auris_core::pipeline::content::SingerInstance {
                frame_hop: 0.01,
                voice: None,
                take: None,
            })),
        },
        Processor {
            id: NodeId(34),
            group: None,
            kind: ProcessorKind::Strip(Default::default()),
        },
        Processor {
            id: NodeId(35),
            group: None,
            kind: ProcessorKind::Send {
                send: auris_core::SendId(36),
                level_db: 0.0,
            },
        },
        Processor {
            id: NodeId(37),
            group: None,
            kind: ProcessorKind::Sum,
        },
        Processor {
            id: NodeId(38),
            group: None,
            kind: ProcessorKind::Output,
        },
        Processor {
            id: NodeId(39),
            group: None,
            kind: ProcessorKind::Automation(AutomationId(30)),
        },
    ];
    let provider = Provider {
        plugin_id: "synth".into(),
        kind: PluginKind::Instrument,
        parameters: vec![],
        sidechain: false,
    };
    let mut nodes = Vec::new();
    for processor in &processors {
        let node = resolve_node(&content, processor, &provider, limits()).unwrap();
        nodes.push(node);
    }
    Pipeline {
        nodes,
        edges: Vec::new(),
    }
    .validate(auris_core::pipeline::PipelineLimits {
        nodes: 16,
        edges: 0,
        ports: 64,
        parameter_keys: 64,
        text_bytes: 4096,
    })
    .unwrap();
}
