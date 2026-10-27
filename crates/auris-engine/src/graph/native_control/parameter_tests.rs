use std::borrow::Cow;
use std::collections::BTreeMap;

use auris_core::AudioBuffer;
use auris_core::ParamId;
use auris_core::param::ParamDescriptor;
use auris_core::pipeline::content::PluginParameter;
use auris_core::pipeline::{AutomationPolicy, ControlDomain, NodeId};
use auris_core::plugin::{
    Effect, Parameterized, PluginCategory, PluginDescriptor, PrepareContext, ProcessContext,
};
use auris_core::project::{AudioSourceBank, EffectSlot, EffectSlotId, Project};

use crate::command::NativeNodeCommand;
use crate::graph::RenderGraph;
use crate::native::{NativeRenderInput, NativeRenderTarget, NodePlacedEffects};

struct ParameterFixture {
    descriptors: Vec<ParamDescriptor>,
    values: Vec<f32>,
    mutate_on_prepare: bool,
}

impl ParameterFixture {
    fn new(reordered: bool) -> Self {
        let mut meter = ParamDescriptor::new(
            if reordered { 0u32 } else { 1u32 },
            "meter",
            "Meter",
            0.0,
            1.0,
            0.25,
        );
        meter.read_only = true;
        let gain = ParamDescriptor::new(
            if reordered { 1u32 } else { 0u32 },
            "gain",
            "Gain",
            0.0,
            1.0,
            0.5,
        );
        let descriptors = if reordered {
            vec![meter, gain]
        } else {
            vec![gain, meter]
        };
        let values = descriptors
            .iter()
            .map(|descriptor| descriptor.default)
            .collect();
        Self {
            descriptors,
            values,
            mutate_on_prepare: false,
        }
    }
}

impl Parameterized for ParameterFixture {
    fn parameters(&self) -> &[ParamDescriptor] {
        &self.descriptors
    }

    fn param(&self, id: ParamId) -> f32 {
        self.values.get(id.index()).copied().unwrap_or_default()
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        if let Some(slot) = self.values.get_mut(id.index()) {
            let descriptor = &self.descriptors[id.index()];
            *slot = value.clamp(descriptor.min, descriptor.max);
        }
    }
}

impl Effect for ParameterFixture {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            "native.test.parameter-effect",
            "Native Parameter Effect",
            "Parameter safety fixture",
            PluginCategory::Utility,
        )
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        if self.mutate_on_prepare {
            self.descriptors[0].key = Cow::Borrowed("changed-after-prepare");
        }
    }

    fn reset(&mut self) {}

    fn process(&mut self, _buffer: &mut AudioBuffer, _ctx: &ProcessContext) {}
}

fn project() -> (Project, EffectSlotId) {
    let mut project = Project::new("parameter safety", 48_000.0);
    let slot = EffectSlotId(1);
    project.master.effects.push(EffectSlot {
        id: slot,
        effect_id: "native.test.parameter-effect".into(),
        enabled: true,
        state: Default::default(),
        file: None,
        sidechain: None,
    });
    (project, slot)
}

fn graph(reordered: bool) -> RenderGraph {
    graph_result(reordered, false, AutomationPolicy::Continuous).expect("native parameter graph")
}

fn graph_result(
    reordered: bool,
    mutate_on_prepare: bool,
    policy: AutomationPolicy,
) -> Result<RenderGraph, crate::native::NativeRenderError> {
    let (project, slot) = project();
    let node = NodeId(2);
    let mut fixture = ParameterFixture::new(reordered);
    fixture.mutate_on_prepare = mutate_on_prepare;
    let contracts = BTreeMap::from([(
        node,
        fixture
            .parameters()
            .iter()
            .cloned()
            .map(|descriptor| PluginParameter {
                domain: ControlDomain::Native {
                    unit: descriptor.unit,
                    min: descriptor.min,
                    max: descriptor.max,
                },
                descriptor,
                policy,
            })
            .collect(),
    )]);
    let input = NativeRenderInput::new(
        project,
        vec![
            (NodeId(1), NativeRenderTarget::Strip(None)),
            (node, NativeRenderTarget::Effect { track: None, slot }),
        ],
        contracts,
    )
    .expect("native parameter input");
    let mut effects: NodePlacedEffects =
        BTreeMap::from([(node, Box::new(fixture) as Box<dyn Effect>)]);
    let mut instruments = BTreeMap::new();
    input.build(
        &AudioSourceBank::new(),
        &crate::testkit::registry(),
        &mut effects,
        &mut instruments,
        128,
        48_000.0,
    )
}

#[test]
fn readonly_parameter_key_has_no_control_handle() {
    let graph = graph(false);
    assert!(graph.native_parameter(NodeId(2), "meter").is_none());
}

#[test]
fn reordered_effect_parameters_require_fresh_generation_handle() {
    let first = graph(false);
    let old = first.native_parameter(NodeId(2), "gain").expect("old gain");
    let mut second = graph(true);
    let current = second
        .native_parameter(NodeId(2), "gain")
        .expect("current gain");
    assert_ne!(old.param, current.param);
    assert!(!second.apply_node_command(
        NodeId(2),
        NativeNodeCommand::Parameter {
            parameter: old,
            value: 0.75
        },
    ));
    assert!(second.apply_node_command(
        NodeId(2),
        NativeNodeCommand::Parameter {
            parameter: current,
            value: 0.75
        },
    ));
    assert_eq!(second.master.effects[0].param(current.param), 0.75);
    assert_eq!(second.master.effects[0].param(ParamId(0)), 0.25);
}

#[test]
fn parameter_contract_is_checked_after_plugin_prepare() {
    assert!(graph_result(false, true, AutomationPolicy::Continuous).is_err());
}

#[test]
fn only_continuous_parameters_get_native_control_handles() {
    for policy in [
        AutomationPolicy::PreparationOnly,
        AutomationPolicy::Structural,
        AutomationPolicy::RegenerationRequired,
        AutomationPolicy::EventSampled,
    ] {
        let graph = graph_result(false, false, policy).expect("policy graph");
        assert!(graph.native_parameter(NodeId(2), "gain").is_none());
    }
    let graph = graph_result(false, false, AutomationPolicy::Continuous).expect("continuous graph");
    assert!(graph.native_parameter(NodeId(2), "gain").is_some());
}
