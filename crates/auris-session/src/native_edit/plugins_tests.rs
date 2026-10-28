use super::super::history::Edit;
use super::super::native_edit::NativeAutomationEdit;
use super::super::native_edit::NativePluginEdit;
use super::super::native_edit::{
    NativeDocumentEditor, NativeEdit, NativeEditError, NativeEditOwner,
};
use auris_core::automation::{AutomationCurve, AutomationPoint};
use auris_core::pipeline::ControlDomain;
use auris_core::pipeline::EdgeId;
use auris_core::pipeline::NodeId;
use auris_core::pipeline::content::{
    ContentLimits, ParameterAddress, PluginInstance, ProcessorKind,
};
use auris_core::pipeline::document::NativeDocument;
use auris_core::project::Project;
use auris_core::{ParamUnit, PluginState, Ticks};

fn limits() -> ContentLimits {
    ContentLimits {
        sources: 32,
        clips: 64,
        stages: 64,
        processors: 128,
        groups: 32,
        tracks: 32,
        edges: 256,
        external_ids: 64,
        automations: 16,
        automation_points: 128,
        bindings: 32,
    }
}

fn fixture() -> (NativeDocumentEditor, NodeId, NodeId) {
    let mut project = Project::new("plugins", 48_000.0);
    let track = project.add_instrument_track("Keys", "unavailable.instrument");
    let slot = project
        .add_effect(Some(track), "unavailable.effect")
        .unwrap();
    let document = auris_io::project_to_native(&project, limits()).unwrap();
    let instrument = document
        .content
        .processors
        .iter()
        .find(|processor| matches!(&processor.kind, ProcessorKind::Instrument(_)))
        .map(|processor| processor.id)
        .unwrap();
    let effect = document
        .content
        .processors
        .iter()
        .find(|processor| {
            matches!(&processor.kind, ProcessorKind::Effect { slot: id, .. } if *id == slot)
        })
        .map(|processor| processor.id)
        .unwrap();
    (
        NativeDocumentEditor::new(document, limits()).unwrap(),
        instrument,
        effect,
    )
}

fn apply(editor: &mut NativeDocumentEditor, operation: NativeEdit) {
    editor
        .apply(
            editor.revision().document,
            NativeEditOwner::Human,
            Edit::ExternalChanges,
            &[operation],
        )
        .unwrap();
}

fn plugin(document: &NativeDocument, node: NodeId) -> &PluginInstance {
    document
        .content
        .processors
        .iter()
        .find_map(|processor| match &processor.kind {
            ProcessorKind::Instrument(plugin) | ProcessorKind::Effect { plugin, .. }
                if processor.id == node =>
            {
                Some(plugin)
            }
            _ => None,
        })
        .unwrap()
}

fn add_shared_curve(editor: &mut NativeDocumentEditor, instrument: NodeId, effect: NodeId) {
    let strip = editor
        .document()
        .content
        .processors
        .iter()
        .find_map(|processor| {
            matches!(&processor.kind, ProcessorKind::Strip(_)).then_some(processor.id)
        })
        .unwrap();
    let receipt = editor
        .apply(
            0,
            NativeEditOwner::Human,
            Edit::ExternalChanges,
            &[NativeEdit::Automation(NativeAutomationEdit::Add {
                domain: ControlDomain::Native {
                    unit: ParamUnit::Decibels,
                    min: -60.0,
                    max: 12.0,
                },
                curve: AutomationCurve::Hold,
                points: vec![AutomationPoint::new(Ticks::ZERO, 0.5)],
                target: ParameterAddress {
                    node: strip,
                    key: "gain".into(),
                },
                enabled: true,
            })],
        )
        .unwrap();
    let automation = receipt
        .created
        .iter()
        .find_map(|created| match created {
            super::super::native_edit::NativeCreated::Automation(id) => Some(*id),
            _ => None,
        })
        .unwrap();
    let source = editor
        .document()
        .content
        .processors
        .iter()
        .find_map(|processor| match &processor.kind {
            ProcessorKind::Automation(id) if *id == automation => Some(processor.id),
            _ => None,
        })
        .unwrap();
    let mut document = editor.document().clone();
    let binding = document
        .content
        .automation_bindings
        .iter_mut()
        .find(|binding| binding.source == source)
        .unwrap();
    binding.target = ParameterAddress {
        node: instrument,
        key: "cutoff".into(),
    };
    let mut second = binding.clone();
    second.id = EdgeId(document.content.next_id);
    document.content.next_id = document.content.next_id.saturating_add(1);
    second.target = ParameterAddress {
        node: effect,
        key: "gain".into(),
    };
    document.content.automation_bindings.push(second);
    *editor = NativeDocumentEditor::new(document, limits()).unwrap();
}

#[test]
fn replacing_plugin_keeps_node_and_removes_only_target_bindings() {
    let (mut editor, instrument, effect) = fixture();
    let before = editor.document().clone();
    apply(
        &mut editor,
        NativeEdit::Plugin(NativePluginEdit::ReplaceInstrument {
            node: instrument,
            plugin: PluginInstance {
                plugin_id: "replacement.instrument".into(),
                state: PluginState::default(),
                file: None,
            },
        }),
    );
    assert_eq!(
        plugin(editor.document(), instrument).plugin_id,
        "replacement.instrument"
    );
    assert!(
        editor
            .document()
            .content
            .processors
            .iter()
            .any(|processor| processor.id == effect)
    );
    assert_eq!(editor.document().content.edges, before.content.edges);
    assert_eq!(editor.document().content.sources, before.content.sources);
}

#[test]
fn replacing_effect_preserves_slot_enabled_edges_and_group() {
    let (mut editor, _, effect) = fixture();
    let before = editor.document().clone();
    let before_processor = before
        .content
        .processors
        .iter()
        .find(|processor| processor.id == effect)
        .unwrap();
    let (slot, enabled, group) = match &before_processor.kind {
        ProcessorKind::Effect { slot, enabled, .. } => (*slot, *enabled, before_processor.group),
        _ => panic!("fixture effect is not an effect processor"),
    };
    apply(
        &mut editor,
        NativeEdit::Plugin(NativePluginEdit::ReplaceEffect {
            node: effect,
            plugin: PluginInstance {
                plugin_id: "replacement.effect".into(),
                state: PluginState::default(),
                file: None,
            },
        }),
    );
    let after = editor.document();
    let processor = after
        .content
        .processors
        .iter()
        .find(|processor| processor.id == effect)
        .unwrap();
    assert_eq!(processor.group, group);
    assert!(matches!(&processor.kind, ProcessorKind::Effect {
        slot: actual,
        enabled: actual_enabled,
        plugin,
    } if *actual == slot && *actual_enabled == enabled && plugin.plugin_id == "replacement.effect"));
    assert_eq!(after.content.edges, before.content.edges);
}

#[test]
fn duplicated_branch_keeps_shared_source_but_not_plugin_payload() {
    let (mut editor, instrument, _) = fixture();
    let group = editor
        .document()
        .content
        .processors
        .iter()
        .find(|processor| processor.id == instrument)
        .and_then(|processor| processor.group)
        .unwrap();
    let source_count = editor.document().content.sources.len();
    apply(&mut editor, NativeEdit::DuplicateBranch { group });
    assert_eq!(editor.document().content.sources.len(), source_count);
    let other = editor
        .document()
        .content
        .processors
        .iter()
        .find_map(|processor| match &processor.kind {
            ProcessorKind::Instrument(plugin) if processor.id != instrument => {
                Some((processor.id, plugin.plugin_id.clone()))
            }
            _ => None,
        })
        .unwrap();
    apply(
        &mut editor,
        NativeEdit::Plugin(NativePluginEdit::ReplaceInstrument {
            node: instrument,
            plugin: PluginInstance {
                plugin_id: "only.original.changed".into(),
                state: PluginState::default(),
                file: None,
            },
        }),
    );
    assert_eq!(plugin(editor.document(), other.0).plugin_id, other.1);
}

#[test]
fn same_plugin_payload_is_a_noop_and_state_parameter_is_keyed() {
    let (mut editor, instrument, _) = fixture();
    let original = plugin(editor.document(), instrument).clone();
    let receipt = editor
        .apply(
            0,
            NativeEditOwner::Human,
            Edit::ExternalChanges,
            &[NativeEdit::Plugin(NativePluginEdit::ReplaceInstrument {
                node: instrument,
                plugin: original,
            })],
        )
        .unwrap();
    assert!(!receipt.changed);
    apply(
        &mut editor,
        NativeEdit::Plugin(NativePluginEdit::SetParameter {
            node: instrument,
            key: "cutoff".into(),
            value: 0.5,
        }),
    );
    assert_eq!(
        plugin(editor.document(), instrument).state.params["cutoff"],
        0.5
    );
}

#[test]
fn invalid_plugin_payload_is_atomic_and_history_handles_undo_redo() {
    let (mut editor, instrument, _) = fixture();
    let before = editor.document().clone();
    let error = editor.apply(
        0,
        NativeEditOwner::Agent(7),
        Edit::ExternalChanges,
        &[NativeEdit::Plugin(NativePluginEdit::SetParameter {
            node: instrument,
            key: "gain".into(),
            value: f32::NAN,
        })],
    );
    assert!(matches!(error, Err(NativeEditError::Invalid(_))));
    assert_eq!(editor.document(), &before);
    apply(
        &mut editor,
        NativeEdit::Plugin(NativePluginEdit::SetParameter {
            node: instrument,
            key: "gain".into(),
            value: 0.25,
        }),
    );
    assert!(editor.undo(editor.revision().document).unwrap().is_some());
    assert!(editor.redo(editor.revision().document).unwrap().is_some());
}

#[test]
fn plugin_state_round_trip_rejects_nonfinite_values_and_wrong_kind() {
    let (mut editor, instrument, _) = fixture();
    let error = editor.apply(
        0,
        NativeEditOwner::Human,
        Edit::ExternalChanges,
        &[NativeEdit::Plugin(NativePluginEdit::ReplaceState {
            node: instrument,
            state: PluginState {
                params: [("bad".into(), f32::INFINITY)].into(),
                ..PluginState::default()
            },
        })],
    );
    assert!(matches!(error, Err(NativeEditError::Invalid(_))));
    let error = editor.apply(
        0,
        NativeEditOwner::Human,
        Edit::ExternalChanges,
        &[NativeEdit::Plugin(NativePluginEdit::SetParameter {
            node: NodeId(999_999),
            key: "gain".into(),
            value: 0.5,
        })],
    );
    assert!(error.is_err());
}

#[test]
fn replacing_one_plugin_removes_only_its_binding_and_keeps_shared_curve() {
    let (mut editor, instrument, effect) = fixture();
    add_shared_curve(&mut editor, instrument, effect);
    assert_eq!(editor.document().content.automation_bindings.len(), 2);
    let automation = editor.document().content.automation[0].id;
    let source = editor
        .document()
        .content
        .processors
        .iter()
        .find(|processor| matches!(&processor.kind, ProcessorKind::Automation(id) if *id == automation))
        .map(|processor| processor.id)
        .unwrap();
    apply(
        &mut editor,
        NativeEdit::Plugin(NativePluginEdit::ReplaceInstrument {
            node: instrument,
            plugin: PluginInstance {
                plugin_id: "changed.instrument".into(),
                state: PluginState::default(),
                file: None,
            },
        }),
    );
    assert_eq!(editor.document().content.automation.len(), 1);
    assert_eq!(
        editor.document().content.automation[0].points(),
        vec![AutomationPoint::new(Ticks::ZERO, 0.5)]
    );
    assert!(
        editor
            .document()
            .content
            .processors
            .iter()
            .any(|processor| {
                matches!(&processor.kind, ProcessorKind::Automation(id) if *id == automation)
            })
    );
    assert_eq!(editor.document().content.automation_bindings.len(), 1);
    assert_eq!(
        editor.document().content.automation_bindings[0].source,
        source
    );
    assert_eq!(
        editor.document().content.automation_bindings[0].target.node,
        effect
    );
}

#[test]
fn replacing_state_preserves_plugin_metadata_and_automation_bindings() {
    let (mut editor, instrument, effect) = fixture();
    add_shared_curve(&mut editor, instrument, effect);
    let before = plugin(editor.document(), instrument).clone();
    let binding_count = editor.document().content.automation_bindings.len();
    let state = PluginState {
        params: [("cutoff".into(), 0.25)].into(),
        ..PluginState::default()
    };
    apply(
        &mut editor,
        NativeEdit::Plugin(NativePluginEdit::ReplaceState {
            node: instrument,
            state: state.clone(),
        }),
    );
    let after = plugin(editor.document(), instrument);
    assert_eq!(after.plugin_id, before.plugin_id);
    assert_eq!(after.file, before.file);
    assert_eq!(after.state, state);
    assert_eq!(
        editor.document().content.automation_bindings.len(),
        binding_count
    );
}

#[test]
fn plugin_edit_rejects_stale_revision_and_other_transaction_owner() {
    let (mut editor, instrument, _) = fixture();
    let stale = editor.apply(
        99,
        NativeEditOwner::Human,
        Edit::ExternalChanges,
        &[NativeEdit::Plugin(NativePluginEdit::SetParameter {
            node: instrument,
            key: "gain".into(),
            value: 0.5,
        })],
    );
    assert!(matches!(stale, Err(NativeEditError::StaleRevision { .. })));
    editor
        .begin_transaction(0, NativeEditOwner::Human, Edit::ExternalChanges)
        .unwrap();
    let blocked = editor.apply(
        0,
        NativeEditOwner::Agent(42),
        Edit::ExternalChanges,
        &[NativeEdit::Plugin(NativePluginEdit::SetParameter {
            node: instrument,
            key: "gain".into(),
            value: 0.5,
        })],
    );
    assert!(matches!(blocked, Err(NativeEditError::TransactionActive)));
    editor
        .revert_transaction(0, NativeEditOwner::Human)
        .unwrap();
}
