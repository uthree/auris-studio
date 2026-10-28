use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use auris_core::automation::AutomationCurve;
use auris_core::pipeline::content::PluginParameter;
use auris_core::pipeline::{AutomationPolicy, ControlDomain, NodeId};
use auris_core::project::{EffectSlotId, Project, TrackId};
use auris_core::{AssetPath, AudioBuffer, Note, ParamId, ParamTarget, Ticks};

use super::{NativeRenderInput, NativeRenderTarget, RuntimeNodeAddress};
use crate::native::{NodePlacedEffects, NodePlacedInstruments};
use crate::{Transport, render_block};

fn contracts_for_project(
    project: &Project,
    bindings: &[(NodeId, NativeRenderTarget)],
) -> BTreeMap<NodeId, Vec<PluginParameter>> {
    let registry = crate::testkit::registry();
    bindings
        .iter()
        .filter_map(|(node, target)| match *target {
            NativeRenderTarget::Instrument(track) => project
                .track(track)
                .and_then(|track| {
                    track
                        .kind
                        .as_instrument()
                        .map(|instrument| instrument.instrument_id.as_str())
                })
                .and_then(|id| registry.create_instrument(id).ok())
                .map(|instrument| (*node, plugin_contract(instrument.parameters()))),
            NativeRenderTarget::Effect { track, slot } => {
                let strip = match track {
                    Some(id) => &project.track(id)?.mixer,
                    None => &project.master,
                };
                let effect = strip.effects.iter().find(|effect| effect.id == slot)?;
                registry
                    .create_effect(&effect.effect_id)
                    .ok()
                    .map(|effect| (*node, plugin_contract(effect.parameters())))
            }
            _ => None,
        })
        .collect()
}

fn plugin_contract(descriptors: &[auris_core::ParamDescriptor]) -> Vec<PluginParameter> {
    descriptors
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
        .collect()
}

fn empty_mapping() -> Vec<(NodeId, NativeRenderTarget)> {
    vec![(NodeId(10), NativeRenderTarget::Strip(None))]
}

fn instrument_mapping(
    track: TrackId,
    slot: Option<EffectSlotId>,
) -> Vec<(NodeId, NativeRenderTarget)> {
    let mut mapping = vec![
        (NodeId(10), NativeRenderTarget::Strip(None)),
        (NodeId(11), NativeRenderTarget::Strip(Some(track))),
        (NodeId(12), NativeRenderTarget::Instrument(track)),
    ];
    if let Some(slot) = slot {
        mapping.push((
            NodeId(13),
            NativeRenderTarget::Effect {
                track: Some(track),
                slot,
            },
        ));
    }
    mapping
}

#[test]
fn addresses_require_a_complete_one_to_one_owner_mapping() {
    let project = Project::default();
    let input = NativeRenderInput::new(project.clone(), empty_mapping(), BTreeMap::new()).unwrap();
    assert_eq!(
        input.addresses().address(NodeId(10)),
        Some(RuntimeNodeAddress::Strip { track: None })
    );

    assert!(NativeRenderInput::new(project.clone(), Vec::new(), BTreeMap::new()).is_err());
    assert!(
        NativeRenderInput::new(
            project.clone(),
            vec![
                (NodeId(10), NativeRenderTarget::Strip(None)),
                (NodeId(11), NativeRenderTarget::Strip(None)),
            ],
            BTreeMap::new(),
        )
        .is_err()
    );
    assert!(
        NativeRenderInput::new(
            project,
            vec![(NodeId(10), NativeRenderTarget::Strip(Some(TrackId(99))))],
            BTreeMap::new(),
        )
        .is_err()
    );
}

#[test]
fn unknown_and_wrong_kind_targets_are_rejected_without_consuming_maps() {
    let mut project = Project::default();
    let track = project.add_instrument_track("Lead", crate::testkit::TONE_ID);

    let input = NativeRenderInput::new(
        project.clone(),
        vec![
            (NodeId(10), NativeRenderTarget::Strip(None)),
            (NodeId(11), NativeRenderTarget::Instrument(TrackId(999))),
        ],
        BTreeMap::new(),
    );
    assert!(input.is_err());

    let input = NativeRenderInput::new(
        project,
        vec![
            (NodeId(10), NativeRenderTarget::Strip(None)),
            (NodeId(11), NativeRenderTarget::Instrument(track)),
            (
                NodeId(12),
                NativeRenderTarget::Send {
                    track,
                    send: auris_core::SendId(999),
                },
            ),
        ],
        BTreeMap::new(),
    );
    assert!(input.is_err());
}

#[test]
fn wrong_plugin_placement_fails_before_supplied_objects_are_consumed() {
    let mut project = Project::default();
    let track = project.add_instrument_track("Lead", "missing.instrument");
    let mapping = instrument_mapping(track, None);
    let mut contracts = BTreeMap::new();
    contracts.insert(
        NodeId(12),
        plugin_contract(
            crate::testkit::registry()
                .create_instrument(crate::testkit::TONE_ID)
                .unwrap()
                .parameters(),
        ),
    );
    let input = NativeRenderInput::new(project, mapping, contracts).unwrap();
    let mut effects = NodePlacedEffects::new();
    let mut instruments = NodePlacedInstruments::new();
    instruments.insert(
        NodeId(12),
        crate::testkit::registry()
            .create_instrument(crate::testkit::TONE_ID)
            .unwrap(),
    );
    let before = instruments.keys().copied().collect::<BTreeSet<_>>();
    let result = input.build(
        &auris_core::project::AudioSourceBank::new(),
        &crate::testkit::registry(),
        &mut effects,
        &mut instruments,
        128,
        48_000.0,
    );
    assert!(result.is_err());
    assert_eq!(instruments.keys().copied().collect::<BTreeSet<_>>(), before);
}

#[test]
fn missing_hosted_file_render_half_fails_without_consuming_effect() {
    let mut project = Project::default();
    let track = project.add_instrument_track("Lead", crate::testkit::TONE_ID);
    let slot = project
        .add_hosted_effect(
            Some(track),
            crate::testkit::GAIN_ID,
            AssetPath::external("definitely-missing-native-effect.dll"),
        )
        .unwrap();
    let mapping = instrument_mapping(track, Some(slot));
    let contracts = contracts_for_project(&project, &mapping);
    let input = NativeRenderInput::new(project, mapping, contracts).unwrap();
    let mut effects = NodePlacedEffects::new();
    let mut instruments = NodePlacedInstruments::new();
    instruments.insert(
        NodeId(12),
        crate::testkit::registry()
            .create_instrument(crate::testkit::TONE_ID)
            .unwrap(),
    );
    let instrument_keys = instruments.keys().copied().collect::<BTreeSet<_>>();
    let before = effects.keys().copied().collect::<BTreeSet<_>>();
    let result = input.build(
        &auris_core::project::AudioSourceBank::new(),
        &crate::testkit::registry(),
        &mut effects,
        &mut instruments,
        128,
        48_000.0,
    );
    assert!(result.is_err());
    assert_eq!(effects.keys().copied().collect::<BTreeSet<_>>(), before);
    assert_eq!(
        instruments.keys().copied().collect::<BTreeSet<_>>(),
        instrument_keys
    );
}

#[test]
fn sidechain_requires_a_runtime_effect_that_accepts_it() {
    let mut project = Project::default();
    let keyed = project.add_instrument_track("Keyed", crate::testkit::TONE_ID);
    let owner = project.add_instrument_track("Owner", crate::testkit::TONE_ID);
    let slot = project
        .add_effect(Some(owner), crate::testkit::GAIN_ID)
        .unwrap();
    project
        .track_mut(owner)
        .unwrap()
        .mixer
        .effects
        .iter_mut()
        .find(|effect| effect.id == slot)
        .unwrap()
        .sidechain = Some(keyed);
    let mappings = vec![
        (NodeId(10), NativeRenderTarget::Strip(None)),
        (NodeId(11), NativeRenderTarget::Strip(Some(keyed))),
        (NodeId(12), NativeRenderTarget::Instrument(keyed)),
        (NodeId(13), NativeRenderTarget::Strip(Some(owner))),
        (NodeId(14), NativeRenderTarget::Instrument(owner)),
        (
            NodeId(15),
            NativeRenderTarget::Effect {
                track: Some(owner),
                slot,
            },
        ),
    ];
    let contracts = contracts_for_project(&project, &mappings);
    let input = NativeRenderInput::new(project, mappings, contracts).unwrap();
    let mut effects = NodePlacedEffects::new();
    effects.insert(
        NodeId(15),
        crate::testkit::registry()
            .create_effect(crate::testkit::GAIN_ID)
            .unwrap(),
    );
    let mut instruments = NodePlacedInstruments::new();
    instruments.insert(
        NodeId(12),
        crate::testkit::registry()
            .create_instrument(crate::testkit::TONE_ID)
            .unwrap(),
    );
    instruments.insert(
        NodeId(14),
        crate::testkit::registry()
            .create_instrument(crate::testkit::TONE_ID)
            .unwrap(),
    );
    let effect_keys = effects.keys().copied().collect::<BTreeSet<_>>();
    let instrument_keys = instruments.keys().copied().collect::<BTreeSet<_>>();
    assert!(
        input
            .build(
                &auris_core::project::AudioSourceBank::new(),
                &crate::testkit::registry(),
                &mut effects,
                &mut instruments,
                128,
                48_000.0,
            )
            .is_err()
    );
    assert_eq!(
        effects.keys().copied().collect::<BTreeSet<_>>(),
        effect_keys
    );
    assert_eq!(
        instruments.keys().copied().collect::<BTreeSet<_>>(),
        instrument_keys
    );
}

#[test]
fn parameter_contracts_reject_descriptor_drift_before_build() {
    let mut project = Project::default();
    let track = project.add_instrument_track("Lead", crate::testkit::TONE_ID);
    let registry = crate::testkit::registry();
    let instrument = registry.create_instrument(crate::testkit::TONE_ID).unwrap();
    let mut contracts = BTreeMap::new();
    contracts.insert(NodeId(12), plugin_contract(instrument.parameters()));
    let mut drifted = contracts.clone();
    drifted.get_mut(&NodeId(12)).unwrap()[0].descriptor.key = Cow::Owned("wrong-key".into());
    let mapping = instrument_mapping(track, None);
    assert!(NativeRenderInput::new(project.clone(), mapping.clone(), BTreeMap::new()).is_err());
    let mut wrong_key_project = project.clone();
    assert!(wrong_key_project.automation.set_point(
        ParamTarget::Instrument {
            track,
            param: ParamId(0),
        },
        Some("not-a-runtime-key".into()),
        AutomationCurve::Linear,
        auris_core::Ticks::ZERO,
        0.5,
    ));
    assert!(NativeRenderInput::new(wrong_key_project, mapping.clone(), contracts.clone()).is_err());
    let valid = NativeRenderInput::new(project.clone(), mapping.clone(), contracts).unwrap();
    let mut valid_effects = NodePlacedEffects::new();
    let mut valid_instruments = NodePlacedInstruments::new();
    valid_instruments.insert(
        NodeId(12),
        crate::testkit::registry()
            .create_instrument(crate::testkit::TONE_ID)
            .unwrap(),
    );
    let _ = valid
        .build(
            &auris_core::project::AudioSourceBank::new(),
            &crate::testkit::registry(),
            &mut valid_effects,
            &mut valid_instruments,
            128,
            48_000.0,
        )
        .unwrap();

    let input = NativeRenderInput::new(project, mapping, drifted).unwrap();
    let mut effects = NodePlacedEffects::new();
    let mut instruments = NodePlacedInstruments::new();
    instruments.insert(
        NodeId(12),
        crate::testkit::registry()
            .create_instrument(crate::testkit::TONE_ID)
            .unwrap(),
    );
    let before = instruments.keys().copied().collect::<BTreeSet<_>>();
    assert!(
        input
            .build(
                &auris_core::project::AudioSourceBank::new(),
                &crate::testkit::registry(),
                &mut effects,
                &mut instruments,
                128,
                48_000.0,
            )
            .is_err()
    );
    assert_eq!(instruments.keys().copied().collect::<BTreeSet<_>>(), before);
}

#[test]
fn successful_build_consumes_supplied_instruments_and_effects() {
    let mut project = Project::default();
    let track = project.add_instrument_track("Lead", crate::testkit::TONE_ID);
    let slot = project
        .add_effect(Some(track), crate::testkit::GAIN_ID)
        .unwrap();
    let mapping = instrument_mapping(track, Some(slot));
    let contracts = contracts_for_project(&project, &mapping);
    let input = NativeRenderInput::new(project, mapping, contracts).unwrap();
    let mut effects = NodePlacedEffects::new();
    effects.insert(
        NodeId(13),
        crate::testkit::registry()
            .create_effect(crate::testkit::GAIN_ID)
            .unwrap(),
    );
    let mut instruments = NodePlacedInstruments::new();
    instruments.insert(
        NodeId(12),
        crate::testkit::registry()
            .create_instrument(crate::testkit::TONE_ID)
            .unwrap(),
    );
    let _graph = input
        .build(
            &auris_core::project::AudioSourceBank::new(),
            &crate::testkit::registry(),
            &mut effects,
            &mut instruments,
            128,
            48_000.0,
        )
        .unwrap();
    assert!(effects.is_empty());
    assert!(instruments.is_empty());
}

fn tone_with_saved_gain(gain: f32) -> (Project, TrackId, EffectSlotId) {
    let mut project = Project::default();
    let track = project.add_instrument_track("Lead", crate::testkit::TONE_ID);
    let clip = project
        .add_midi_clip(track, "Tone", Ticks::ZERO, Ticks::from_beats(4.0))
        .expect("the tone clip is added");
    project
        .midi_clip_mut(clip)
        .expect("the clip exists")
        .notes
        .push(Note::new(60, Ticks::ZERO, Ticks::from_beats(2.0)));
    let slot = project
        .add_effect(Some(track), crate::testkit::GAIN_ID)
        .expect("the gain is added");
    project
        .track_mut(track)
        .expect("the track exists")
        .mixer
        .effects
        .iter_mut()
        .find(|effect| effect.id == slot)
        .expect("the effect exists")
        .state
        .params
        .insert("gain".into(), gain);
    (project, track, slot)
}

fn render_native_tone(graph: &mut crate::RenderGraph) -> f32 {
    let mut buffer = AudioBuffer::new(2, 128, 48_000.0);
    render_block(graph, &mut Transport::playing_from(0), &mut buffer, false);
    buffer.channel(0)[32]
}

#[test]
fn registry_created_effect_loads_saved_state_before_rendering() {
    let (project, track, slot) = tone_with_saved_gain(2.0);
    let mapping = instrument_mapping(track, Some(slot));
    let contracts = contracts_for_project(&project, &mapping);
    let input = NativeRenderInput::new(project, mapping, contracts).unwrap();
    let registry = crate::testkit::registry();
    let mut effects = NodePlacedEffects::new();
    let mut instruments = NodePlacedInstruments::new();
    let mut graph = input
        .build(
            &auris_core::project::AudioSourceBank::new(),
            &registry,
            &mut effects,
            &mut instruments,
            128,
            48_000.0,
        )
        .unwrap();
    let value = render_native_tone(&mut graph);
    assert!(
        (value - 1.0).abs() < 1e-6,
        "saved gain was not rendered: {value}"
    );
}

#[test]
fn caller_placed_effect_state_wins_over_saved_document_state() {
    let (project, track, slot) = tone_with_saved_gain(2.0);
    let mapping = instrument_mapping(track, Some(slot));
    let contracts = contracts_for_project(&project, &mapping);
    let input = NativeRenderInput::new(project, mapping, contracts).unwrap();
    let registry = crate::testkit::registry();
    let mut supplied = registry.create_effect(crate::testkit::GAIN_ID).unwrap();
    supplied.set_param(ParamId(0), 4.0);
    let mut effects = NodePlacedEffects::new();
    effects.insert(NodeId(13), supplied);
    let mut instruments = NodePlacedInstruments::new();
    let mut graph = input
        .build(
            &auris_core::project::AudioSourceBank::new(),
            &registry,
            &mut effects,
            &mut instruments,
            128,
            48_000.0,
        )
        .unwrap();
    let value = render_native_tone(&mut graph);
    assert!(
        (value - 2.0).abs() < 1e-6,
        "caller state was overwritten: {value}"
    );
    assert!((graph.tracks()[0].strip().effects[0].param(ParamId(0)) - 4.0).abs() < 1e-6);
}
