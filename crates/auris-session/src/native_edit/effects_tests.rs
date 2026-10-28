use super::*;
use auris_core::Project;
use auris_core::pipeline::content::{ContentLimits, PluginInstance};
use auris_core::plugin::PluginState;

fn limits() -> ContentLimits {
    ContentLimits {
        sources: 64,
        clips: 128,
        stages: 512,
        processors: 512,
        groups: 128,
        tracks: 128,
        edges: 1024,
        external_ids: 128,
        automations: 128,
        automation_points: 1024,
        bindings: 128,
    }
}

#[test]
fn sidechain_cycle_rejects_the_whole_batch_without_consuming_an_id() {
    let mut project = Project::new("cycle", 48_000.0);
    let track = project.add_instrument_track("lead", "instrument");
    let slot = project.add_effect(Some(track), "compressor").unwrap();
    let native = auris_io::project_to_native(&project, limits()).unwrap();
    let node = native.content.processors.iter().find_map(|processor| {
        matches!(processor.kind, auris_core::pipeline::content::ProcessorKind::Effect { slot: id, .. } if id == slot)
            .then_some(processor.id)
    }).unwrap();
    let mut editor = NativeDocumentEditor::new(native, limits()).unwrap();
    let before = editor.document().clone();
    let result = editor.apply(
        0,
        NativeEditOwner::Human,
        Edit::ExternalChanges,
        &[
            NativeEdit::RenameTrack {
                track,
                name: "Must not land".into(),
            },
            NativeEdit::Effect(NativeEffectEdit::SetSidechain {
                node,
                source: Some(track),
            }),
        ],
    );
    assert!(result.is_err());
    assert_eq!(editor.document(), &before);
    assert_eq!(editor.revision(), NativeEditRevision::default());
    assert!(!editor.can_undo());
    assert!(!editor.is_dirty());
}

#[test]
fn custom_fan_in_and_sidechain_sources_are_preserved_by_refusing_chain_edits() {
    use auris_core::pipeline::content::ProcessorKind;
    use auris_core::pipeline::{EdgeId, EdgeRole, PipelineEdge, PortId};

    let mut project = Project::new("custom chain", 48_000.0);
    let track = project.add_instrument_track("lead", "instrument");
    let other = project.add_instrument_track("other", "instrument");
    let slot = project.add_effect(Some(track), "compressor").unwrap();
    let native = auris_io::project_to_native(&project, limits()).unwrap();
    let bindings = native
        .content
        .command_bindings(&native.external_ids(), limits())
        .unwrap();
    let effect = bindings.effect(Some(track), slot).unwrap();
    let strip = bindings.strip(Some(track)).unwrap();
    let other_strip = bindings.strip(Some(other)).unwrap();
    let instrument = bindings.instrument(other).unwrap();
    for (from, to, port, role) in [
        (other_strip, effect, "audio_in", EdgeRole::Main),
        (other_strip, strip, "audio_in", EdgeRole::Main),
        (instrument, effect, "sidechain_in", EdgeRole::Sidechain),
        (other_strip, effect, "custom_port", EdgeRole::Main),
    ] {
        let mut document = native.clone();
        document.content.edges.push(PipelineEdge {
            id: EdgeId(document.content.next_id),
            from: (from, PortId::new("audio_out")),
            to: (to, PortId::new(port)),
            role,
        });
        document.content.next_id += 1;
        let mut editor = NativeDocumentEditor::new(document, limits()).unwrap();
        let before = editor.document().clone();
        assert!(
            editor
                .apply(
                    0,
                    NativeEditOwner::Human,
                    Edit::RemoveEffect,
                    &[NativeEdit::Effect(NativeEffectEdit::Remove {
                        node: effect
                    })]
                )
                .is_err()
        );
        assert_eq!(editor.document(), &before);
        assert!(!editor.can_undo());
    }
    // A disconnected insert is authored content, not an unused allocation to discard.
    let mut document = native;
    let mut disconnected = document
        .content
        .processors
        .iter()
        .find(|p| p.id == effect)
        .unwrap()
        .clone();
    disconnected.id = auris_core::pipeline::NodeId(document.content.next_id);
    document.content.next_id += 1;
    if let ProcessorKind::Effect { slot, .. } = &mut disconnected.kind {
        *slot = auris_core::EffectSlotId(document.content.next_id);
        document.content.next_id += 1;
    }
    document.content.processors.push(disconnected);
    let mut editor = NativeDocumentEditor::new(document, limits()).unwrap();
    let before = editor.document().clone();
    assert!(
        editor
            .apply(
                0,
                NativeEditOwner::Human,
                Edit::RemoveEffect,
                &[NativeEdit::Effect(NativeEffectEdit::Remove {
                    node: effect
                })]
            )
            .is_err()
    );
    assert_eq!(editor.document(), &before);
}

fn editor() -> NativeDocumentEditor {
    NativeDocumentEditor::new(
        auris_io::project_to_native(&Project::new("effects", 48_000.0), limits()).unwrap(),
        limits(),
    )
    .unwrap()
}

fn group(editor: &mut NativeDocumentEditor) -> auris_core::pipeline::content::GroupId {
    let receipt = editor
        .apply(
            editor.revision().document,
            NativeEditOwner::Human,
            crate::Edit::AddInstrumentTrack,
            &[NativeEdit::Track(NativeTrackEdit::Add {
                name: "lead".into(),
                kind: NativeTrackKindSpec::Instrument(PluginInstance {
                    plugin_id: "test.instrument".into(),
                    state: PluginState::empty(),
                    file: None,
                }),
            })],
        )
        .unwrap();
    let track = receipt
        .created
        .into_iter()
        .find_map(|created| match created {
            NativeCreated::Track(id) => Some(id),
            _ => None,
        })
        .unwrap();
    editor
        .document()
        .content
        .tracks
        .iter()
        .find(|binding| binding.track == track)
        .unwrap()
        .group
}

fn plugin(name: &str) -> PluginInstance {
    PluginInstance {
        plugin_id: name.into(),
        state: PluginState::empty(),
        file: None,
    }
}

#[test]
fn insert_move_remove_preserve_chain_and_undo() {
    let mut editor = editor();
    let group = group(&mut editor);
    let first = editor
        .apply(
            editor.revision().document,
            NativeEditOwner::Human,
            crate::Edit::AddEffect,
            &[NativeEdit::Effect(NativeEffectEdit::Insert {
                group,
                index: 0,
                plugin: plugin("one"),
            })],
        )
        .unwrap()
        .created
        .into_iter()
        .find_map(|created| match created {
            NativeCreated::Node(node) => Some(node),
            _ => None,
        })
        .unwrap();
    let second = editor
        .apply(
            editor.revision().document,
            NativeEditOwner::Human,
            crate::Edit::AddEffect,
            &[NativeEdit::Effect(NativeEffectEdit::Insert {
                group,
                index: 1,
                plugin: plugin("two"),
            })],
        )
        .unwrap()
        .created
        .into_iter()
        .find_map(|created| match created {
            NativeCreated::Node(node) => Some(node),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        super::effects::effect_chain(editor.document(), group).unwrap(),
        vec![first, second]
    );
    editor
        .apply(
            editor.revision().document,
            NativeEditOwner::Human,
            crate::Edit::ReorderEffects,
            &[NativeEdit::Effect(NativeEffectEdit::Move {
                node: second,
                index: 0,
            })],
        )
        .unwrap();
    assert_eq!(
        super::effects::effect_chain(editor.document(), group).unwrap(),
        vec![second, first]
    );
    editor
        .apply(
            editor.revision().document,
            NativeEditOwner::Human,
            crate::Edit::RemoveEffect,
            &[NativeEdit::Effect(NativeEffectEdit::Remove {
                node: second,
            })],
        )
        .unwrap();
    assert_eq!(
        super::effects::effect_chain(editor.document(), group).unwrap(),
        vec![first]
    );
    editor.undo(editor.revision().document).unwrap();
    assert_eq!(
        super::effects::effect_chain(editor.document(), group).unwrap(),
        vec![second, first]
    );
}

#[test]
fn no_op_move_does_not_consume_an_identity() {
    let mut editor = editor();
    let group = group(&mut editor);
    let node = editor
        .apply(
            editor.revision().document,
            NativeEditOwner::Human,
            crate::Edit::AddEffect,
            &[NativeEdit::Effect(NativeEffectEdit::Insert {
                group,
                index: 0,
                plugin: plugin("one"),
            })],
        )
        .unwrap()
        .created
        .into_iter()
        .find_map(|created| match created {
            NativeCreated::Node(node) => Some(node),
            _ => None,
        })
        .unwrap();
    let before = editor.document().content.next_id;
    editor
        .apply(
            editor.revision().document,
            NativeEditOwner::Human,
            crate::Edit::ReorderEffects,
            &[NativeEdit::Effect(NativeEffectEdit::Move {
                node,
                index: 0,
            })],
        )
        .unwrap();
    assert_eq!(editor.document().content.next_id, before);
}

#[test]
fn sidechain_retarget_keeps_edge_identity_and_clear_removes_it() {
    let mut project = Project::new("sidechain retarget", 48_000.0);
    let first = project.add_instrument_track("target", "test.instrument");
    let second = project.add_instrument_track("key one", "test.instrument");
    let third = project.add_instrument_track("key two", "test.instrument");
    let mut editor = NativeDocumentEditor::new(
        auris_io::project_to_native(&project, limits()).unwrap(),
        limits(),
    )
    .unwrap();
    let group = editor
        .document()
        .content
        .tracks
        .iter()
        .find(|binding| binding.track == first)
        .unwrap()
        .group;
    let node = editor
        .apply(
            editor.revision().document,
            NativeEditOwner::Human,
            crate::Edit::AddEffect,
            &[NativeEdit::Effect(NativeEffectEdit::Insert {
                group,
                index: 0,
                plugin: plugin("effect"),
            })],
        )
        .unwrap()
        .created
        .into_iter()
        .find_map(|created| match created {
            NativeCreated::Node(node) => Some(node),
            _ => None,
        })
        .unwrap();
    editor
        .apply(
            editor.revision().document,
            NativeEditOwner::Human,
            crate::Edit::SetEffectSidechain,
            &[NativeEdit::Effect(NativeEffectEdit::SetSidechain {
                node,
                source: Some(second),
            })],
        )
        .unwrap();
    let sidechain_id = editor
        .document()
        .content
        .edges
        .iter()
        .find(|edge| edge.to.0 == node && edge.role == auris_core::pipeline::EdgeRole::Sidechain)
        .unwrap()
        .id;
    editor
        .apply(
            editor.revision().document,
            NativeEditOwner::Human,
            crate::Edit::SetEffectSidechain,
            &[NativeEdit::Effect(NativeEffectEdit::SetSidechain {
                node,
                source: Some(third),
            })],
        )
        .unwrap();
    assert_eq!(
        editor
            .document()
            .content
            .edges
            .iter()
            .find(|edge| edge.to.0 == node && edge.role == auris_core::pipeline::EdgeRole::Sidechain)
            .unwrap()
            .id,
        sidechain_id
    );
    editor
        .apply(
            editor.revision().document,
            NativeEditOwner::Human,
            crate::Edit::SetEffectSidechain,
            &[NativeEdit::Effect(NativeEffectEdit::SetSidechain {
                node,
                source: None,
            })],
        )
        .unwrap();
    assert!(!editor.document().content.edges.iter().any(|edge| {
        edge.to.0 == node && edge.role == auris_core::pipeline::EdgeRole::Sidechain
    }));
}

#[test]
fn branched_chain_is_rejected_without_mutating_or_allocating() {
    let mut editor = editor();
    let group = group(&mut editor);
    let node = editor
        .apply(
            editor.revision().document,
            NativeEditOwner::Human,
            crate::Edit::AddEffect,
            &[NativeEdit::Effect(NativeEffectEdit::Insert {
                group,
                index: 0,
                plugin: plugin("effect"),
            })],
        )
        .unwrap()
        .created
        .into_iter()
        .find_map(|created| match created {
            NativeCreated::Node(node) => Some(node),
            _ => None,
        })
        .unwrap();
    let mut document = editor.document().clone();
    let sum = document
        .content
        .processors
        .iter()
        .find(|processor| {
            processor.group == Some(group)
                && matches!(
                    &processor.kind,
                    auris_core::pipeline::content::ProcessorKind::Sum
                )
        })
        .unwrap()
        .id;
    let next_id = document.content.next_id;
    document
        .content
        .edges
        .push(auris_core::pipeline::PipelineEdge {
            id: auris_core::pipeline::EdgeId(next_id),
            from: (sum, auris_core::pipeline::PortId::new("audio_out")),
            to: (node, auris_core::pipeline::PortId::new("audio_in")),
            role: auris_core::pipeline::EdgeRole::Main,
        });
    document.content.next_id += 1;
    let mut branched = NativeDocumentEditor::new(document, limits()).unwrap();
    let before = branched.document().clone();
    let before_id = branched.document().content.next_id;
    assert!(
        branched
            .apply(
                branched.revision().document,
                NativeEditOwner::Human,
                crate::Edit::AddEffect,
                &[NativeEdit::Effect(NativeEffectEdit::Insert {
                    group,
                    index: 0,
                    plugin: plugin("new"),
                })],
            )
            .is_err()
    );
    assert_eq!(branched.document(), &before);
    assert_eq!(branched.document().content.next_id, before_id);
}

#[test]
fn existing_cycle_is_rejected_before_effect_edit() {
    let mut editor = editor();
    let group = group(&mut editor);
    let mut document = editor.document().clone();
    let strip = document
        .content
        .groups
        .iter()
        .find(|entry| entry.id == group)
        .unwrap()
        .strip;
    let sum = document
        .content
        .processors
        .iter()
        .find(|processor| {
            processor.group == Some(group)
                && matches!(
                    &processor.kind,
                    auris_core::pipeline::content::ProcessorKind::Sum
                )
        })
        .unwrap()
        .id;
    let edge = document
        .content
        .edges
        .iter()
        .find(|edge| edge.from.0 == sum && edge.to.0 == strip)
        .unwrap();
    let role = edge.role;
    let cycle_id = document.content.next_id;
    document.content.next_id += 1;
    document
        .content
        .edges
        .push(auris_core::pipeline::PipelineEdge {
            id: auris_core::pipeline::EdgeId(cycle_id),
            from: (strip, auris_core::pipeline::PortId::new("audio_out")),
            to: (sum, auris_core::pipeline::PortId::new("audio_in")),
            role,
        });
    let mut cyclic = NativeDocumentEditor::new(document, limits()).unwrap();
    let before = cyclic.document().clone();
    assert!(
        cyclic
            .apply(
                cyclic.revision().document,
                NativeEditOwner::Human,
                crate::Edit::AddEffect,
                &[NativeEdit::Effect(NativeEffectEdit::Insert {
                    group,
                    index: 0,
                    plugin: plugin("new"),
                })],
            )
            .is_err()
    );
    assert_eq!(cyclic.document(), &before);
}

#[test]
fn stale_revision_and_wrong_owner_do_not_change_effect_chain() {
    let mut editor = editor();
    let group = group(&mut editor);
    let before = editor.document().clone();
    assert!(
        editor
            .apply(
                99,
                NativeEditOwner::Human,
                crate::Edit::AddEffect,
                &[NativeEdit::Effect(NativeEffectEdit::Insert {
                    group,
                    index: 0,
                    plugin: plugin("new"),
                })],
            )
            .is_err()
    );
    assert_eq!(editor.document(), &before);
}

#[test]
fn processor_limit_rejects_insert_before_consuming_ids() {
    let base = editor();
    let group = base.document().content.master;
    let mut constrained = limits();
    constrained.processors = base.document().content.processors.len();
    constrained.edges = base.document().content.edges.len();
    let mut editor = NativeDocumentEditor::new(base.document().clone(), constrained).unwrap();
    let before = editor.document().clone();
    assert!(
        editor
            .apply(
                editor.revision().document,
                NativeEditOwner::Human,
                crate::Edit::AddEffect,
                &[NativeEdit::Effect(NativeEffectEdit::Insert {
                    group,
                    index: 0,
                    plugin: plugin("unavailable-at-runtime"),
                })],
            )
            .is_err()
    );
    assert_eq!(editor.document(), &before);
}

#[test]
fn one_new_insert_edge_fits_exact_edge_limit() {
    let base = editor();
    let group = base.document().content.master;
    let mut constrained = limits();
    constrained.edges = base.document().content.edges.len() + 1;
    let mut editor = NativeDocumentEditor::new(base.document().clone(), constrained).unwrap();
    assert!(
        editor
            .apply(
                editor.revision().document,
                NativeEditOwner::Human,
                crate::Edit::AddEffect,
                &[NativeEdit::Effect(NativeEffectEdit::Insert {
                    group,
                    index: 0,
                    plugin: plugin("fits"),
                })],
            )
            .is_ok()
    );
}
