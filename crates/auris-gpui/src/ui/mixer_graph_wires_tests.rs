use super::*;
use crate::{
    dock::{Dock, Panel},
    harness::{click, open, paint, right_press},
};
use gpui::{Modifiers, TestAppContext};
use gpui_flow::{
    EdgeType,
    edges::{
        EdgePath, compute_edge_label_position, compute_edge_path, flowing_edge_segments,
        hit_test_edges,
    },
};

#[test]
fn wire_hover_includes_insert_links_and_respects_labels_nodes_bounds_and_gestures() {
    use gpui_flow::edges::hit_test_hover_edges;
    let nodes = vec![
        measured_node("a", 0., 0., HandleDef::source(HandlePosition::Right)),
        measured_node("b", 400., 0., HandleDef::target(HandlePosition::Left)),
    ];
    let mut edge = FlowEdge::new("insert", "a", "b");
    edge.selectable = false;
    let mut state = FlowState::new(nodes, vec![edge]);
    state.origin = (30., 70.);
    let at = compute_edge_label_position(&state, &state.edges[0]).unwrap();
    assert_eq!(hit_test_edges(&state, at.0, at.1, 6.), None);
    assert_eq!(
        hit_test_hover_edges(&state, at.0, at.1, 6.),
        Some("insert".into())
    );
    // A label owns its whole rectangle, including portions outside the stroke tolerance.
    let bounds = gpui::Bounds::new(
        gpui::point(px(at.0), px(at.1)),
        gpui::size(px(90.), px(22.)),
    );
    state.edge_label_bounds.push(("insert".into(), bounds));
    assert_eq!(
        hit_test_hover_edges(&state, at.0 + 80., at.1 + 18., 6.),
        Some("insert".into())
    );
    assert_eq!(hit_test_hover_edges(&state, 60., 100., 6.), None);
    state.selection_box = Some(gpui_flow::SelectionBox {
        start: at,
        current: at,
    });
    assert_eq!(hit_test_hover_edges(&state, at.0, at.1, 6.), None);
    state.selection_box = None;
    state.edges[0].hidden = true;
    assert_eq!(hit_test_hover_edges(&state, at.0, at.1, 6.), None);
    state.edges[0].hidden = false;
    state.canvas_bounds = Some(gpui::Bounds::new(
        gpui::point(px(0.), px(0.)),
        gpui::size(px(20.), px(20.)),
    ));
    assert_eq!(hit_test_hover_edges(&state, at.0, at.1, 6.), None);
}

#[gpui::test]
fn wire_tooltip_tracks_parallel_routes_live_names_and_dismissal(cx: &mut TestAppContext) {
    use gpui_flow::edges::hit_test_hover_edges;
    let (app, cx) = open(cx);
    // GPUI 0.2.2 retains debug-selector entries after elements disappear. Read the graph's
    // actual tooltip target for dismissal, and use selectors only to verify newly painted text.
    let tooltip_target = |cx: &gpui::VisualTestContext| {
        app.read_with(cx, |this, cx| {
            this.mixer_canvas
                .as_ref()
                .unwrap()
                .view
                .read(cx)
                .edge_tooltip_target()
                .cloned()
        })
    };
    let (a, b, sends) = app.update(cx, |this, _| {
        this.panels.set_detached(Panel::Mixer, false);
        this.panels.move_to(Panel::Mixer, Dock::Bottom);
        this.panels.set_size(Dock::Bottom, px(520.));
        this.panels.show(Panel::Mixer);
        let a = this.session.add_bus_track("Source");
        let b = this.session.add_bus_track("Destination");
        let sends = [
            this.session.add_send(a, b).unwrap(),
            this.session.add_send(a, b).unwrap(),
        ];
        (a, b, sends)
    });
    paint(&app, cx);
    app.update(cx, |this, cx| {
        let canvas = this.mixer_canvas.as_ref().unwrap();
        canvas.state.update(cx, |state, _| {
            for node in &mut state.nodes {
                node.position = if node.id == node_id(MixerNode::Strip(Some(a))) {
                    FlowPoint::new(24., 24.)
                } else if node.id == node_id(MixerNode::Strip(Some(b))) {
                    FlowPoint::new(600., 24.)
                } else {
                    FlowPoint::new(3000., 3000.)
                };
            }
        });
        canvas.view.update(cx, |_, cx| cx.notify());
    });
    paint(&app, cx);
    for (index, send) in sends.into_iter().enumerate() {
        let at = app.read_with(cx, |this, cx| {
            let state = this.mixer_canvas.as_ref().unwrap().state.read(cx);
            let id: SharedString = format!("mix-send-{}", send.0).into();
            if index == 0 {
                // Hover the actual curve, away from its label.
                let edge = state.edges.iter().find(|edge| edge.id == id).unwrap();
                let path = compute_edge_path(state, edge).unwrap();
                let at = path
                    .points
                    .iter()
                    .copied()
                    .find(|at| {
                        hit_test_hover_edges(state, at.0, at.1, 6.).as_ref() == Some(&id)
                            && !state.edge_label_bounds.iter().any(|(_, bounds)| {
                                bounds.contains(&gpui::point(px(at.0), px(at.1)))
                            })
                    })
                    .unwrap();
                gpui::point(px(at.0), px(at.1))
            } else {
                // Hover the label's outer edge, where another curve might be closer.
                let bounds = state
                    .edge_label_bounds
                    .iter()
                    .find(|(label, _)| *label == id)
                    .unwrap()
                    .1;
                gpui::point(bounds.right() - px(2.), bounds.center().y)
            }
        });
        cx.simulate_mouse_move(at, None::<MouseButton>, Modifiers::none());
        cx.run_until_parked();
        assert!(tooltip_target(cx).is_none());
        cx.background_executor
            .advance_clock(Duration::from_millis(550));
        cx.run_until_parked();
        assert!(cx.debug_bounds("mixer-wire-tooltip").is_some());
        assert_eq!(
            tooltip_target(cx),
            Some(format!("mix-send-{}", send.0).into())
        );
        let send_label = app.read_with(cx, |this, _| {
            format!("mixer-wire-tooltip-kind:{} #{}", this.t(Key::Sends), send.0)
        });
        // GPUI's test selector API requires static strings, as do the harness's dynamic rows.
        assert!(
            cx.debug_bounds(Box::leak(send_label.into_boxed_str()))
                .is_some()
        );
        for selector in [
            format!("mixer-wire-tooltip-from:Source #{}", a.0),
            format!("mixer-wire-tooltip-to:Destination #{}", b.0),
            "mixer-wire-tooltip-level:−∞ dBFS".to_string(),
        ] {
            assert!(
                cx.debug_bounds(Box::leak(selector.clone().into_boxed_str()))
                    .is_some(),
                "{selector}"
            );
        }
        // The live view must keep reading the document without another mouse move.
        app.update(cx, |this, cx| {
            this.session.rename_track(b, "Renamed").unwrap();
            cx.notify();
        });
        paint(&app, cx);
        assert!(
            cx.debug_bounds(Box::leak(
                format!("mixer-wire-tooltip-to:Renamed #{}", b.0).into_boxed_str()
            ))
            .is_some()
        );
        cx.simulate_keystrokes("escape");
        paint(&app, cx);
        assert!(tooltip_target(cx).is_none());
        cx.background_executor
            .advance_clock(Duration::from_millis(600));
        cx.run_until_parked();
        assert!(tooltip_target(cx).is_none());
        cx.simulate_mouse_move(at, None::<MouseButton>, Modifiers::none());
        cx.run_until_parked();
        cx.background_executor
            .advance_clock(Duration::from_millis(550));
        cx.run_until_parked();
        assert!(cx.debug_bounds("mixer-wire-tooltip").is_some());
        // Clicking through a tooltip still selects and deletes this exact parallel wire.
        cx.simulate_click(at, Modifiers::none());
        paint(&app, cx);
        assert!(tooltip_target(cx).is_none());
        cx.simulate_keystrokes("delete");
        paint(&app, cx);
        assert!(!app.read_with(cx, |this, _| {
            this.session
                .mixer_graph()
                .edges
                .iter()
                .any(|edge| edge_id(edge).as_ref() == format!("mix-send-{}", send.0))
        }));
        cx.simulate_keystrokes("secondary-z");
        app.update(cx, |this, _| {
            this.session.rename_track(b, "Destination").unwrap();
        });
        paint(&app, cx);
    }
    let outside = gpui::point(px(1900.), px(80.));
    cx.simulate_mouse_move(outside, None::<MouseButton>, Modifiers::none());
    cx.background_executor
        .advance_clock(Duration::from_millis(600));
    cx.run_until_parked();
    assert!(tooltip_target(cx).is_none());
}

fn measured_node(id: &str, x: f32, y: f32, handle: HandleDef) -> FlowNode {
    let mut node = FlowNode::new(id.to_string(), x, y).handles(vec![handle]);
    node.measured_width = Some(px(100.0));
    node.measured_height = Some(px(80.0));
    node
}

#[test]
fn signal_glow_has_a_logarithmic_floor_and_never_accepts_invalid_peaks() {
    for peak in [0., -1., f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0001] {
        assert_eq!(wire_activity(peak), 0.);
    }
    assert!((wire_activity(0.001)).abs() < 1e-6);
    assert!((wire_activity(0.01) - 1. / 3.).abs() < 1e-6);
    assert!((wire_activity(0.1) - 2. / 3.).abs() < 1e-6);
    assert_eq!(wire_activity(1.), 1.);
    assert_eq!(wire_activity(4.), 1.);
}

#[test]
fn signal_highlights_follow_arc_length_through_bends_and_reversed_routes() {
    let path = EdgePath {
        points: vec![(0., 0.), (40., 0.), (40., 90.), (120., 90.)],
        label: (0., 0.),
    };
    let first = flowing_edge_segments(&path, 0.5, 120., 24.);
    let later = flowing_edge_segments(&path, 0.55, 120., 24.);
    assert_eq!(*first[2].0.points.last().unwrap(), (40., 20.));
    let next = later[2].0.points.last().unwrap();
    assert!((next.0 - 40.).abs() < 1e-5 && (next.1 - 26.).abs() < 1e-5);
    assert_eq!(first[0].0.points, vec![(36., 0.), (40., 0.), (40., 4.)]);
    assert!(first[0].1 < first[1].1 && first[1].1 < first[2].1);
    for (section, _) in &first {
        assert!(
            section
                .points
                .iter()
                .all(|point| path.distance(*point) < 1e-5)
        );
    }
    let reversed = EdgePath {
        points: vec![(100., 0.), (0., 0.)],
        label: (0., 0.),
    };
    let from = flowing_edge_segments(&reversed, 0.2, 200., 24.);
    let to = flowing_edge_segments(&reversed, 0.3, 200., 24.);
    assert!((from[2].0.points.last().unwrap().0 - 60.).abs() < 1e-5);
    assert!((to[2].0.points.last().unwrap().0 - 40.).abs() < 1e-5);
}

#[test]
fn signal_highlights_wrap_scale_and_bound_pathological_inputs() {
    let path = EdgePath {
        points: vec![(0., 0.), (0., 0.), (0., 100.), (300., 100.)],
        label: (0., 0.),
    };
    let reference = flowing_edge_segments(&path, 0.25, 120., 24.);
    for phase in [1.25, -0.75] {
        let wrapped = flowing_edge_segments(&path, phase, 120., 24.);
        assert_eq!(wrapped.len(), reference.len());
        for (a, b) in wrapped.iter().zip(&reference) {
            assert_eq!(a.0.points, b.0.points);
            assert_eq!(a.1, b.1);
        }
    }
    let scaled = EdgePath {
        points: path
            .points
            .iter()
            .map(|p| (p.0 * 2. + 37., p.1 * 2. - 80.))
            .collect(),
        label: (0., 0.),
    };
    let highlights = flowing_edge_segments(&scaled, 0.25, 240., 48.);
    for ((actual, _), (original, _)) in highlights.iter().zip(&reference) {
        for (a, b) in actual.points.iter().zip(&original.points) {
            assert!((a.0 - (b.0 * 2. + 37.)).abs() < 1e-4);
            assert!((a.1 - (b.1 * 2. - 80.)).abs() < 1e-4);
        }
    }
    for (phase, spacing, length) in [
        (f32::NAN, 120., 24.),
        (0., 0., 24.),
        (0., 120., f32::INFINITY),
    ] {
        assert!(flowing_edge_segments(&path, phase, spacing, length).is_empty());
    }
    let zero = EdgePath {
        points: vec![(0., 0.); 3],
        label: (0., 0.),
    };
    assert!(flowing_edge_segments(&zero, 0.5, 120., 24.).is_empty());
    let long = EdgePath {
        points: vec![(0., 0.), (1e10, 0.)],
        label: (0., 0.),
    };
    assert!(flowing_edge_segments(&long, 0.5, 1., 24.).len() <= 128 * 3);
}

#[test]
fn signal_animation_uses_elapsed_time_even_after_a_long_session() {
    assert!((signal_flow_phase(Duration::from_millis(100)) - 0.075).abs() < 1e-6);
    assert!((signal_flow_phase(Duration::from_millis(1000)) - 0.75).abs() < 1e-6);
    assert!((signal_flow_phase(Duration::from_millis(1500)) - 0.125).abs() < 1e-6);
    let year = Duration::from_secs(365 * 24 * 60 * 60);
    assert!((signal_flow_phase(year + Duration::from_millis(100)) - 0.075).abs() < 1e-6);
}

#[gpui::test]
fn live_wire_updates_preserve_individual_selection_and_pointer_geometry(cx: &mut TestAppContext) {
    let theme = Theme::from_scheme(&crate::theme::SCHEMES[0]);
    let nodes = vec![
        measured_node("a", 40., 100., HandleDef::source(HandlePosition::Right)).size(100., 80.),
        measured_node("b", 640., 100., HandleDef::target(HandlePosition::Left)).size(100., 80.),
    ];
    let roles = [
        ("out", MixerEdgeKind::Output),
        (
            "send",
            MixerEdgeKind::Send {
                track: TrackId(1),
                send: SendId(1),
                level_db: -6.,
                pre_fader: true,
            },
        ),
        ("key", MixerEdgeKind::Sidechain),
    ];
    let edges = roles
        .into_iter()
        .enumerate()
        .map(|(index, (id, kind))| {
            let (color, stroke) = wire_style(kind, &theme);
            let mut edge = FlowEdge::new(id, "a", "b")
                .label(id)
                .color(u32::from(color.to_rgb()) >> 8);
            edge.stroke = stroke;
            edge.lane_offset = (index as f32 - 1.) * WIRE_LANE_SPACING;
            edge.activity = wire_activity(0.1 * (index + 1) as f32);
            edge.animated = true;
            edge
        })
        .collect();
    let state = cx.new(|_| {
        let mut state = FlowState::new(nodes, edges);
        state.edge_flow = Some(EdgeFlow {
            phase: 0.1,
            spacing: 120.,
            length: 24.,
            color: u32::from(theme.text.to_rgb()) >> 8,
        });
        state
    });
    let (view, cx) = cx.add_window_view(|_, cx| {
        FlowGraph::new(state.clone(), cx)
            .default_renderer(|_, _, _| div().w(px(100.)).h(px(80.)).into_any_element())
    });
    cx.run_until_parked();
    for index in 0..3 {
        let (points, at, id, stroke, color) = state.read_with(cx, |state, _| {
            let edge = &state.edges[index];
            let path = compute_edge_path(state, edge).unwrap();
            (
                path.points,
                path.label,
                edge.id.clone(),
                edge.stroke,
                edge.color,
            )
        });
        cx.simulate_click(gpui::point(px(at.0), px(at.1)), Modifiers::none());
        cx.run_until_parked();
        for activity in [1., 0.25, 0.] {
            state.update(cx, |state, _| {
                state.edges[index].activity = activity;
                state.edge_flow.as_mut().unwrap().phase = activity * 0.5;
            });
            view.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
            state.read_with(cx, |state, _| {
                let edge = &state.edges[index];
                assert!(edge.selected);
                assert_eq!(state.edges.iter().filter(|edge| edge.selected).count(), 1);
                assert_eq!(edge.stroke, stroke);
                assert_eq!(edge.color, color);
                assert_eq!(compute_edge_path(state, edge).unwrap().points, points);
                assert_eq!(hit_test_edges(state, at.0, at.1, 5.), Some(id.clone()));
            });
        }
        right_press(cx, gpui::point(px(at.0), px(at.1)));
        state.read_with(cx, |state, _| {
            assert_eq!(state.pending_edge_menu.as_ref().unwrap().0, id)
        });
        cx.simulate_mouse_up(
            gpui::point(px(at.0), px(at.1)),
            MouseButton::Right,
            Modifiers::none(),
        );
    }
}

#[gpui::test]
fn signal_motion_can_be_disabled_without_editing_the_mix_or_losing_selection(
    cx: &mut TestAppContext,
) {
    let _appearance = crate::harness::AppearanceTestGuard::new();
    let (app, cx) = open(cx);
    app.update(cx, |this, cx| {
        this.appearance.animate_mixer_signals = true;
        this.panels.move_to(Panel::Mixer, Dock::Bottom);
        this.panels.show(Panel::Mixer);
        this.session.add_bus_track("Signal");
        cx.notify();
    });
    paint(&app, cx);
    let (id, start, before) = app.update(cx, |this, cx| {
        let canvas = this.mixer_canvas.as_ref().unwrap();
        let id = canvas.state.read(cx).edges[0].id.clone();
        canvas
            .state
            .update(cx, |state, _| state.select_edge(&id, false));
        (
            id,
            canvas.animation_started,
            serde_json::to_value(this.project()).unwrap(),
        )
    });
    for animated in [false, true] {
        click("mixer-animate-signals", cx);
        paint(&app, cx);
        app.read_with(cx, |this, cx| {
            assert_eq!(this.appearance.animate_mixer_signals, animated);
            let canvas = this.mixer_canvas.as_ref().unwrap();
            assert_eq!(canvas.animation_started, start);
            let state = canvas.state.read(cx);
            assert_eq!(state.edge_flow.is_some(), animated);
            assert!(
                state
                    .edges
                    .iter()
                    .find(|edge| edge.id == id)
                    .unwrap()
                    .selected
            );
            assert!(state.edges.iter().all(|edge| edge.animated == animated));
            assert_eq!(serde_json::to_value(this.project()).unwrap(), before);
            let stored: crate::appearance::Appearance =
                serde_json::from_str(&serde_json::to_string(&this.appearance).unwrap()).unwrap();
            assert_eq!(stored.animate_mixer_signals, animated);
        });
    }
}

#[test]
fn curved_lanes_share_geometry_for_painting_labels_and_nearest_hit_testing() {
    let nodes = vec![
        measured_node("a", 0., 0., HandleDef::source(HandlePosition::Right)),
        measured_node("b", 10_000., 600., HandleDef::target(HandlePosition::Left)),
    ];
    let mut edges = Vec::new();
    for (index, offset) in [-60., 0., 60.].into_iter().enumerate() {
        let mut edge = FlowEdge::new(format!("wire-{index}"), "a", "b");
        edge.lane_offset = offset;
        edges.push(edge);
    }
    let mut state = FlowState::new(nodes, edges);
    state.origin = (37., 211.);
    state.viewport.x = -80.;
    state.viewport.y = 25.;
    state.viewport.zoom = 1.6;
    state.edge_lane_scale = 1.5;
    let mut endpoints = None;
    for edge in &state.edges {
        let path = compute_edge_path(&state, edge).unwrap();
        let source = *path.points.first().unwrap();
        let target = *path.points.last().unwrap();
        assert_eq!(*endpoints.get_or_insert((source, target)), (source, target));
        assert_eq!(
            compute_edge_label_position(&state, edge).unwrap(),
            path.label
        );
        let curve = gpui_flow::edges::bezier::get_bezier_path(
            source.0,
            source.1,
            HandlePosition::Right,
            target.0,
            target.1,
            HandlePosition::Left,
            0.25,
        )
        .with_lane_offset(edge.lane_offset * state.viewport.zoom * state.edge_lane_scale);
        for t in [0.005, 0.1371, 0.32711, 0.7317, 0.99] {
            let point = curve.point(t);
            assert!(path.distance(point) < 0.26);
            assert_eq!(
                hit_test_edges(&state, point.0, point.1, 5.).as_ref(),
                Some(&edge.id),
                "t={t}"
            );
        }
    }
}

#[test]
fn orthogonal_wires_are_hit_on_the_visible_bends_and_ignore_hidden_edges() {
    let nodes = vec![
        measured_node("a", 0., 0., HandleDef::source(HandlePosition::Right)),
        measured_node("b", 400., 240., HandleDef::target(HandlePosition::Top)),
    ];
    let mut edge = FlowEdge::new("key", "a", "b").edge_type(EdgeType::SmoothStep {
        border_radius: 8.,
        offset: 30.,
    });
    edge.stroke = EdgeStroke::Dotted;
    let mut state = FlowState::new(nodes, vec![edge]);
    let path = compute_edge_path(&state, &state.edges[0]).unwrap();
    for pair in path.points.windows(2) {
        let at = ((pair[0].0 + pair[1].0) / 2., (pair[0].1 + pair[1].1) / 2.);
        assert_eq!(hit_test_edges(&state, at.0, at.1, 0.1), Some("key".into()));
    }
    let label = path.label;
    assert_eq!(
        hit_test_edges(&state, label.0, label.1, 0.1),
        Some("key".into())
    );
    state.edges[0].hidden = true;
    assert_eq!(hit_test_edges(&state, label.0, label.1, 5.), None);
}

#[test]
fn lane_order_and_wire_vocabulary_survive_parameter_changes_and_all_schemes() {
    let mut session =
        auris_session::Session::new(auris_session::SessionOptions::headless()).unwrap();
    let a = session.add_bus_track("A");
    let b = session.add_bus_track("B");
    session.add_track_output(a, Output::Bus(b)).unwrap();
    let one = session.add_send(a, b).unwrap();
    session.add_send(a, b).unwrap();
    let graph = session.mixer_graph();
    let lanes = wire_lanes(&graph);
    session.set_send_level(a, one, -12.).unwrap();
    session.set_send_pre_fader(a, one, true).unwrap();
    assert_eq!(wire_lanes(&session.mixer_graph()), lanes);
    let mut reordered = graph.clone();
    reordered.edges.reverse();
    assert_eq!(wire_lanes(&reordered), lanes);
    for scheme in crate::theme::SCHEMES {
        let theme = Theme::from_scheme(scheme);
        let kinds = [
            MixerEdgeKind::Output,
            MixerEdgeKind::Send {
                track: a,
                send: one,
                level_db: 0.,
                pre_fader: false,
            },
            MixerEdgeKind::Sidechain,
        ];
        let styles: Vec<_> = kinds
            .into_iter()
            .map(|kind| wire_style(kind, &theme))
            .collect();
        assert_eq!(
            styles.iter().map(|(_, stroke)| *stroke).collect::<Vec<_>>(),
            vec![EdgeStroke::Solid, EdgeStroke::Dashed, EdgeStroke::Dotted]
        );
        for (color, _) in styles {
            assert!(
                crate::theme::contrast_ratio(color, theme.surface_sunken) >= 4.5,
                "{}",
                scheme.id
            );
        }
        let mut custom = theme.clone();
        custom.track_palette = [theme.surface_sunken; 8];
        assert_eq!(
            wire_palette(&custom),
            wire_palette(&theme),
            "document colors must not change interface port types"
        );
        assert_eq!(custom.mixer_output_color(), custom.mixer_send_color());
        assert_eq!(custom.mixer_output_color(), custom.mixer_sidechain_color());
        for color in [
            custom.mixer_output_color(),
            custom.mixer_send_color(),
            custom.mixer_sidechain_color(),
        ] {
            assert!(crate::theme::contrast_ratio(color, custom.surface_sunken) >= 4.5);
            assert!(crate::theme::contrast_ratio(color, custom.background) >= 4.5);
        }
    }
}

#[gpui::test]
fn overlapping_output_sends_and_sidechain_are_individually_selected_deleted_and_undone(
    cx: &mut TestAppContext,
) {
    let (app, cx) = open(cx);
    let (a, b, slot, sends) = app.update(cx, |this, _| {
        this.panels.set_detached(Panel::Mixer, false);
        this.panels.move_to(Panel::Mixer, Dock::Bottom);
        this.panels.set_size(Dock::Bottom, px(520.));
        this.panels.show(Panel::Mixer);
        let a = this.session.add_bus_track("A");
        let b = this.session.add_bus_track("B");
        this.session.add_track_output(a, Output::Bus(b)).unwrap();
        let one = this.session.add_send(a, b).unwrap();
        let two = this.session.add_send(a, b).unwrap();
        this.session.set_send_level(a, one, -6.).unwrap();
        this.session.set_send_level(a, two, -12.).unwrap();
        let slot = this
            .session
            .add_effect(Some(b), "auris.fx.compressor")
            .unwrap();
        this.session
            .set_effect_sidechain(Some(b), slot, Some(a))
            .unwrap();
        (a, b, slot, [one, two])
    });
    paint(&app, cx);
    app.update(cx, |this, cx| {
        let canvas = this.mixer_canvas.as_ref().unwrap();
        canvas.state.update(cx, |state, _| {
            state.viewport.x = 20.;
            state.viewport.y = 10.;
            for node in &mut state.nodes {
                node.position = if node.id == node_id(MixerNode::Strip(Some(a))) {
                    FlowPoint::new(24., 24.)
                } else if node.id == node_id(MixerNode::Strip(Some(b))) {
                    FlowPoint::new(380., 24.)
                } else if node.id == node_id(MixerNode::Effect(Some(b), slot)) {
                    FlowPoint::new(680., 24.)
                } else {
                    FlowPoint::new(3000., 3000.)
                };
            }
        });
        canvas.view.update(cx, |_, cx| cx.notify());
    });
    paint(&app, cx);
    app.read_with(cx, |this, cx| {
        let state = this.mixer_canvas.as_ref().unwrap().state.read(cx);
        for send in sends {
            let edge = state
                .edges
                .iter()
                .find(|edge| edge.id.as_ref() == format!("mix-send-{}", send.0))
                .unwrap();
            assert_eq!(edge.stroke, EdgeStroke::Dashed);
            assert!(
                edge.label
                    .as_ref()
                    .unwrap()
                    .contains(&format!("#{}", send.0))
            );
        }
        let sidechain = state
            .edges
            .iter()
            .find(|edge| edge.target_handle == Some("sidechain".into()))
            .unwrap();
        assert_eq!(sidechain.stroke, EdgeStroke::Dotted);
        let output = state
            .edges
            .iter()
            .find(|edge| {
                edge.target == node_id(MixerNode::Strip(Some(b)))
                    && edge.id.starts_with("mix-output-")
            })
            .unwrap();
        assert_eq!(output.stroke, EdgeStroke::Solid);
        // Both carry audio; routing roles remain distinct through stroke and label.
        assert_eq!(output.color, sidechain.color);
        assert_ne!(output.stroke, sidechain.stroke);
        assert!(
            state
                .nodes
                .iter()
                .flat_map(|node| &node.handles)
                .all(|handle| handle.color == output.color),
            "audio ports and their wires share the same data-type color"
        );
    });
    let mut original = app.read_with(cx, |this, _| serde_json::to_value(this.project()).unwrap());
    original.as_object_mut().unwrap().remove("mixer_graph_view");
    let wires: Vec<_> = app.read_with(cx, |this, _| {
        this.session
            .mixer_graph()
            .edges
            .into_iter()
            .filter(|edge| {
                matches!(
                    edge.kind,
                    MixerEdgeKind::Send { .. } | MixerEdgeKind::Sidechain
                ) || (edge.kind == MixerEdgeKind::Output
                    && edge.target == MixerNode::Strip(Some(b)))
            })
            .map(|edge| edge_id(&edge))
            .collect()
    });
    assert_eq!(wires.len(), 4);
    for (index, id) in wires.iter().enumerate() {
        app.update(cx, |this, cx| {
            let canvas = this.mixer_canvas.as_ref().unwrap();
            canvas.state.update(cx, |state, _| {
                state.zoom_at([0.75, 1.25, 1.5, 0.5][index], 0., 0.)
            });
            canvas.view.update(cx, |_, cx| cx.notify());
        });
        paint(&app, cx);
        let at = app.read_with(cx, |this, cx| {
            let state = this.mixer_canvas.as_ref().unwrap().state.read(cx);
            let edge = state.edges.iter().find(|edge| edge.id == *id).unwrap();
            let at = compute_edge_label_position(state, edge).unwrap();
            gpui::point(px(at.0), px(at.1))
        });
        assert!(cx.debug_bounds("flow-graph").unwrap().contains(&at));
        cx.simulate_click(at, Modifiers::none());
        paint(&app, cx);
        app.read_with(cx, |this, cx| {
            let state = this.mixer_canvas.as_ref().unwrap().state.read(cx);
            assert_eq!(
                state
                    .edges
                    .iter()
                    .filter(|edge| edge.selected)
                    .map(|edge| &edge.id)
                    .collect::<Vec<_>>(),
                vec![id]
            );
        });
        // A theme change must retain selection, identities and lane ownership.
        app.update(cx, |this, cx| {
            this.theme =
                Theme::from_scheme(&crate::theme::SCHEMES[index % crate::theme::SCHEMES.len()]);
            cx.notify();
        });
        paint(&app, cx);
        if index % 2 == 0 {
            cx.simulate_keystrokes("delete");
        } else {
            right_press(cx, at);
            paint(&app, cx);
            click("menu-item-0", cx);
        }
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            let remaining: Vec<_> = this
                .session
                .mixer_graph()
                .edges
                .iter()
                .map(edge_id)
                .collect();
            assert!(!remaining.contains(id));
            for other in wires.iter().filter(|other| *other != id) {
                assert!(remaining.contains(other));
            }
        });
        cx.dispatch_action(crate::actions::Undo);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            let mut restored = serde_json::to_value(this.project()).unwrap();
            restored.as_object_mut().unwrap().remove("mixer_graph_view");
            assert_eq!(restored, original);
            // Musical Undo keeps the zoom selected for this iteration.
            assert_eq!(
                this.project().mixer_graph_view.zoom,
                [0.75, 1.25, 1.5, 0.5][index]
            );
        });
    }
    app.read_with(cx, |this, _| {
        assert_eq!(
            this.project()
                .track(a)
                .unwrap()
                .sends
                .iter()
                .map(|send| send.id)
                .collect::<Vec<_>>(),
            sends
        )
    });
}
