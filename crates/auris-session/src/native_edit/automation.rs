//! Identity based edits for project-time automation in the native document.
//!
//! An automation curve is an authored object, while its source processor and control binding
//! are graph objects.  The operations below keep those three lifetimes explicit: adding a curve
//! creates all three, changing points only changes the curve, and removal requires the binding to
//! have been detached first.  No plugin is loaded and no legacy `Project` is reconstructed here.

use auris_core::automation::{AutomationCurve, AutomationPoint};
use auris_core::pipeline::content::{
    AutomationBinding, AutomationDocument, AutomationId, ContentError, ContentLimits, ContentRef,
    ParameterAddress, Processor, ProcessorKind,
};
use auris_core::pipeline::document::NativeDocument;
use auris_core::pipeline::{ControlDomain, EdgeId, NodeId};

use super::{NativeCreated, NativeEditError, allocate};

/// A bounded edit to a shared project-time automation curve and its native graph connections.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub enum NativeAutomationEdit {
    /// Creates a curve, an independent automation source processor and one control binding.
    Add {
        /// Value domain of the curve, supplied by a built-in or resolved plugin descriptor.
        domain: ControlDomain,
        /// Interpolation between points.
        curve: AutomationCurve,
        /// Strictly increasing project-tick points.
        points: Vec<AutomationPoint>,
        /// Native processor and stable parameter key receiving the values.
        target: ParameterAddress,
        /// Whether the binding is active immediately.
        enabled: bool,
    },
    /// Adds another target binding to an existing shared curve.
    AddBinding {
        /// Curve identity.
        automation: AutomationId,
        /// Source processor identity for the curve.
        source: NodeId,
        /// Native processor and stable parameter key receiving the values.
        target: ParameterAddress,
        /// Whether the control stream is active immediately.
        enabled: bool,
    },
    /// Changes interpolation while preserving points, identity and bindings.
    SetCurve {
        /// Curve identity.
        automation: AutomationId,
        /// New interpolation mode.
        curve: AutomationCurve,
    },
    /// Replaces all points while retaining curve and binding identities.
    ReplacePoints {
        /// Curve identity.
        automation: AutomationId,
        /// New strictly increasing project-tick points.
        points: Vec<AutomationPoint>,
    },
    /// Enables or disables a binding without deleting authored automation.
    SetBindingEnabled {
        /// Binding identity.
        binding: EdgeId,
        /// Whether the control stream is active.
        enabled: bool,
    },
    /// Detaches a binding from its source curve.
    RemoveBinding {
        /// Binding identity.
        binding: EdgeId,
    },
    /// Removes a curve and its source processor after all bindings were detached.
    Remove {
        /// Curve identity.
        automation: AutomationId,
        /// Source processor identity created with the curve.
        source: NodeId,
    },
}

/// Applies an automation edit to a cloned native candidate.
pub(super) fn apply(
    document: &mut NativeDocument,
    edit: &NativeAutomationEdit,
    limits: ContentLimits,
) -> Result<Vec<NativeCreated>, NativeEditError> {
    match edit {
        NativeAutomationEdit::Add {
            domain,
            curve,
            points,
            target,
            enabled,
        } => add(
            document,
            AddCurve {
                domain: *domain,
                curve: *curve,
                points,
                target,
                enabled: *enabled,
                limits,
            },
        ),
        NativeAutomationEdit::AddBinding {
            automation,
            source,
            target,
            enabled,
        } => add_binding(document, *automation, *source, target, *enabled, limits),
        NativeAutomationEdit::ReplacePoints { automation, points } => {
            replace_points(document, *automation, points)
        }
        NativeAutomationEdit::SetCurve { automation, curve } => {
            let entry = document
                .content
                .automation
                .iter_mut()
                .find(|candidate| candidate.id == *automation)
                .ok_or(ContentError::Missing(ContentRef::Automation(*automation)))?;
            let Some(domain) = entry.domain else {
                return Err(NativeEditError::Invalid(
                    "an unresolved automation domain cannot be edited".into(),
                ));
            };
            let replacement =
                AutomationDocument::new(*automation, domain, *curve, entry.points().to_vec())
                    .map_err(|error| NativeEditError::Content(ContentError::Automation(error)))?;
            *entry = replacement;
            Ok(Vec::new())
        }
        NativeAutomationEdit::SetBindingEnabled { binding, enabled } => {
            let existing = document
                .content
                .automation_bindings
                .iter()
                .find(|candidate| candidate.id == *binding)
                .ok_or(ContentError::Missing(ContentRef::Edge(*binding)))?;
            if *enabled {
                let target = existing.target.clone();
                let curve_id = document
                    .content
                    .processors
                    .iter()
                    .find(|processor| processor.id == existing.source)
                    .and_then(|processor| match processor.kind {
                        ProcessorKind::Automation(id) => Some(id),
                        _ => None,
                    })
                    .ok_or(ContentError::Role(
                        ContentRef::Edge(*binding),
                        "binding source is not an automation processor",
                    ))?;
                let domain = document
                    .content
                    .automation
                    .iter()
                    .find(|curve| curve.id == curve_id)
                    .ok_or(ContentError::Missing(ContentRef::Automation(curve_id)))?
                    .domain
                    .ok_or_else(|| {
                        NativeEditError::Invalid(
                            "an unresolved automation domain cannot be enabled".into(),
                        )
                    })?;
                validate_target_domain(document, &target, domain)?;
                if document
                    .content
                    .automation_bindings
                    .iter()
                    .any(|candidate| {
                        candidate.id != *binding && candidate.target == target && candidate.enabled
                    })
                {
                    return Err(NativeEditError::Invalid(
                        "automation target already has an enabled binding".into(),
                    ));
                }
            }
            let binding = document
                .content
                .automation_bindings
                .iter_mut()
                .find(|candidate| candidate.id == *binding)
                .ok_or(ContentError::Missing(ContentRef::Edge(*binding)))?;
            binding.enabled = *enabled;
            Ok(Vec::new())
        }
        NativeAutomationEdit::RemoveBinding { binding } => {
            let before = document.content.automation_bindings.len();
            document
                .content
                .automation_bindings
                .retain(|candidate| candidate.id != *binding);
            if before == document.content.automation_bindings.len() {
                return Err(ContentError::Missing(ContentRef::Edge(*binding)).into());
            }
            Ok(Vec::new())
        }
        NativeAutomationEdit::Remove { automation, source } => {
            remove(document, *automation, *source, limits)?;
            Ok(Vec::new())
        }
    }
}

struct AddCurve<'a> {
    domain: ControlDomain,
    curve: AutomationCurve,
    points: &'a [AutomationPoint],
    target: &'a ParameterAddress,
    enabled: bool,
    limits: ContentLimits,
}

fn add(
    document: &mut NativeDocument,
    request: AddCurve<'_>,
) -> Result<Vec<NativeCreated>, NativeEditError> {
    let AddCurve {
        domain,
        curve,
        points,
        target,
        enabled,
        limits,
    } = request;
    if target.key.is_empty() {
        return Err(ContentError::ParameterKeyRequired.into());
    }
    if !document.content.processors.iter().any(|processor| {
        processor.id == target.node && !matches!(processor.kind, ProcessorKind::Automation(_))
    }) {
        return Err(ContentError::Missing(ContentRef::Node(target.node)).into());
    }
    validate_target_domain(document, target, domain)?;
    if document
        .content
        .automation_bindings
        .iter()
        .any(|binding| binding.target == *target)
    {
        return Err(NativeEditError::Invalid(
            "automation target already has a binding".into(),
        ));
    }
    if document.content.automation.len() >= limits.automations {
        return Err(ContentError::Limit("automations").into());
    }
    let points_used = document
        .content
        .automation
        .iter()
        .fold(0usize, |total, curve| {
            total.saturating_add(curve.points().len())
        });
    if points.len() > limits.automation_points.saturating_sub(points_used) {
        return Err(ContentError::Limit("automation_points").into());
    }
    // Validate the complete payload before consuming any native identity.
    AutomationDocument::new(AutomationId(1), domain, curve, points.to_vec())
        .map_err(|error| NativeEditError::Content(ContentError::Automation(error)))?;
    let automation = AutomationId(allocate(document)?);
    let source = NodeId(allocate(document)?);
    let binding_id = EdgeId(allocate(document)?);
    document.content.automation.push(
        AutomationDocument::new(automation, domain, curve, points.to_vec())
            .map_err(|error| NativeEditError::Content(ContentError::Automation(error)))?,
    );
    document.content.processors.push(Processor {
        id: source,
        group: None,
        kind: ProcessorKind::Automation(automation),
    });
    document
        .content
        .automation_bindings
        .push(AutomationBinding {
            id: binding_id,
            source,
            target: target.clone(),
            enabled,
        });
    Ok(vec![
        NativeCreated::Automation(automation),
        NativeCreated::Node(source),
        NativeCreated::Edge(binding_id),
    ])
}

fn validate_target_domain(
    document: &NativeDocument,
    target: &ParameterAddress,
    domain: ControlDomain,
) -> Result<(), NativeEditError> {
    let processor = document
        .content
        .processors
        .iter()
        .find(|processor| processor.id == target.node)
        .ok_or(ContentError::Missing(ContentRef::Node(target.node)))?;
    let descriptor = match (&processor.kind, target.key.as_str()) {
        (ProcessorKind::Strip(_), "gain") => auris_core::param::MixerControl::Gain.descriptor(),
        (ProcessorKind::Strip(_), "pan") => auris_core::param::MixerControl::Pan.descriptor(),
        (ProcessorKind::Send { .. }, "send") => auris_core::param::MixerControl::Send.descriptor(),
        (ProcessorKind::Strip(_) | ProcessorKind::Send { .. }, _) => {
            return Err(NativeEditError::Invalid(
                "automation target key is not a capability of this mixer processor".into(),
            ));
        }
        _ => {
            return Err(NativeEditError::Invalid(
                "automation target capability is not resolved for this processor".into(),
            ));
        }
    };
    let expected = ControlDomain::Native {
        unit: descriptor.unit,
        min: descriptor.min,
        max: descriptor.max,
    };
    if domain != expected {
        return Err(NativeEditError::Invalid(
            "automation domain does not match the target parameter".into(),
        ));
    }
    Ok(())
}

fn add_binding(
    document: &mut NativeDocument,
    automation: AutomationId,
    source: NodeId,
    target: &ParameterAddress,
    enabled: bool,
    limits: ContentLimits,
) -> Result<Vec<NativeCreated>, NativeEditError> {
    if target.key.is_empty() {
        return Err(ContentError::ParameterKeyRequired.into());
    }
    let curve = document
        .content
        .automation
        .iter()
        .find(|candidate| candidate.id == automation)
        .ok_or(ContentError::Missing(ContentRef::Automation(automation)))?;
    let source_processor = document
        .content
        .processors
        .iter()
        .find(|candidate| candidate.id == source)
        .ok_or(ContentError::Missing(ContentRef::Node(source)))?;
    if source_processor.group.is_some()
        || !matches!(source_processor.kind, ProcessorKind::Automation(id) if id == automation)
    {
        return Err(ContentError::Role(
            ContentRef::Node(source),
            "binding source does not own the requested automation curve",
        )
        .into());
    }
    if !document.content.processors.iter().any(|processor| {
        processor.id == target.node && !matches!(processor.kind, ProcessorKind::Automation(_))
    }) {
        return Err(ContentError::Missing(ContentRef::Node(target.node)).into());
    }
    let Some(domain) = curve.domain else {
        return Err(NativeEditError::Invalid(
            "an unresolved automation domain cannot be bound".into(),
        ));
    };
    validate_target_domain(document, target, domain)?;
    if document.content.automation_bindings.len() >= limits.bindings {
        return Err(ContentError::Limit("bindings").into());
    }
    if document
        .content
        .automation_bindings
        .iter()
        .any(|binding| binding.target == *target)
    {
        return Err(NativeEditError::Invalid(
            "automation target already has a binding from this curve".into(),
        ));
    }
    let binding_id = EdgeId(allocate(document)?);
    document
        .content
        .automation_bindings
        .push(AutomationBinding {
            id: binding_id,
            source,
            target: target.clone(),
            enabled,
        });
    Ok(vec![NativeCreated::Edge(binding_id)])
}

fn replace_points(
    document: &mut NativeDocument,
    automation: AutomationId,
    points: &[AutomationPoint],
) -> Result<Vec<NativeCreated>, NativeEditError> {
    let entry = document
        .content
        .automation
        .iter_mut()
        .find(|candidate| candidate.id == automation)
        .ok_or(ContentError::Missing(ContentRef::Automation(automation)))?;
    let Some(domain) = entry.domain else {
        return Err(NativeEditError::Invalid(
            "an unresolved automation domain cannot be edited".into(),
        ));
    };
    let replacement = AutomationDocument::new(automation, domain, entry.curve, points.to_vec())
        .map_err(|error| NativeEditError::Content(ContentError::Automation(error)))?;
    *entry = replacement;
    Ok(Vec::new())
}

fn remove(
    document: &mut NativeDocument,
    automation: AutomationId,
    source: NodeId,
    limits: ContentLimits,
) -> Result<(), NativeEditError> {
    let curve_id = document
        .content
        .automation
        .iter()
        .find(|candidate| candidate.id == automation)
        .ok_or(ContentError::Missing(ContentRef::Automation(automation)))?
        .id;
    let processor = document
        .content
        .processors
        .iter()
        .find(|candidate| candidate.id == source)
        .ok_or(ContentError::Missing(ContentRef::Node(source)))?;
    if !matches!(processor.kind, ProcessorKind::Automation(id) if id == automation)
        || processor.group.is_some()
    {
        return Err(ContentError::Role(
            ContentRef::Node(source),
            "source processor does not own the requested automation curve",
        )
        .into());
    }
    if document
        .content
        .automation_bindings
        .iter()
        .any(|binding| binding.source == source)
    {
        return Err(
            ContentError::AutomationInUse(automation, vec![ContentRef::Node(source)]).into(),
        );
    }
    document
        .content
        .processors
        .retain(|candidate| candidate.id != source);
    document
        .content
        .automation
        .retain(|candidate| candidate.id != curve_id);
    document
        .content
        .validate_ownership(&document.external_ids(), limits)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Edit;
    use crate::native_edit::{NativeDocumentEditor, NativeEdit, NativeEditOwner};
    use auris_core::automation::AutomationPoint;
    use auris_core::pipeline::content::ProcessorKind;
    use auris_core::project::Project;
    use auris_core::time::Ticks;

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

    fn fixture() -> (NativeDocumentEditor, NodeId) {
        let mut project = Project::new("automation", 48_000.0);
        project.add_instrument_track("Keys", "unavailable.test.instrument");
        let native = auris_io::project_to_native(&project, limits()).unwrap();
        let node = native
            .content
            .tracks
            .first()
            .and_then(|track| {
                native
                    .content
                    .groups
                    .iter()
                    .find(|group| group.id == track.group)
                    .map(|group| group.strip)
            })
            .unwrap();
        (NativeDocumentEditor::new(native, limits()).unwrap(), node)
    }

    fn add_edit(node: NodeId) -> NativeEdit {
        NativeEdit::Automation(NativeAutomationEdit::Add {
            domain: ControlDomain::Native {
                unit: auris_core::ParamUnit::Decibels,
                min: -60.0,
                max: 12.0,
            },
            curve: AutomationCurve::Linear,
            points: vec![
                AutomationPoint::new(Ticks::ZERO, -12.0),
                AutomationPoint::new(Ticks::QUARTER, 0.0),
            ],
            target: ParameterAddress {
                node,
                key: "gain".into(),
            },
            enabled: true,
        })
    }

    fn add_edit_target(node: NodeId, key: &str) -> NativeEdit {
        let NativeEdit::Automation(NativeAutomationEdit::Add {
            domain,
            curve,
            points,
            enabled,
            ..
        }) = add_edit(node)
        else {
            unreachable!();
        };
        NativeEdit::Automation(NativeAutomationEdit::Add {
            domain,
            curve,
            points,
            target: ParameterAddress {
                node,
                key: key.into(),
            },
            enabled,
        })
    }

    #[test]
    fn automation_batch_is_atomic_and_rejects_invalid_points() {
        let (mut editor, node) = fixture();
        let before = editor.document().clone();
        let invalid = NativeEdit::Automation(NativeAutomationEdit::Add {
            domain: ControlDomain::Native {
                unit: auris_core::ParamUnit::Decibels,
                min: -60.0,
                max: 12.0,
            },
            curve: AutomationCurve::Linear,
            points: vec![
                AutomationPoint::new(Ticks::QUARTER, 0.0),
                AutomationPoint::new(Ticks::ZERO, 0.0),
            ],
            target: ParameterAddress {
                node,
                key: "gain".into(),
            },
            enabled: true,
        });
        assert!(
            editor
                .apply(0, NativeEditOwner::Human, Edit::ExternalChanges, &[invalid])
                .is_err()
        );
        assert_eq!(editor.document(), &before);
    }

    #[test]
    fn automation_ids_survive_undo_redo_and_native_round_trip() {
        let (mut editor, node) = fixture();
        let receipt = editor
            .apply(
                0,
                NativeEditOwner::Human,
                Edit::ExternalChanges,
                &[add_edit(node)],
            )
            .unwrap();
        let automation = editor.document().content.automation[0].id;
        let source = editor
            .document()
            .content
            .processors
            .iter()
            .find_map(|processor| {
                matches!(processor.kind, ProcessorKind::Automation(id) if id == automation)
                    .then_some(processor.id)
            })
            .unwrap();
        assert!(
            receipt
                .created
                .iter()
                .any(|created| matches!(created, NativeCreated::Node(id) if *id == source))
        );
        editor.undo(1).unwrap();
        editor.redo(2).unwrap();
        assert_eq!(editor.document().content.automation[0].id, automation);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Automation.auris");
        auris_io::save_native(&path, editor.document(), limits()).unwrap();
        let reopened = auris_io::load_native(&path, limits()).unwrap();
        assert_eq!(reopened.content.automation[0].id, automation);
        assert_eq!(reopened.content.automation_bindings[0].source, source);
    }

    #[test]
    fn stale_revision_does_not_consume_automation_edit() {
        let (mut editor, node) = fixture();
        editor
            .apply(
                0,
                NativeEditOwner::Human,
                Edit::ExternalChanges,
                &[add_edit(node)],
            )
            .unwrap();
        let before = editor.document().clone();
        assert!(matches!(
            editor.apply(
                0,
                NativeEditOwner::Human,
                Edit::ExternalChanges,
                &[add_edit(node)]
            ),
            Err(super::super::NativeEditError::StaleRevision { .. })
        ));
        assert_eq!(editor.document(), &before);
    }

    #[test]
    fn one_curve_can_drive_multiple_mixer_targets_until_bindings_are_removed() {
        let (mut editor, node) = fixture();
        editor
            .apply(
                0,
                NativeEditOwner::Human,
                Edit::ExternalChanges,
                &[add_edit(node)],
            )
            .unwrap();
        let automation = editor.document().content.automation[0].id;
        let source = editor
            .document()
            .content
            .processors
            .iter()
            .find_map(|processor| {
                matches!(processor.kind, ProcessorKind::Automation(id) if id == automation)
                    .then_some(processor.id)
            })
            .unwrap();
        let master = editor.document().content.groups[0].strip;
        let receipt = editor
            .apply(
                1,
                NativeEditOwner::Human,
                Edit::ExternalChanges,
                &[NativeEdit::Automation(NativeAutomationEdit::AddBinding {
                    automation,
                    source,
                    target: ParameterAddress {
                        node: master,
                        key: "gain".into(),
                    },
                    enabled: true,
                })],
            )
            .unwrap();
        assert_eq!(receipt.created.len(), 1);
        assert_eq!(editor.document().content.automation_bindings.len(), 2);
        let first_binding = editor.document().content.automation_bindings[0].id;
        editor
            .apply(
                2,
                NativeEditOwner::Human,
                Edit::ExternalChanges,
                &[NativeEdit::Automation(
                    NativeAutomationEdit::RemoveBinding {
                        binding: first_binding,
                    },
                )],
            )
            .unwrap();
        assert_eq!(editor.document().content.automation.len(), 1);
        assert_eq!(editor.document().content.automation_bindings.len(), 1);
    }

    #[test]
    fn an_independent_curve_cannot_claim_an_existing_target() {
        let (mut editor, node) = fixture();
        editor
            .apply(
                0,
                NativeEditOwner::Human,
                Edit::ExternalChanges,
                &[add_edit(node)],
            )
            .unwrap();
        let result = editor.apply(
            1,
            NativeEditOwner::Human,
            Edit::ExternalChanges,
            &[add_edit(node)],
        );
        assert!(matches!(
            result,
            Err(super::super::NativeEditError::Invalid(_))
        ));
        assert_eq!(editor.document().content.automation.len(), 1);
    }

    #[test]
    fn another_owner_cannot_edit_during_an_automation_gesture() {
        let (mut editor, node) = fixture();
        editor
            .begin_transaction(0, NativeEditOwner::Human, Edit::ExternalChanges)
            .unwrap();
        let result = editor.apply(
            0,
            NativeEditOwner::Agent(7),
            Edit::ExternalChanges,
            &[add_edit(node)],
        );
        assert!(matches!(
            result,
            Err(super::super::NativeEditError::TransactionActive)
        ));
        editor.end_transaction(0, NativeEditOwner::Human).unwrap();
    }

    #[test]
    fn automation_rejects_plugin_sum_and_wrong_mixer_capabilities() {
        let (mut editor, strip) = fixture();
        let instrument = editor
            .document()
            .content
            .processors
            .iter()
            .find(|processor| matches!(processor.kind, ProcessorKind::Instrument(_)))
            .map(|processor| processor.id)
            .unwrap();
        let sum = editor
            .document()
            .content
            .processors
            .iter()
            .find(|processor| matches!(processor.kind, ProcessorKind::Sum))
            .map(|processor| processor.id)
            .unwrap();
        for (node, key) in [(instrument, "gain"), (sum, "pan"), (strip, "send")] {
            let result = editor.apply(
                editor.revision().document,
                NativeEditOwner::Human,
                Edit::ExternalChanges,
                &[add_edit_target(node, key)],
            );
            assert!(matches!(
                result,
                Err(super::super::NativeEditError::Invalid(_))
            ));
            assert!(editor.document().content.automation.is_empty());
        }
    }

    #[test]
    fn send_accepts_only_its_send_descriptor() {
        let (mut editor, _) = fixture();
        let track = editor.document().content.tracks[0].track;
        let destination = editor.document().content.master;
        editor
            .apply(
                0,
                NativeEditOwner::Human,
                Edit::ExternalChanges,
                &[NativeEdit::Routing(
                    crate::native_edit::NativeRoutingEdit::AddSend {
                        track,
                        destination,
                        level_db: -12.0,
                        pre_fader: false,
                    },
                )],
            )
            .unwrap();
        let send = editor
            .document()
            .content
            .processors
            .iter()
            .find(|processor| matches!(processor.kind, ProcessorKind::Send { .. }))
            .map(|processor| processor.id)
            .unwrap();
        assert!(matches!(
            editor.apply(
                1,
                NativeEditOwner::Human,
                Edit::ExternalChanges,
                &[add_edit_target(send, "gain")],
            ),
            Err(super::super::NativeEditError::Invalid(_))
        ));
        let valid = NativeEdit::Automation(NativeAutomationEdit::Add {
            domain: ControlDomain::Native {
                unit: auris_core::ParamUnit::Decibels,
                min: -60.0,
                max: 0.0,
            },
            curve: AutomationCurve::Linear,
            points: vec![AutomationPoint::new(Ticks::ZERO, -12.0)],
            target: ParameterAddress {
                node: send,
                key: "send".into(),
            },
            enabled: true,
        });
        editor
            .apply(1, NativeEditOwner::Human, Edit::ExternalChanges, &[valid])
            .unwrap();
        assert_eq!(editor.document().content.automation.len(), 1);
    }

    #[test]
    fn editor_commit_rejects_a_candidate_over_the_automation_point_budget() {
        let (mut editor, _) = fixture();
        let mut snapshot = editor.document().clone();
        let points = (0..=limits().automation_points)
            .map(|index| AutomationPoint::new(Ticks(index as i64), 0.0))
            .collect();
        let id = AutomationId(snapshot.content.next_id);
        snapshot.content.next_id += 1;
        snapshot.content.automation.push(
            AutomationDocument::new(
                id,
                ControlDomain::Native {
                    unit: auris_core::ParamUnit::Decibels,
                    min: -60.0,
                    max: 12.0,
                },
                AutomationCurve::Linear,
                points,
            )
            .unwrap(),
        );
        let result =
            editor.replace_snapshot(0, NativeEditOwner::Human, Edit::ExternalChanges, snapshot);
        assert!(matches!(
            result,
            Err(super::super::NativeEditError::Content(
                auris_core::pipeline::content::ContentError::Limit("automation_points")
            ))
        ));
        assert!(editor.document().content.automation.is_empty());
    }
}
