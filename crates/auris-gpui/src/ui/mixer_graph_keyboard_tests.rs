use super::*;
use crate::{
    dock::{Dock, Panel},
    harness::{click, open, paint},
};
use gpui::{Modifiers, TestAppContext, px};
use std::collections::HashSet;

fn mixer_fixture(
    cx: &mut TestAppContext,
) -> (gpui::Entity<AurisApp>, &mut gpui::VisualTestContext) {
    let (app, cx) = open(cx);
    app.update(cx, |this, _| {
        this.panels.set_detached(Panel::Mixer, false);
        this.panels.move_to(Panel::Mixer, Dock::Bottom);
        this.panels.set_size(Dock::Bottom, px(600.));
        this.panels.show(Panel::Mixer);
        let source = this.session.add_bus_track("Keyboard source");
        let destination = this.session.add_bus_track("Keyboard destination");
        this.session.add_send(source, destination).unwrap();
        this.session.add_send(source, destination).unwrap();
        this.session.forget_history();
    });
    paint(&app, cx);
    (app, cx)
}

#[gpui::test]
fn keyboard_connections_select_parallel_routes_open_menu_escape_delete_and_undo(
    cx: &mut TestAppContext,
) {
    let (app, cx) = mixer_fixture(cx);

    click("mixer-focus-connections", cx);
    paint(&app, cx);
    let first = app.read_with(cx, |this, cx| {
        this.selected_mixer_connection(cx)
            .expect("toolbar selects a connection")
    });
    assert!(!matches!(first.kind, MixerEdgeKind::Chain));

    // Parallel sends have distinct stable IDs and are visited independently by the keyboard.
    cx.simulate_keystrokes("right");
    paint(&app, cx);
    let second = app.read_with(cx, |this, cx| {
        this.selected_mixer_connection(cx)
            .expect("arrow selects the next connection")
    });
    assert_ne!(first, second);
    assert!(!matches!(second.kind, MixerEdgeKind::Chain));

    let mut parallel_send_ids = HashSet::new();
    for _ in 0..8 {
        if let Some(edge) = app.read_with(cx, |this, cx| this.selected_mixer_connection(cx))
            && let MixerEdgeKind::Send { send, .. } = edge.kind
        {
            parallel_send_ids.insert(send);
        }
        cx.simulate_keystrokes("right");
        paint(&app, cx);
    }
    assert!(
        parallel_send_ids.len() >= 2,
        "parallel sends must be distinct keyboard targets"
    );

    cx.simulate_keystrokes("enter");
    paint(&app, cx);
    assert!(app.read_with(cx, |this, _| this.menu.is_some()));
    let menu_edge = app.read_with(cx, |this, cx| this.selected_mixer_connection(cx));
    cx.simulate_keystrokes("down");
    paint(&app, cx);
    app.read_with(cx, |this, cx| {
        assert!(this.menu.as_ref().unwrap().highlighted.is_some());
        assert_eq!(this.selected_mixer_connection(cx), menu_edge);
    });
    cx.simulate_keystrokes("escape");
    paint(&app, cx);
    assert!(app.read_with(cx, |this, _| this.menu.is_none()));

    // Escape restores graph focus; the next arrow must work without another toolbar click.
    cx.simulate_keystrokes("right");
    paint(&app, cx);
    let selected = app.read_with(cx, |this, cx| {
        this.selected_mixer_connection(cx)
            .expect("selected route before delete")
    });
    cx.simulate_keystrokes("delete");
    paint(&app, cx);
    assert!(!app.read_with(cx, |this, _| {
        this.session.mixer_graph().edges.contains(&selected)
    }));

    cx.simulate_keystrokes("secondary-z");
    paint(&app, cx);
    assert!(app.read_with(cx, |this, _| {
        this.session.mixer_graph().edges.contains(&selected)
    }));
}

#[gpui::test]
fn connection_selection_keeps_pointer_sync_and_does_not_intercept_modified_arrows(
    cx: &mut TestAppContext,
) {
    let (app, cx) = mixer_fixture(cx);
    click("mixer-focus-connections", cx);
    paint(&app, cx);
    let before = app.read_with(cx, |this, cx| {
        let canvas = this.mixer_canvas.as_ref().unwrap();
        (
            this.selected_mixer_connection(cx).unwrap(),
            this.project().mixer_graph_view.clone(),
            this.session.mixer_graph(),
            canvas.state.read(cx).viewport.zoom,
        )
    });

    // Ctrl/Command arrows are reserved by the flow canvas and must not advance selection.
    cx.simulate_keystrokes("secondary-right");
    paint(&app, cx);
    assert_eq!(
        app.read_with(cx, |this, cx| this.selected_mixer_connection(cx)),
        Some(before.0)
    );

    // A pointer selection must expose the same edge through the Inspector accessor.
    let label = app.read_with(cx, |this, cx| {
        let canvas = this.mixer_canvas.as_ref().unwrap();
        let id = canvas
            .state
            .read(cx)
            .edges
            .iter()
            .find(|edge| edge.selectable && !edge.hidden)
            .unwrap()
            .id
            .clone();
        canvas
            .state
            .read(cx)
            .edge_label_bounds
            .iter()
            .find(|(edge_id, _)| *edge_id == id)
            .map(|(_, bounds)| bounds.center())
            .unwrap()
    });
    cx.simulate_click(label, Modifiers::none());
    paint(&app, cx);
    assert!(app.read_with(cx, |this, cx| this.selected_mixer_connection(cx).is_some()));

    // Selection is view state only: it neither creates history nor changes layout/document data.
    app.read_with(cx, |this, cx| {
        assert_eq!(this.project().mixer_graph_view, before.1);
        assert_eq!(this.session.mixer_graph(), before.2);
        assert_eq!(
            this.mixer_canvas
                .as_ref()
                .unwrap()
                .state
                .read(cx)
                .viewport
                .zoom,
            before.3
        );
        assert!(!this.session.can_undo());
    });
}

#[gpui::test]
fn connection_inspector_reveals_a_track_without_editing_the_graph(cx: &mut TestAppContext) {
    let (app, cx) = mixer_fixture(cx);
    click("mixer-focus-connections", cx);
    paint(&app, cx);

    // The fixture has output and parallel-send edges. Select a send so its destination is a
    // real track (the master output deliberately has no revealable destination track).
    for _ in 0..8 {
        if app.read_with(cx, |this, cx| {
            matches!(
                this.selected_mixer_connection(cx).map(|edge| edge.kind),
                Some(MixerEdgeKind::Send { .. })
            )
        }) {
            break;
        }
        cx.simulate_keystrokes("right");
        paint(&app, cx);
    }
    let destination = app.read_with(cx, |this, cx| {
        let edge = this
            .selected_mixer_connection(cx)
            .expect("a connection remains selected");
        let MixerEdgeKind::Send { .. } = edge.kind else {
            panic!(
                "connection focus did not reach a send edge: {:?}",
                edge.kind
            );
        };
        let MixerNode::Strip(Some(track)) = edge.target else {
            panic!("send destination must be a track: {:?}", edge.target);
        };
        (
            track,
            this.project().mixer_graph_view.clone(),
            this.session.mixer_graph(),
        )
    });
    assert!(cx.debug_bounds("pipeline-reveal-target").is_some());

    click("pipeline-reveal-target", cx);
    paint(&app, cx);

    app.read_with(cx, |this, _| {
        assert_eq!(this.selected_track, Some(destination.0));
        assert_eq!(this.selected_clip, None);
        assert!(this.panels.is_open(Panel::Inspector));
        assert_eq!(this.project().mixer_graph_view, destination.1);
        assert_eq!(this.session.mixer_graph(), destination.2);
        assert!(!this.session.can_undo());
    });
}

#[gpui::test]
fn keyboard_selection_preserves_zoom_limits(cx: &mut TestAppContext) {
    let (app, cx) = mixer_fixture(cx);
    app.update(cx, |this, cx| {
        let state = this.mixer_canvas.as_ref().unwrap().state.clone();
        state.update(cx, |state, _| {
            state.zoom_at(0.0, 100., 100.);
            assert_eq!(state.viewport.zoom, MIN_ZOOM);
            state.zoom_at(99.0, 100., 100.);
            assert_eq!(state.viewport.zoom, MAX_ZOOM);
        });
    });
    paint(&app, cx);
}
