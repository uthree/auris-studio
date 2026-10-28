//! High-level mixer and automation commands addressed by the legacy stable parameter targets.
//!
//! These adapters resolve a target against the native content snapshot and then submit one or
//! more identity-based operations through [`NativeDocumentEditor::apply`]. They never construct a
//! mutable legacy `Project`; plugin parameters require an explicit stable key and are therefore
//! rejected until the host has resolved that key.

use auris_core::automation::{AutomationCurve, AutomationPoint};
use auris_core::param::{ParamTarget, mixer_descriptor};
use auris_core::pipeline::ControlDomain;
use auris_core::pipeline::content::AutomationId;
use auris_core::pipeline::document::NativeDocument;
use auris_core::time::Ticks;

use super::{
    NativeAutomationEdit, NativeDocumentEditor, NativeEdit, NativeEditError, NativeEditOwner,
    NativeEditReceipt, NativeRoutingEdit,
};
use crate::Edit;

impl NativeDocumentEditor {
    /// Sets one built-in mixer control through its persistent native processor identity.
    pub fn set_mixer_parameter(
        &mut self,
        expected_revision: u64,
        owner: NativeEditOwner,
        target: ParamTarget,
        value: f32,
    ) -> Result<NativeEditReceipt, NativeEditError> {
        self.check_revision(expected_revision)?;
        self.check_owner(owner)?;
        let bindings = self
            .document
            .content
            .command_bindings(&self.document.external_ids(), self.limits)?;
        let operation = match target {
            ParamTarget::TrackGain(track) => NativeEdit::SetStrip {
                node: bindings.strip(Some(track))?,
                gain_db: Some(value),
                pan: None,
                mute: None,
                solo: None,
            },
            ParamTarget::TrackPan(track) => NativeEdit::SetStrip {
                node: bindings.strip(Some(track))?,
                gain_db: None,
                pan: Some(value),
                mute: None,
                solo: None,
            },
            ParamTarget::MasterGain => NativeEdit::SetStrip {
                node: bindings.strip(None)?,
                gain_db: Some(value),
                pan: None,
                mute: None,
                solo: None,
            },
            ParamTarget::MasterPan => NativeEdit::SetStrip {
                node: bindings.strip(None)?,
                gain_db: None,
                pan: Some(value),
                mute: None,
                solo: None,
            },
            ParamTarget::Send { track, send } => {
                bindings.send(track, send)?;
                NativeEdit::Routing(NativeRoutingEdit::SetSendLevel {
                    send,
                    level_db: value,
                })
            }
            _ => {
                return Err(NativeEditError::Invalid(
                    "native mixer parameters currently support built-in controls only".into(),
                ));
            }
        };
        self.apply(
            expected_revision,
            owner,
            Edit::AdjustParameter(target),
            &[operation],
        )
    }

    /// Writes one point, creating a native curve and binding when needed.
    pub fn set_automation_point(
        &mut self,
        expected_revision: u64,
        owner: NativeEditOwner,
        target: ParamTarget,
        at: Ticks,
        value: f32,
    ) -> Result<NativeEditReceipt, NativeEditError> {
        self.check_revision(expected_revision)?;
        self.check_owner(owner)?;
        if !value.is_finite() {
            return Err(NativeEditError::Invalid(
                "automation value must be finite".into(),
            ));
        }
        let address = resolve_parameter(&self.document, self.limits, target)?;
        let descriptor = mixer_descriptor(target).ok_or_else(|| {
            NativeEditError::Invalid("plugin automation requires a resolved descriptor".into())
        })?;
        let value = descriptor.clamp(value);
        let tick = at.max_zero();
        let operation =
            if let Some((automation, points)) = curve_for_target(&self.document, &address)? {
                let mut points = points;
                points.retain(|point| point.tick != tick);
                points.push(AutomationPoint::new(tick, value));
                points.sort_by_key(|point| point.tick);
                NativeEdit::Automation(NativeAutomationEdit::ReplacePoints { automation, points })
            } else {
                NativeEdit::Automation(NativeAutomationEdit::Add {
                    domain: ControlDomain::Native {
                        unit: descriptor.unit,
                        min: descriptor.min,
                        max: descriptor.max,
                    },
                    curve: AutomationCurve::Linear,
                    points: vec![AutomationPoint::new(tick, value)],
                    target: address,
                    enabled: true,
                })
            };
        self.apply(
            expected_revision,
            owner,
            Edit::WriteAutomation(target),
            &[operation],
        )
    }

    /// Moves one point and replaces a point already present at the destination tick.
    pub fn move_automation_point(
        &mut self,
        expected_revision: u64,
        owner: NativeEditOwner,
        target: ParamTarget,
        from: Ticks,
        to: Ticks,
        value: f32,
    ) -> Result<NativeEditReceipt, NativeEditError> {
        self.check_revision(expected_revision)?;
        self.check_owner(owner)?;
        let address = resolve_parameter(&self.document, self.limits, target)?;
        let (automation, mut points) = curve_for_target(&self.document, &address)?
            .ok_or_else(|| NativeEditError::Invalid("automation target has no curve".into()))?;
        let from = from.max_zero();
        let to = to.max_zero();
        if !points.iter().any(|point| point.tick == from) {
            return Err(NativeEditError::Invalid(
                "automation point does not exist".into(),
            ));
        }
        let descriptor = mixer_descriptor(target).ok_or_else(|| {
            NativeEditError::Invalid("plugin automation requires a resolved descriptor".into())
        })?;
        if !value.is_finite() {
            return Err(NativeEditError::Invalid(
                "automation value must be finite".into(),
            ));
        }
        points.retain(|point| point.tick != from && point.tick != to);
        points.push(AutomationPoint::new(to, descriptor.clamp(value)));
        points.sort_by_key(|point| point.tick);
        self.apply(
            expected_revision,
            owner,
            Edit::WriteAutomation(target),
            &[NativeEdit::Automation(
                NativeAutomationEdit::ReplacePoints { automation, points },
            )],
        )
    }

    /// Removes one point, deleting only its binding and curve when it was the last point.
    pub fn remove_automation_point(
        &mut self,
        expected_revision: u64,
        owner: NativeEditOwner,
        target: ParamTarget,
        at: Ticks,
    ) -> Result<NativeEditReceipt, NativeEditError> {
        self.check_revision(expected_revision)?;
        self.check_owner(owner)?;
        let address = resolve_parameter(&self.document, self.limits, target)?;
        let (automation, mut points) = curve_for_target(&self.document, &address)?
            .ok_or_else(|| NativeEditError::Invalid("automation target has no curve".into()))?;
        let tick = at.max_zero();
        let before = points.len();
        points.retain(|point| point.tick != tick);
        if points.len() == before {
            return Err(NativeEditError::Invalid(
                "automation point does not exist".into(),
            ));
        }
        let binding = self
            .document
            .content
            .automation_bindings
            .iter()
            .find(|binding| binding.target == address)
            .ok_or_else(|| NativeEditError::Invalid("automation binding is missing".into()))?;
        let source = binding.source;
        let operation = if points.is_empty() {
            let shared = self
                .document
                .content
                .automation_bindings
                .iter()
                .any(|candidate| candidate.source == source && candidate.id != binding.id);
            let mut operations = vec![NativeEdit::Automation(
                NativeAutomationEdit::RemoveBinding {
                    binding: binding.id,
                },
            )];
            if !shared {
                operations.push(NativeEdit::Automation(NativeAutomationEdit::Remove {
                    automation,
                    source,
                }));
            }
            operations
        } else {
            vec![NativeEdit::Automation(
                NativeAutomationEdit::ReplacePoints { automation, points },
            )]
        };
        self.apply(expected_revision, owner, Edit::EraseAutomation, &operation)
    }

    /// Removes the target binding and its curve, preserving other curves and bindings.
    pub fn clear_automation(
        &mut self,
        expected_revision: u64,
        owner: NativeEditOwner,
        target: ParamTarget,
    ) -> Result<NativeEditReceipt, NativeEditError> {
        self.check_revision(expected_revision)?;
        self.check_owner(owner)?;
        let address = resolve_parameter(&self.document, self.limits, target)?;
        let (automation, _) = curve_for_target(&self.document, &address)?
            .ok_or_else(|| NativeEditError::Invalid("automation target has no curve".into()))?;
        let binding = self
            .document
            .content
            .automation_bindings
            .iter()
            .find(|binding| binding.target == address)
            .ok_or_else(|| NativeEditError::Invalid("automation binding is missing".into()))?;
        let shared = self
            .document
            .content
            .automation_bindings
            .iter()
            .any(|candidate| candidate.source == binding.source && candidate.id != binding.id);
        let mut operations = vec![NativeEdit::Automation(
            NativeAutomationEdit::RemoveBinding {
                binding: binding.id,
            },
        )];
        if !shared {
            operations.push(NativeEdit::Automation(NativeAutomationEdit::Remove {
                automation,
                source: binding.source,
            }));
        }
        self.apply(expected_revision, owner, Edit::EraseAutomation, &operations)
    }

    /// Changes interpolation for an existing target curve without changing its points.
    pub fn set_automation_curve(
        &mut self,
        expected_revision: u64,
        owner: NativeEditOwner,
        target: ParamTarget,
        curve: AutomationCurve,
    ) -> Result<NativeEditReceipt, NativeEditError> {
        self.check_revision(expected_revision)?;
        self.check_owner(owner)?;
        let address = resolve_parameter(&self.document, self.limits, target)?;
        let (automation, _) = curve_for_target(&self.document, &address)?
            .ok_or_else(|| NativeEditError::Invalid("automation target has no curve".into()))?;
        self.apply(
            expected_revision,
            owner,
            Edit::WriteAutomation(target),
            &[NativeEdit::Automation(NativeAutomationEdit::SetCurve {
                automation,
                curve,
            })],
        )
    }
}

fn resolve_parameter(
    document: &NativeDocument,
    limits: auris_core::pipeline::content::ContentLimits,
    target: ParamTarget,
) -> Result<auris_core::pipeline::content::ParameterAddress, NativeEditError> {
    document
        .content
        .command_bindings(&document.external_ids(), limits)
        .and_then(|bindings| bindings.parameter(target, None))
        .map_err(NativeEditError::Content)
}

fn curve_for_target(
    document: &NativeDocument,
    target: &auris_core::pipeline::content::ParameterAddress,
) -> Result<Option<(AutomationId, Vec<AutomationPoint>)>, NativeEditError> {
    let bindings: Vec<_> = document
        .content
        .automation_bindings
        .iter()
        .filter(|binding| binding.target == *target)
        .collect();
    if bindings.len() > 1 {
        return Err(NativeEditError::Invalid(
            "automation target has ambiguous bindings".into(),
        ));
    }
    let Some(binding) = bindings.first() else {
        return Ok(None);
    };
    let automation = document
        .content
        .processors
        .iter()
        .find(|processor| processor.id == binding.source)
        .and_then(|processor| match processor.kind {
            auris_core::pipeline::content::ProcessorKind::Automation(id) => Some(id),
            _ => None,
        })
        .ok_or_else(|| NativeEditError::Invalid("automation binding source is missing".into()))?;
    let curve = document
        .content
        .automation
        .iter()
        .find(|curve| curve.id == automation)
        .ok_or_else(|| NativeEditError::Invalid("automation curve is missing".into()))?;
    Ok(Some((curve.id, curve.points().to_vec())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::pipeline::content::{ContentLimits, ProcessorKind};
    use auris_core::project::Project;

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

    fn fixture() -> (NativeDocumentEditor, auris_core::TrackId) {
        let mut project = Project::new("mixer commands", 48_000.0);
        let track = project.add_instrument_track("Keys", "unavailable.test.instrument");
        let native = auris_io::project_to_native(&project, limits()).unwrap();
        (NativeDocumentEditor::new(native, limits()).unwrap(), track)
    }

    #[test]
    fn mixer_targets_resolve_track_and_master_strips() {
        let (mut editor, track) = fixture();
        editor
            .set_mixer_parameter(
                0,
                NativeEditOwner::Human,
                ParamTarget::TrackGain(track),
                -9.0,
            )
            .unwrap();
        editor
            .set_mixer_parameter(1, NativeEditOwner::Human, ParamTarget::MasterGain, -3.0)
            .unwrap();
        let strips: Vec<_> = editor
            .document()
            .content
            .processors
            .iter()
            .filter_map(|processor| match &processor.kind {
                ProcessorKind::Strip(strip) => Some(strip.gain_db),
                _ => None,
            })
            .collect();
        assert!(strips.contains(&-9.0));
        assert!(strips.contains(&-3.0));
    }

    #[test]
    fn stale_mixer_edit_is_rejected_without_a_second_change() {
        let (mut editor, track) = fixture();
        editor
            .set_mixer_parameter(
                0,
                NativeEditOwner::Human,
                ParamTarget::TrackPan(track),
                0.25,
            )
            .unwrap();
        let error = editor
            .set_mixer_parameter(0, NativeEditOwner::Human, ParamTarget::TrackPan(track), 0.5)
            .unwrap_err();
        assert!(matches!(error, NativeEditError::StaleRevision { .. }));
        assert_eq!(editor.revision().document, 1);
    }

    #[test]
    fn automation_point_collision_and_undo_redo_keep_one_curve_identity() {
        let (mut editor, track) = fixture();
        editor
            .set_automation_point(
                0,
                NativeEditOwner::Human,
                ParamTarget::TrackGain(track),
                Ticks::ZERO,
                -12.0,
            )
            .unwrap();
        let automation = editor.document().content.automation[0].id;
        editor
            .set_automation_point(
                1,
                NativeEditOwner::Human,
                ParamTarget::TrackGain(track),
                Ticks::ZERO,
                -6.0,
            )
            .unwrap();
        assert_eq!(editor.document().content.automation[0].id, automation);
        assert_eq!(
            editor.document().content.automation[0].points()[0].value,
            -6.0
        );
        editor.undo(2).unwrap();
        assert_eq!(
            editor.document().content.automation[0].points()[0].value,
            -12.0
        );
        editor.redo(3).unwrap();
        assert_eq!(
            editor.document().content.automation[0].points()[0].value,
            -6.0
        );
    }

    #[test]
    fn clearing_one_binding_keeps_a_shared_curve_for_the_other_target() {
        let (mut editor, track) = fixture();
        editor
            .set_automation_point(
                0,
                NativeEditOwner::Human,
                ParamTarget::TrackGain(track),
                Ticks::ZERO,
                -12.0,
            )
            .unwrap();
        let document = editor.document();
        let bindings = document
            .content
            .command_bindings(&document.external_ids(), limits())
            .unwrap();
        let track_address = bindings
            .parameter(ParamTarget::TrackGain(track), None)
            .unwrap();
        let master_address = bindings.parameter(ParamTarget::MasterGain, None).unwrap();
        let binding = editor
            .document()
            .content
            .automation_bindings
            .iter()
            .find(|binding| binding.target == track_address)
            .unwrap();
        let automation = editor
            .document()
            .content
            .processors
            .iter()
            .find(|processor| processor.id == binding.source)
            .and_then(|processor| match processor.kind {
                ProcessorKind::Automation(id) => Some(id),
                _ => None,
            })
            .unwrap();
        editor
            .apply(
                1,
                NativeEditOwner::Human,
                Edit::WriteAutomation(ParamTarget::MasterGain),
                &[NativeEdit::Automation(NativeAutomationEdit::AddBinding {
                    automation,
                    source: binding.source,
                    target: master_address.clone(),
                    enabled: true,
                })],
            )
            .unwrap();
        editor
            .clear_automation(2, NativeEditOwner::Human, ParamTarget::TrackGain(track))
            .unwrap();
        assert_eq!(editor.document().content.automation.len(), 1);
        assert_eq!(editor.document().content.automation_bindings.len(), 1);
        assert_eq!(
            editor.document().content.automation_bindings[0].target,
            master_address
        );
    }

    #[test]
    fn ambiguous_native_bindings_are_rejected_before_editing_the_curve() {
        let (editor, track) = fixture();
        let mut document = editor.document().clone();
        let target = document
            .content
            .command_bindings(&document.external_ids(), limits())
            .unwrap()
            .parameter(ParamTarget::TrackGain(track), None)
            .unwrap();
        let mut editor = NativeDocumentEditor::new(document.clone(), limits()).unwrap();
        editor
            .set_automation_point(
                0,
                NativeEditOwner::Human,
                ParamTarget::TrackGain(track),
                Ticks::ZERO,
                -12.0,
            )
            .unwrap();
        document = editor.document().clone();
        let existing = document.content.automation_bindings[0].clone();
        let duplicate_id =
            auris_core::pipeline::EdgeId(super::super::allocate(&mut document).unwrap());
        document.content.automation_bindings.push(
            auris_core::pipeline::content::AutomationBinding {
                id: duplicate_id,
                source: existing.source,
                target,
                enabled: false,
            },
        );
        let mut editor = NativeDocumentEditor::new(document, limits()).unwrap();
        let error = editor
            .set_automation_point(
                0,
                NativeEditOwner::Human,
                ParamTarget::TrackGain(track),
                Ticks::ZERO,
                -6.0,
            )
            .unwrap_err();
        assert!(error.to_string().contains("ambiguous"));
        assert_eq!(editor.revision().document, 0);
    }
}
