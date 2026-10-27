//! Explicit, revision-bound snapshots; painting never evaluates note transforms.

use auris_i18n::{Key, pipeline};
use auris_session::pipeline_graph::{PipelineNode, PipelineNodeData};
use auris_session::prelude::*;
use auris_session::{
    PipelineInspectionDetail, PipelineInspectionStatus, PipelineNodeInspection, PipelineStageState,
};
use gpui::{AnyElement, IntoElement, SharedString, div, prelude::*};

use crate::app::AurisApp;
use crate::ui::widgets::{ButtonState, ButtonStyle, bounded_button_enabled};

pub(super) struct InspectionSnapshot {
    pub(super) revision: u64,
    pub(super) node: String,
    pub(super) value: Option<PipelineNodeInspection>,
}

impl AurisApp {
    pub(super) fn pipeline_state_details(
        &self,
        node: &PipelineNode,
        cx: &mut gpui::Context<Self>,
    ) -> Option<AnyElement> {
        if !matches!(
            node.data,
            PipelineNodeData::ClipStage { .. }
                | PipelineNodeData::Automation { .. }
                | PipelineNodeData::Fader { .. }
                | PipelineNodeData::Send { .. }
        ) {
            return None;
        }
        let revision = self.pipeline_inspector.model.revision()?;
        let snapshot = self
            .pipeline_inspector
            .inspection
            .as_ref()
            .filter(|snapshot| snapshot.node == node.id && snapshot.revision == revision);
        let id = node.id.clone();
        let mut details = div()
            .flex()
            .flex_col()
            .min_w_0()
            .gap_1()
            .child(bounded_button_enabled(
                SharedString::from(format!("pipeline-inspect-{}", node.id)),
                self.t(if snapshot.is_some() {
                    Key::PipelineRefreshState
                } else {
                    Key::PipelineInspectState
                }),
                ButtonStyle::Ghost,
                ButtonState::available(false, true),
                self.theme.accent_soft,
                &self.theme,
                cx.listener(move |this, _, _, cx| {
                    if this.session.revision() != revision {
                        this.pipeline_inspector.inspection = None;
                        this.status = this.t(Key::PipelineChanged).into();
                    } else {
                        let value = this
                            .session
                            .inspect_pipeline_node(&id, this.playhead_ticks())
                            .ok();
                        this.pipeline_inspector.inspection = Some(InspectionSnapshot {
                            revision,
                            node: id.clone(),
                            value,
                        });
                    }
                    cx.notify();
                }),
            ));
        let Some(snapshot) = snapshot else {
            return Some(details.into_any_element());
        };
        let line = |text: String| {
            div()
                .min_w_0()
                .text_xs()
                .text_color(self.theme.text_muted)
                .child(text)
        };
        let mut state = div()
            .id(SharedString::from(format!("pipeline-state-{}", node.id)))
            .debug_selector({
                let id = node.id.clone();
                move || format!("pipeline-state-{id}")
            })
            .flex()
            .flex_col()
            .min_w_0()
            .gap_1();
        let Some(value) = &snapshot.value else {
            return Some(
                details
                    .child(line(self.t(Key::PipelineReadFailed).into()))
                    .into_any_element(),
            );
        };
        match &value.detail {
            PipelineInspectionDetail::Stage { state: stage } => {
                state = state.child(line(self.t(Key::PipelineFirstPass).into()));
                if matches!(stage.availability, PipelineInspectionStatus::Limited { .. }) {
                    state = state.child(line(self.t(Key::PipelineWorkLimited).into()));
                } else {
                    if stage.state == PipelineStageState::DeferredControl {
                        state = state.child(line(self.t(Key::PipelineDeferredState).into()));
                    }
                    state = state.child(line(self.t(Key::PipelineNoteColumns).into()));
                    for (label, count, notes) in [
                        (Key::PipelineBefore, stage.input_count, &stage.input),
                        (Key::PipelineAfter, stage.output_count, &stage.output),
                    ] {
                        state = state.child(div().text_xs().text_color(self.theme.text).child(
                            format!(
                                "{} · {}",
                                self.t(label),
                                pipeline::count_notes(self.language(), count)
                            ),
                        ));
                        for note in notes.iter().take(4) {
                            state = state.child(line(format!(
                                "{} · {:.2} · {} / {}",
                                crate::ui::timeline::pitch_name(note.pitch),
                                note.velocity,
                                note.start.0,
                                note.length.0
                            )));
                        }
                    }
                    if stage.input_count > 4 || stage.output_count > 4 {
                        state = state.child(line(self.t(Key::PipelineNoteSample).into()));
                    }
                }
            }
            PipelineInspectionDetail::Parameters { values } => {
                state = state
                    .child(line(self.t(Key::PipelineParameterSnapshot).into()))
                    .child(line(format!("{} tick", value.at_tick.0)));
                for parameter in values {
                    let name = match parameter.target {
                        ParamTarget::TrackPan(_) | ParamTarget::MasterPan => self.t(Key::Pan),
                        ParamTarget::TrackGain(_)
                        | ParamTarget::MasterGain
                        | ParamTarget::Send { .. } => self.t(Key::SongPartGain),
                        _ => parameter
                            .key
                            .as_deref()
                            .unwrap_or(self.t(Key::PipelineAutomation)),
                    };
                    state = state.child(
                        div()
                            .text_xs()
                            .text_color(self.theme.text)
                            .child(name.to_owned()),
                    );
                    if !parameter.binding_checked {
                        state = state.child(line(self.t(Key::PipelineParameterUnchecked).into()));
                    }
                    for (key, number, missing) in [
                        (
                            Key::PipelineStaticValue,
                            parameter.static_value,
                            Key::PipelineNoValue,
                        ),
                        (
                            Key::PipelineAutomationValue,
                            parameter.automation_value,
                            Key::PipelineNoAutomation,
                        ),
                        (
                            Key::PipelineEffectiveValue,
                            parameter.effective_value,
                            Key::PipelineNoValue,
                        ),
                    ] {
                        let formatted = number.map_or_else(
                            || self.t(missing).to_owned(),
                            |number| match parameter.unit {
                                Some(ParamUnit::Decibels) => format!("{number:.2} dB"),
                                Some(ParamUnit::Pan) => format!("{number:.2}"),
                                _ => format!("{number:.3}"),
                            },
                        );
                        state = state.child(line(format!("{}: {formatted}", self.t(key))));
                    }
                }
            }
            PipelineInspectionDetail::Unavailable { .. } => {
                state = state.child(line(self.t(Key::PipelineNoValue).into()))
            }
        }
        details = details.child(state);
        Some(details.into_any_element())
    }
}
