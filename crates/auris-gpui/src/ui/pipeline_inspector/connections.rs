//! Selected connection details share the canvas's authoritative edge selection.

use auris_i18n::Key;
use auris_session::mixer_graph::{MixerEdge, MixerEdgeKind, MixerNode};
use gpui::{AnyElement, IntoElement, SharedString, div, prelude::*};

use crate::app::AurisApp;
use crate::dock::Panel;
use crate::ui::plugin_window::PluginSubject;
use crate::ui::widgets::{ButtonState, ButtonStyle, bounded_button_enabled};

impl AurisApp {
    /// Reveals connection information while keeping keyboard focus on the graph.
    pub(crate) fn show_pipeline_connection_details(&mut self, cx: &mut gpui::Context<Self>) {
        self.pipeline_inspector.expanded = true;
        // Following a transient wire selection must not save panel-layout preferences.
        self.panels.show(Panel::Inspector);
        self.inspector_scroll.set_offset(gpui::Point::default());
        cx.notify();
    }

    pub(super) fn render_pipeline_connection(
        &self,
        edge: MixerEdge,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let kind = match edge.kind {
            MixerEdgeKind::Chain => self.t(Key::Effects).to_owned(),
            MixerEdgeKind::Output => self.t(Key::OutputShort).to_owned(),
            MixerEdgeKind::Sidechain => self.t(Key::MenuSidechain).to_owned(),
            MixerEdgeKind::Send {
                send, pre_fader, ..
            } => format!(
                "{} #{} · {}",
                self.t(Key::Sends),
                send.0,
                self.t(if pre_fader {
                    Key::PipelinePreFader
                } else {
                    Key::PipelinePostFader
                })
            ),
        };
        let mut card = div()
            .id("pipeline-connection")
            .debug_selector(|| "pipeline-connection".into())
            .flex()
            .flex_col()
            .min_w_0()
            .gap_2()
            .py_2()
            .border_b_1()
            .border_color(self.theme.border_subtle)
            .child(
                div()
                    .text_sm()
                    .text_color(self.theme.text)
                    .child(self.t(Key::PipelineConnection)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(self.theme.text_muted)
                    .child(kind),
            );
        for (key, action_key, node, name) in [
            (
                Key::MixerConnectionFrom,
                Key::PipelineRevealSource,
                edge.source,
                "source",
            ),
            (
                Key::MixerConnectionTo,
                Key::PipelineRevealTarget,
                edge.target,
                "target",
            ),
        ] {
            let label = self
                .wire_node_name(node)
                .unwrap_or_else(|| self.t(Key::PipelineUnavailable).into());
            let selector = format!("pipeline-connection-{name}");
            card = card.child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.text_muted)
                            .child(self.t(key)),
                    )
                    .child(
                        div()
                            .id(SharedString::from(selector.clone()))
                            .debug_selector(move || selector.clone())
                            .text_sm()
                            .text_color(self.theme.text)
                            .child(label),
                    )
                    .when(!matches!(node, MixerNode::Strip(None)), |row| {
                        row.child(bounded_button_enabled(
                            SharedString::from(format!("pipeline-reveal-{name}")),
                            self.t(action_key),
                            ButtonStyle::Ghost,
                            ButtonState::available(false, true),
                            self.theme.accent_soft,
                            &self.theme,
                            cx.listener(move |this, _, _, cx| {
                                match node {
                                    MixerNode::Strip(Some(track)) => {
                                        this.select_track(track);
                                        this.select_clip(None);
                                    }
                                    MixerNode::Strip(None) => {}
                                    MixerNode::Effect(track, slot) => this
                                        .open_plugin_window(PluginSubject::Insert { track, slot }),
                                }
                                this.pipeline_inspector.track_scope = true;
                                cx.notify();
                            }),
                        ))
                    }),
            );
        }
        let level = if !self.mixer_graph_visible || !self.panels.is_open(Panel::Mixer) {
            self.t(Key::PipelineMonitorUnavailable).into()
        } else if self.session.is_playing() {
            let peak = self.session.mixer_edge_peak(&edge);
            if peak.is_finite() && peak > 0.0 {
                format!("{:.1} dBFS", 20.0 * peak.log10())
            } else {
                "−∞ dBFS".into()
            }
        } else {
            self.t(Key::PipelinePlaybackStopped).into()
        };
        card.child(
            div()
                .text_xs()
                .text_color(self.theme.text_muted)
                .child(self.t(Key::PipelineMonitorLevel)),
        )
        .child(div().text_sm().text_color(self.theme.text).child(level))
        .into_any_element()
    }
}
