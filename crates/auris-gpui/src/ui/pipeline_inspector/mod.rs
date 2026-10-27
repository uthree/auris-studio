//! Progressive disclosure of the selected document's processing dependencies.
//!
//! The existing inspector owns scrolling and selection. This surface only reads the
//! session projection; editing goes through the session commands of each owner.

mod actions;
mod connections;
mod inspection;
mod model;

#[cfg(test)]
mod actions_tests;
#[cfg(test)]
mod inspection_tests;
#[cfg(test)]
mod tests;

use auris_i18n::{Key, pipeline};
use auris_session::pipeline_graph::{
    PipelineError, PipelineNode, PipelineNodeData, PipelineStageEvaluation,
};
use auris_session::prelude::*;
use gpui::{AnyElement, FocusHandle, IntoElement, SharedString, Window, div, prelude::*};

use crate::app::AurisApp;
use crate::dock::Panel;
use crate::ui::tooltip::keyed_tip;
use crate::ui::widgets::{
    ButtonState, ButtonStyle, ClaimDisclosureActivation, bounded_button_enabled, button_enabled,
    disclosure,
};

use model::InspectorModel;

/// Presentation only: neither disclosure nor paging belongs to the saved project.
pub(crate) struct PipelineInspector {
    expanded: bool,
    track_scope: bool,
    focus: FocusHandle,
    keyboard_armed: bool,
    model: InspectorModel,
    inspection: Option<inspection::InspectionSnapshot>,
}

impl PipelineInspector {
    pub(crate) fn new(cx: &mut gpui::App) -> Self {
        Self {
            expanded: false,
            track_scope: false,
            focus: cx.focus_handle(),
            keyboard_armed: false,
            model: InspectorModel::new(),
            inspection: None,
        }
    }
}

struct NodeText {
    kind: Key,
    title: String,
    detail: String,
    domain: Key,
}

impl AurisApp {
    pub(crate) fn show_pipeline_details(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.pipeline_inspector.expanded = true;
        self.pipeline_inspector.keyboard_armed = false;
        self.show_panel(Panel::Inspector);
        self.inspector_scroll.set_offset(gpui::Point::default());
        if !self.panels.is_detached(Panel::Inspector) {
            window.focus(&self.pipeline_inspector.focus);
        }
        cx.notify();
    }

    pub(crate) fn render_pipeline_inspector(&mut self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let expanded = self.pipeline_inspector.expanded;
        let theme = self.theme.clone();
        let mut section = div()
            .id("pipeline-section")
            .debug_selector(|| "pipeline-section".into())
            .flex()
            .flex_col()
            .min_w_0()
            .gap_1()
            .pt_2()
            .child(
                disclosure(
                    "pipeline-disclosure",
                    self.t(Key::PipelineDetails),
                    expanded,
                    &theme,
                    cx.listener(|this, event, _, cx| {
                        // A palette command focuses this disclosure before its Enter key-up.
                        // Only activate keyboard clicks whose key-down was received here.
                        let armed = std::mem::take(&mut this.pipeline_inspector.keyboard_armed);
                        if matches!(event, gpui::ClickEvent::Keyboard(_)) && !armed {
                            return;
                        }
                        this.pipeline_inspector.expanded = !this.pipeline_inspector.expanded;
                        cx.notify();
                    }),
                )
                .track_focus(&self.pipeline_inspector.focus)
                .capture_action(cx.listener(|this, _: &ClaimDisclosureActivation, _, _| {
                    this.pipeline_inspector.keyboard_armed = true;
                }))
                .h_7()
                .px_0(),
            );
        if !expanded {
            return section.into_any_element();
        }

        let connection = self.selected_mixer_connection(cx);
        if let Some(edge) = connection {
            section = section.child(self.render_pipeline_connection(edge, cx));
        }

        let clip = self
            .selected_clip
            .filter(|_| !self.pipeline_inspector.track_scope);
        self.pipeline_inspector
            .model
            .refresh(&self.session, clip, self.selected_track);
        if self
            .pipeline_inspector
            .inspection
            .as_ref()
            .is_some_and(|snapshot| {
                snapshot.revision != self.session.revision()
                    || !self
                        .pipeline_inspector
                        .model
                        .nodes()
                        .any(|node| node.id == snapshot.node)
            })
        {
            self.pipeline_inspector.inspection = None;
        }
        let track_scope = matches!(
            self.pipeline_inspector.model.scope(),
            model::InspectorScope::Track(_)
        );
        section = section
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .child(button_enabled(
                        "pipeline-scope-track",
                        self.t(Key::PipelineTrack),
                        ButtonStyle::Ghost,
                        ButtonState::available(track_scope, self.selected_track.is_some()),
                        theme.accent_soft,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.pipeline_inspector.track_scope = true;
                            cx.notify();
                        }),
                    ))
                    .child(button_enabled(
                        "pipeline-scope-clip",
                        self.t(Key::PipelineClip),
                        ButtonStyle::Ghost,
                        ButtonState::available(!track_scope, self.selected_clip.is_some()),
                        theme.accent_soft,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.pipeline_inspector.track_scope = false;
                            cx.notify();
                        }),
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::PipelineReadOnly)),
            );

        let model = &self.pipeline_inspector.model;
        if let Some(error) = model.error() {
            let key = if matches!(error, PipelineError::UnknownScope) {
                Key::PipelineUnavailable
            } else {
                Key::PipelineReadFailed
            };
            return section
                .child(
                    div()
                        .id("pipeline-error")
                        .debug_selector(|| "pipeline-error".into())
                        .py_2()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(self.t(key)),
                )
                .into_any_element();
        }
        if model.is_empty() {
            if connection.is_some() {
                return section.into_any_element();
            }
            return section
                .child(
                    div()
                        .id("pipeline-empty")
                        .debug_selector(|| "pipeline-empty".into())
                        .py_2()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(self.t(Key::PipelineEmpty)),
                )
                .into_any_element();
        }
        if let Some(page) = model.page() {
            let offset = model.offset();
            let has_next = page.next_offset.is_some();
            let range = pipeline::page_range(
                self.language(),
                offset,
                offset + page.nodes.len(),
                page.total_nodes,
            );
            section = section.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .items_center()
                    .min_w_0()
                    .child(div().text_xs().text_color(theme.text_muted).child(range))
                    .when(offset > 0 || has_next, |row| {
                        row.child(button_enabled(
                            "pipeline-previous",
                            self.t(Key::PipelinePrevious),
                            ButtonStyle::Ghost,
                            ButtonState::available(false, offset > 0),
                            theme.accent_soft,
                            &theme,
                            cx.listener(|this, _, _, cx| {
                                this.pipeline_inspector.model.previous_page(&this.session);
                                cx.notify();
                            }),
                        ))
                        .child(button_enabled(
                            "pipeline-next",
                            self.t(Key::PipelineNext),
                            ButtonStyle::Ghost,
                            ButtonState::available(false, has_next),
                            theme.accent_soft,
                            &theme,
                            cx.listener(|this, _, _, cx| {
                                this.pipeline_inspector.model.next_page(&this.session);
                                cx.notify();
                            }),
                        ))
                    }),
            );
        }
        section
            .children(model.nodes().map(|node| self.pipeline_row(node, cx)))
            .into_any_element()
    }

    fn pipeline_row(&self, node: &PipelineNode, cx: &mut gpui::Context<Self>) -> AnyElement {
        let text = self.pipeline_node_text(node);
        let theme = &self.theme;
        let selector = format!("pipeline-node-{}", node.id);
        let mut row = div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .flex()
            .flex_col()
            .min_w_0()
            .gap_1()
            .py_2()
            .border_b_1()
            .border_color(theme.border_subtle)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .items_center()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(text.kind))
                    .child(self.t(text.domain)),
            )
            .child(
                div()
                    .id(SharedString::from(format!("pipeline-title-{}", node.id)))
                    .w_full()
                    .min_w_0()
                    // Keep width-sensitive text measurement during column layout. Nowrap
                    // can cache an ellipsis from GPUI's initial minimum-width measurement.
                    .text_ellipsis()
                    .line_clamp(1)
                    .text_sm()
                    .text_color(theme.text)
                    .tooltip(keyed_tip(text.title.clone(), "", theme))
                    .child(text.title),
            )
            .when(!text.detail.is_empty(), |row| {
                row.child(
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(text.detail),
                )
            });
        if let PipelineNodeData::Clip { .. } = node.data
            && let (Some(track), Some(clip)) = (node.track_id, node.clip_id)
        {
            row = row.child(bounded_button_enabled(
                SharedString::from(format!("pipeline-select-clip-{clip}")),
                self.t(Key::PipelineInspectClip),
                ButtonStyle::Ghost,
                ButtonState::available(false, true),
                theme.accent_soft,
                theme,
                cx.listener(move |this, _, _, cx| {
                    this.select_track(TrackId(track));
                    this.select_clip(Some(ClipId(clip)));
                    this.pipeline_inspector.track_scope = false;
                    this.inspector_scroll.set_offset(gpui::Point::default());
                    cx.notify();
                }),
            ));
        } else if let PipelineNodeData::StripInput = node.data
            && let Some(track) = node.track_id
        {
            row = row.child(bounded_button_enabled(
                SharedString::from(format!("pipeline-select-track-{track}")),
                self.t(Key::PipelineInspectTrack),
                ButtonStyle::Ghost,
                ButtonState::available(false, true),
                theme.accent_soft,
                theme,
                cx.listener(move |this, _, _, cx| {
                    this.select_track(TrackId(track));
                    this.pipeline_inspector.track_scope = true;
                    this.inspector_scroll.set_offset(gpui::Point::default());
                    cx.notify();
                }),
            ));
        }
        if let Some(actions) = self.pipeline_row_actions(node, cx) {
            row = row.child(actions);
        }
        if let Some(inspection) = self.pipeline_state_details(node, cx) {
            row = row.child(inspection);
        }
        row.into_any_element()
    }

    fn pipeline_node_text(&self, node: &PipelineNode) -> NodeText {
        use PipelineNodeData as N;
        let mut title = node.label.clone();
        let (kind, detail, domain) = match &node.data {
            N::NoteSource { note_count, .. } => (
                Key::PipelineNoteSource,
                pipeline::count_notes(self.language(), *note_count),
                Key::PipelineEventDomain,
            ),
            N::AudioSource { registered, .. } => {
                if !*registered {
                    title = self.t(Key::PipelineAudioSource).into();
                }
                (
                    Key::PipelineAudioSource,
                    if *registered {
                        String::new()
                    } else {
                        self.t(Key::PipelineSourceMissing).into()
                    },
                    Key::PipelineAudioDomain,
                )
            }
            N::Clip { muted, .. } => (
                Key::PipelinePlacement,
                if *muted {
                    self.t(Key::Mute).into()
                } else {
                    String::new()
                },
                if node
                    .outputs
                    .contains(&auris_session::pipeline_graph::PipelineDomain::Audio)
                {
                    Key::PipelineAudioDomain
                } else {
                    Key::PipelineEventDomain
                },
            ),
            N::ClipStage {
                stage_index,
                transform_kind,
                evaluation,
                ..
            } => {
                title = pipeline::stage_title(
                    self.language(),
                    *stage_index,
                    &pipeline::transform_name(self.language(), transform_kind),
                );
                (
                    Key::PerformHeading,
                    self.t(match evaluation {
                        PipelineStageEvaluation::PerPassNotes => Key::PipelineStageNotes,
                        PipelineStageEvaluation::AfterNoteStages => Key::PipelineStageCurves,
                        PipelineStageEvaluation::Composite => Key::PipelineStageComposite,
                    })
                    .into(),
                    Key::PipelineEventDomain,
                )
            }
            N::Instrument {
                plugin_id,
                hosted,
                singer_guide,
                audition_only,
            } => {
                title = node.track_id.map_or_else(
                    || self.plugin_label(plugin_id),
                    |id| self.instrument_label(TrackId(id), plugin_id),
                );
                (
                    Key::PipelineInstrument,
                    if *singer_guide {
                        self.t(if *audition_only {
                            Key::PipelineGuideAudition
                        } else {
                            Key::PipelineGuideFallback
                        })
                        .into()
                    } else if *hosted {
                        self.t(Key::PipelinePluginUnchecked).into()
                    } else {
                        String::new()
                    },
                    Key::PipelineAudioDomain,
                )
            }
            N::SingerTake { .. } => (
                Key::PipelineSingerTake,
                self.t(Key::PipelineTakeUnchecked).into(),
                Key::PipelineAudioDomain,
            ),
            N::StripInput => {
                if node.track_id.is_none() {
                    title = self.t(Key::Master).into();
                }
                (
                    Key::PipelineStripInput,
                    String::new(),
                    Key::PipelineAudioDomain,
                )
            }
            N::Effect {
                slot,
                plugin_id,
                enabled,
                hosted,
            } => {
                title = self.effect_label(EffectSlotId(*slot), plugin_id);
                let detail = [
                    (!*enabled).then(|| self.t(Key::PipelineBypassed)),
                    (*hosted).then(|| self.t(Key::PipelinePluginUnchecked)),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
                (Key::PipelineEffect, detail, Key::PipelineAudioDomain)
            }
            N::Fader {
                gain_db,
                pan,
                mute,
                solo,
            } => {
                if node.track_id.is_none() {
                    title = self.t(Key::Master).into();
                }
                let mut detail = format!("{gain_db:.1} dB · {} {pan:.2}", self.t(Key::Pan));
                if *mute {
                    detail.push_str(&format!(" · {}", self.t(Key::Mute)));
                }
                if *solo {
                    detail.push_str(&format!(" · {}", self.t(Key::Solo)));
                }
                (Key::PipelineFader, detail, Key::PipelineAudioDomain)
            }
            N::Send {
                send,
                level_db,
                pre_fader,
                ..
            } => {
                title = self
                    .t(if *pre_fader {
                        Key::PipelinePreFader
                    } else {
                        Key::PipelinePostFader
                    })
                    .into();
                let destination = node
                    .track_id
                    .and_then(|track| self.project().track(TrackId(track)))
                    .and_then(|track| track.sends.iter().find(|entry| entry.id.0 == *send))
                    .and_then(|send| self.project().track(send.target));
                if let Some(destination) = destination {
                    title.push_str(&format!(" → {}", destination.name));
                }
                (
                    Key::PipelineSend,
                    format!("{level_db:.1} dB"),
                    Key::PipelineAudioDomain,
                )
            }
            N::Automation {
                target,
                key,
                point_count,
                owner_present,
                parameter_checked,
                ..
            } => {
                title = match target {
                    ParamTarget::TrackGain(_) | ParamTarget::MasterGain => {
                        self.t(Key::Volume).into()
                    }
                    ParamTarget::TrackPan(_) | ParamTarget::MasterPan => self.t(Key::Pan).into(),
                    ParamTarget::Send { .. } => self.t(Key::PipelineSend).into(),
                    ParamTarget::Instrument { .. } | ParamTarget::Effect { .. } => key
                        .as_deref()
                        .map(|key| self.param_label(key))
                        .unwrap_or_else(|| self.t(Key::PipelineAutomation).into()),
                };
                let mut detail = pipeline::count_points(self.language(), *point_count);
                if !*owner_present {
                    detail.push_str(&format!(" · {}", self.t(Key::PipelineTargetMissing)));
                } else if !*parameter_checked {
                    detail.push_str(&format!(" · {}", self.t(Key::PipelineParameterUnchecked)));
                }
                (Key::PipelineAutomation, detail, Key::PipelineControlDomain)
            }
            N::Output => {
                title = self.t(Key::Master).into();
                (Key::PipelineOutput, String::new(), Key::PipelineAudioDomain)
            }
        };
        NodeText {
            kind,
            title,
            detail,
            domain,
        }
    }
}
