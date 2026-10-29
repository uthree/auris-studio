//! A graph presentation of the session's mixer, with document-authoritative connections.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use auris_i18n::Key;
use auris_session::mixer_graph::{MixerEdge, MixerEdgeKind, MixerGraph, MixerNode};
use auris_session::prelude::*;
use gpui::{AnyElement, Axis, Entity, IntoElement, MouseButton, SharedString, div, prelude::*, px};
use gpui_flow::{
    Connection, EdgeFlow, EdgeStroke, FlowEdge, FlowGraph, FlowNode, FlowPoint, FlowState,
    HandleDef, HandlePosition,
};

use crate::app::AurisApp;
use crate::theme::{Metrics, Theme};
use crate::ui::context_menu::{ContextMenu, MenuCommand};
use crate::ui::plugin_window::PluginSubject;
use crate::ui::widgets::{ButtonState, ButtonStyle, button, button_enabled, channel_meter};

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = auris_mixer_connections, no_json)]
pub(crate) struct NavigateMixerConnection {
    pub(crate) key: &'static str,
}

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = auris_mixer_connections, no_json)]
pub(crate) struct OpenMixerConnectionMenu;

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = auris_mixer_connections, no_json)]
pub(crate) struct ClearMixerConnectionSelection;

pub(crate) fn key_bindings() -> [gpui::KeyBinding; 6] {
    [
        gpui::KeyBinding::new(
            "left",
            NavigateMixerConnection { key: "left" },
            Some("AurisMixerConnections"),
        ),
        gpui::KeyBinding::new(
            "right",
            NavigateMixerConnection { key: "right" },
            Some("AurisMixerConnections"),
        ),
        gpui::KeyBinding::new(
            "up",
            NavigateMixerConnection { key: "up" },
            Some("AurisMixerConnections"),
        ),
        gpui::KeyBinding::new(
            "down",
            NavigateMixerConnection { key: "down" },
            Some("AurisMixerConnections"),
        ),
        gpui::KeyBinding::new(
            "enter",
            OpenMixerConnectionMenu,
            Some("AurisMixerConnections"),
        ),
        gpui::KeyBinding::new(
            "escape",
            ClearMixerConnectionSelection,
            Some("AurisMixerConnections"),
        ),
    ]
}

const NODE_WIDTH: f32 = 176.0;
const COLUMN: f32 = 360.0;
const ROW: f32 = 280.0;
// Curve control points are separated enough to leave a label-height gap at their midpoints.
const WIRE_LANE_SPACING: f32 = 48.0;
const SIGNAL_FLOW_SPACING: f32 = 120.0;
const SIGNAL_FLOW_LENGTH: f32 = 24.0;
const SIGNAL_FLOW_SPEED: f64 = 90.0;
const MIN_ZOOM: f32 = 0.5;
const MAX_ZOOM: f32 = 2.0;

#[path = "mixer_graph_tooltip.rs"]
mod wire_tooltip;

#[cfg(test)]
#[path = "mixer_graph_wires_tests.rs"]
mod wire_tests;

#[cfg(test)]
#[path = "mixer_graph_keyboard_tests.rs"]
mod keyboard_tests;

#[cfg(test)]
#[path = "mixer_graph_zoom_tests.rs"]
mod zoom_tests;

#[cfg(test)]
#[path = "mixer_graph_persistence_tests.rs"]
mod persistence_tests;

#[cfg(test)]
#[path = "mixer_graph_layout_tests.rs"]
mod layout_tests;

/// Presentation state kept across paints and panel moves.
pub(crate) struct MixerCanvas {
    state: Entity<FlowState>,
    view: Entity<FlowGraph>,
    document: MixerGraph,
    by_id: HashMap<SharedString, MixerNode>,
    send: bool,
    palette: [u32; 4],
    animation_started: Instant,
    last_view: MixerGraphView,
    persist_layout: bool,
    layout_dragging: bool,
    stored_nodes: BTreeMap<String, MixerNodePosition>,
    last_selected_edge: Option<SharedString>,
}

impl MixerCanvas {
    fn presentation(&self, cx: &gpui::App) -> MixerGraphView {
        let state = self.state.read(cx);
        MixerGraphView {
            nodes: state
                .nodes
                .iter()
                .filter_map(|node| {
                    self.by_id.get(&node.id).map(|identity| {
                        (
                            identity.key(),
                            MixerNodePosition {
                                x: node.position.x,
                                y: node.position.y,
                            },
                        )
                    })
                })
                .collect(),
            zoom: state.viewport.zoom,
            pan_x: state.viewport.x,
            pan_y: state.viewport.y,
        }
    }
}

fn wire_style(kind: MixerEdgeKind, theme: &Theme) -> (gpui::Hsla, EdgeStroke) {
    match kind {
        MixerEdgeKind::Output => (theme.mixer_output_color(), EdgeStroke::Solid),
        MixerEdgeKind::Send { .. } => (theme.mixer_send_color(), EdgeStroke::Dashed),
        MixerEdgeKind::Sidechain => (theme.mixer_sidechain_color(), EdgeStroke::Dotted),
        MixerEdgeKind::Chain => (theme.mixer_output_color(), EdgeStroke::Solid),
    }
}

fn wire_activity(peak: f32) -> f32 {
    if !peak.is_finite() || peak <= 0.0 {
        return 0.0;
    }
    // A logarithmic scale makes quiet but audible routes visible without flashing on digital
    // noise. Directional highlights use the same measured strength as the stationary glow.
    ((20.0 * peak.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
}

fn signal_flow_phase(elapsed: Duration) -> f32 {
    // Reduce in double precision before converting, so long-running sessions keep smooth motion.
    (elapsed.as_secs_f64() * SIGNAL_FLOW_SPEED / f64::from(SIGNAL_FLOW_SPACING)).rem_euclid(1.0)
        as f32
}

fn wire_palette(theme: &Theme) -> [u32; 4] {
    [
        theme.mixer_output_color(),
        theme.mixer_send_color(),
        theme.mixer_sidechain_color(),
        theme.accent,
    ]
    .map(|color| u32::from(color.to_rgb()) >> 8)
}

fn wire_lanes(graph: &MixerGraph) -> HashMap<SharedString, f32> {
    let mut pairs: HashMap<_, Vec<_>> = HashMap::new();
    for edge in &graph.edges {
        if !matches!(edge.kind, MixerEdgeKind::Chain) {
            pairs
                .entry((edge.source, edge.target))
                .or_default()
                .push(edge);
        }
    }
    pairs
        .into_values()
        .flat_map(|mut edges| {
            edges.sort_by_key(|edge| match edge.kind {
                MixerEdgeKind::Output => (0, 0),
                MixerEdgeKind::Send { send, .. } => (1, send.0),
                MixerEdgeKind::Sidechain => (2, 0),
                MixerEdgeKind::Chain => (3, 0),
            });
            let center = (edges.len() - 1) as f32 / 2.0;
            edges.into_iter().enumerate().map(move |(index, edge)| {
                (edge_id(edge), (index as f32 - center) * WIRE_LANE_SPACING)
            })
        })
        .collect()
}

fn wire_legend(label: &'static str, color: gpui::Hsla, stroke: EdgeStroke) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_1()
        .child(
            gpui::canvas(
                |_, _, _| (),
                move |bounds, (), window, _| {
                    let y = f32::from(bounds.center().y);
                    let path = gpui_flow::edges::EdgePath {
                        points: vec![
                            (f32::from(bounds.left()), y),
                            (f32::from(bounds.right()), y),
                        ],
                        label: (0.0, 0.0),
                    };
                    gpui_flow::edges::paint_edge_path(&path, stroke, 2.0, color.into(), window);
                },
            )
            .w_6()
            .h_2(),
        )
        .child(label)
        .into_any_element()
}

#[expect(
    clippy::too_many_arguments,
    reason = "Mirror the shared button API while adding the graph's presentation scale."
)]
fn node_button<I, L, F>(
    zoom: f32,
    id: I,
    label: L,
    style: ButtonStyle,
    active: bool,
    color: gpui::Hsla,
    theme: &Theme,
    click: F,
) -> gpui::Stateful<gpui::Div>
where
    I: Into<gpui::ElementId>,
    L: Into<SharedString>,
    F: Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
{
    // The canvas scales rem-based children; Atelier tokens are Pixels and need the same
    // graph zoom explicitly. Interface zoom clamps would break Fit graph at very small scales.
    button(id, label, style, active, color, theme, click)
        .h(theme.atelier.density.control_sm * zoom)
        .px(theme.atelier.density.control_padding * 0.75 * zoom)
        .rounded(
            theme
                .atelier
                .control_radius(theme.atelier.density.control_sm)
                * zoom,
        )
        .text_size(theme.atelier.typography.size_sm * zoom)
}

fn node_id(node: MixerNode) -> SharedString {
    match node {
        MixerNode::Strip(owner) => format!("mix-track-{}", owner.map_or(0, |id| id.0)),

        MixerNode::Effect(_, slot) => format!("mix-effect-{}", slot.0),
    }
    .into()
}

fn edge_id(edge: &MixerEdge) -> SharedString {
    let owner = match edge.source {
        MixerNode::Strip(owner) | MixerNode::Effect(owner, _) => owner,
    };
    let source = MixerNode::Strip(owner).key();
    match edge.kind {
        MixerEdgeKind::Send { send, .. } => format!("mix-send-{}", send.0),
        MixerEdgeKind::Output => format!("mix-output-{source}-{}", edge.target.key()),
        MixerEdgeKind::Sidechain => format!("mix-key-{source}-{}", edge.target.key()),
        MixerEdgeKind::Chain => format!("mix-chain-{}-{}", edge.source.key(), edge.target.key()),
    }
    .into()
}

fn next_editable_edge_index(
    editable: &[SharedString],
    selected: Option<&SharedString>,
    forward: bool,
) -> Option<usize> {
    if editable.is_empty() {
        return None;
    }
    let current = selected.and_then(|id| editable.iter().position(|candidate| candidate == id));
    Some(match current {
        Some(index) if forward => (index + 1) % editable.len(),
        Some(index) => (index + editable.len() - 1) % editable.len(),
        None => 0,
    })
}

#[cfg(test)]
mod keyboard_selection_tests {
    use super::next_editable_edge_index;
    use gpui::SharedString;

    fn ids(values: &[&str]) -> Vec<SharedString> {
        values
            .iter()
            .map(|value| (*value).to_owned().into())
            .collect()
    }

    #[test]
    fn selection_skips_internal_chain_edges_and_wraps_by_stable_id() {
        let editable = ids(&["output-a", "send-7", "sidechain-a"]);
        let selected = SharedString::from("send-7");
        assert_eq!(
            next_editable_edge_index(&editable, Some(&selected), true),
            Some(2)
        );
        assert_eq!(
            next_editable_edge_index(&editable, Some(&selected), false),
            Some(0)
        );
        assert_eq!(next_editable_edge_index(&editable, None, true), Some(0));
    }

    #[test]
    fn empty_selection_has_no_keyboard_target() {
        let empty: Vec<SharedString> = Vec::new();
        assert_eq!(next_editable_edge_index(&empty, None, true), None);
    }
}

fn gesture(graph: &MixerGraph, connection: &Connection) -> Option<(TrackId, MixerNode)> {
    let source = graph.nodes.iter().find_map(|node| match node {
        MixerNode::Strip(Some(track)) | MixerNode::Effect(Some(track), _)
            if node_id(*node) == connection.source =>
        {
            Some(*track)
        }
        _ => None,
    })?;
    let target = graph
        .nodes
        .iter()
        .find(|node| node_id(**node) == connection.target)?;
    Some((source, *target))
}

/// Longest-path columns make every dependency point forward, including sidechains.
fn positions(graph: &MixerGraph) -> HashMap<MixerNode, FlowPoint> {
    let indices: HashMap<_, _> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (*n, i))
        .collect();
    let mut incoming = vec![0; graph.nodes.len()];
    let mut outgoing = vec![Vec::new(); graph.nodes.len()];
    for edge in &graph.edges {
        if let (Some(&from), Some(&to)) = (indices.get(&edge.source), indices.get(&edge.target)) {
            outgoing[from].push(to);
            incoming[to] += 1;
        }
    }
    let mut columns = vec![0; graph.nodes.len()];
    let mut ready: VecDeque<_> = incoming
        .iter()
        .enumerate()
        .filter_map(|(i, &n)| (n == 0).then_some(i))
        .collect();
    while let Some(from) = ready.pop_front() {
        for &to in &outgoing[from] {
            columns[to] = columns[to].max(columns[from] + 1);
            incoming[to] -= 1;
            if incoming[to] == 0 {
                ready.push_back(to);
            }
        }
    }
    let mut rows = HashMap::new();
    let mut y = 24.0;
    for node in &graph.nodes {
        if let MixerNode::Strip(owner) = node {
            rows.insert(*owner, y);
            let sends = graph.edges.iter().filter(|edge| matches!(edge.kind, MixerEdgeKind::Send { track, .. } if Some(track) == *owner)).count();
            y += ROW + sends as f32 * 64.0;
        }
    }
    graph
        .nodes
        .iter()
        .enumerate()
        .map(|(i, node)| {
            let owner = match node {
                MixerNode::Strip(t) | MixerNode::Effect(t, _) => t,
            };
            (
                *node,
                FlowPoint::new(24.0 + columns[i] as f32 * COLUMN, rows[owner]),
            )
        })
        .collect()
}

impl AurisApp {
    fn store_mixer_view(&mut self, view: &Entity<FlowGraph>, cx: &mut gpui::Context<Self>) {
        let Some(canvas) = self
            .mixer_canvas
            .as_mut()
            .filter(|canvas| canvas.view == *view)
        else {
            return;
        };
        let current = canvas.presentation(cx);
        let dragging = canvas.state.read(cx).drag_state.is_some();
        let start = dragging && !canvas.layout_dragging;
        let finish = !dragging && canvas.layout_dragging;
        if current == canvas.last_view && !canvas.persist_layout && !start && !finish {
            return;
        }
        canvas.persist_layout = false;
        canvas.layout_dragging = dragging;
        if start {
            self.session
                .seed_mixer_node_positions(&canvas.last_view.nodes);
            self.session.begin_transaction(Edit::MoveMixerNodes);
        }
        if dragging || finish {
            self.session.move_mixer_nodes(current.nodes.clone());
        }
        let mut stored = self.session.project().mixer_graph_view.clone();
        if !dragging && !finish {
            // Passive synchronization retains the layout of temporarily removed identities.
            stored.nodes.extend(canvas.last_view.nodes.clone());
            stored.nodes.extend(current.nodes.clone());
        }
        canvas.last_view = current.clone();
        stored.zoom = current.zoom;
        stored.pan_x = current.pan_x;
        stored.pan_y = current.pan_y;
        self.session.set_mixer_graph_view(stored);
        if finish {
            self.session.end_transaction();
        }
        canvas.stored_nodes = self.session.project().mixer_graph_view.nodes.clone();
        cx.notify();
    }

    /// Put away the live wire card before Escape falls through to the transport panic command.
    pub(crate) fn dismiss_mixer_wire_tooltip(&mut self, cx: &mut gpui::Context<Self>) -> bool {
        self.mixer_canvas.as_ref().is_some_and(|canvas| {
            canvas
                .view
                .update(cx, |view, cx| view.dismiss_edge_tooltip(cx))
        })
    }

    /// Returns the document edge currently selected in the mixer canvas.
    ///
    /// The flow widget owns selection state while the session owns the authoritative edge. The
    /// lookup deliberately resolves the stable flow ID back through the current graph so an
    /// Inspector never describes a deleted or replaced connection.
    pub(crate) fn selected_mixer_connection(&self, cx: &gpui::App) -> Option<MixerEdge> {
        let canvas = self.mixer_canvas.as_ref()?;
        let id = canvas
            .state
            .read(cx)
            .edges
            .iter()
            .find(|edge| edge.selected)
            .map(|edge| edge.id.clone())?;
        canvas
            .document
            .edges
            .iter()
            .find(|edge| edge_id(edge) == id)
            .copied()
    }

    fn handle_mixer_keyboard(
        &mut self,
        key: &str,
        modifiers: gpui::Modifiers,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> bool {
        let Some((view, state)) = self
            .mixer_canvas
            .as_ref()
            .map(|canvas| (canvas.view.clone(), canvas.state.clone()))
        else {
            return false;
        };
        if !view.read(cx).is_focused(window) {
            return false;
        }
        if key == "escape" {
            // Context menus own Escape while open; let the event reach them first.
            if self.menu.is_some() {
                return false;
            }
            self.dismiss_mixer_wire_tooltip(cx);
            state.update(cx, |state, cx| {
                let changed = state.edges.iter().any(|edge| edge.selected);
                for edge in &mut state.edges {
                    edge.selected = false;
                }
                if changed {
                    cx.notify();
                }
            });
            view.update(cx, |view, _| view.focus(window));
            return true;
        }
        if !matches!(key, "left" | "right" | "up" | "down" | "return" | "enter") {
            return false;
        }
        // Modified arrows belong to zoom/navigation and must not change the edge selection.
        if modifiers.secondary() || modifiers.control || modifiers.alt || modifiers.shift {
            return false;
        }
        let editable: Vec<_> = state
            .read(cx)
            .edges
            .iter()
            .filter(|edge| edge.selectable && !edge.hidden)
            .map(|edge| edge.id.clone())
            .collect();
        if editable.is_empty() {
            return false;
        }
        if matches!(key, "return" | "enter") {
            let Some(id) = state
                .read(cx)
                .edges
                .iter()
                .find(|edge| edge.selected)
                .map(|edge| edge.id.clone())
            else {
                return false;
            };
            state.update(cx, |state, cx| {
                let at = state
                    .edge_label_bounds
                    .iter()
                    .find(|(edge_id, _)| *edge_id == id)
                    .map(|(_, bounds)| bounds.center())
                    .or_else(|| state.canvas_bounds.map(|bounds| bounds.center()))
                    .unwrap_or_default();
                state.pending_edge_menu = Some((id, at));
                cx.notify();
            });
            return true;
        }
        let current_id = state
            .read(cx)
            .edges
            .iter()
            .find(|edge| edge.selected)
            .map(|edge| edge.id.clone());
        let forward = matches!(key, "right" | "down");
        let Some(next) = next_editable_edge_index(&editable, current_id.as_ref(), forward) else {
            return false;
        };
        state.update(cx, |state, cx| {
            state.select_edge(&editable[next], false);
            cx.notify();
        });
        true
    }
    pub(crate) fn poll_mixer_signals(&mut self) {
        self.session.watch_mixer_signals(
            self.mixer_graph_visible && self.panels.is_open(crate::dock::Panel::Mixer),
        );
    }

    fn focus_mixer_connections(&mut self, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        let Some((view, state)) = self
            .mixer_canvas
            .as_ref()
            .map(|canvas| (canvas.view.clone(), canvas.state.clone()))
        else {
            return;
        };
        view.update(cx, |view, _| view.focus(window));
        let first = state
            .read(cx)
            .edges
            .iter()
            .find(|edge| edge.selectable && !edge.hidden)
            .map(|edge| edge.id.clone());
        if let Some(first) = first {
            state.update(cx, |state, cx| {
                state.select_edge(&first, false);
                cx.notify();
            });
        }
    }

    fn reveal_mixer_connection_if_changed(
        &mut self,
        state: &Entity<FlowState>,
        cx: &mut gpui::Context<Self>,
    ) {
        let selected = state
            .read(cx)
            .edges
            .iter()
            .find(|edge| edge.selected && edge.selectable && !edge.hidden)
            .map(|edge| edge.id.clone());
        let changed = self
            .mixer_canvas
            .as_ref()
            .is_some_and(|canvas| canvas.last_selected_edge != selected);
        if let Some(canvas) = self
            .mixer_canvas
            .as_mut()
            .filter(|canvas| canvas.state == *state)
        {
            canvas.last_selected_edge = selected.clone();
        }
        if changed && selected.is_some() {
            // The graph remains the focus owner; this only reveals the Inspector card.
            self.show_pipeline_connection_details(cx);
        }
    }

    /// The graph is rebuilt from the document, preserving the positions of surviving identities.
    pub(crate) fn render_mixer_graph(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        self.poll_mixer_signals();
        let document = self.session.mixer_graph();
        if self.mixer_canvas.is_none() {
            let saved = self.project().mixer_graph_view.clone().sanitized();
            let state = cx.new(|_| {
                let mut state = FlowState::new(Vec::new(), Vec::new());
                state.externally_managed = true;
                state.min_zoom = MIN_ZOOM.min(saved.zoom);
                state.max_zoom = MAX_ZOOM;
                state.zoom_at(saved.zoom, 0.0, 0.0);
                state.viewport.x = saved.pan_x;
                state.viewport.y = saved.pan_y;
                state
            });
            cx.observe(&state, |this, state, cx| {
                this.apply_mixer_connections(&state, cx);
                this.reveal_mixer_connection_if_changed(&state, cx);
                if let Some(view) = this
                    .mixer_canvas
                    .as_ref()
                    .filter(|canvas| canvas.state == state)
                    .map(|canvas| canvas.view.clone())
                {
                    this.store_mixer_view(&view, cx);
                }
                cx.notify();
            })
            .detach();
            let app = cx.entity().downgrade();
            let tooltip_app = app.clone();
            let view = cx.new(|cx| {
                FlowGraph::new(state.clone(), cx)
                    .no_node_chrome()
                    .edge_tooltip(move |id, _, cx| {
                        wire_tooltip::build(tooltip_app.clone(), id.clone(), cx)
                    })
                    .default_renderer(move |node, _, cx| {
                        app.update(cx, |this, cx| this.render_mixer_node(&node.id, cx))
                            .unwrap_or_else(|_| div().into_any_element())
                    })
            });
            cx.observe(&view, |this, view, cx| this.store_mixer_view(&view, cx))
                .detach();
            self.mixer_canvas = Some(MixerCanvas {
                state,
                view,
                document: MixerGraph {
                    nodes: Vec::new(),
                    edges: Vec::new(),
                },
                by_id: HashMap::new(),
                send: !self.mixer_connect_send,
                palette: [0; 4],
                animation_started: Instant::now(),
                last_view: MixerGraphView::default(),
                persist_layout: false,
                layout_dragging: false,
                stored_nodes: saved.nodes,
                last_selected_edge: None,
            });
        }
        self.sync_mixer_graph(&document, cx);
        if let Some(canvas) = &self.mixer_canvas {
            canvas.state.update(cx, |state, _| {
                state.edge_flow = self.appearance.animate_mixer_signals.then_some(EdgeFlow {
                    phase: signal_flow_phase(canvas.animation_started.elapsed()),
                    spacing: SIGNAL_FLOW_SPACING,
                    length: SIGNAL_FLOW_LENGTH,
                    color: u32::from(self.theme.text.to_rgb()) >> 8,
                });
                for (wire, edge) in state.edges.iter_mut().zip(&canvas.document.edges) {
                    wire.activity = wire_activity(self.session.mixer_edge_peak(edge));
                    wire.animated = self.appearance.animate_mixer_signals;
                }
            });
        }
        let theme = self.theme.clone();
        let view = self.mixer_canvas.as_ref().map(|canvas| canvas.view.clone());
        let connection_keys = !self.keys_are_claimed()
            && view
                .as_ref()
                .is_some_and(|view| view.read(cx).is_focused(window));
        let (zoom, min_zoom) = self
            .mixer_canvas
            .as_ref()
            .map_or((1.0, MIN_ZOOM), |canvas| {
                let state = canvas.state.read(cx);
                (state.viewport.zoom, state.min_zoom)
            });
        let percent = zoom * 100.0;
        let zoom_label = if percent >= 1.0 {
            format!("{percent:.0}%")
        } else if percent >= 0.01 {
            format!("{percent:.2}%")
        } else {
            "<0.01%".to_string()
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .bg(theme.background)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(theme.atelier.spacing.xs)
                    .px(theme.atelier.spacing.sm)
                    .min_h(Metrics::PANEL_HEADER_HEIGHT)
                    .py(theme.atelier.spacing.xs)
                    .bg(theme.surface)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text)
                            .child(self.t(Key::MixerGraph)),
                    )
                    .child(button(
                        "mixer-connect-output",
                        self.t(Key::OutputShort),
                        ButtonStyle::Ghost,
                        !self.mixer_connect_send,
                        theme.accent_soft,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.mixer_connect_send = false;
                            cx.notify();
                        }),
                    ))
                    .child(button(
                        "mixer-connect-send",
                        self.t(Key::Sends),
                        ButtonStyle::Ghost,
                        self.mixer_connect_send,
                        theme.accent_soft,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.mixer_connect_send = true;
                            cx.notify();
                        }),
                    ))
                    .child(button(
                        "mixer-add-bus",
                        self.t(Key::MenuNewBusTrack),
                        ButtonStyle::Ghost,
                        false,
                        theme.accent_soft,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.add_bus_track();
                            cx.notify();
                        }),
                    ))
                    .child(button(
                        "mixer-layout",
                        self.t(Key::MixerLayout),
                        ButtonStyle::Ghost,
                        false,
                        theme.accent_soft,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.arrange_mixer_nodes(cx);
                            cx.notify();
                        }),
                    ))
                    .child(div().flex_1())
                    .child(
                        button(
                            "mixer-focus-connections",
                            self.t(Key::MixerFocusConnections),
                            ButtonStyle::Ghost,
                            false,
                            theme.accent_soft,
                            &theme,
                            cx.listener(|this, _, window, cx| {
                                this.focus_mixer_connections(window, cx);
                            }),
                        )
                        .tooltip(self.tip(Key::MixerConnectionKeyboardHint, "")),
                    )
                    .child(button(
                        "mixer-pipeline-details",
                        self.t(Key::PipelineDetails),
                        ButtonStyle::Ghost,
                        false,
                        theme.accent_soft,
                        &theme,
                        cx.listener(|this, _, window, cx| {
                            this.show_pipeline_details(window, cx);
                        }),
                    ))
                    .child(
                        button_enabled(
                            "mixer-zoom-out",
                            "−",
                            ButtonStyle::Ghost,
                            ButtonState::available(false, zoom > min_zoom),
                            theme.accent_soft,
                            &theme,
                            cx.listener(|this, _, _, cx| this.zoom_mixer(Some(1.0 / 1.2), cx)),
                        )
                        .tooltip(self.tip(Key::CmdZoomOut, "")),
                    )
                    .child(
                        button(
                            "mixer-zoom-reset",
                            zoom_label,
                            ButtonStyle::Ghost,
                            false,
                            theme.accent_soft,
                            &theme,
                            cx.listener(|this, _, _, cx| this.zoom_mixer(None, cx)),
                        )
                        .tooltip(self.tip(Key::MixerZoomReset, "")),
                    )
                    .child(
                        button_enabled(
                            "mixer-zoom-in",
                            "+",
                            ButtonStyle::Ghost,
                            ButtonState::available(false, zoom < MAX_ZOOM),
                            theme.accent_soft,
                            &theme,
                            cx.listener(|this, _, _, cx| this.zoom_mixer(Some(1.2), cx)),
                        )
                        .tooltip(self.tip(Key::CmdZoomIn, "")),
                    )
                    .child(button(
                        "mixer-fit-view",
                        self.t(Key::MixerFitView),
                        ButtonStyle::Ghost,
                        false,
                        theme.accent_soft,
                        &theme,
                        cx.listener(|this, _, window, cx| this.fit_mixer_view(window, cx)),
                    ))
                    .child(button(
                        "mixer-animate-signals",
                        self.t(Key::MixerAnimateSignals),
                        ButtonStyle::Ghost,
                        self.appearance.animate_mixer_signals,
                        theme.accent_soft,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            let mut appearance = this.appearance.clone();
                            appearance.animate_mixer_signals = !appearance.animate_mixer_signals;
                            if let Err(error) = this.apply_appearance(appearance, cx) {
                                this.set_failed_status(format!(
                                    "{}: {error}",
                                    this.t(Key::AppearanceSaveFailed)
                                ));
                            }
                            cx.notify();
                        }),
                    ))
                    .child(button(
                        "mixer-show-console",
                        self.t(Key::MixerConsole),
                        ButtonStyle::Ghost,
                        false,
                        theme.accent_soft,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.mixer_graph_visible = false;
                            cx.notify();
                        }),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .when(connection_keys, |element| {
                        element.key_context("AurisMixerConnections")
                    })
                    .on_action(
                        cx.listener(|this, action: &NavigateMixerConnection, window, cx| {
                            if this.handle_mixer_keyboard(
                                action.key,
                                gpui::Modifiers::none(),
                                window,
                                cx,
                            ) {
                                cx.stop_propagation();
                            } else {
                                cx.propagate();
                            }
                        }),
                    )
                    .on_action(
                        cx.listener(|this, _: &OpenMixerConnectionMenu, window, cx| {
                            if this.handle_mixer_keyboard(
                                "enter",
                                gpui::Modifiers::none(),
                                window,
                                cx,
                            ) {
                                cx.stop_propagation();
                            } else {
                                cx.propagate();
                            }
                        }),
                    )
                    .on_action(cx.listener(
                        |this, _: &ClearMixerConnectionSelection, window, cx| {
                            if this.handle_mixer_keyboard(
                                "escape",
                                gpui::Modifiers::none(),
                                window,
                                cx,
                            ) {
                                cx.stop_propagation();
                            } else {
                                cx.propagate();
                            }
                        },
                    ))
                    .children(view),
            )
            .child(
                div()
                    .px(theme.atelier.spacing.sm)
                    .py(theme.atelier.spacing.xs)
                    .text_size(theme.atelier.typography.size_xs)
                    .text_color(theme.text_muted)
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(theme.atelier.spacing.md)
                            .child(self.t(Key::PipelineAudioDomain))
                            .child(wire_legend(
                                self.t(Key::OutputShort),
                                theme.mixer_output_color(),
                                EdgeStroke::Solid,
                            ))
                            .child(wire_legend(
                                self.t(Key::Sends),
                                theme.mixer_send_color(),
                                EdgeStroke::Dashed,
                            ))
                            .child(wire_legend(
                                self.t(Key::MenuSidechain),
                                theme.mixer_sidechain_color(),
                                EdgeStroke::Dotted,
                            )),
                    )
                    .child(self.t(Key::MixerGraphHelp)),
            )
            .into_any_element()
    }

    fn apply_mixer_connections(&mut self, state: &Entity<FlowState>, cx: &mut gpui::Context<Self>) {
        let (pending, menu) = state.update(cx, |state, _| {
            (
                std::mem::take(&mut state.pending_connections),
                state.pending_edge_menu.take(),
            )
        });
        if let Some((_, at)) = menu {
            let selected: HashSet<_> = state
                .read(cx)
                .edges
                .iter()
                .filter(|edge| edge.selected && edge.deletable)
                .map(|edge| edge.id.clone())
                .collect();
            let edges: Vec<_> = self
                .session
                .mixer_graph()
                .edges
                .into_iter()
                .filter(|edge| selected.contains(&edge_id(edge)))
                .collect();
            if !edges.is_empty() {
                self.open_menu(ContextMenu::new(at, self.t(Key::MixerGraph)).item(
                    self.t(Key::MixerDeleteConnections),
                    MenuCommand::DeleteMixerConnections(edges),
                ));
            }
        }
        for connection in pending {
            let document = self.session.mixer_graph();
            let result = gesture(&document, &connection).map(|(from, to)| {
                self.session
                    .connect_mixer(from, to, self.mixer_connect_send)
            });
            match result {
                Some(Ok(true)) => {}
                Some(Err(error)) => self.set_failed_status(self.failure(Key::MixerGraph, &error)),
                _ => self.set_failed_status(self.t(Key::MixerConnectionRejected).to_string()),
            }
        }
    }

    pub(crate) fn delete_mixer_connections(&mut self, edges: &[MixerEdge]) {
        if let Err(error) = self.session.disconnect_mixer_edges(edges) {
            self.set_failed_status(self.failure(Key::MixerGraph, &error));
        }
    }

    pub(crate) fn delete_focused_mixer_connections(
        &mut self,
        window: &gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> bool {
        let Some(canvas) = self
            .mixer_canvas
            .as_ref()
            .filter(|canvas| canvas.view.read(cx).is_focused(window))
        else {
            return false;
        };
        let selected: HashSet<_> = canvas
            .state
            .read(cx)
            .edges
            .iter()
            .filter(|edge| edge.selected && edge.deletable)
            .map(|edge| edge.id.clone())
            .collect();
        let edges: Vec<_> = self
            .session
            .mixer_graph()
            .edges
            .into_iter()
            .filter(|edge| selected.contains(&edge_id(edge)))
            .collect();
        self.delete_mixer_connections(&edges);
        true
    }

    fn sync_mixer_graph(&mut self, document: &MixerGraph, cx: &mut gpui::Context<Self>) {
        let palette = wire_palette(&self.theme);
        let changed = self.mixer_canvas.as_ref().is_some_and(|canvas| {
            canvas.document != *document
                || canvas.send != self.mixer_connect_send
                || canvas.palette != palette
        });
        let routing_changed = self.mixer_canvas.as_ref().is_some_and(|canvas| {
            canvas.send != self.mixer_connect_send
                || canvas.document.nodes != document.nodes
                || canvas.document.edges.len() != document.edges.len()
                || canvas
                    .document
                    .edges
                    .iter()
                    .zip(&document.edges)
                    .any(|(a, b)| {
                        a.source != b.source
                            || a.target != b.target
                            || std::mem::discriminant(&a.kind) != std::mem::discriminant(&b.kind)
                    })
        });
        let mut allowed = HashSet::new();
        if routing_changed {
            for node in &document.nodes {
                let source = match node {
                    MixerNode::Strip(Some(id)) | MixerNode::Effect(Some(id), _) => Some(*id),
                    _ => None,
                };
                if let Some(source) = source
                    && self.session.mixer_graph_output(Some(source)) == *node
                {
                    for target in &document.nodes {
                        if self
                            .session
                            .can_connect_mixer(source, *target, self.mixer_connect_send)
                        {
                            allowed.insert((node_id(*node), node_id(*target)));
                        }
                    }
                }
            }
        }
        let theme = self.theme.clone();
        let saved_nodes = &self.session.project().mixer_graph_view.nodes;
        if let Some(canvas) = self.mixer_canvas.as_mut()
            && canvas.stored_nodes != *saved_nodes
        {
            let layout = positions(document);
            canvas.state.update(cx, |state, _| {
                for node in &mut state.nodes {
                    if let Some(identity) = canvas.by_id.get(&node.id) {
                        let key = identity.key();
                        if canvas.stored_nodes.get(&key) != saved_nodes.get(&key) {
                            node.position = saved_nodes.get(&key).map_or_else(
                                || layout.get(identity).copied().unwrap_or(node.position),
                                |position| FlowPoint::new(position.x, position.y),
                            );
                        }
                    }
                }
                state.edge_label_bounds.clear();
            });
            canvas.stored_nodes = saved_nodes.clone();
            canvas.last_view = canvas.presentation(cx);
        }
        let Some(mut canvas) = self.mixer_canvas.take() else {
            return;
        };
        canvas.view.update(cx, |view, cx| {
            view.set_colors(
                theme.background,
                theme.border_subtle,
                theme.accent,
                theme.text_muted,
            );
            cx.notify();
        });
        if !changed {
            self.mixer_canvas = Some(canvas);
            return;
        }
        let saved = self.project().mixer_graph_view.clone().sanitized();
        let layout = positions(document);
        let lanes = wire_lanes(document);
        let old_owners: HashSet<_> = canvas.document.nodes.iter().copied().collect();
        let mut sidechain_slots = HashSet::new();
        for node in &document.nodes {
            if let MixerNode::Effect(track, slot) = node
                && self.session.effect_wants_sidechain(*track, *slot)
            {
                sidechain_slots.insert(*slot);
            }
        }
        canvas.state.update(cx, |state, _| {
            let nodes = document
                .nodes
                .iter()
                .map(|node| {
                    let id = node_id(*node);
                    let existing = state.get_node(&id).filter(|_| old_owners.contains(node));
                    let position = existing
                        .map(|n| n.position)
                        .or_else(|| {
                            saved
                                .nodes
                                .get(&node.key())
                                .map(|p| FlowPoint::new(p.x, p.y))
                        })
                        .unwrap_or(layout[node]);
                    let handles = match node {
                        MixerNode::Strip(owner) => {
                            let mut handles = Vec::new();
                            if owner.is_some() && self.session.mixer_graph_output(*owner) == *node {
                                handles.push(HandleDef::source(HandlePosition::Right));
                            }
                            if owner.is_none_or(|id| {
                                self.project()
                                    .track(id)
                                    .is_some_and(|track| track.kind.is_bus())
                            }) {
                                handles.push(HandleDef::target(HandlePosition::Left));
                            }
                            handles
                        }
                        MixerNode::Effect(owner, slot) => {
                            let mut handles = Vec::new();
                            if owner.is_some() && self.session.mixer_graph_output(*owner) == *node {
                                handles.push(HandleDef::source(HandlePosition::Right));
                            }
                            if sidechain_slots.contains(slot) {
                                handles
                                    .push(HandleDef::target(HandlePosition::Top).id("sidechain"));
                            }
                            handles
                        }
                    };
                    let mut flow = FlowNode::new(id, position.x, position.y)
                        .size(NODE_WIDTH, 136.0)
                        .handles(
                            handles
                                .into_iter()
                                .map(|handle| {
                                    handle
                                        .color(u32::from(theme.mixer_output_color().to_rgb()) >> 8)
                                })
                                .collect(),
                        );
                    flow.deletable = false;
                    if let Some(existing) = existing {
                        flow.measured_width = existing.measured_width;
                        flow.measured_height = existing.measured_height;
                        flow.selected = existing.selected;
                        flow.dragging = existing.dragging;
                    }
                    flow
                })
                .collect();
            let edges = document
                .edges
                .iter()
                .map(|edge| {
                    let id = edge_id(edge);
                    let (color, stroke) = wire_style(edge.kind, &theme);
                    let mut flow =
                        FlowEdge::new(id.clone(), node_id(edge.source), node_id(edge.target))
                            .color(u32::from(color.to_rgb()) >> 8);
                    flow.stroke = stroke;
                    flow.lane_offset = lanes.get(&id).copied().unwrap_or_default();
                    flow.selected = state
                        .edges
                        .iter()
                        .any(|existing| existing.id == id && existing.selected);
                    flow.deletable = !matches!(edge.kind, MixerEdgeKind::Chain);
                    flow.selectable = flow.deletable;
                    if !flow.deletable {
                        flow.stroke_width = Some(1.0);
                    }
                    match edge.kind {
                        MixerEdgeKind::Send {
                            send,
                            level_db,
                            pre_fader,
                            ..
                        } => {
                            flow = flow.label(format!(
                                "{} #{} {level_db:.1} dB{}",
                                self.t(Key::Sends),
                                send.0,
                                if pre_fader {
                                    format!(" · {}", self.t(Key::SendPreFaderMark))
                                } else {
                                    String::new()
                                }
                            ));
                        }
                        MixerEdgeKind::Sidechain => {
                            flow = flow
                                .label(self.t(Key::MenuSidechain))
                                .target_handle("sidechain");
                        }
                        MixerEdgeKind::Output => flow = flow.label(self.t(Key::OutputShort)),
                        _ => {}
                    }
                    flow
                })
                .collect();
            state.set_nodes(nodes);
            state.set_edges(edges);
            state.edge_selection_color = Some(u32::from(theme.accent.to_rgb()) >> 8);
            if routing_changed {
                state.allowed_connections = Some(allowed);
            }
        });
        let initial = canvas.document.nodes.is_empty();
        // A route edit may change automatic columns/rows without moving existing cards.
        // Persist those cards too, otherwise reopening would silently rearrange them.
        canvas.persist_layout |= !initial && canvas.document != *document;
        canvas.document = document.clone();
        canvas.by_id = document
            .nodes
            .iter()
            .map(|node| (node_id(*node), *node))
            .collect();
        canvas.send = self.mixer_connect_send;
        canvas.palette = palette;
        // Opening a canvas must not turn a clean project into an unsaved document.
        if initial {
            canvas.last_view = canvas.presentation(cx);
        }
        let persist = canvas.persist_layout.then(|| canvas.view.clone());
        self.mixer_canvas = Some(canvas);
        if let Some(view) = persist {
            // GPUI suppresses observer notifications during drawing. Store structural layout
            // changes after paint, even when they did not require a pointer gesture.
            let app = cx.entity().downgrade();
            cx.defer(move |cx| {
                let _ = app.update(cx, |this, cx| this.store_mixer_view(&view, cx));
            });
        }
    }

    fn arrange_mixer_nodes(&mut self, cx: &mut gpui::Context<Self>) {
        let layout = positions(&self.session.mixer_graph());
        if let Some(canvas) = &mut self.mixer_canvas {
            let before = canvas.presentation(cx);
            self.session.seed_mixer_node_positions(&before.nodes);
            let nodes = layout
                .iter()
                .map(|(node, position)| {
                    (
                        node.key(),
                        MixerNodePosition {
                            x: position.x,
                            y: position.y,
                        },
                    )
                })
                .collect();
            self.session.arrange_mixer_nodes(nodes);
            canvas.state.update(cx, |state, _| {
                for (node, position) in layout {
                    if let Some(node) = state.get_node_mut(&node_id(node)) {
                        node.position = position;
                    }
                }
                state.zoom_at(1.0, 0.0, 0.0);
                state.viewport = Default::default();
            });
            let current = canvas.presentation(cx);
            let mut stored = self.session.project().mixer_graph_view.clone();
            stored.zoom = current.zoom;
            stored.pan_x = current.pan_x;
            stored.pan_y = current.pan_y;
            self.session.set_mixer_graph_view(stored);
            canvas.last_view = current;
            canvas.stored_nodes = self.session.project().mixer_graph_view.nodes.clone();
            canvas.view.update(cx, |_, cx| cx.notify());
        }
    }

    /// Zoom about the visible canvas centre; a missing factor restores the native scale.
    fn zoom_mixer(&mut self, factor: Option<f32>, cx: &mut gpui::Context<Self>) {
        if let Some(canvas) = &self.mixer_canvas {
            canvas.state.update(cx, |state, _| {
                if let Some(bounds) = state.canvas_bounds {
                    let zoom = factor.map_or(1.0, |factor| state.viewport.zoom * factor);
                    state.zoom_at(
                        zoom,
                        f32::from(bounds.size.width) / 2.0,
                        f32::from(bounds.size.height) / 2.0,
                    );
                }
            });
            canvas.view.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    /// Frame the entire graph without rearranging nodes or editing audio routing.
    fn fit_mixer_view(&mut self, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        self.fit_mixer_view_pass(window, cx, 2);
    }

    /// Refine once newly visible cards have their actual heights, rather than fallback estimates.
    fn fit_mixer_view_pass(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
        remaining: u8,
    ) {
        if let Some(canvas) = &self.mixer_canvas {
            let padding = f32::from(window.rem_size()) * 1.5;
            canvas.state.update(cx, |state, _| {
                if let Some(bounds) = state.canvas_bounds {
                    state.min_zoom = MIN_ZOOM;
                    state.fit_view(
                        padding,
                        f32::from(bounds.size.width),
                        f32::from(bounds.size.height),
                    );
                }
            });
            canvas.view.update(cx, |_, cx| cx.notify());
            if remaining > 0 {
                let identity = canvas.state.entity_id();
                let viewport = canvas.state.read(cx).viewport;
                let expected = (viewport.x, viewport.y, viewport.zoom);
                // The first callback precedes the requested frame. The second runs after its
                // node and label measurements are available. Navigation meanwhile cancels it.
                cx.on_next_frame(window, move |_, window, cx| {
                    cx.on_next_frame(window, move |this, window, cx| {
                        let unchanged = this.mixer_canvas.as_ref().is_some_and(|canvas| {
                            let viewport = canvas.state.read(cx).viewport;
                            canvas.state.entity_id() == identity
                                && (viewport.x, viewport.y, viewport.zoom) == expected
                        });
                        if unchanged
                            && this.mixer_graph_visible
                            && this.panels.is_open(crate::dock::Panel::Mixer)
                        {
                            this.fit_mixer_view_pass(window, cx, remaining - 1);
                        }
                    });
                });
            }
        }
        cx.notify();
    }

    fn render_mixer_node(&mut self, id: &SharedString, cx: &mut gpui::Context<Self>) -> AnyElement {
        let Some(node) = self
            .mixer_canvas
            .as_ref()
            .and_then(|canvas| canvas.by_id.get(id))
            .copied()
        else {
            return div().into_any_element();
        };
        let owner = match node {
            MixerNode::Strip(t) | MixerNode::Effect(t, _) => t,
        };
        let theme = self.theme.clone();
        let (zoom, selected) = self.mixer_canvas.as_ref().map_or((1.0, false), |canvas| {
            let state = canvas.state.read(cx);
            (
                state.viewport.zoom,
                state.get_node(id).is_some_and(|node| node.selected),
            )
        });
        let name = owner.and_then(|id| self.project().track(id)).map_or_else(
            || self.t(Key::Master).to_string(),
            |track| track.name.clone(),
        );
        let mut card = div()
            .w(px(NODE_WIDTH * zoom))
            .min_h(px(96.0 * zoom))
            .p(theme.atelier.spacing.sm * zoom)
            .flex()
            .flex_col()
            .gap(theme.atelier.spacing.xs * zoom)
            .bg(theme.surface)
            .border_1()
            .border_color(if selected { theme.accent } else { theme.border })
            .rounded(theme.atelier.shape.radius_md * zoom)
            .text_size(theme.atelier.typography.size_xs * zoom)
            .text_color(theme.text)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    if let Some(id) = owner {
                        this.select_track(id);
                    }
                    cx.notify();
                }),
            )
            .child(
                div()
                    .debug_selector({
                        let id = id.clone();
                        move || format!("graph-node-title-{id}")
                    })
                    .min_w_0()
                    .truncate()
                    .rounded(theme.atelier.shape.radius_xs * zoom)
                    .when(selected, |header| header.bg(theme.atelier.colors.selected))
                    .text_color(theme.text_muted)
                    .child(name),
            );
        match node {
            MixerNode::Effect(_, slot) => {
                let effect = owner
                    .and_then(|id| self.project().track(id))
                    .map_or(&self.project().master, |t| &t.mixer)
                    .effects
                    .iter()
                    .find(|effect| effect.id == slot);
                if let Some(effect) = effect {
                    let label = self.effect_label(slot, &effect.effect_id.clone());
                    let enabled = effect.enabled;
                    let menu_label = label.clone();
                    let subject = PluginSubject::Insert { track: owner, slot };
                    let open = self.plugin_editor_is_open(subject);
                    card = card
                        .child(div().truncate().child(label))
                        .child(node_button(
                            zoom,
                            SharedString::from(format!("graph-effect-open-{id}")),
                            self.t(if open {
                                Key::PluginRaiseEditor
                            } else {
                                Key::PluginOpenEditor
                            }),
                            ButtonStyle::Ghost,
                            false,
                            theme.accent_soft,
                            &theme,
                            cx.listener(move |this, _, _, cx| {
                                this.show_plugin_editor(subject);
                                cx.notify();
                            }),
                        ))
                        .child(node_button(
                            zoom,
                            SharedString::from(format!("graph-effect-enabled-{id}")),
                            self.t(Key::MenuEnabled),
                            ButtonStyle::Ghost,
                            enabled,
                            theme.accent_soft,
                            &theme,
                            cx.listener(move |this, _, _, cx| {
                                this.session.set_effect_enabled(owner, slot, !enabled);
                                cx.notify();
                            }),
                        ))
                        .child(node_button(
                            zoom,
                            SharedString::from(format!("graph-effect-menu-{id}")),
                            self.t(Key::Effect),
                            ButtonStyle::Ghost,
                            false,
                            theme.accent_soft,
                            &theme,
                            Self::opens_menu(cx, move |this, at| {
                                this.effect_menu(at, owner, slot, menu_label.clone())
                            }),
                        ));
                }
            }
            MixerNode::Strip(_) => {
                let strip = owner
                    .and_then(|id| self.project().track(id))
                    .map_or(&self.project().master, |t| &t.mixer);
                let (gain, pan, muted, soloed) = (strip.gain_db, strip.pan, strip.mute, strip.solo);
                card = card
                    .child(node_button(
                        zoom,
                        SharedString::from(format!("graph-add-fx-{id}")),
                        self.t(Key::MenuAddEffect),
                        ButtonStyle::Ghost,
                        false,
                        theme.accent_soft,
                        &theme,
                        Self::opens_menu(cx, move |this, at| this.effect_picker_menu(at, owner)),
                    ))
                    .child(
                        self.fader_control(
                            SharedString::from(format!("graph-gain-{id}")),
                            self.t(Key::Volume),
                            owner.map_or(ParamTarget::MasterGain, ParamTarget::TrackGain),
                            gain,
                            cx,
                        )
                        .h(Metrics::CONTROL_HEIGHT * zoom),
                    )
                    .child(
                        self.fader_control(
                            SharedString::from(format!("graph-pan-{id}")),
                            self.t(Key::Pan),
                            owner.map_or(ParamTarget::MasterPan, ParamTarget::TrackPan),
                            pan,
                            cx,
                        )
                        .h(Metrics::CONTROL_HEIGHT * zoom),
                    );
                if let Some(track) = owner {
                    let index = self.project().track_index(track).unwrap_or(0);
                    let sends: Vec<_> = self
                        .project()
                        .track(track)
                        .into_iter()
                        .flat_map(|track| &track.sends)
                        .cloned()
                        .collect();
                    let meter = self.track_meter(index);
                    card = card
                        .child(
                            div()
                                .flex()
                                .gap_1()
                                .child(
                                    node_button(
                                        zoom,
                                        SharedString::from(format!("graph-mute-{id}")),
                                        self.t(Key::MuteInitial),
                                        ButtonStyle::Normal,
                                        muted,
                                        theme.mute,
                                        &theme,
                                        cx.listener(move |this, _, _, cx| {
                                            this.toggle_mute(track);
                                            cx.notify();
                                        }),
                                    )
                                    .tooltip(self.tip(Key::CmdToggleTrackMute, "track.mute")),
                                )
                                .child(
                                    node_button(
                                        zoom,
                                        SharedString::from(format!("graph-solo-{id}")),
                                        self.t(Key::SoloInitial),
                                        ButtonStyle::Normal,
                                        soloed,
                                        theme.solo,
                                        &theme,
                                        cx.listener(move |this, _, _, cx| {
                                            this.toggle_solo(track);
                                            cx.notify();
                                        }),
                                    )
                                    .tooltip(self.tip(Key::CmdToggleTrackSolo, "track.solo")),
                                ),
                        )
                        .child(div().h(px(6.0 * zoom)).child(channel_meter(
                            meter,
                            self.session.meters().track_clipped(index),
                            Axis::Horizontal,
                            &theme,
                        )))
                        .child(node_button(
                            zoom,
                            SharedString::from(format!("graph-route-menu-{id}")),
                            self.t(Key::OutputShort),
                            ButtonStyle::Ghost,
                            false,
                            theme.accent_soft,
                            &theme,
                            Self::opens_menu(cx, move |this, at| this.output_menu(at, track)),
                        ));
                    if self.session.buses().next().is_some() {
                        card = card.child(node_button(
                            zoom,
                            SharedString::from(format!("graph-send-menu-{id}")),
                            self.t(Key::Sends),
                            ButtonStyle::Ghost,
                            false,
                            theme.accent_soft,
                            &theme,
                            Self::opens_menu(cx, move |this, at| this.send_picker_menu(at, track)),
                        ));
                    }
                    for (position, send) in sends.iter().enumerate() {
                        card = card.child(self.send_row(
                            index,
                            position,
                            track,
                            send.id,
                            send.target,
                            send.level_db,
                            send.pre_fader,
                            zoom,
                            cx,
                        ));
                    }
                } else {
                    let meter = self.master_meter();
                    card = card.child(div().h(px(6.0 * zoom)).child(channel_meter(
                        meter,
                        self.session.meters().master_clipped(),
                        Axis::Horizontal,
                        &theme,
                    )));
                }
            }
        }
        card.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::{Dock, Panel};
    use crate::harness::{click, drag, open, paint};
    use gpui::{TestAppContext, VisualTestContext};

    fn selector(text: String) -> &'static str {
        Box::leak(text.into_boxed_str())
    }

    fn expose_pair(
        app: &Entity<AurisApp>,
        cx: &mut VisualTestContext,
        from: MixerNode,
        to: MixerNode,
    ) {
        app.update(cx, |this, cx| {
            let canvas = this.mixer_canvas.as_ref().unwrap();
            canvas.state.update(cx, |state, _| {
                for node in &mut state.nodes {
                    node.position = if node.id == node_id(from) {
                        FlowPoint::new(24.0, 24.0)
                    } else if node.id == node_id(to) {
                        FlowPoint::new(380.0, 24.0)
                    } else {
                        FlowPoint::new(3000.0, 3000.0)
                    };
                }
            });
            canvas.view.update(cx, |_, cx| cx.notify());
        });
        paint(app, cx);
    }

    fn connect(cx: &mut VisualTestContext, from: MixerNode, to: MixerNode) {
        let source = cx
            .debug_bounds(selector(format!("flow-handle-{}-0", node_id(from))))
            .unwrap();
        let target = cx
            .debug_bounds(selector(format!("flow-handle-{}-1", node_id(to))))
            .unwrap();
        let canvas = cx.debug_bounds("flow-graph").unwrap();
        assert!(
            canvas.contains(&source.center()) && canvas.contains(&target.center()),
            "the gesture must land within the mixer"
        );
        drag(cx, source.center(), target.center());
    }

    fn show(this: &mut AurisApp) {
        this.panels.set_detached(Panel::Mixer, false);
        this.panels.move_to(Panel::Mixer, Dock::Bottom);
        this.panels.set_size(Dock::Bottom, px(520.0));
        this.panels.show(Panel::Mixer);
    }

    #[gpui::test]
    fn pointer_connections_change_the_document_and_follow_undo(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let (a, b) = app.update(cx, |this, _| {
            show(this);
            (
                this.session.add_bus_track("A"),
                this.session.add_bus_track("B"),
            )
        });
        paint(&app, cx);
        expose_pair(
            &app,
            cx,
            MixerNode::Strip(Some(a)),
            MixerNode::Strip(Some(b)),
        );
        connect(cx, MixerNode::Strip(Some(a)), MixerNode::Strip(Some(b)));
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.project().track(a).unwrap().outputs,
                vec![Output::Master, Output::Bus(b)]
            )
        });
        cx.dispatch_action(crate::actions::Undo);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.project().track(a).unwrap().outputs,
                vec![Output::Master]
            );
            assert_eq!(
                this.mixer_canvas.as_ref().unwrap().document,
                this.session.mixer_graph()
            );
        });
        click("mixer-show-console", cx);
        paint(&app, cx);
        assert!(cx.debug_bounds("mixer-show-graph").is_some());
        click("mixer-show-graph", cx);
        paint(&app, cx);
        assert!(cx.debug_bounds("flow-graph").is_some());
    }

    #[gpui::test]
    fn track_and_fader_controls_share_one_graph_node(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let track = app.update(cx, |this, _| {
            show(this);
            this.session.add_bus_track("Bus")
        });
        paint(&app, cx);
        assert!(
            cx.debug_bounds(selector(format!(
                "graph-gain-{}",
                node_id(MixerNode::Strip(Some(track)))
            )))
            .is_some()
        );
        app.read_with(cx, |this, _| {
            let nodes = &this.mixer_canvas.as_ref().unwrap().document.nodes;
            assert_eq!(nodes.len(), this.project().tracks.len() + 1);
            assert_eq!(
                nodes
                    .iter()
                    .filter(|node| **node == MixerNode::Strip(Some(track)))
                    .count(),
                1,
                "a track and its fader have one shared node"
            );
            assert!(nodes.iter().all(|node| matches!(node, MixerNode::Strip(_))));
        });
    }

    #[gpui::test]
    fn selecting_a_wire_deletes_only_that_output_and_supports_undo_and_context_menu(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = open(cx);
        let (a, b) = app.update(cx, |this, _| {
            show(this);
            let a = this.session.add_bus_track("A");
            let b = this.session.add_bus_track("B");
            this.session.add_track_output(a, Output::Bus(b)).unwrap();
            (a, b)
        });
        paint(&app, cx);
        expose_pair(
            &app,
            cx,
            MixerNode::Strip(Some(a)),
            MixerNode::Strip(Some(b)),
        );
        let source = cx
            .debug_bounds(selector(format!(
                "flow-handle-{}-0",
                node_id(MixerNode::Strip(Some(a)))
            )))
            .unwrap()
            .center();
        let target = cx
            .debug_bounds(selector(format!(
                "flow-handle-{}-1",
                node_id(MixerNode::Strip(Some(b)))
            )))
            .unwrap()
            .center();
        let wire = gpui::point((source.x + target.x) / 2., (source.y + target.y) / 2.);
        cx.simulate_click(wire, gpui::Modifiers::none());
        paint(&app, cx);
        app.read_with(cx, |this, cx| {
            let state = this.mixer_canvas.as_ref().unwrap().state.read(cx);
            assert_eq!(state.edges.iter().filter(|edge| edge.selected).count(), 1);
        });
        cx.simulate_keystrokes("delete");
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.project().track(a).unwrap().outputs,
                vec![Output::Master]
            )
        });
        cx.dispatch_action(crate::actions::Undo);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.project().track(a).unwrap().outputs,
                vec![Output::Master, Output::Bus(b)]
            )
        });
        cx.dispatch_action(crate::actions::Redo);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.project().track(a).unwrap().outputs,
                vec![Output::Master]
            )
        });
        cx.dispatch_action(crate::actions::Undo);
        paint(&app, cx);
        crate::harness::right_press(cx, wire);
        paint(&app, cx);
        assert!(cx.debug_bounds("context-menu-panel").is_some());
        click("menu-item-0", cx);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.project().track(a).unwrap().outputs,
                vec![Output::Master]
            )
        });
        cx.dispatch_action(crate::actions::Undo);
        paint(&app, cx);
        cx.simulate_click(wire, gpui::Modifiers::none());
        cx.simulate_keystrokes("backspace");
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.project().track(a).unwrap().outputs,
                vec![Output::Master]
            )
        });
    }

    #[gpui::test]
    fn pointer_feedback_is_rejected_without_a_speculative_edge(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let (a, b) = app.update(cx, |this, _| {
            show(this);
            let a = this.session.add_bus_track("A");
            let b = this.session.add_bus_track("B");
            this.session.set_track_output(a, Output::Bus(b)).unwrap();
            (a, b)
        });
        paint(&app, cx);
        let before = app.read_with(cx, |this, _| this.session.mixer_graph());
        expose_pair(
            &app,
            cx,
            MixerNode::Strip(Some(b)),
            MixerNode::Strip(Some(a)),
        );
        connect(cx, MixerNode::Strip(Some(b)), MixerNode::Strip(Some(a)));
        paint(&app, cx);
        app.read_with(cx, |this, cx| {
            assert_eq!(this.session.mixer_graph(), before);
            let canvas = this.mixer_canvas.as_ref().unwrap();
            assert_eq!(canvas.state.read(cx).edges.len(), before.edges.len());
        });
    }

    #[gpui::test]
    fn modifier_selection_deletes_two_outputs_in_one_undo_step(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let (a, b, c) = app.update(cx, |this, _| {
            show(this);
            let a = this.session.add_bus_track("A");
            let b = this.session.add_bus_track("B");
            let c = this.session.add_bus_track("C");
            this.session.add_track_output(a, Output::Bus(b)).unwrap();
            this.session.add_track_output(a, Output::Bus(c)).unwrap();
            (a, b, c)
        });
        paint(&app, cx);
        expose_pair(
            &app,
            cx,
            MixerNode::Strip(Some(a)),
            MixerNode::Strip(Some(b)),
        );
        app.update(cx, |this, cx| {
            let canvas = this.mixer_canvas.as_ref().unwrap();
            canvas.state.update(cx, |state, _| {
                state
                    .get_node_mut(&node_id(MixerNode::Strip(Some(c))))
                    .unwrap()
                    .position = FlowPoint::new(380., 240.);
            });
            canvas.view.update(cx, |_, cx| cx.notify());
        });
        paint(&app, cx);
        let from = cx
            .debug_bounds(selector(format!(
                "flow-handle-{}-0",
                node_id(MixerNode::Strip(Some(a)))
            )))
            .unwrap()
            .center();
        for (index, target) in [b, c].into_iter().enumerate() {
            let to = cx
                .debug_bounds(selector(format!(
                    "flow-handle-{}-1",
                    node_id(MixerNode::Strip(Some(target)))
                )))
                .unwrap()
                .center();
            cx.simulate_click(
                gpui::point((from.x + to.x) / 2., (from.y + to.y) / 2.),
                if index == 0 {
                    gpui::Modifiers::none()
                } else {
                    gpui::Modifiers::secondary_key()
                },
            );
            paint(&app, cx);
        }
        app.read_with(cx, |this, cx| {
            assert_eq!(
                this.mixer_canvas
                    .as_ref()
                    .unwrap()
                    .state
                    .read(cx)
                    .edges
                    .iter()
                    .filter(|edge| edge.selected)
                    .count(),
                2
            )
        });
        cx.simulate_keystrokes("delete");
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.project().track(a).unwrap().outputs,
                vec![Output::Master]
            )
        });
        cx.dispatch_action(crate::actions::Undo);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.project().track(a).unwrap().outputs,
                vec![Output::Master, Output::Bus(b), Output::Bus(c)]
            )
        });
    }

    #[gpui::test]
    fn send_mode_adds_a_branch_and_effect_nodes_control_the_real_insert(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let (a, b, slot) = app.update(cx, |this, _| {
            show(this);
            let a = this.session.add_bus_track("A");
            let b = this.session.add_bus_track("B");
            let slot = this.session.add_effect(Some(a), "auris.fx.reverb").unwrap();
            (a, b, slot)
        });
        paint(&app, cx);
        click("mixer-connect-send", cx);
        paint(&app, cx);
        expose_pair(
            &app,
            cx,
            MixerNode::Effect(Some(a), slot),
            MixerNode::Strip(Some(b)),
        );
        connect(
            cx,
            MixerNode::Effect(Some(a), slot),
            MixerNode::Strip(Some(b)),
        );
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            let track = this.project().track(a).unwrap();
            assert_eq!(track.outputs, vec![Output::Master]);
            assert_eq!(track.sends[0].target, b);
        });
        expose_pair(
            &app,
            cx,
            MixerNode::Effect(Some(a), slot),
            MixerNode::Strip(Some(b)),
        );
        click(
            selector(format!(
                "graph-effect-enabled-{}",
                node_id(MixerNode::Effect(Some(a), slot))
            )),
            cx,
        );
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.session.effect_enabled(Some(a), slot), Some(false))
        });
    }

    #[test]
    fn automatic_layout_points_every_dependency_forward() {
        let mut session =
            auris_session::Session::new(auris_session::SessionOptions::headless()).unwrap();
        let a = session.add_bus_track("A");
        let b = session.add_bus_track("B");
        let slot = session.add_effect(Some(b), "auris.fx.compressor").unwrap();
        session
            .set_effect_sidechain(Some(b), slot, Some(a))
            .unwrap();
        let graph = session.mixer_graph();
        let layout = positions(&graph);
        assert!(
            graph
                .edges
                .iter()
                .all(|edge| layout[&edge.source].x < layout[&edge.target].x)
        );
    }
}
