use crate::PixelsExt;
use std::collections::HashMap;
use std::{rc::Rc, time::Duration};

use gpui::prelude::FluentBuilder;
use gpui::*;

use crate::edges;
use crate::rem_scope::RemScope;
use crate::store::FlowState;
use crate::types::*;

type NodeRendererFn = Box<dyn Fn(&FlowNode, &mut Window, &mut App) -> AnyElement>;
type EdgeTooltipFn = Box<dyn Fn(&EdgeId, &mut Window, &mut App) -> AnyView>;

/// The top-level flow graph component.
///
/// Renders nodes as positioned divs and edges via a canvas paint layer.
pub struct FlowGraph {
    state: Entity<FlowState>,
    focus_handle: FocusHandle,
    node_renderers: HashMap<SharedString, NodeRendererFn>,
    default_renderer: Option<NodeRendererFn>,
    edge_tooltip: Option<EdgeTooltipFn>,
    hovered_edge: Option<EdgeId>,
    hover_task: Option<Task<()>>,
    hover_ready: bool,
    tooltip_view: Option<AnyView>,
    /// Called when a new connection is completed.
    on_connect: Option<Box<dyn Fn(&Connection, &mut FlowState)>>,
    /// Custom validation for connections. Return false to reject.
    is_valid_connection: Option<Box<dyn Fn(&Connection, &FlowState) -> bool>>,
    /// Whether to show the default node wrapper chrome (bg, border, shadow, padding).
    show_node_chrome: bool,
    /// Background color for the canvas (default: 0xf8f8f8).
    bg_color: u32,
    /// Dot grid color (default: 0xd4d4d4).
    grid_color: u32,
    /// Background pattern style.
    bg_pattern: BackgroundPattern,
    /// Node wrapper background color.
    node_bg_color: u32,
    /// Node wrapper border color.
    node_border_color: u32,
    /// Whether we've done the initial measurement pass.
    measured: bool,
    selection_color: u32,
    text_color: u32,
}

impl FlowGraph {
    /// Whether keyboard commands currently target the canvas itself rather than a child control.
    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    /// Give keyboard focus to the graph canvas from an embedding toolbar action.
    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus_handle);
    }

    pub fn new(state: Entity<FlowState>, cx: &mut Context<Self>) -> Self {
        Self {
            state,
            focus_handle: cx.focus_handle(),
            node_renderers: HashMap::new(),
            default_renderer: None,
            edge_tooltip: None,
            hovered_edge: None,
            hover_task: None,
            hover_ready: false,
            tooltip_view: None,
            on_connect: None,
            is_valid_connection: None,
            show_node_chrome: true,
            bg_color: 0xf8f8f8,
            grid_color: 0xd4d4d4,
            bg_pattern: BackgroundPattern::Dots,
            node_bg_color: 0xffffff,
            node_border_color: 0xe2e2e2,
            measured: false,
            selection_color: 0x3b82f6,
            text_color: 0x1a1a1a,
        }
    }

    /// Apply an embedding application's semantic palette.
    pub fn set_colors(&mut self, background: Hsla, border: Hsla, selection: Hsla, text: Hsla) {
        self.bg_color = u32::from(background.to_rgb()) >> 8;
        self.grid_color = u32::from(border.to_rgb()) >> 8;
        self.node_bg_color = self.bg_color;
        self.node_border_color = self.grid_color;
        self.selection_color = u32::from(selection.to_rgb()) >> 8;
        self.text_color = u32::from(text.to_rgb()) >> 8;
    }

    /// Register a renderer for a specific node type.
    pub fn node_renderer(
        mut self,
        node_type: impl Into<SharedString>,
        renderer: impl Fn(&FlowNode, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.node_renderers
            .insert(node_type.into(), Box::new(renderer));
        self
    }

    /// Set the default renderer for nodes without a specific type renderer.
    pub fn default_renderer(
        mut self,
        renderer: impl Fn(&FlowNode, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.default_renderer = Some(Box::new(renderer));
        self
    }

    /// Set the on_connect callback.
    pub fn on_connect(mut self, callback: impl Fn(&Connection, &mut FlowState) + 'static) -> Self {
        self.on_connect = Some(Box::new(callback));
        self
    }

    /// Supply a live tooltip view for a wire. Hover follows the painted path and label bounds.
    pub fn edge_tooltip(
        mut self,
        renderer: impl Fn(&EdgeId, &mut Window, &mut App) -> AnyView + 'static,
    ) -> Self {
        self.edge_tooltip = Some(Box::new(renderer));
        self
    }

    /// Dismiss a visible wire tooltip without moving focus. Returns whether a card was open.
    pub fn dismiss_edge_tooltip(&mut self, cx: &mut Context<Self>) -> bool {
        let visible = self.edge_tooltip_target().is_some();
        self.hover_edge(None, cx);
        visible
    }

    /// The route whose tooltip is currently open, excluding a pending hover delay.
    pub fn edge_tooltip_target(&self) -> Option<&EdgeId> {
        self.tooltip_view.as_ref().and(self.hovered_edge.as_ref())
    }

    fn hover_edge(&mut self, id: Option<EdgeId>, cx: &mut Context<Self>) {
        if self.hovered_edge == id {
            return;
        }
        self.hovered_edge = id;
        self.hover_task = None;
        self.hover_ready = false;
        self.tooltip_view = None;
        if self.hovered_edge.is_some() {
            self.hover_task = Some(cx.spawn(async |this, cx| {
                // Match GPUI's control tooltip delay; the canvas itself has no rectangular target.
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.hover_ready = true;
                    cx.notify();
                });
            }));
        }
        cx.notify();
    }

    /// Hide the default node wrapper chrome (bg, border, shadow, padding).
    pub fn no_node_chrome(mut self) -> Self {
        self.show_node_chrome = false;
        self
    }

    /// Set the canvas background color.
    pub fn bg_color(mut self, color: u32) -> Self {
        self.bg_color = color;
        self
    }

    /// Set the dot grid color.
    pub fn grid_color(mut self, color: u32) -> Self {
        self.grid_color = color;
        self
    }

    /// Set the node wrapper background color.
    pub fn node_bg_color(mut self, color: u32) -> Self {
        self.node_bg_color = color;
        self
    }

    /// Set the node wrapper border color.
    pub fn node_border_color(mut self, color: u32) -> Self {
        self.node_border_color = color;
        self
    }

    /// Set the background pattern (Dots, Lines, Cross).
    pub fn bg_pattern(mut self, pattern: BackgroundPattern) -> Self {
        self.bg_pattern = pattern;
        self
    }

    /// Set custom connection validation.
    pub fn validate_connection(
        mut self,
        validator: impl Fn(&Connection, &FlowState) -> bool + 'static,
    ) -> Self {
        self.is_valid_connection = Some(Box::new(validator));
        self
    }

    /// Render a single node using the appropriate renderer.
    fn render_node(
        &self,
        node: &FlowNode,
        viewport: &Viewport,
        is_connecting: bool,
        snap_node_id: Option<&NodeId>,
        _entity_id: EntityId,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        if node.hidden {
            return div().into_any_element();
        }

        let (screen_x, screen_y) = viewport.flow_to_screen(node.position);

        // Choose renderer
        let content = if let Some(node_type) = &node.node_type {
            if let Some(renderer) = self.node_renderers.get(node_type) {
                renderer(node, window, cx)
            } else {
                self.render_default_node(node, window, cx)
            }
        } else {
            self.render_default_node(node, window, cx)
        };

        let node_id = node.id.clone();
        let state = self.state.clone();
        let selected = node.selected;
        let dragging = node.dragging;
        let show_chrome = self.show_node_chrome;
        let node_bg = self.node_bg_color;
        let node_border = self.node_border_color;
        let selection_color = self.selection_color;
        let element_id: ElementId = ElementId::Name(node.id.clone());

        // Build handle dot elements (skip if not connecting to reduce overhead)
        let handle_elements = if !node.handles.is_empty() {
            let can_receive = {
                let flow = state.read(cx);
                flow.connecting.as_ref().is_some_and(|draft| {
                    let pair = if draft.from_type == HandleType::Source {
                        (draft.from_node.clone(), node.id.clone())
                    } else {
                        (node.id.clone(), draft.from_node.clone())
                    };
                    flow.allowed_connections
                        .as_ref()
                        .is_none_or(|allowed| allowed.contains(&pair))
                })
            };
            Self::render_handles(
                &node.handles,
                &node.id,
                &state,
                is_connecting && can_receive,
                snap_node_id,
                node_bg,
                node_border,
                self.selection_color,
                viewport.zoom,
            )
        } else {
            Vec::new()
        };

        // Per-node measurement canvas: captures actual wrapper size.
        // Prepaint resolves endpoints before the edge layer paints. Notify only on a size change
        // so labels and culling get the exact new bounds after zooming or adding a send row.
        let measure_state = self.state.clone();
        let measure_node_id = node.id.clone();
        let prev_w = node.measured_width;
        let prev_h = node.measured_height;
        let measure_canvas = canvas(
            move |bounds, _window, cx| {
                let w = bounds.size.width;
                let h = bounds.size.height;
                if prev_w != Some(w) || prev_h != Some(h) {
                    measure_state.update(cx, |state, _cx| {
                        if let Some(node) = state.get_node_mut(&measure_node_id) {
                            node.measured_width = Some(w);
                            node.measured_height = Some(h);
                        }
                    });
                    cx.notify(_entity_id);
                }
            },
            |_, (), _, _| {},
        )
        .size_full()
        .absolute();

        let element = div()
            .id(element_id)
            .debug_selector({
                let id = node.id.clone();
                move || id.to_string()
            })
            .absolute()
            .left(px(screen_x))
            .top(px(screen_y))
            // Node box styling on the wrapper so handles align to visual edges
            .when(show_chrome, |el: Stateful<Div>| {
                el.bg(gpui::rgb(node_bg))
                    .border_1()
                    .border_color(gpui::rgb(node_border))
                    .rounded_lg()
                    .shadow_sm()
                    .px_4()
                    .py_2()
            })
            .cursor(if dragging {
                CursorStyle::ClosedHand
            } else {
                CursorStyle::OpenHand
            })
            .when(selected && show_chrome, |el: Stateful<Div>| {
                el.border_1().border_color(gpui::rgb(selection_color))
            })
            .on_mouse_down(MouseButton::Left, {
                let node_id = node_id.clone();
                let state = state.clone();
                move |event, _window, cx| {
                    let multi = event.modifiers.secondary();
                    let mouse_pos = event.position;

                    state.update(cx, |state, _cx| {
                        // Don't start node drag if we're connecting
                        if state.connecting.is_some() {
                            return;
                        }

                        // Preserve a selected group when dragging any of its nodes.
                        let selected = state.get_node(&node_id).is_some_and(|n| n.selected);
                        if !multi && !selected {
                            for n in &mut state.nodes {
                                n.selected = false;
                            }
                        }
                        // Z-index elevation
                        let max_z = state.nodes.iter().map(|n| n.z_index).max().unwrap_or(0);
                        if let Some(n) = state.get_node_mut(&node_id) {
                            n.selected = !n.selected || !multi;
                            if n.selected {
                                n.z_index = max_z + 1;
                            }
                        }

                        // Start drag — collect all selected nodes
                        let mut node_origins = Vec::new();
                        if let Some(n) = state.get_node(&node_id) {
                            if n.selected && n.draggable {
                                for n in &state.nodes {
                                    if n.selected && n.draggable {
                                        node_origins.push((n.id.clone(), n.position));
                                    }
                                }
                            }
                        }

                        if !node_origins.is_empty() {
                            state.push_undo();
                            state.drag_state = Some(DragState {
                                origin_mouse: (mouse_pos.x.as_f32(), mouse_pos.y.as_f32()),
                                node_origins,
                            });
                            for n in &mut state.nodes {
                                if n.selected && n.draggable {
                                    n.dragging = true;
                                }
                            }
                            // Hosts group the following position updates into one document edit.
                            _cx.notify();
                        }
                    });
                }
            })
            .child(content)
            .child(measure_canvas)
            .children(handle_elements)
            .into_any_element();
        RemScope {
            child: element,
            rem_size: window.rem_size() * viewport.zoom,
        }
        .into_any_element()
    }

    /// Render handle dots for a node.
    fn render_handles(
        handles: &[HandleDef],
        node_id: &NodeId,
        state: &Entity<FlowState>,
        is_connecting: bool,
        snap_node_id: Option<&NodeId>,
        default_bg: u32,
        default_border: u32,
        selection_color: u32,
        zoom: f32,
    ) -> Vec<AnyElement> {
        let handle_size = 10.0 * zoom;
        let half = handle_size / 2.0;
        let is_snapped_node = snap_node_id == Some(node_id);

        handles
            .iter()
            .enumerate()
            .map(|(i, handle)| {
                let node_id = node_id.clone();
                let handle_id = handle.id.clone();
                let handle_type = handle.handle_type;
                let handle_position = handle.position;
                let default_bg = handle.color.unwrap_or(default_bg);
                let state = state.clone();

                // Highlight: strongly if this is the snapped target, mildly if potential target
                let is_snap_target = is_snapped_node && handle_type == HandleType::Target;
                let is_potential_target = is_connecting && handle_type == HandleType::Target;

                let (bg_color, border_color, size_mult) = if is_snap_target {
                    (gpui::rgb(selection_color), gpui::rgb(selection_color), 1.4)
                } else if is_potential_target {
                    (gpui::rgb(default_bg), gpui::rgb(selection_color), 1.0)
                } else {
                    (gpui::rgb(default_bg), gpui::rgb(default_border), 1.0)
                };
                let dot_size = handle_size * size_mult;

                let dot = div()
                    .w(px(dot_size))
                    .h(px(dot_size))
                    .rounded_full()
                    .bg(bg_color)
                    .border_1()
                    .border_color(border_color)
                    .cursor(CursorStyle::Crosshair)
                    .flex_shrink_0();

                let container = div()
                    .id(ElementId::Integer(i as u64))
                    .debug_selector({
                        let id = node_id.clone();
                        move || format!("flow-handle-{id}-{i}")
                    })
                    .absolute()
                    .flex()
                    .items_center()
                    .justify_center()
                    // Handle mouse down → start connection
                    .on_mouse_down(MouseButton::Left, {
                        let state = state.clone();
                        let node_id = node_id.clone();
                        let handle_id = handle_id.clone();
                        move |event, _window, cx| {
                            let mouse_pos = event.position;
                            state.update(cx, |state, _cx| {
                                // Find handle center for the from_point
                                let from_point = state
                                    .find_handle_center(&node_id, &handle_id, handle_position)
                                    .unwrap_or((mouse_pos.x.as_f32(), mouse_pos.y.as_f32()));

                                state.connecting = Some(ConnectionDraft {
                                    from_node: node_id.clone(),
                                    from_handle: handle_id.clone(),
                                    from_type: handle_type,
                                    from_position: handle_position,
                                    from_point,
                                    to_point: (mouse_pos.x.as_f32(), mouse_pos.y.as_f32()),
                                    snap_target: None,
                                });
                                // Prevent node drag
                                state.drag_state = None;
                            });
                            cx.stop_propagation();
                        }
                    })
                    // Handle mouse up → complete connection if valid
                    .on_mouse_up(MouseButton::Left, {
                        let state = state.clone();
                        let node_id = node_id.clone();
                        let handle_id = handle_id.clone();
                        move |_event, _window, cx| {
                            state.update(cx, |state, _cx| {
                                if state
                                    .connecting
                                    .as_ref()
                                    .is_some_and(|draft| draft.from_type == handle_type)
                                {
                                    state.connecting = None;
                                    return;
                                }
                                if let Some(draft) = state.connecting.take() {
                                    // Build the connection
                                    let (source, target, source_handle, target_handle) =
                                        if draft.from_type == HandleType::Source {
                                            (
                                                draft.from_node.clone(),
                                                node_id.clone(),
                                                draft.from_handle.clone(),
                                                handle_id.clone(),
                                            )
                                        } else {
                                            (
                                                node_id.clone(),
                                                draft.from_node.clone(),
                                                handle_id.clone(),
                                                draft.from_handle.clone(),
                                            )
                                        };

                                    let connection = Connection {
                                        source,
                                        target,
                                        source_handle,
                                        target_handle,
                                    };

                                    state.complete_connection(connection);
                                    _cx.notify();
                                }
                            });
                            cx.stop_propagation();
                        }
                    });

                let container = match handle.position {
                    HandlePosition::Left => container
                        .left(px(-half))
                        .top_0()
                        .bottom_0()
                        .w(px(handle_size)),
                    HandlePosition::Right => container
                        .right(px(-half))
                        .top_0()
                        .bottom_0()
                        .w(px(handle_size)),
                    HandlePosition::Top => container
                        .top(px(-half))
                        .left_0()
                        .right_0()
                        .h(px(handle_size)),
                    HandlePosition::Bottom => container
                        .bottom(px(-half))
                        .left_0()
                        .right_0()
                        .h(px(handle_size)),
                };

                container.child(dot).into_any_element()
            })
            .collect()
    }

    /// Default node rendering — just the label content (styling is on the wrapper).
    fn render_default_node(
        &self,
        node: &FlowNode,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        if let Some(renderer) = &self.default_renderer {
            return renderer(node, window, cx);
        }
        div()
            .min_w(px(80.0))
            .text_sm()
            .text_color(gpui::rgb(self.text_color))
            .child(if node.label.is_empty() {
                node.id.to_string()
            } else {
                node.label.to_string()
            })
            .into_any_element()
    }

    /// Paint the background dot grid.
    fn paint_grid(
        bounds: &Bounds<Pixels>,
        viewport: &Viewport,
        grid_color: u32,
        pattern: BackgroundPattern,
        window: &mut Window,
    ) {
        let color = gpui::rgb(grid_color);
        let spacing = 20.0 * viewport.zoom;

        if spacing < 5.0 {
            return;
        }

        let start_x = viewport.x % spacing;
        let start_y = viewport.y % spacing;
        let bw = bounds.size.width.as_f32();
        let bh = bounds.size.height.as_f32();
        let ox = bounds.origin.x;
        let oy = bounds.origin.y;

        match pattern {
            BackgroundPattern::Dots => {
                let dot_size = px(1.5 * viewport.zoom.min(1.0));
                let mut x = start_x;
                while x < bw {
                    let mut y = start_y;
                    while y < bh {
                        let dot_bounds = Bounds::new(
                            Point::new(ox + px(x) - dot_size / 2.0, oy + px(y) - dot_size / 2.0),
                            Size {
                                width: dot_size,
                                height: dot_size,
                            },
                        );
                        window.paint_quad(gpui::fill(dot_bounds, color));
                        y += spacing;
                    }
                    x += spacing;
                }
            }
            BackgroundPattern::Lines => {
                let line_w = px(0.5);
                let mut x = start_x;
                while x < bw {
                    let line = Bounds::new(
                        Point::new(ox + px(x), oy),
                        Size {
                            width: line_w,
                            height: bounds.size.height,
                        },
                    );
                    window.paint_quad(gpui::fill(line, color));
                    x += spacing;
                }
            }
            BackgroundPattern::Cross => {
                let line_w = px(0.5);
                // Vertical lines
                let mut x = start_x;
                while x < bw {
                    let line = Bounds::new(
                        Point::new(ox + px(x), oy),
                        Size {
                            width: line_w,
                            height: bounds.size.height,
                        },
                    );
                    window.paint_quad(gpui::fill(line, color));
                    x += spacing;
                }
                // Horizontal lines
                let mut y = start_y;
                while y < bh {
                    let line = Bounds::new(
                        Point::new(ox, oy + px(y)),
                        Size {
                            width: bounds.size.width,
                            height: line_w,
                        },
                    );
                    window.paint_quad(gpui::fill(line, color));
                    y += spacing;
                }
            }
        }
    }

    /// Paint a selection box rectangle.
    fn paint_selection_box(sel: &SelectionBox, window: &mut Window) {
        let x = sel.start.0.min(sel.current.0);
        let y = sel.start.1.min(sel.current.1);
        let w = (sel.start.0 - sel.current.0).abs();
        let h = (sel.start.1 - sel.current.1).abs();

        if w < 1.0 || h < 1.0 {
            return;
        }

        let bounds = Bounds::new(
            Point::new(px(x), px(y)),
            Size {
                width: px(w),
                height: px(h),
            },
        );

        // Semi-transparent blue fill
        window.paint_quad(fill(bounds, gpui::rgba(0x3b82f618)));

        // Blue border
        let border_color: Background = gpui::rgb(0x3b82f6).into();
        // Top
        let top = Bounds::new(
            Point::new(px(x), px(y)),
            Size {
                width: px(w),
                height: px(1.0),
            },
        );
        window.paint_quad(fill(top, border_color.clone()));
        // Bottom
        let bottom = Bounds::new(
            Point::new(px(x), px(y + h - 1.0)),
            Size {
                width: px(w),
                height: px(1.0),
            },
        );
        window.paint_quad(fill(bottom, border_color.clone()));
        // Left
        let left = Bounds::new(
            Point::new(px(x), px(y)),
            Size {
                width: px(1.0),
                height: px(h),
            },
        );
        window.paint_quad(fill(left, border_color.clone()));
        // Right
        let right = Bounds::new(
            Point::new(px(x + w - 1.0), px(y)),
            Size {
                width: px(1.0),
                height: px(h),
            },
        );
        window.paint_quad(fill(right, border_color));
    }

    /// Paint a draft connection line from handle to mouse cursor.
    fn paint_connection_draft(draft: &ConnectionDraft, window: &mut Window) {
        let color: Background = gpui::rgba(0x3b82f680).into();
        let (sx, sy) = draft.from_point;
        let (tx, ty) = draft.to_point;

        let mut builder = PathBuilder::stroke(px(2.0));
        builder.move_to(Point::new(px(sx), px(sy)));

        // Simple bezier toward cursor
        let dx = (tx - sx).abs() * 0.5;
        let (cx1, cy1, cx2, cy2) = match draft.from_position {
            HandlePosition::Right => (sx + dx, sy, tx - dx, ty),
            HandlePosition::Left => (sx - dx, sy, tx + dx, ty),
            HandlePosition::Bottom => (sx, sy + dx, tx, ty - dx),
            HandlePosition::Top => (sx, sy - dx, tx, ty + dx),
        };

        builder.cubic_bezier_to(
            Point::new(px(tx), px(ty)),
            Point::new(px(cx1), px(cy1)),
            Point::new(px(cx2), px(cy2)),
        );

        if let Ok(path) = builder.build() {
            window.paint_path(path, color);
        }
    }
}

impl Render for FlowGraph {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.state.update(cx, |state, _| {
            state.edge_lane_scale = (window.rem_size().as_f32() / 16.0).max(1.0);
        });
        // On second render, edges will have correct node measurements
        // from the first render's measurement canvases.
        if !self.measured {
            self.measured = true;
            cx.notify(); // triggers one immediate re-render
        }

        let entity_id = cx.entity_id();
        let window_size = window.viewport_size();
        let win_w = window_size.width.as_f32();
        let win_h = window_size.height.as_f32();
        let cull_margin = 200.0;

        // Read state, extract what we need, then release the borrow
        let (
            viewport,
            is_panning,
            is_connecting,
            snap_node_id,
            connecting_draft,
            selection_box,
            visible_nodes,
            edge_label_elements,
            edge_label_bounds,
        ) = {
            let state = self.state.read(cx);
            let viewport = state.viewport;
            let is_panning = state.pan_drag.is_some();
            let is_connecting = state.connecting.is_some();
            let snap_node_id = state
                .connecting
                .as_ref()
                .and_then(|d| d.snap_target.as_ref())
                .map(|t| t.node_id.clone());
            let connecting_draft = state.connecting.clone();
            let selection_box = state.selection_box;

            // Build visible node indices
            let mut visible_indices: Vec<usize> = Vec::new();
            for (i, node) in state.nodes.iter().enumerate() {
                if node.hidden {
                    continue;
                }
                let (sx, sy) = viewport.flow_to_screen(node.position);
                let nw = node.measured_width.map(|p| p.as_f32()).unwrap_or(150.0);
                let nh = node.measured_height.map(|p| p.as_f32()).unwrap_or(50.0);
                if sx + nw < -cull_margin
                    || sx > win_w + cull_margin
                    || sy + nh < -cull_margin
                    || sy > win_h + cull_margin
                {
                    continue;
                }
                visible_indices.push(i);
            }
            visible_indices.sort_by_key(|&i| state.nodes[i].z_index);

            let visible_nodes: Vec<FlowNode> = visible_indices
                .iter()
                .map(|&i| state.nodes[i].clone())
                .collect();

            // Edge labels
            let bg = self.bg_color;
            let mut edge_label_elements: Vec<AnyElement> = Vec::new();
            let mut edge_label_bounds = Vec::new();
            for edge in &state.edges {
                if edge.hidden {
                    continue;
                }
                if let Some(ref label) = edge.label {
                    if let Some((lx, ly)) = edges::compute_edge_label_position(state, edge) {
                        let label_color = edge.color.unwrap_or(0xb1b1b7);
                        let mut label_border: Hsla = gpui::rgb(label_color).into();
                        label_border.a = 0.35;
                        let rem_size = window.rem_size() * viewport.zoom;
                        let font_size = rem_size * 0.75;
                        let line_height = rem_size;
                        let padding_x = rem_size * 0.5;
                        let padding_y = rem_size * 0.125;
                        let run = window.text_style().to_run(label.len());
                        let text_width = window
                            .text_system()
                            .shape_line(label.clone(), font_size, &[run], None)
                            .width;
                        edge_label_bounds.push((
                            edge.id.clone(),
                            Bounds::new(
                                point(
                                    px(lx) - text_width / 2.0 - padding_x - px(1.0),
                                    px(ly) - line_height / 2.0 - padding_y - px(1.0),
                                ),
                                size(
                                    text_width + padding_x * 2.0 + px(2.0),
                                    line_height + padding_y * 2.0 + px(2.0),
                                ),
                            ),
                        ));
                        let label_id = edge.id.clone();
                        let select_id = edge.id.clone();
                        let menu_id = edge.id.clone();
                        let select_state = self.state.clone();
                        let menu_state = self.state.clone();
                        let select_focus = self.focus_handle.clone();
                        let menu_focus = self.focus_handle.clone();
                        let element = div()
                            .id(SharedString::from(format!("flow-edge-label-{}", edge.id)))
                            .debug_selector(move || format!("flow-edge-label-{label_id}"))
                            .absolute()
                            .left(px(lx - state.origin.0) - text_width / 2.0 - padding_x - px(1.0))
                            .top(px(ly - state.origin.1) - line_height / 2.0 - padding_y - px(1.0))
                            .px_2()
                            .py_0p5()
                            .bg(gpui::rgb(bg))
                            .rounded_sm()
                            .text_size(font_size)
                            .line_height(line_height)
                            .border_1()
                            .border_color(if edge.selected {
                                Hsla::from(gpui::rgb(self.selection_color))
                            } else {
                                label_border
                            })
                            .text_color(gpui::rgb(label_color))
                            .when(edge.selectable, |element| {
                                element
                                    .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                                        window.focus(&select_focus);
                                        select_state.update(cx, |state, _| {
                                            state.select_edge(
                                                &select_id,
                                                event.modifiers.secondary(),
                                            )
                                        });
                                        cx.notify(entity_id);
                                        cx.stop_propagation();
                                    })
                                    .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                                        window.focus(&menu_focus);
                                        menu_state.update(cx, |state, cx| {
                                            if !state
                                                .edges
                                                .iter()
                                                .any(|edge| edge.id == menu_id && edge.selected)
                                            {
                                                state.select_edge(&menu_id, false);
                                            }
                                            state.pending_edge_menu =
                                                Some((menu_id.clone(), event.position));
                                            cx.notify();
                                        });
                                        cx.notify(entity_id);
                                        cx.stop_propagation();
                                    })
                            })
                            .child(label.to_string())
                            .into_any_element();
                        edge_label_elements.push(
                            RemScope {
                                child: element,
                                rem_size,
                            }
                            .into_any_element(),
                        );
                    }
                }
            }

            (
                viewport,
                is_panning,
                is_connecting,
                snap_node_id,
                connecting_draft,
                selection_box,
                visible_nodes,
                edge_label_elements,
                edge_label_bounds,
            )
        }; // state borrow ends here

        self.state
            .update(cx, |state, _| state.edge_label_bounds = edge_label_bounds);
        let mouse = window.mouse_position();
        let hovered = edges::hit_test_hover_edges(
            self.state.read(cx),
            mouse.x.as_f32(),
            mouse.y.as_f32(),
            6.0,
        );
        if self.hovered_edge.is_some() && self.hovered_edge != hovered {
            self.hover_edge(None, cx);
        }
        if self.hover_ready && self.tooltip_view.is_none() {
            if let (Some(id), Some(builder)) = (&self.hovered_edge, &self.edge_tooltip) {
                self.tooltip_view = Some(builder(id, window, cx));
            }
        }
        let tooltip = self.hovered_edge.clone().zip(self.tooltip_view.clone());
        let tooltip_graph = cx.entity().downgrade();
        let hover_graph = tooltip_graph.clone();

        // Render only visible nodes
        let mut node_elements: Vec<AnyElement> = Vec::with_capacity(visible_nodes.len());
        for node in &visible_nodes {
            node_elements.push(self.render_node(
                node,
                &viewport,
                is_connecting,
                snap_node_id.as_ref(),
                entity_id,
                window,
                cx,
            ));
        }

        let state_for_canvas = self.state.clone();
        let viewport_for_canvas = viewport;
        let bg_color = self.bg_color;
        let grid_color = self.grid_color;
        let bg_pattern = self.bg_pattern;
        let state_for_scroll = self.state.clone();
        let state_for_mouse_down = self.state.clone();
        let state_for_mouse_move = self.state.clone();
        let state_for_mouse_up = self.state.clone();
        let state_for_key = self.state.clone();
        let state_for_cancel = self.state.clone();
        let state_for_context = self.state.clone();
        let canvas_focus = self.focus_handle.clone();
        let context_focus = self.focus_handle.clone();

        div()
            .id("flow-graph")
            .debug_selector(|| "flow-graph".to_string())
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_hidden()
            .relative()
            .bg(gpui::rgb(bg_color))
            .cursor(if is_panning {
                CursorStyle::ClosedHand
            } else if is_connecting {
                CursorStyle::Crosshair
            } else {
                CursorStyle::Arrow
            })
            // Background grid + edge painting layer
            .child(
                canvas(
                    {
                        let state = self.state.clone();
                        move |bounds, window, cx| {
                            state.update(cx, |state, _| {
                                state.origin = (bounds.origin.x.as_f32(), bounds.origin.y.as_f32());
                                state.canvas_bounds = Some(bounds);
                            });
                            if let Some((id, view)) = tooltip {
                                window.set_tooltip(AnyTooltip {
                                    view,
                                    mouse_position: window.mouse_position(),
                                    check_visible_and_update: Rc::new(move |_, window, cx| {
                                        tooltip_graph.upgrade().is_some_and(|graph| {
                                            let graph = graph.read(cx);
                                            let mouse = window.mouse_position();
                                            graph.hovered_edge.as_ref() == Some(&id)
                                                && graph.hover_ready
                                                && edges::hit_test_hover_edges(
                                                    graph.state.read(cx),
                                                    mouse.x.as_f32(),
                                                    mouse.y.as_f32(),
                                                    6.0,
                                                )
                                                .as_ref()
                                                    == Some(&id)
                                        })
                                    }),
                                });
                            }
                            window.insert_hitbox(bounds, HitboxBehavior::Normal)
                        }
                    },
                    move |bounds, hitbox, window, cx| {
                        // Observe the whole window so leaving the panel also cancels the delay.
                        window.on_mouse_event({
                            let graph = hover_graph.clone();
                            move |event: &MouseMoveEvent, phase, window, cx| {
                                if phase == DispatchPhase::Capture {
                                    let _ = graph.update(cx, |graph, cx| {
                                        if graph.edge_tooltip.is_some() {
                                            let id = event
                                                .pressed_button
                                                .is_none()
                                                .then_some(())
                                                .filter(|_| hitbox.is_hovered(window))
                                                .and_then(|_| {
                                                    edges::hit_test_hover_edges(
                                                        graph.state.read(cx),
                                                        event.position.x.as_f32(),
                                                        event.position.y.as_f32(),
                                                        6.0,
                                                    )
                                                });
                                            graph.hover_edge(id, cx);
                                        }
                                    });
                                }
                            }
                        });
                        window.on_mouse_event({
                            let graph = hover_graph.clone();
                            move |_: &MouseDownEvent, phase, _, cx| {
                                if phase == DispatchPhase::Capture {
                                    let _ =
                                        graph.update(cx, |graph, cx| graph.hover_edge(None, cx));
                                }
                            }
                        });
                        window.on_mouse_event({
                            let graph = hover_graph.clone();
                            move |_: &ScrollWheelEvent, phase, _, cx| {
                                if phase == DispatchPhase::Capture {
                                    let _ =
                                        graph.update(cx, |graph, cx| graph.hover_edge(None, cx));
                                }
                            }
                        });
                        Self::paint_grid(
                            &bounds,
                            &viewport_for_canvas,
                            grid_color,
                            bg_pattern,
                            window,
                        );
                        let state = state_for_canvas.read(cx);
                        edges::paint_edges(state, window);

                        // Paint draft connection line
                        if let Some(ref draft) = connecting_draft {
                            Self::paint_connection_draft(draft, window);
                        }

                        // Paint selection box
                        if let Some(ref sel) = selection_box {
                            Self::paint_selection_box(sel, window);
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
            // Node layer
            .children(node_elements)
            // Edge labels
            .children(edge_label_elements)
            // Mouse down on empty space → start panning, deselect, or edge selection
            .on_mouse_down(MouseButton::Left, {
                let entity_id = entity_id;
                move |event, window, cx| {
                    window.focus(&canvas_focus);
                    let mouse_pos = event.position;
                    state_for_mouse_down.update(cx, |state, _cx| {
                        // If a node drag or connection was already started, skip
                        if state.drag_state.is_some() || state.connecting.is_some() {
                            return;
                        }

                        // Try edge hit testing
                        let mx = mouse_pos.x.as_f32();
                        let my = mouse_pos.y.as_f32();
                        if let Some(edge_id) = edges::hit_test_edges(state, mx, my, 5.0) {
                            if !event.modifiers.secondary() {
                                for n in &mut state.nodes {
                                    n.selected = false;
                                }
                                for e in &mut state.edges {
                                    e.selected = false;
                                }
                            }
                            if let Some(edge) = state.edges.iter_mut().find(|e| e.id == edge_id) {
                                edge.selected = true;
                            }
                            return;
                        }

                        // Shift+drag → start box selection
                        if event.modifiers.shift {
                            if !event.modifiers.secondary() {
                                for n in &mut state.nodes {
                                    n.selected = false;
                                }
                                for e in &mut state.edges {
                                    e.selected = false;
                                }
                            }
                            state.selection_box = Some(SelectionBox {
                                start: (mx, my),
                                current: (mx, my),
                            });
                            return;
                        }

                        // Deselect all nodes and edges
                        if !event.modifiers.secondary() {
                            for n in &mut state.nodes {
                                n.selected = false;
                            }
                            for e in &mut state.edges {
                                e.selected = false;
                            }
                        }

                        // Start pan drag
                        state.pan_drag = Some(PanDragState {
                            start_mouse: (mx, my),
                            start_viewport: (state.viewport.x, state.viewport.y),
                        });
                    });
                    cx.notify(entity_id);
                }
            })
            .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                let edge = edges::hit_test_edges(
                    state_for_context.read(cx),
                    event.position.x.as_f32(),
                    event.position.y.as_f32(),
                    6.0,
                );
                if let Some(id) = edge {
                    window.focus(&context_focus);
                    state_for_context.update(cx, |state, cx| {
                        if !state
                            .edges
                            .iter()
                            .any(|edge| edge.id == id && edge.selected)
                        {
                            for node in &mut state.nodes {
                                node.selected = false;
                            }
                            for edge in &mut state.edges {
                                edge.selected = edge.id == id;
                            }
                        }
                        state.pending_edge_menu = Some((id, event.position));
                        cx.notify();
                    });
                    cx.stop_propagation();
                    cx.notify(entity_id);
                }
            })
            // Global mouse move → handle dragging, panning, or connecting
            .on_mouse_move({
                let entity_id = entity_id;
                move |event, _window, cx| {
                    let mouse_pos = event.position;
                    let mx = mouse_pos.x.as_f32();
                    let my = mouse_pos.y.as_f32();

                    let mut changed = false;

                    state_for_mouse_move.update(cx, |state, _cx| {
                        // Box selection
                        if let Some(ref mut sel) = state.selection_box {
                            sel.current = (mx, my);
                            // Select nodes whose screen bounds intersect the box
                            let (sx, sy, ex, ey) = (
                                sel.start.0.min(sel.current.0),
                                sel.start.1.min(sel.current.1),
                                sel.start.0.max(sel.current.0),
                                sel.start.1.max(sel.current.1),
                            );
                            let mut viewport = state.viewport;
                            viewport.x += state.origin.0;
                            viewport.y += state.origin.1;
                            for node in &mut state.nodes {
                                if node.hidden {
                                    continue;
                                }
                                let (nx, ny) = viewport.flow_to_screen(node.position);
                                let nw = node.measured_width.map(|p| p.as_f32()).unwrap_or(150.0);
                                let nh = node.measured_height.map(|p| p.as_f32()).unwrap_or(40.0);
                                // AABB intersection
                                let intersects = nx < ex && nx + nw > sx && ny < ey && ny + nh > sy;
                                node.selected = intersects;
                            }
                            changed = true;
                        }
                        // Connection dragging with snap-to-handle
                        else if state.connecting.is_some() {
                            // Clone draft to avoid borrow conflict with find_snap_target
                            let mut draft = state.connecting.clone().unwrap();
                            let snap = state.find_snap_target(&draft, mx, my);
                            if let Some(ref target) = snap {
                                draft.to_point = target.point;
                            } else {
                                draft.to_point = (mx, my);
                            }
                            draft.snap_target = snap;
                            state.connecting = Some(draft);
                            changed = true;
                        }
                        // Node dragging
                        else if let Some(ref drag) = state.drag_state {
                            let dx = (mx - drag.origin_mouse.0) / state.viewport.zoom;
                            let dy = (my - drag.origin_mouse.1) / state.viewport.zoom;

                            let origins = drag.node_origins.clone();
                            let snap = state.snap_to_grid;
                            let snap_grid = state.snap_grid;
                            for (node_id, origin) in &origins {
                                let mut new_x = origin.x + dx;
                                let mut new_y = origin.y + dy;

                                if snap {
                                    let (gx, gy) = snap_grid;
                                    new_x = (new_x / gx).round() * gx;
                                    new_y = (new_y / gy).round() * gy;
                                }

                                if let Some(node) = state.get_node_mut(node_id) {
                                    node.position = FlowPoint::new(new_x, new_y);
                                }
                            }
                            changed = true;
                        }
                        // Canvas panning
                        else if let Some(pan) = state.pan_drag {
                            state.viewport.x = pan.start_viewport.0 + (mx - pan.start_mouse.0);
                            state.viewport.y = pan.start_viewport.1 + (my - pan.start_mouse.1);
                            changed = true;
                        }
                    });

                    if changed {
                        cx.notify(entity_id);
                    }
                }
            })
            // Global mouse up → end dragging, panning, or cancel connection
            .on_mouse_up(MouseButton::Left, {
                let entity_id = entity_id;
                move |_event, _window, cx| {
                    let mut changed = false;

                    state_for_mouse_up.update(cx, |state, _cx| {
                        if state.selection_box.is_some() {
                            state.selection_box = None;
                            changed = true;
                        }
                        if let Some(draft) = state.connecting.take() {
                            if let Some(snap) = draft.snap_target {
                                // Complete the connection
                                let (source, target, source_handle, target_handle) =
                                    if draft.from_type == HandleType::Source {
                                        (
                                            draft.from_node.clone(),
                                            snap.node_id.clone(),
                                            draft.from_handle.clone(),
                                            snap.handle_id.clone(),
                                        )
                                    } else {
                                        (
                                            snap.node_id.clone(),
                                            draft.from_node.clone(),
                                            snap.handle_id.clone(),
                                            draft.from_handle.clone(),
                                        )
                                    };

                                let connection = Connection {
                                    source,
                                    target,
                                    source_handle,
                                    target_handle,
                                };

                                state.complete_connection(connection);
                                _cx.notify();
                            }
                            // else: connection cancelled (no snap target)
                            changed = true;
                        }
                        if state.drag_state.is_some() {
                            state.drag_state = None;
                            for n in &mut state.nodes {
                                n.dragging = false;
                            }
                            changed = true;
                            _cx.notify();
                        }
                        if state.pan_drag.is_some() {
                            state.pan_drag = None;
                            changed = true;
                        }
                    });

                    if changed {
                        cx.notify(entity_id);
                    }
                }
            })
            .on_mouse_up_out(MouseButton::Left, move |_, _, cx| {
                state_for_cancel.update(cx, |state, cx| {
                    let was_dragging = state.drag_state.is_some();
                    state.connecting = None;
                    state.drag_state = None;
                    state.pan_drag = None;
                    state.selection_box = None;
                    for node in &mut state.nodes {
                        node.dragging = false;
                    }
                    if was_dragging {
                        cx.notify();
                    }
                });
                cx.notify(entity_id);
            })
            // Keyboard shortcuts (delete, undo/redo)
            .on_key_down({
                let entity_id = entity_id;
                let focus = self.focus_handle.clone();
                move |event: &KeyDownEvent, window, cx| {
                    let key: &str = event.keystroke.key.as_ref();

                    // Only handle destructive keys (delete/backspace) when the
                    // flow graph itself is focused — not when a child element
                    // (like an Input inside a node) has focus.
                    let graph_focused = focus.is_focused(window);

                    // Undo: Cmd+Z (always allowed)
                    if !state_for_key.read(cx).externally_managed
                        && key == "z"
                        && event.keystroke.modifiers.secondary()
                        && !event.keystroke.modifiers.shift
                    {
                        state_for_key.update(cx, |state, _cx| {
                            state.undo();
                        });
                        cx.notify(entity_id);
                        return;
                    }
                    // Redo: Cmd+Shift+Z
                    if !state_for_key.read(cx).externally_managed
                        && key == "z"
                        && event.keystroke.modifiers.secondary()
                        && event.keystroke.modifiers.shift
                    {
                        state_for_key.update(cx, |state, _cx| {
                            state.redo();
                        });
                        cx.notify(entity_id);
                        return;
                    }

                    // Select all: Cmd+A
                    if key == "a" && event.keystroke.modifiers.secondary() && graph_focused {
                        state_for_key.update(cx, |state, _cx| {
                            state.select_all();
                        });
                        cx.notify(entity_id);
                        return;
                    }

                    if !state_for_key.read(cx).externally_managed
                        && (key == "backspace" || key == "delete")
                        && graph_focused
                    {
                        state_for_key.update(cx, |state, _cx| {
                            state.push_undo();
                            // Collect IDs of selected deletable nodes
                            let node_ids_to_remove: Vec<NodeId> = state
                                .nodes
                                .iter()
                                .filter(|n| n.selected && n.deletable)
                                .map(|n| n.id.clone())
                                .collect();

                            // Remove edges connected to deleted nodes + selected edges
                            state.edges.retain(|e| {
                                let connected_to_removed = node_ids_to_remove.contains(&e.source)
                                    || node_ids_to_remove.contains(&e.target);
                                let selected_and_deletable = e.selected && e.deletable;
                                !connected_to_removed && !selected_and_deletable
                            });

                            // Remove selected nodes
                            state.nodes.retain(|n| !node_ids_to_remove.contains(&n.id));
                            if !node_ids_to_remove.is_empty() {
                                state.rebuild_lookup();
                            }
                        });
                        cx.notify(entity_id);
                    }
                }
            })
            // Scroll to zoom / pan
            .on_scroll_wheel({
                move |event, _window, cx| {
                    let delta = event.delta.pixel_delta(px(20.0));
                    let mouse_pos = event.position;

                    if event.modifiers.secondary() || event.modifiers.control {
                        // Zoom towards mouse
                        let factor = (delta.y.as_f32() * 0.01).clamp(-4.0, 4.0).exp();
                        state_for_scroll.update(cx, |state, cx| {
                            let mx = mouse_pos.x.as_f32() - state.origin.0;
                            let my = mouse_pos.y.as_f32() - state.origin.1;
                            state.zoom_at(state.viewport.zoom * factor, mx, my);
                            cx.notify();
                        });
                    } else {
                        // Pan
                        state_for_scroll.update(cx, |state, _cx| {
                            state.viewport.x += delta.x.as_f32();
                            state.viewport.y += delta.y.as_f32();
                        });
                    }

                    cx.notify(entity_id);
                    cx.stop_propagation();
                }
            })
    }
}
