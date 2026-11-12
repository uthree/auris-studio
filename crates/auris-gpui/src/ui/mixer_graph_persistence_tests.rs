use super::*;
use crate::dock::{Dock, Panel};
use crate::harness::{drag, open, paint};
use gpui::{Modifiers, ScrollDelta, ScrollWheelEvent, TestAppContext, point};

#[gpui::test]
fn routing_edits_preserve_untouched_card_positions_across_reopening(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    let (source, bus) = app.update(cx, |this, _| {
        this.panels.set_detached(Panel::Mixer, false);
        this.panels.move_to(Panel::Mixer, Dock::Bottom);
        this.panels.show(Panel::Mixer);
        let source = this.session.add_bus_track("Source");
        let bus = this.session.add_bus_track("Return");
        this.session.forget_history();
        (source, bus)
    });
    paint(&app, cx);
    let before = app.read_with(cx, |this, cx| {
        this.mixer_canvas.as_ref().unwrap().presentation(cx)
    });
    app.update(cx, |this, _| {
        this.session
            .add_track_output(source, Destination::Bus(bus))
            .unwrap()
    });
    paint(&app, cx);
    app.update(cx, |this, cx| {
        let current = this.mixer_canvas.as_ref().unwrap().presentation(cx);
        assert_eq!(current, before);
        assert_eq!(this.project().mixer_graph_view, before);
        let automatic = positions(&this.session.mixer_graph(), this.session.project());
        let owner = MixerNode(Some(bus));
        assert_ne!(before.nodes[&owner.key()].y, automatic[&owner].y);
        this.reset_view();
    });
    paint(&app, cx);
    app.read_with(cx, |this, cx| {
        assert_eq!(this.mixer_canvas.as_ref().unwrap().presentation(cx), before)
    });
}

#[gpui::test]
fn gestures_save_and_reopen_restore_each_node_and_the_canvas_camera(cx: &mut TestAppContext) {
    let scratch = tempfile::tempdir().unwrap();
    let (app, cx) = open(cx);
    let track = app.update(cx, |this, _| {
        this.panels.set_detached(Panel::Mixer, false);
        this.panels.move_to(Panel::Mixer, Dock::Bottom);
        this.panels.set_size(Dock::Bottom, px(600.));
        this.panels.show(Panel::Mixer);
        let track = this.session.add_bus_track("Layout");
        this.session.add_effect(None, "auris.fx.gain").unwrap();
        this.session.forget_history();
        track
    });
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert!(!this.session.is_dirty(), "initial layout is not an edit");
        assert!(this.project().mixer_graph_view.nodes.is_empty());
    });
    let id = node_id(MixerNode(Some(track)));
    let selector = Box::leak(id.to_string().into_boxed_str());
    let node = cx.debug_bounds(selector).unwrap();
    let from = node.origin + point(px(12.), px(10.));
    let original = app.read_with(cx, |this, cx| {
        this.mixer_canvas
            .as_ref()
            .unwrap()
            .state
            .read(cx)
            .get_node(&id)
            .unwrap()
            .position
    });
    drag(cx, from, from + point(px(70.), px(40.)));
    paint(&app, cx);
    app.read_with(cx, |this, cx| {
        let state = this.mixer_canvas.as_ref().unwrap().state.read(cx);
        let position = state.get_node(&id).unwrap().position;
        assert!((position.x - original.x - 70.).abs() < 1e-3);
        assert!((position.y - original.y - 40.).abs() < 1e-3);
        assert!(this.session.is_dirty());
        assert!(this.session.can_undo());
    });
    let at = cx.debug_bounds(selector).unwrap().center();
    for (control, delta) in [
        (true, point(px(0.), px(25.))),
        (false, point(px(55.), px(-33.))),
    ] {
        cx.simulate_event(ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Pixels(delta),
            modifiers: Modifiers {
                control,
                ..Modifiers::none()
            },
            ..Default::default()
        });
        paint(&app, cx);
    }
    let (saved, document, identity) = app.update(cx, |this, cx| {
        let canvas = this.mixer_canvas.as_ref().unwrap();
        let saved = canvas.presentation(cx);
        assert_eq!(this.project().mixer_graph_view, saved);
        assert_eq!(
            saved.nodes.len(),
            this.session.mixer_graph().nodes.len(),
            "every strip and insert is stored"
        );
        assert!(saved.zoom > 1.);
        let identity = canvas.state.entity_id();
        let report = this
            .session
            .save_as(&scratch.path().join("Layout.auris"))
            .unwrap();
        assert!(!this.session.is_dirty());
        (saved, report.document, identity)
    });
    // Replacing the document must discard positions even when the new track reuses an ID.
    app.update(cx, |this, _| {
        this.new_project();
        assert!(this.mixer_canvas.is_none());
        this.session.add_bus_track("Other");
        this.session.forget_history();
    });
    paint(&app, cx);
    app.read_with(cx, |this, cx| {
        let canvas = this.mixer_canvas.as_ref().unwrap();
        assert_ne!(canvas.state.entity_id(), identity);
        assert_eq!(canvas.state.read(cx).viewport.zoom, 1.);
        assert!(!this.session.is_dirty());
    });
    app.update(cx, |this, cx| this.open_project_at(document, cx));
    cx.run_until_parked();
    paint(&app, cx);
    app.read_with(cx, |this, cx| {
        assert!(this.background_command.is_none());
        assert!(!this.status_failed);
        assert_eq!(this.project().mixer_graph_view, saved);
        assert_eq!(this.mixer_canvas.as_ref().unwrap().presentation(cx), saved);
        assert!(
            !this.session.is_dirty(),
            "restoring a layout is not an edit"
        );
        assert!(!this.session.can_undo());
    });
}

#[gpui::test]
fn restored_fit_scale_and_surviving_positions_allow_new_nodes_and_musical_undo(
    cx: &mut TestAppContext,
) {
    let (app, cx) = open(cx);
    let (track, far) = app.update(cx, |this, _| {
        this.panels.set_detached(Panel::Mixer, false);
        this.panels.move_to(Panel::Mixer, Dock::Bottom);
        this.panels.show(Panel::Mixer);
        let track = this.session.add_bus_track("Existing");
        let far = this.session.add_bus_track("Far");
        this.session.set_mixer_graph_view(MixerGraphView {
            nodes: [
                (
                    MixerNode(Some(track)).key(),
                    MixerNodePosition { x: -400., y: 600. },
                ),
                (
                    MixerNode(Some(far)).key(),
                    MixerNodePosition {
                        x: 12000.,
                        y: -8000.,
                    },
                ),
                // Older projects saved effect cards' places; they are ignored.
                ("effect:999999".into(), MixerNodePosition { x: 99., y: 99. }),
            ]
            .into(),
            zoom: 0.035,
            pan_x: 65.,
            pan_y: -10.,
        });
        this.session.forget_history();
        (track, far)
    });
    paint(&app, cx);
    let before = app.read_with(cx, |this, cx| {
        let canvas = this.mixer_canvas.as_ref().unwrap();
        assert_eq!(canvas.state.read(cx).min_zoom, 0.035);
        assert_eq!(canvas.state.read(cx).viewport.zoom, 0.035);
        assert!(!this.session.is_dirty());
        canvas.presentation(cx)
    });
    let new = app.update(cx, |this, _| this.session.add_bus_track("New"));
    paint(&app, cx);
    app.read_with(cx, |this, cx| {
        let canvas = this.mixer_canvas.as_ref().unwrap();
        let state = canvas.state.read(cx);
        assert_eq!(state.viewport.zoom, before.zoom);
        assert_eq!(
            canvas.presentation(cx).nodes[&MixerNode(Some(track)).key()],
            before.nodes[&MixerNode(Some(track)).key()]
        );
        let layout = positions(&this.session.mixer_graph(), this.session.project());
        assert_eq!(
            state
                .get_node(&node_id(MixerNode(Some(new))))
                .unwrap()
                .position,
            layout[&MixerNode(Some(new))]
        );
    });
    app.update(cx, |this, _| this.session.remove_track(far).unwrap());
    paint(&app, cx);
    app.update(cx, |this, _| this.undo());
    paint(&app, cx);
    app.read_with(cx, |this, cx| {
        let current = this.mixer_canvas.as_ref().unwrap().presentation(cx);
        let key = MixerNode(Some(far)).key();
        assert_eq!(current.nodes[&key], before.nodes[&key]);
        assert_eq!(
            (current.zoom, current.pan_x, current.pan_y),
            (before.zoom, before.pan_x, before.pan_y)
        );
    });
}
