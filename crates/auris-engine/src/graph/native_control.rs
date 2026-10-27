//! Allocation-free controls addressed by native pipeline node identities.

use auris_core::param::MixerControl;
use auris_core::pipeline::NodeId;

use super::RenderGraph;
use crate::command::NativeNodeCommand;

impl RenderGraph {
    /// Reads the generation-bound native address table prepared with this graph.
    pub fn native_addresses(&self) -> &crate::native::NativeRuntimeAddresses {
        &self.native_addresses
    }

    /// Resolves a stable plugin parameter key to a generation-bound runtime handle.
    ///
    /// This is a preparation-thread lookup. The returned handle is deliberately bounded to the
    /// current graph generation so a parameter reorder cannot make a later audio-thread command
    /// address a different control.
    pub fn native_parameter(
        &self,
        node: NodeId,
        key: &str,
    ) -> Option<crate::native::NativeParameterHandle> {
        let address = self.native_addresses.address(node)?;
        let descriptor = match address {
            crate::native::RuntimeNodeAddress::Instrument { track } => {
                let track = self.tracks.get(track)?;
                let instrument = match track.source() {
                    super::RenderSource::Instrument { instrument, .. }
                    | super::RenderSource::Sung { instrument, .. } => instrument,
                    _ => return None,
                };
                instrument.descriptor_for_key(key)?
            }
            crate::native::RuntimeNodeAddress::Effect { track, slot } => {
                let strip = match track {
                    Some(index) => &self.tracks.get(index)?.strip,
                    None => &self.master,
                };
                strip.effects.get(slot)?.descriptor_for_key(key)?
            }
            _ => return None,
        };
        if descriptor.read_only {
            return None;
        }
        if !self.native_addresses.accepts_parameter(node, descriptor.id) {
            return None;
        }
        Some(crate::native::NativeParameterHandle {
            node,
            param: descriptor.id,
            generation: self.native_addresses.generation,
            min: descriptor.min,
            max: descriptor.max,
        })
    }

    /// Applies a native node command to the current graph.
    ///
    /// The native address table is prepared with the graph and is the only authority used here;
    /// this prevents a stale track index from reaching a reordered graph. Invalid numeric input,
    /// unknown nodes and controls sent to the wrong node role are ignored without allocation.
    pub fn apply_node_command(&mut self, node: NodeId, command: NativeNodeCommand) -> bool {
        let Some(address) = self.native_addresses.address(node) else {
            return false;
        };
        match (address, command) {
            (
                crate::native::RuntimeNodeAddress::Strip { track },
                NativeNodeCommand::Gain(value),
            ) if mixer_value_in_range(MixerControl::Gain, value) => {
                match track {
                    Some(index) => self.set_track_gain_db(index, value),
                    None => self.set_master_gain_db(value),
                }
                true
            }
            (crate::native::RuntimeNodeAddress::Strip { track }, NativeNodeCommand::Pan(value))
                if mixer_value_in_range(MixerControl::Pan, value) =>
            {
                match track {
                    Some(index) => self.set_track_pan(index, value),
                    None => self.set_master_pan(value),
                }
                true
            }
            (
                crate::native::RuntimeNodeAddress::Strip { track },
                NativeNodeCommand::Mute(value),
            ) => {
                match track {
                    Some(index) => self.set_track_mute(index, value),
                    None => self.master.set_mute(value),
                }
                true
            }
            (
                crate::native::RuntimeNodeAddress::Send { track, send },
                NativeNodeCommand::Gain(value),
            ) if mixer_value_in_range(MixerControl::Send, value) => {
                self.set_send_level_db(track, send, value);
                true
            }
            (
                crate::native::RuntimeNodeAddress::Instrument { track },
                NativeNodeCommand::Parameter { parameter, value },
            ) if parameter.node == node
                && parameter.generation != 0
                && parameter.generation == self.native_addresses.generation
                && parameter.min <= parameter.max
                && value >= parameter.min
                && value <= parameter.max
                && value.is_finite() =>
            {
                self.set_instrument_param(track, parameter.param, value);
                true
            }
            (
                crate::native::RuntimeNodeAddress::Effect { track, slot },
                NativeNodeCommand::Parameter { parameter, value },
            ) if parameter.node == node
                && parameter.generation != 0
                && parameter.generation == self.native_addresses.generation
                && parameter.min <= parameter.max
                && value >= parameter.min
                && value <= parameter.max
                && value.is_finite() =>
            {
                self.set_effect_param(track, slot, parameter.param, value);
                true
            }
            (
                crate::native::RuntimeNodeAddress::Instrument { track },
                NativeNodeCommand::NoteOn { pitch, velocity },
            ) if pitch <= 127 && velocity.is_finite() && (0.0..=1.0).contains(&velocity) => {
                self.note_on(track, pitch, velocity);
                true
            }
            (
                crate::native::RuntimeNodeAddress::Instrument { track },
                NativeNodeCommand::NoteOff { pitch },
            ) if pitch <= 127 => {
                self.note_off(track, pitch);
                true
            }
            (
                crate::native::RuntimeNodeAddress::Instrument { track },
                NativeNodeCommand::PitchBend(value),
            ) if value.is_finite()
                && (-auris_core::project::BEND_LIMIT..=auris_core::project::BEND_LIMIT)
                    .contains(&value) =>
            {
                self.pitch_bend(track, value);
                true
            }
            (
                crate::native::RuntimeNodeAddress::Instrument { track },
                NativeNodeCommand::Controller { number, value },
            ) if number <= 127 && value.is_finite() && (0.0..=1.0).contains(&value) => {
                self.controller(track, number, value);
                true
            }
            _ => false,
        }
    }
}

fn mixer_value_in_range(control: MixerControl, value: f32) -> bool {
    let descriptor = control.descriptor();
    value.is_finite() && value >= descriptor.min && value <= descriptor.max
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::NativeNodeCommand;
    use crate::native::{NativeRenderInput, NativeRenderTarget};
    use auris_core::pipeline::content::PluginParameter;
    use auris_core::pipeline::{AutomationPolicy, ControlDomain};
    use auris_core::project::{AudioSourceBank, Project};
    use std::collections::BTreeMap;

    fn input(
        project: Project,
        track_nodes: &[(auris_core::TrackId, NodeId, NodeId)],
    ) -> NativeRenderInput {
        let mut bindings = vec![(NodeId(1), NativeRenderTarget::Strip(None))];
        let registry = crate::testkit::registry();
        let parameters = registry
            .create_instrument(crate::testkit::TONE_ID)
            .expect("tone instrument")
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
                policy: AutomationPolicy::Continuous,
            })
            .collect::<Vec<_>>();
        let mut contracts = BTreeMap::new();
        for (track, strip, instrument) in track_nodes {
            bindings.push((*strip, NativeRenderTarget::Strip(Some(*track))));
            bindings.push((*instrument, NativeRenderTarget::Instrument(*track)));
            contracts.insert(*instrument, parameters.clone());
        }
        NativeRenderInput::new(project, bindings, contracts).expect("native input")
    }

    fn graph(project: Project, nodes: &[(auris_core::TrackId, NodeId, NodeId)]) -> RenderGraph {
        let input = input(project, nodes);
        let mut effects = crate::native::NodePlacedEffects::new();
        let mut instruments = crate::native::NodePlacedInstruments::new();
        input
            .build(
                &AudioSourceBank::new(),
                &crate::testkit::registry(),
                &mut effects,
                &mut instruments,
                128,
                48_000.0,
            )
            .expect("native graph")
    }

    fn instrument_project() -> (Project, auris_core::TrackId) {
        let mut project = Project::new("native controls", 48_000.0);
        let track = project.add_instrument_track("Tone", crate::testkit::TONE_ID);
        (project, track)
    }

    #[test]
    fn generation_bound_parameter_handle_rejects_stale_and_accepts_current() {
        let (project, track) = instrument_project();
        let first = graph(project.clone(), &[(track, NodeId(2), NodeId(3))]);
        let stale = first
            .native_parameter(NodeId(3), "amplitude")
            .expect("parameter");
        let mut second = graph(project, &[(track, NodeId(2), NodeId(3))]);
        assert!(!second.apply_node_command(
            NodeId(3),
            NativeNodeCommand::Parameter {
                parameter: stale,
                value: 0.25
            },
        ));
        let current = second
            .native_parameter(NodeId(3), "amplitude")
            .expect("parameter");
        let (allocations, deallocations) = crate::testkit::count_heap_operations(|| {
            assert!(second.apply_node_command(
                NodeId(3),
                NativeNodeCommand::Parameter {
                    parameter: current,
                    value: 0.25
                },
            ));
        });
        assert_eq!((allocations, deallocations), (0, 0));
        let crate::graph::RenderSource::Instrument { instrument, .. } = &second.tracks[0].source
        else {
            panic!("instrument source");
        };
        assert_eq!(instrument.param(auris_core::ParamId(0)), 0.25);
        assert_eq!(
            first
                .native_parameter(NodeId(3), "amplitude")
                .unwrap()
                .param,
            current.param
        );
        let (allocations, deallocations) = crate::testkit::count_heap_operations(|| {
            assert!(!second.apply_node_command(
                NodeId(3),
                NativeNodeCommand::Parameter {
                    parameter: stale,
                    value: 0.25
                },
            ));
        });
        assert_eq!((allocations, deallocations), (0, 0));
    }

    #[test]
    fn parameter_lookup_rejects_unknown_keys() {
        let (project, track) = instrument_project();
        let graph = graph(project, &[(track, NodeId(2), NodeId(3))]);
        assert!(graph.native_parameter(NodeId(3), "missing").is_none());
        assert!(graph.native_parameter(NodeId(2), "amplitude").is_none());
    }

    #[test]
    fn parameter_controls_reject_wrong_role_nonfinite_and_out_of_range_values() {
        let (project, track) = instrument_project();
        let mut graph = graph(project, &[(track, NodeId(2), NodeId(3))]);
        let handle = graph
            .native_parameter(NodeId(3), "amplitude")
            .expect("parameter");
        assert!(!graph.apply_node_command(
            NodeId(2),
            NativeNodeCommand::Parameter {
                parameter: handle,
                value: 0.5
            },
        ));
        assert!(!graph.apply_node_command(
            NodeId(3),
            NativeNodeCommand::Parameter {
                parameter: handle,
                value: f32::NAN
            },
        ));
        assert!(!graph.apply_node_command(
            NodeId(3),
            NativeNodeCommand::Parameter {
                parameter: handle,
                value: 1.1
            },
        ));
    }

    #[test]
    fn node_strip_address_follows_track_reorder() {
        let mut first = Project::new("native controls", 48_000.0);
        let a = first.add_instrument_track("A", crate::testkit::TONE_ID);
        let b = first.add_instrument_track("B", crate::testkit::TONE_ID);
        let mut second = first.clone();
        second.tracks.swap(0, 1);
        let mut graph = graph(
            second,
            &[(a, NodeId(10), NodeId(20)), (b, NodeId(11), NodeId(21))],
        );
        assert!(graph.apply_node_command(NodeId(11), NativeNodeCommand::Gain(-6.0)));
        assert!(
            (graph.tracks[0].strip.gain() - auris_core::param::db_to_gain(-6.0)).abs() < 1.0e-6
        );
        assert!((graph.tracks[1].strip.gain() - auris_core::param::db_to_gain(0.0)).abs() < 1.0e-6);
        assert_eq!(
            graph.native_addresses().address(NodeId(11)),
            Some(crate::native::RuntimeNodeAddress::Strip { track: Some(0) })
        );
    }
}

#[cfg(test)]
mod parameter_tests;
