use super::super::history::Edit;
use super::super::native_edit::{
    NativeCreated, NativeDocumentEditor, NativeEditError, NativeEditOwner, NativePluginCommand,
};
use crate::PluginWindow;
use auris_core::PluginState;
use auris_core::param::{ParamId, ParamTarget};
use auris_core::pipeline::NodeId;
use auris_core::pipeline::content::{ContentLimits, PluginInstance, ProcessorKind};
use auris_core::project::{EffectSlotId, Project, TrackId};

fn limits() -> ContentLimits {
    super::tests::limits()
}

fn fixture() -> (NativeDocumentEditor, TrackId, EffectSlotId, EffectSlotId) {
    let mut project = Project::new("plugin commands", 48_000.0);
    let track = project.add_instrument_track("Keys", "unavailable.instrument");
    let first = project.add_effect(None, "master.first").unwrap();
    let second = project.add_effect(None, "master.second").unwrap();
    let document = auris_io::project_to_native(&project, limits()).unwrap();
    (
        NativeDocumentEditor::new(document, limits()).unwrap(),
        track,
        first,
        second,
    )
}

fn plugin(id: &str) -> PluginInstance {
    PluginInstance {
        plugin_id: id.into(),
        state: PluginState::default(),
        file: None,
    }
}

fn effect_node(editor: &NativeDocumentEditor, slot: EffectSlotId) -> NodeId {
    editor
        .document()
        .content
        .processors
        .iter()
        .find_map(|processor| match &processor.kind {
            ProcessorKind::Effect { slot: id, .. } if *id == slot => Some(processor.id),
            _ => None,
        })
        .unwrap()
}

#[test]
fn set_instrument_uses_designated_primary_and_preserves_shared_source() {
    let (mut editor, track, _, _) = fixture();
    let before_sources = editor.document().content.sources.clone();
    let receipt = editor
        .apply_plugin_command(
            0,
            NativeEditOwner::Human,
            &NativePluginCommand::SetInstrument {
                track,
                plugin: plugin("replacement.instrument"),
            },
        )
        .unwrap();
    assert!(receipt.changed);
    assert_eq!(editor.document().content.sources, before_sources);
    assert!(editor
        .document()
        .content
        .processors
        .iter()
        .any(|processor| matches!(&processor.kind, ProcessorKind::Instrument(instance) if instance.plugin_id == "replacement.instrument")));
}

#[test]
fn wrong_track_slot_is_rejected_without_mutating_document() {
    let (mut editor, track, slot, _) = fixture();
    let before = editor.document().clone();
    let result = editor.apply_plugin_command(
        0,
        NativeEditOwner::Human,
        &NativePluginCommand::ReplaceEffect {
            track: Some(track),
            slot,
            plugin: plugin("wrong.owner"),
        },
    );
    assert!(result.is_err());
    assert_eq!(editor.document(), &before);
    assert!(!editor.can_undo());
}

#[test]
fn master_insert_and_extreme_move_keep_chain_valid_and_record_labels() {
    let (mut editor, _, first, second) = fixture();
    let inserted = editor
        .apply_plugin_command(
            0,
            NativeEditOwner::Human,
            &NativePluginCommand::InsertEffect {
                track: None,
                index: 1,
                plugin: plugin("master.inserted"),
            },
        )
        .unwrap();
    let inserted_slot = inserted
        .created
        .iter()
        .find_map(|created| match created {
            NativeCreated::Effect(slot) => Some(*slot),
            _ => None,
        })
        .unwrap();
    let before = editor.document().clone();
    let moved = editor
        .apply_plugin_command(
            editor.revision().document,
            NativeEditOwner::Human,
            &NativePluginCommand::MoveEffect {
                track: None,
                slot: inserted_slot,
                offset: isize::MIN,
            },
        )
        .unwrap();
    assert!(moved.changed);
    assert_ne!(editor.document(), &before);
    assert!(editor.can_undo());
    let slots: Vec<_> = editor
        .document()
        .content
        .processors
        .iter()
        .filter_map(|processor| match &processor.kind {
            ProcessorKind::Effect { slot, .. } if [first, second, inserted_slot].contains(slot) => {
                Some(*slot)
            }
            _ => None,
        })
        .collect();
    assert_eq!(slots.len(), 3);

    let snapshot = editor.document().clone();
    let revision = editor.revision().document;
    let noop = editor
        .apply_plugin_command(
            revision,
            NativeEditOwner::Human,
            &NativePluginCommand::MoveEffect {
                track: None,
                slot: inserted_slot,
                offset: 0,
            },
        )
        .unwrap();
    assert!(!noop.changed);
    assert_eq!(editor.document(), &snapshot);
    assert_eq!(editor.revision().document, revision);
}

#[test]
fn keyed_parameter_does_not_fallback_to_numeric_id_and_replace_state_uses_window() {
    let (mut editor, track, slot, _) = fixture();
    let invalid = editor.apply_plugin_command(
        0,
        NativeEditOwner::Human,
        &NativePluginCommand::SetParameter {
            target: ParamTarget::Instrument {
                track,
                param: ParamId(0),
            },
            key: "stable.cutoff".into(),
            value: 0.5,
        },
    );
    assert!(invalid.is_ok());
    let instrument_node = editor
        .document()
        .content
        .processors
        .iter()
        .find(|processor| matches!(&processor.kind, ProcessorKind::Instrument(_)))
        .map(|processor| processor.id)
        .unwrap();
    assert!(editor.document().content.processors.iter().any(|processor| {
        processor.id == instrument_node
            && matches!(&processor.kind, ProcessorKind::Instrument(plugin) if plugin.state.params["stable.cutoff"] == 0.5)
    }));
    editor
        .apply_plugin_command(
            editor.revision().document,
            NativeEditOwner::Human,
            &NativePluginCommand::ReplaceState {
                window: PluginWindow::Effect(slot),
                state: PluginState {
                    params: [("gain".into(), 0.75)].into(),
                    ..PluginState::default()
                },
            },
        )
        .unwrap();
    let node = effect_node(&editor, slot);
    assert!(editor.document().content.processors.iter().any(|processor| {
        processor.id == node
            && matches!(&processor.kind, ProcessorKind::Effect { plugin, .. } if plugin.state.params["gain"] == 0.75)
    }));
}

#[test]
fn stale_revision_and_foreign_owner_leave_history_untouched() {
    let (mut editor, track, _, _) = fixture();
    let before = editor.document().clone();
    let stale = editor.apply_plugin_command(
        99,
        NativeEditOwner::Human,
        &NativePluginCommand::SetInstrument {
            track,
            plugin: plugin("stale"),
        },
    );
    assert!(matches!(stale, Err(NativeEditError::StaleRevision { .. })));
    editor
        .begin_transaction(0, NativeEditOwner::Human, Edit::ChangeInstrument)
        .unwrap();
    let blocked = editor.apply_plugin_command(
        0,
        NativeEditOwner::Agent(5),
        &NativePluginCommand::SetInstrument {
            track,
            plugin: plugin("foreign"),
        },
    );
    assert!(matches!(blocked, Err(NativeEditError::TransactionActive)));
    assert_eq!(editor.document(), &before);
    editor
        .revert_transaction(0, NativeEditOwner::Human)
        .unwrap();
}
