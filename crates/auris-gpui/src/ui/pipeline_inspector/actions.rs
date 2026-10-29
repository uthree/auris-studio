//! Owner commands reachable from a bounded pipeline page.

use auris_i18n::Key;
use auris_session::ClipTransformEdit;
use auris_session::pipeline_graph::{PipelineNode, PipelineNodeData};
use auris_session::prelude::*;
use gpui::{AnyElement, IntoElement, Pixels, Point, SharedString, div, prelude::*};

use crate::app::AurisApp;
use crate::ui::context_menu::{ContextMenu, MenuCommand};
use crate::ui::plugin_window::PluginSubject;
use crate::ui::widgets::{ButtonState, ButtonStyle, bounded_button_enabled};

impl AurisApp {
    pub(super) fn pipeline_row_actions(
        &self,
        node: &PipelineNode,
        cx: &mut gpui::Context<Self>,
    ) -> Option<AnyElement> {
        let revision = self.pipeline_inspector.model.revision()?;
        match &node.data {
            PipelineNodeData::ClipStage { stage_index, .. } => {
                let clip = ClipId(node.clip_id?);
                let index = *stage_index;
                let title = self.pipeline_node_text(node).title;
                Some(
                    bounded_button_enabled(
                        SharedString::from(format!("pipeline-stage-actions-{}-{index}", clip.0)),
                        self.t(Key::PipelineStageActions),
                        ButtonStyle::Ghost,
                        ButtonState::available(false, true),
                        self.theme.accent_soft,
                        &self.theme,
                        Self::opens_menu(cx, move |this, at| {
                            this.pipeline_stage_menu(at, clip, index, revision, title.clone())
                        }),
                    )
                    .into_any_element(),
                )
            }
            PipelineNodeData::Effect { slot, .. } => {
                let subject = PluginSubject::Insert {
                    track: node.track_id.map(TrackId),
                    slot: EffectSlotId(*slot),
                };
                Some(
                    div()
                        .flex()
                        .flex_wrap()
                        .min_w_0()
                        .gap(self.theme.atelier.spacing.xs)
                        .child(bounded_button_enabled(
                            SharedString::from(format!("pipeline-parameters-{slot}")),
                            self.t(Key::PluginParameters),
                            ButtonStyle::Ghost,
                            ButtonState::available(false, true),
                            self.theme.accent_soft,
                            &self.theme,
                            cx.listener(move |this, _, _, cx| {
                                this.open_plugin_window(subject);
                                cx.notify();
                            }),
                        ))
                        .child(self.plugin_menu_button(
                            SharedString::from(format!("pipeline-effect-menu-{slot}")),
                            subject,
                            self.pipeline_node_text(node).title,
                            cx,
                        ))
                        .into_any_element(),
                )
            }
            PipelineNodeData::Automation {
                target,
                owner_present: true,
                ..
            } if target.track().is_some() => {
                let target = *target;
                Some(
                    bounded_button_enabled(
                        SharedString::from(format!("pipeline-edit-{}", node.id)),
                        self.t(Key::PipelineEditAutomation),
                        ButtonStyle::Ghost,
                        ButtonState::available(false, true),
                        self.theme.accent_soft,
                        &self.theme,
                        cx.listener(move |this, _, _, cx| {
                            this.reveal_pipeline_automation(target, revision);
                            cx.notify();
                        }),
                    )
                    .into_any_element(),
                )
            }
            _ => None,
        }
    }

    fn pipeline_stage_menu(
        &mut self,
        at: Point<Pixels>,
        clip: ClipId,
        index: usize,
        revision: u64,
        title: String,
    ) -> ContextMenu {
        let menu = ContextMenu::new(at, title);
        if self.session.revision() != revision {
            self.set_failed_status(self.t(Key::PipelineChanged).to_owned());
            return menu;
        }
        let Ok(stack) = self.session.clip_transforms(clip) else {
            return menu;
        };
        if index >= stack.len() {
            return menu;
        }
        let command = |edit| MenuCommand::PipelineStage {
            clip,
            revision,
            edit,
        };
        menu.item_if(
            index > 0,
            self.t(Key::PipelineMoveEarlier),
            command(ClipTransformEdit::Move { index, delta: -1 }),
        )
        .item_if(
            index + 1 < stack.len(),
            self.t(Key::PipelineMoveLater),
            command(ClipTransformEdit::Move { index, delta: 1 }),
        )
        .separator()
        .item(
            self.t(Key::MenuRemove),
            command(ClipTransformEdit::Remove { index }),
        )
    }

    pub(crate) fn edit_pipeline_stage(
        &mut self,
        clip: ClipId,
        revision: u64,
        edit: ClipTransformEdit,
    ) {
        if let Err(error) = self.session.edit_clip_transform(clip, revision, edit) {
            self.set_failed_status(self.failure(Key::EditSetClipTransforms, &error));
        }
    }

    fn reveal_pipeline_automation(&mut self, target: ParamTarget, revision: u64) {
        if self.session.revision() != revision {
            self.set_failed_status(self.t(Key::PipelineChanged).to_owned());
            return;
        }
        let Some(track) = target.track() else {
            return;
        };
        let Some(lane) = self.session.automation().lane(target) else {
            self.set_failed_status(self.t(Key::PipelineBindingUnavailable).to_owned());
            return;
        };
        let key = lane.key().map(str::to_owned);
        // A saved plugin key must still name this exact parameter. Do not redirect a curve
        // to a different numeric slot, or silently open a new empty lane, after plugin reload.
        let matches = self
            .session
            .descriptor_for(target)
            .is_some_and(|descriptor| {
                key.as_deref()
                    .is_none_or(|key| descriptor.key.as_ref() == key)
            });
        if !matches {
            self.set_failed_status(self.t(Key::PipelineBindingUnavailable).to_owned());
            return;
        }
        if self.automation_lanes.get(&track) != Some(&target) {
            self.show_automation(track, target);
        }
        self.select_track(track);
        self.set_track_grid(false);
        self.reveal_track(track);
    }
}
