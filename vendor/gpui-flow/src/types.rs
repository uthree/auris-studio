use gpui::{Pixels, SharedString};

/// Unique identifier for a node.
pub type NodeId = SharedString;

/// Unique identifier for an edge.
pub type EdgeId = SharedString;

/// A 2D point in flow (graph) coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FlowPoint {
    pub x: f32,
    pub y: f32,
}

impl FlowPoint {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// The type of a handle — determines connection direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HandleType {
    Source,
    Target,
}

/// Where on the node a handle is positioned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HandlePosition {
    Top,
    Right,
    Bottom,
    Left,
}

/// Definition of a handle on a node.
#[derive(Debug, Clone)]
pub struct HandleDef {
    pub id: Option<SharedString>,
    pub handle_type: HandleType,
    pub position: HandlePosition,
    /// Whether this handle can accept/start connections. Default: true.
    pub is_connectable: bool,
    /// Optional data-type color supplied by the embedding application.
    pub color: Option<u32>,
}

impl HandleDef {
    pub fn source(position: HandlePosition) -> Self {
        Self {
            id: None,
            handle_type: HandleType::Source,
            position,
            is_connectable: true,
            color: None,
        }
    }

    pub fn target(position: HandlePosition) -> Self {
        Self {
            id: None,
            handle_type: HandleType::Target,
            position,
            is_connectable: true,
            color: None,
        }
    }

    pub fn connectable(mut self, connectable: bool) -> Self {
        self.is_connectable = connectable;
        self
    }

    /// Colors this port with its data type's packed RGB value.
    pub fn color(mut self, color: u32) -> Self {
        self.color = Some(color);
        self
    }

    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
        self
    }
}

/// Background grid pattern style.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BackgroundPattern {
    Dots,
    Lines,
    Cross,
}

impl Default for BackgroundPattern {
    fn default() -> Self {
        BackgroundPattern::Dots
    }
}

/// The visual type and parameters of an edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EdgeType {
    Bezier { curvature: f32 },
    SmoothStep { border_radius: f32, offset: f32 },
    Straight,
}

impl Default for EdgeType {
    fn default() -> Self {
        EdgeType::Bezier { curvature: 0.25 }
    }
}

/// A node in the flow graph.
#[derive(Debug, Clone)]
pub struct FlowNode {
    pub id: NodeId,
    pub position: FlowPoint,
    pub node_type: Option<SharedString>,
    pub label: SharedString,
    pub handles: Vec<HandleDef>,
    pub selected: bool,
    pub dragging: bool,
    pub draggable: bool,
    pub selectable: bool,
    pub connectable: bool,
    pub deletable: bool,
    pub hidden: bool,
    pub z_index: i32,
    pub measured_width: Option<Pixels>,
    pub measured_height: Option<Pixels>,
}

impl FlowNode {
    pub fn new(id: impl Into<SharedString>, x: f32, y: f32) -> Self {
        Self {
            id: id.into(),
            position: FlowPoint::new(x, y),
            node_type: None,
            label: SharedString::default(),
            handles: Vec::new(),
            selected: false,
            dragging: false,
            draggable: true,
            selectable: true,
            connectable: true,
            deletable: true,
            hidden: false,
            z_index: 0,
            measured_width: None,
            measured_height: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }

    pub fn node_type(mut self, node_type: impl Into<SharedString>) -> Self {
        self.node_type = Some(node_type.into());
        self
    }

    pub fn handles(mut self, handles: Vec<HandleDef>) -> Self {
        self.handles = handles;
        self
    }

    /// Set initial estimated size (width, height in pixels).
    /// Used for edge positioning before the node is rendered and measured.
    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.measured_width = Some(gpui::px(width));
        self.measured_height = Some(gpui::px(height));
        self
    }

    pub fn z_index(mut self, z: i32) -> Self {
        self.z_index = z;
        self
    }
}

/// A wire's stroke pattern, independent of its color and selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EdgeStroke {
    /// A continuous signal path.
    #[default]
    Solid,
    /// Repeated long strokes separated by gaps.
    Dashed,
    /// Short marks separated by gaps.
    Dotted,
}

/// An edge connecting two nodes.
#[derive(Debug, Clone)]
pub struct FlowEdge {
    pub id: EdgeId,
    pub source: NodeId,
    pub target: NodeId,
    pub source_handle: Option<SharedString>,
    pub target_handle: Option<SharedString>,
    pub edge_type: EdgeType,
    pub selected: bool,
    pub selectable: bool,
    pub deletable: bool,
    pub hidden: bool,
    /// Draw directional highlights when activity is positive and the owner supplies an edge flow.
    pub animated: bool,
    /// Optional label displayed at the edge midpoint.
    pub label: Option<SharedString>,
    /// Custom edge color as 0xRRGGBB. If None, uses the default.
    pub color: Option<u32>,
    /// Custom stroke width. If None, uses default (2.0).
    pub stroke_width: Option<f32>,
    /// Stroke pattern, independent of selection and color.
    pub stroke: EdgeStroke,
    /// Perpendicular separation of a Bezier lane, in flow coordinates.
    /// Endpoints remain attached to their original handles.
    pub lane_offset: f32,
    /// Signal-driven glow strength in 0..1; zero leaves the normal wire unchanged.
    /// Presentation only: it never changes geometry, selection or hit testing.
    pub activity: f32,
}

/// Application-owned timing and style for directional signal highlights.
#[derive(Clone, Copy, Debug)]
pub struct EdgeFlow {
    /// Increasing cycle position in 0..1, moving from source to target.
    pub phase: f32,
    /// Distance between highlight heads, in flow coordinates.
    pub spacing: f32,
    /// Length of a fading trail behind each head, in flow coordinates.
    pub length: f32,
    /// Theme foreground used to highlight the wire's own role color, as 0xRRGGBB.
    pub color: u32,
}

impl FlowEdge {
    pub fn new(
        id: impl Into<SharedString>,
        source: impl Into<SharedString>,
        target: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            source: source.into(),
            target: target.into(),
            source_handle: None,
            target_handle: None,
            edge_type: EdgeType::default(),
            selected: false,
            selectable: true,
            deletable: true,
            hidden: false,
            animated: false,
            label: None,
            color: None,
            stroke_width: None,
            stroke: EdgeStroke::Solid,
            lane_offset: 0.0,
            activity: 0.0,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn color(mut self, color: u32) -> Self {
        self.color = Some(color);
        self
    }

    pub fn stroke_width(mut self, width: f32) -> Self {
        self.stroke_width = Some(width);
        self
    }

    pub fn source_handle(mut self, handle: impl Into<SharedString>) -> Self {
        self.source_handle = Some(handle.into());
        self
    }

    pub fn target_handle(mut self, handle: impl Into<SharedString>) -> Self {
        self.target_handle = Some(handle.into());
        self
    }

    pub fn edge_type(mut self, edge_type: EdgeType) -> Self {
        self.edge_type = edge_type;
        self
    }
}

/// Viewport state: pan offset and zoom level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            zoom: 1.0,
        }
    }
}

impl Viewport {
    /// Convert a point from flow coordinates to screen coordinates.
    pub fn flow_to_screen(&self, point: FlowPoint) -> (f32, f32) {
        (point.x * self.zoom + self.x, point.y * self.zoom + self.y)
    }

    /// Convert a point from screen coordinates to flow coordinates.
    pub fn screen_to_flow(&self, screen_x: f32, screen_y: f32) -> FlowPoint {
        FlowPoint::new(
            (screen_x - self.x) / self.zoom,
            (screen_y - self.y) / self.zoom,
        )
    }
}

/// State tracked during a node drag operation.
#[derive(Debug, Clone)]
pub struct DragState {
    /// Screen coordinates where the drag started.
    pub origin_mouse: (f32, f32),
    /// Original flow-space positions of all dragged nodes.
    pub node_origins: Vec<(NodeId, FlowPoint)>,
}

/// State tracked during a canvas pan drag.
#[derive(Debug, Clone, Copy)]
pub struct PanDragState {
    /// Screen coordinates where the drag started.
    pub start_mouse: (f32, f32),
    /// Viewport position at drag start.
    pub start_viewport: (f32, f32),
}

/// State tracked during a box/lasso selection.
#[derive(Debug, Clone, Copy)]
pub struct SelectionBox {
    /// Screen coordinates where the selection started.
    pub start: (f32, f32),
    /// Current screen coordinates of the selection end.
    pub current: (f32, f32),
}

/// State tracked while dragging a connection from a handle.
#[derive(Debug, Clone)]
pub struct ConnectionDraft {
    pub from_node: NodeId,
    pub from_handle: Option<SharedString>,
    pub from_type: HandleType,
    pub from_position: HandlePosition,
    /// Screen coordinates of the source handle center.
    pub from_point: (f32, f32),
    /// Current mouse screen coordinates (the dangling end).
    pub to_point: (f32, f32),
    /// If the cursor is near a valid target handle, snap to it.
    pub snap_target: Option<SnapTarget>,
}

/// A handle that the connection draft is snapping to.
#[derive(Debug, Clone)]
pub struct SnapTarget {
    pub node_id: NodeId,
    pub handle_id: Option<SharedString>,
    pub handle_type: HandleType,
    pub position: HandlePosition,
    /// Screen coordinates of the snapped handle center.
    pub point: (f32, f32),
}

/// A completed connection between two handles.
#[derive(Debug, Clone)]
pub struct Connection {
    pub source: NodeId,
    pub target: NodeId,
    pub source_handle: Option<SharedString>,
    pub target_handle: Option<SharedString>,
}

/// Change events for nodes.
#[derive(Debug, Clone)]
pub enum NodeChange {
    Position {
        id: NodeId,
        position: FlowPoint,
        dragging: bool,
    },
    Dimensions {
        id: NodeId,
        width: Pixels,
        height: Pixels,
    },
    Select {
        id: NodeId,
        selected: bool,
    },
    Remove {
        id: NodeId,
    },
}

/// Change events for edges.
#[derive(Debug, Clone)]
pub enum EdgeChange {
    Select { id: EdgeId, selected: bool },
    Remove { id: EdgeId },
}
