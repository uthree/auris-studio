use super::*;
use crate::dock::{Dock, Panel};
use crate::harness::{click, drag, open, paint, resize};
use crate::theme::Theme;
use gpui::{Modifiers, ScrollDelta, ScrollWheelEvent, TestAppContext, point};

#[test]
fn fit_frames_distant_nodes_with_pixel_padding_and_extends_the_zoom_range() {
    let mut a = FlowNode::new("a", -900., -400.);
    a.measured_width = Some(px(176.));
    a.measured_height = Some(px(240.));
    let mut b = FlowNode::new("b", 4200., 2100.);
    b.measured_width = Some(px(200.));
    b.measured_height = Some(px(180.));
    let mut hidden = FlowNode::new("hidden", 1e6, -1e6);
    hidden.hidden = true;
    let mut state = FlowState::new(vec![a, b, hidden], vec![]);
    state.min_zoom = MIN_ZOOM;
    state.max_zoom = MAX_ZOOM;
    state.zoom_at(1.6, 70., 30.);
    state.viewport.x = -500.;
    state.viewport.y = 250.;
    state.origin = (73., 91.);
    state.fit_view(24., 600., 400.);
    assert!(state.viewport.zoom < MIN_ZOOM);
    assert_eq!(state.min_zoom, state.viewport.zoom);
    for node in state.nodes.iter().filter(|node| !node.hidden) {
        let (x, y) = state.viewport.flow_to_screen(node.position);
        assert!(x >= 24. - 1e-3 && y >= 24. - 1e-3);
        assert!(x + f32::from(node.measured_width.unwrap()) <= 576. + 1e-3);
        assert!(y + f32::from(node.measured_height.unwrap()) <= 376. + 1e-3);
    }
    let fitted = state.viewport;
    state.fit_view(24., 600., 400.);
    assert!((state.viewport.zoom - fitted.zoom).abs() < 1e-6);
    assert!((state.viewport.x - fitted.x).abs() < 1e-3);
    assert!((state.viewport.y - fitted.y).abs() < 1e-3);
    state.zoom_at(fitted.zoom * 1.2, 300., 200.);
    assert!(state.viewport.zoom > fitted.zoom && state.viewport.zoom < MIN_ZOOM);
}

#[test]
fn fit_leaves_an_empty_or_unmeasurable_canvas_unchanged() {
    let mut hidden = FlowNode::new("hidden", -1000., 1000.);
    hidden.hidden = true;
    for nodes in [vec![], vec![hidden]] {
        let mut state = FlowState::new(nodes, vec![]);
        state.viewport.x = 17.;
        state.viewport.y = -33.;
        let before = (
            state.viewport.x,
            state.viewport.y,
            state.viewport.zoom,
            state.min_zoom,
        );
        for (padding, width, height) in [
            (24., 600., 400.),
            (24., 0., 400.),
            (24., f32::NAN, 400.),
            (f32::NAN, 600., 400.),
        ] {
            state.fit_view(padding, width, height);
            assert_eq!(
                (
                    state.viewport.x,
                    state.viewport.y,
                    state.viewport.zoom,
                    state.min_zoom
                ),
                before
            );
        }
    }
}

#[gpui::test]
fn fit_button_frames_all_nodes_curves_and_labels_without_rearranging_or_editing_audio(
    cx: &mut TestAppContext,
) {
    let (app, cx) = open(cx);
    app.update(cx, |this, _| {
        this.panels.set_detached(Panel::Mixer, false);
        this.panels.move_to(Panel::Mixer, Dock::Bottom);
        this.panels.set_size(Dock::Bottom, px(620.));
        this.panels.show(Panel::Mixer);
        let a = this.session.add_bus_track("Source");
        let b = this.session.add_bus_track("Bus");
        this.session.add_track_output(a, Output::Bus(b)).unwrap();
        for _ in 0..20 {
            this.session.add_send(a, b).unwrap();
        }
        let slot = this
            .session
            .add_effect(Some(b), "auris.fx.compressor")
            .unwrap();
        this.session
            .set_effect_sidechain(Some(b), slot, Some(a))
            .unwrap();
    });
    paint(&app, cx);
    // Measure cards before moving the complete graph beyond the viewport.
    app.update(cx, |this, cx| {
        let canvas = this.mixer_canvas.as_ref().unwrap();
        canvas.state.update(cx, |state, _| {
            for (index, node) in state.nodes.iter_mut().enumerate() {
                node.position = FlowPoint::new(30. + index as f32 * 230., 24.);
            }
        });
        canvas.view.update(cx, |_, cx| cx.notify());
    });
    paint(&app, cx);
    app.update(cx, |this, cx| {
        let canvas = this.mixer_canvas.as_ref().unwrap();
        canvas.state.update(cx, |state, _| {
            for (index, node) in state.nodes.iter_mut().enumerate() {
                node.position = FlowPoint::new(
                    -2200. + index as f32 * 2400.,
                    if index == 2 { -800. } else { 1800. },
                );
            }
            // A tall offscreen strip may never have been measured before Fit graph.
            state.nodes[0].measured_width = None;
            state.nodes[0].measured_height = None;
            state.edges[0].selected = true;
            state.zoom_at(1.7, 100., 200.);
            state.viewport.x = -8000.;
            state.viewport.y = 5000.;
        });
        canvas.view.update(cx, |_, cx| cx.notify());
    });
    paint(&app, cx);
    let before = app.read_with(cx, |this, cx| {
        let state = this.mixer_canvas.as_ref().unwrap().state.read(cx);
        (
            musical_document(this.project()),
            state
                .nodes
                .iter()
                .map(|node| (node.id.clone(), node.position))
                .collect::<Vec<_>>(),
            state
                .edges
                .iter()
                .filter(|edge| edge.selected)
                .map(|edge| edge.id.clone())
                .collect::<Vec<_>>(),
        )
    });
    click("mixer-fit-view", cx);
    paint(&app, cx);
    let fitted = app.read_with(cx, |this, cx| {
        let state = this.mixer_canvas.as_ref().unwrap().state.read(cx);
        let bounds = state.canvas_bounds.unwrap();
        assert!(state.viewport.zoom < MIN_ZOOM);
        for node in &state.nodes {
            let (x, y) = state.viewport.flow_to_screen(node.position);
            let left = x + state.origin.0;
            let top = y + state.origin.1;
            assert!(
                left >= f32::from(bounds.left()) + 4. && top >= f32::from(bounds.top()) + 4.,
                "{}: ({left}, {top})",
                node.id
            );
            assert!(
                left + f32::from(node.measured_width.unwrap()) <= f32::from(bounds.right()) - 4.,
                "{}",
                node.id
            );
            assert!(
                top + f32::from(node.measured_height.unwrap()) <= f32::from(bounds.bottom()) - 4.,
                "{}",
                node.id
            );
        }
        for edge in &state.edges {
            let path = gpui_flow::edges::compute_edge_path(state, edge).unwrap();
            assert!(
                path.points
                    .iter()
                    .all(|&(x, y)| bounds.contains(&point(px(x), px(y)))),
                "{}",
                edge.id
            );
        }
        for (id, label) in &state.edge_label_bounds {
            assert!(
                bounds.contains(&label.origin) && bounds.contains(&label.bottom_right()),
                "{id}"
            );
        }
        assert_eq!(musical_document(this.project()), before.0);
        assert_eq!(
            state
                .nodes
                .iter()
                .map(|node| (node.id.clone(), node.position))
                .collect::<Vec<_>>(),
            before.1
        );
        assert_eq!(
            state
                .edges
                .iter()
                .filter(|edge| edge.selected)
                .map(|edge| edge.id.clone())
                .collect::<Vec<_>>(),
            before.2
        );
        state.viewport.zoom
    });
    click("mixer-zoom-in", cx);
    paint(&app, cx);
    assert!(
        app.read_with(cx, |this, cx| this
            .mixer_canvas
            .as_ref()
            .unwrap()
            .state
            .read(cx)
            .viewport
            .zoom)
            > fitted
    );
    click("mixer-zoom-out", cx);
    paint(&app, cx);
    assert!(
        (app.read_with(cx, |this, cx| this
            .mixer_canvas
            .as_ref()
            .unwrap()
            .state
            .read(cx)
            .viewport
            .zoom)
            - fitted)
            .abs()
            < 1e-6
    );
    let saved = app.update(cx, |this, cx| {
        let saved = this.mixer_canvas.as_ref().unwrap().presentation(cx);
        assert_eq!(this.project().mixer_graph_view, saved);
        this.session.forget_history();
        this.reset_view();
        saved
    });
    paint(&app, cx);
    app.read_with(cx, |this, cx| {
        assert_eq!(this.mixer_canvas.as_ref().unwrap().presentation(cx), saved);
        assert!(!this.session.is_dirty());
    });
}

#[test]
fn zoom_keeps_the_pointer_anchor_and_cached_node_bounds_consistent_at_both_limits() {
    let mut node = FlowNode::new("a", 24., 30.);
    node.measured_width = Some(px(176.));
    node.measured_height = Some(px(240.));
    let mut state = FlowState::new(vec![node], vec![]);
    state.min_zoom = MIN_ZOOM;
    state.max_zoom = MAX_ZOOM;
    state.viewport.x = -42.;
    state.viewport.y = 17.;
    let anchor = state.viewport.screen_to_flow(351., 197.);
    for requested in [1.4, 10., 0.01, 1.] {
        state.zoom_at(requested, 351., 197.);
        let actual = state.viewport.screen_to_flow(351., 197.);
        assert!((actual.x - anchor.x).abs() < 1e-4);
        assert!((actual.y - anchor.y).abs() < 1e-4);
        assert_eq!(state.viewport.zoom, requested.clamp(MIN_ZOOM, MAX_ZOOM));
        assert!(
            (f32::from(state.nodes[0].measured_width.unwrap()) - 176. * state.viewport.zoom).abs()
                < 1e-4
        );
    }
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        state.zoom_at(invalid, 351., 197.);
        assert_eq!(state.viewport.zoom, 1.);
    }
}

#[gpui::test]
fn control_wheel_scales_nodes_handles_and_controls_and_pans_without_editing_audio(
    cx: &mut TestAppContext,
) {
    let (app, cx) = open(cx);
    let track = app.update(cx, |this, _| {
        this.panels.set_detached(Panel::Mixer, false);
        this.panels.move_to(Panel::Mixer, Dock::Bottom);
        this.panels.set_size(Dock::Bottom, px(600.));
        this.panels.show(Panel::Mixer);
        this.session.add_bus_track("Zoom")
    });
    paint(&app, cx);
    let id = node_id(MixerNode::Strip(Some(track)));
    let node_selector = Box::leak(id.to_string().into_boxed_str());
    let gain_selector = Box::leak(format!("graph-gain-{id}").into_boxed_str());
    let title_selector = Box::leak(format!("graph-node-title-{id}").into_boxed_str());
    let handle_selector = Box::leak(format!("flow-handle-{id}-0").into_boxed_str());
    app.update(cx, |this, cx| {
        let canvas = this.mixer_canvas.as_ref().unwrap();
        canvas.state.update(cx, |state, _| {
            state.viewport.x = 0.;
            state.viewport.y = 0.;
            for node in &mut state.nodes {
                node.position = if node.id == id {
                    FlowPoint::new(180., 70.)
                } else {
                    FlowPoint::new(3000., 3000.)
                };
            }
        });
        canvas.view.update(cx, |_, cx| cx.notify());
    });
    paint(&app, cx);
    let node_before = cx.debug_bounds(node_selector).unwrap();
    let gain_before = cx.debug_bounds(gain_selector).unwrap();
    let title_before = cx.debug_bounds(title_selector).unwrap();
    let handle_before = cx.debug_bounds(handle_selector).unwrap();
    let document = app.read_with(cx, |this, _| musical_document(this.project()));
    let at = node_before.center();
    let before = app.read_with(cx, |this, cx| {
        let state = this.mixer_canvas.as_ref().unwrap().state.read(cx);
        state.viewport.screen_to_flow(
            f32::from(at.x) - state.origin.0,
            f32::from(at.y) - state.origin.1,
        )
    });
    cx.simulate_event(ScrollWheelEvent {
        position: at,
        delta: ScrollDelta::Pixels(point(px(0.), px(30.))),
        modifiers: Modifiers {
            control: true,
            ..Modifiers::none()
        },
        ..Default::default()
    });
    paint(&app, cx);
    let zoom = app.read_with(cx, |this, cx| {
        let state = this.mixer_canvas.as_ref().unwrap().state.read(cx);
        let after = state.viewport.screen_to_flow(
            f32::from(at.x) - state.origin.0,
            f32::from(at.y) - state.origin.1,
        );
        assert!((after.x - before.x).abs() < 1e-3 && (after.y - before.y).abs() < 1e-3);
        state.viewport.zoom
    });
    assert!(zoom > 1.);
    let node_after = cx.debug_bounds(node_selector).unwrap();
    let gain_after = cx.debug_bounds(gain_selector).unwrap();
    let title_after = cx.debug_bounds(title_selector).unwrap();
    let handle_after = cx.debug_bounds(handle_selector).unwrap();
    // These are control hit regions, not raster assertions: interactions must grow with the card.
    assert!((node_after.size.width / node_before.size.width - zoom).abs() < 0.02);
    assert!((gain_after.size.height / gain_before.size.height - zoom).abs() < 0.02);
    assert!((handle_after.size.width / handle_before.size.width - zoom).abs() < 0.02);
    // Token-sized text and insets must follow the same graph-space scale as the hit targets.
    assert!(
        (title_after.size.height / title_before.size.height - zoom).abs() < 0.02,
        "node title must scale with the canvas"
    );
    let inset_before = gain_before.left() - node_before.left() - px(1.0);
    let inset_after = gain_after.left() - node_after.left() - px(1.0);
    // Layout rounds the two frames to device pixels (8 * 1.3499 becomes 11). Compare the
    // distance with a one-pixel bound rather than magnifying that rounding into a ratio.
    assert!(
        (inset_after - inset_before * zoom).abs() <= px(1.0),
        "node content inset must scale with the canvas: before={inset_before:?}, after={inset_after:?}, zoom={zoom}"
    );
    assert_eq!(
        app.read_with(cx, |this, _| musical_document(this.project())),
        document
    );
    // Ordinary wheel input pans, including over a fader, without changing the gain or zoom.
    cx.simulate_event(ScrollWheelEvent {
        position: gain_after.center(),
        delta: ScrollDelta::Pixels(point(px(12.), px(-24.))),
        ..Default::default()
    });
    paint(&app, cx);
    assert_eq!(
        app.read_with(cx, |this, cx| this
            .mixer_canvas
            .as_ref()
            .unwrap()
            .state
            .read(cx)
            .viewport
            .zoom),
        zoom
    );
    assert_eq!(
        app.read_with(cx, |this, _| musical_document(this.project())),
        document
    );
    // A scaled fader still edits and resets through the real pointer handlers.
    let gain = cx.debug_bounds(gain_selector).unwrap();
    drag(cx, gain.center(), gain.center() + point(px(30.), px(0.)));
    paint(&app, cx);
    assert_ne!(
        app.read_with(cx, |this, _| this
            .project()
            .track(track)
            .unwrap()
            .mixer
            .gain_db),
        0.
    );
    click("mixer-zoom-reset", cx);
    paint(&app, cx);
    assert_eq!(
        app.read_with(cx, |this, cx| this
            .mixer_canvas
            .as_ref()
            .unwrap()
            .state
            .read(cx)
            .viewport
            .zoom),
        1.
    );
    click("mixer-zoom-out", cx);
    paint(&app, cx);
    assert!(
        app.read_with(cx, |this, cx| this
            .mixer_canvas
            .as_ref()
            .unwrap()
            .state
            .read(cx)
            .viewport
            .zoom)
            < 1.
    );
    click("mixer-zoom-in", cx);
    paint(&app, cx);
    assert!(
        (app.read_with(cx, |this, cx| this
            .mixer_canvas
            .as_ref()
            .unwrap()
            .state
            .read(cx)
            .viewport
            .zoom)
            - 1.)
            .abs()
            < 1e-6
    );
}

#[gpui::test]
fn toolbar_zoom_reaches_both_limits_in_small_localised_themed_windows(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    app.update(cx, |this, _| {
        this.panels.set_detached(Panel::Mixer, false);
        this.panels.move_to(Panel::Mixer, Dock::Bottom);
        this.panels.set_size(Dock::Bottom, px(180.));
        this.panels.show(Panel::Mixer);
        let source = this.session.add_bus_track("Zoom source");
        let destination = this.session.add_bus_track("Zoom destination");
        this.session.add_send(source, destination).unwrap();
        this.session.forget_history();
    });
    paint(&app, cx);
    let before = app.read_with(cx, |this, _| {
        (musical_document(this.project()), this.session.mixer_graph())
    });

    // This is a reachability check for the real toolbar controls, rather than a direct state
    // mutation. Japanese labels and the larger rem size are the tightest toolbar case.
    for (scheme, language, rem_size) in [
        ("daylight", auris_i18n::Language::English, 13.),
        ("midnight", auris_i18n::Language::English, 16.),
        ("daylight", auris_i18n::Language::Japanese, 16.),
        ("midnight", auris_i18n::Language::Japanese, 16.),
    ] {
        app.update(cx, |this, _| {
            this.theme = Theme::named(scheme);
            this.language = language;
        });
        cx.update(|window, _| window.set_rem_size(px(rem_size)));
        resize(&app, cx, gpui::size(px(640.), px(480.)));
        for selector in ["mixer-zoom-out", "mixer-zoom-reset", "mixer-zoom-in"] {
            assert!(
                cx.debug_bounds(selector).is_some(),
                "{selector} is unreachable"
            );
        }

        for _ in 0..32 {
            click("mixer-zoom-out", cx);
            paint(&app, cx);
        }
        let min_zoom = app.read_with(cx, |this, cx| {
            this.mixer_canvas.as_ref().unwrap().state.read(cx).min_zoom
        });
        assert_eq!(
            app.read_with(cx, |this, cx| {
                this.mixer_canvas
                    .as_ref()
                    .unwrap()
                    .state
                    .read(cx)
                    .viewport
                    .zoom
            }),
            min_zoom
        );
        click("mixer-focus-connections", cx);
        paint(&app, cx);
        let first = app.read_with(cx, |this, cx| {
            this.selected_mixer_connection(cx)
                .expect("connection focus selects an edge at minimum zoom")
        });
        cx.simulate_keystrokes("right");
        paint(&app, cx);
        assert_ne!(
            app.read_with(cx, |this, cx| {
                this.selected_mixer_connection(cx)
                    .expect("arrow traversal selects an edge at minimum zoom")
            }),
            first
        );
        for _ in 0..64 {
            click("mixer-zoom-in", cx);
            paint(&app, cx);
        }
        let max_zoom = app.read_with(cx, |this, cx| {
            this.mixer_canvas.as_ref().unwrap().state.read(cx).max_zoom
        });
        assert_eq!(
            app.read_with(cx, |this, cx| {
                this.mixer_canvas
                    .as_ref()
                    .unwrap()
                    .state
                    .read(cx)
                    .viewport
                    .zoom
            }),
            max_zoom
        );
        click("mixer-focus-connections", cx);
        paint(&app, cx);
        let first = app.read_with(cx, |this, cx| {
            this.selected_mixer_connection(cx)
                .expect("connection focus selects an edge at maximum zoom")
        });
        cx.simulate_keystrokes("right");
        paint(&app, cx);
        assert_ne!(
            app.read_with(cx, |this, cx| {
                this.selected_mixer_connection(cx)
                    .expect("arrow traversal selects an edge at maximum zoom")
            }),
            first
        );
        app.read_with(cx, |this, _| {
            assert_eq!(musical_document(this.project()), before.0);
            assert_eq!(this.session.mixer_graph(), before.1);
            assert!(!this.session.can_undo());
        });
    }
}

// Camera and layout are saved metadata; these checks protect the audible document.
fn musical_document(project: &Project) -> serde_json::Value {
    let mut value = serde_json::to_value(project).unwrap();
    value.as_object_mut().unwrap().remove("mixer_graph_view");
    value
}
