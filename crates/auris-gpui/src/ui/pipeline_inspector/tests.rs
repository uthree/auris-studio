use super::*;

use crate::dock::{Dock, Panel};
use crate::harness::{click, paint, with_a_clip};
use crate::theme::Theme;
use gpui::{TestAppContext, px};

fn show_mixer(app: &gpui::Entity<AurisApp>, cx: &mut gpui::VisualTestContext) {
    app.update(cx, |this, _| {
        this.panels.set_detached(Panel::Mixer, false);
        this.panels.move_to(Panel::Mixer, Dock::Bottom);
        this.panels.set_size(Dock::Bottom, px(520.));
        this.panels.show(Panel::Mixer);
    });
    paint(app, cx);
}

fn show_details(app: &gpui::Entity<AurisApp>, cx: &mut gpui::VisualTestContext) {
    show_mixer(app, cx);
    click("mixer-pipeline-details", cx);
    paint(app, cx);
    assert!(cx.debug_bounds("pipeline-disclosure").is_some());
}

fn selector(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

fn with_selected_clip(
    cx: &mut TestAppContext,
) -> (
    gpui::Entity<AurisApp>,
    &mut gpui::VisualTestContext,
    TrackId,
    ClipId,
) {
    let (app, cx, track, clip) = with_a_clip(cx);
    app.update(cx, |this, _| {
        this.select_track(track);
        this.select_clip(Some(clip));
    });
    paint(&app, cx);
    (app, cx, track, clip)
}

#[gpui::test]
fn pipeline_details_are_keyboard_disclosable_without_document_or_layout_changes(
    cx: &mut TestAppContext,
) {
    let (app, cx, _, clip) = with_selected_clip(cx);
    let (before_project, before_view, before_revision) = app.read_with(cx, |this, _| {
        (
            serde_json::to_value(this.project()).unwrap(),
            this.project().mixer_graph_view.clone(),
            this.session.revision(),
        )
    });
    show_details(&app, cx);
    let row = selector(format!("pipeline-node-clip:{}", clip.0));
    assert!(cx.debug_bounds(row).is_some());
    let expanded_height = cx.debug_bounds("pipeline-section").unwrap().size.height;
    click("pipeline-disclosure", cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| assert!(!this.pipeline_inspector.expanded));
    // GPUI retains old child selectors; the section itself is painted in both states.
    assert!(cx.debug_bounds("pipeline-section").unwrap().size.height < expanded_height);
    cx.simulate_keystrokes("space");
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("space").unwrap(),
    });
    paint(&app, cx);
    app.read_with(cx, |this, _| assert!(this.pipeline_inspector.expanded));
    assert_eq!(
        cx.debug_bounds("pipeline-section").unwrap().size.height,
        expanded_height
    );
    app.read_with(cx, |this, _| {
        assert_eq!(
            serde_json::to_value(this.project()).unwrap(),
            before_project
        );
        assert_eq!(this.project().mixer_graph_view, before_view);
        assert_eq!(this.session.revision(), before_revision);
    });
}

#[gpui::test]
fn palette_enter_release_keeps_the_revealed_pipeline_open(cx: &mut TestAppContext) {
    let (app, cx, _, _) = with_selected_clip(cx);
    let label = app.read_with(cx, |this, _| this.t(Key::PipelineDetails));
    cx.simulate_keystrokes("secondary-shift-p");
    cx.simulate_input(label);
    cx.simulate_keystrokes("enter");
    paint(&app, cx);
    app.read_with(cx, |this, _| assert!(this.pipeline_inspector.expanded));
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("enter").unwrap(),
    });
    paint(&app, cx);
    app.read_with(cx, |this, _| assert!(this.pipeline_inspector.expanded));
    // A new complete key press on the disclosure still works without losing focus.
    cx.simulate_keystrokes("enter");
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("enter").unwrap(),
    });
    paint(&app, cx);
    app.read_with(cx, |this, _| assert!(!this.pipeline_inspector.expanded));
}

#[gpui::test]
fn pipeline_scope_buttons_follow_clip_then_track_selection(cx: &mut TestAppContext) {
    let (app, cx, track, clip) = with_selected_clip(cx);
    show_details(&app, cx);
    click("pipeline-scope-clip", cx);
    paint(&app, cx);
    assert!(
        cx.debug_bounds(selector(format!("pipeline-node-clip:{}", clip.0)))
            .is_some()
    );
    click("pipeline-scope-track", cx);
    paint(&app, cx);
    assert!(
        cx.debug_bounds(selector(format!("pipeline-node-track:{}", track.0)))
            .is_some()
    );
    app.read_with(cx, |this, _| {
        assert_eq!(
            this.pipeline_inspector.model.scope(),
            model::InspectorScope::Track(track)
        );
    });
}

#[gpui::test]
fn pipeline_row_can_select_a_different_clip_owner(cx: &mut TestAppContext) {
    let (app, cx, track, first_clip) = with_selected_clip(cx);
    let second_clip = app.update(cx, |this, _| {
        let clip = this
            .session
            .add_midi_clip(track, "Second", Ticks::QUARTER * 8, Ticks::QUARTER * 4)
            .unwrap();
        this.select_track(track);
        this.select_clip(None);
        clip
    });
    show_details(&app, cx);
    click("pipeline-scope-track", cx);
    paint(&app, cx);
    app.update(cx, |this, _| this.inspector_scroll.scroll_to_bottom());
    paint(&app, cx);
    let second_selector = selector(format!("pipeline-select-clip-{}", second_clip.0));
    assert!(cx.debug_bounds(second_selector).is_some());
    click(second_selector, cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert_eq!(this.selected_clip, Some(second_clip));
        assert_ne!(this.selected_clip, Some(first_clip));
        assert!(!this.pipeline_inspector.track_scope);
    });
}

#[gpui::test]
fn transform_and_undo_refresh_the_cached_pipeline_page(cx: &mut TestAppContext) {
    let (app, cx, _, clip) = with_selected_clip(cx);
    show_details(&app, cx);
    let stage = selector(format!("pipeline-node-clip:{}/stage:0", clip.0));
    assert!(cx.debug_bounds(stage).is_none());
    app.update(cx, |this, _| {
        this.session
            .set_clip_transforms(clip, vec![NoteTransform::Lean { ticks: 10 }])
            .unwrap();
    });
    paint(&app, cx);
    assert!(cx.debug_bounds(stage).is_some());
    app.update(cx, |this, _| {
        assert!(this.session.undo().is_some());
    });
    paint(&app, cx);
    assert!(cx.debug_bounds(stage).is_none());
}

#[gpui::test]
fn pipeline_pagination_is_reachable_through_the_next_button(cx: &mut TestAppContext) {
    let (app, cx, track, _) = with_selected_clip(cx);
    app.update(cx, |this, _| {
        for index in 0..40 {
            this.session
                .add_effect(
                    Some(track),
                    if index % 2 == 0 {
                        "auris.fx.gain"
                    } else {
                        "auris.fx.delay"
                    },
                )
                .unwrap();
        }
    });
    show_details(&app, cx);
    click("pipeline-scope-track", cx);
    paint(&app, cx);
    assert!(cx.debug_bounds("pipeline-next").is_some());
    click("pipeline-next", cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert_eq!(this.pipeline_inspector.model.offset(), 32);
        assert!(this.pipeline_inspector.model.page().unwrap().nodes.len() <= 32);
    });
    assert!(cx.debug_bounds("pipeline-previous").is_some());
}

#[gpui::test]
fn pipeline_text_distinguishes_send_taps_and_singer_guide_fallback(cx: &mut TestAppContext) {
    let (app, cx, track, _) = with_selected_clip(cx);
    let (pre, post, singer) = app.update(cx, |this, _| {
        let bus = this.session.add_bus_track("Bus");
        let pre = this.session.add_send(track, bus).unwrap();
        this.session.set_send_pre_fader(track, pre, true).unwrap();
        let post = this.session.add_send(track, bus).unwrap();
        this.session.set_send_pre_fader(track, post, false).unwrap();
        let singer = this.session.add_singer_track("Singer");
        (pre, post, singer)
    });
    show_details(&app, cx);
    click("pipeline-scope-track", cx);
    paint(&app, cx);
    let (pre_title, post_title) = app.read_with(cx, |this, _| {
        let page = this.pipeline_inspector.model.page().unwrap();
        let mut titles = page.nodes.iter().filter_map(|node| {
            if let PipelineNodeData::Send { send, .. } = &node.data {
                Some((*send, this.pipeline_node_text(node).title))
            } else {
                None
            }
        });
        let pre_title = titles.find(|(id, _)| *id == pre.0).unwrap().1.clone();
        let post_title = titles.find(|(id, _)| *id == post.0).unwrap().1.clone();
        (pre_title, post_title)
    });
    assert_eq!(
        pre_title,
        app.read_with(cx, |this, _| format!(
            "{} → Bus",
            this.t(Key::PipelinePreFader)
        ))
    );
    assert_eq!(
        post_title,
        app.read_with(cx, |this, _| format!(
            "{} → Bus",
            this.t(Key::PipelinePostFader)
        ))
    );
    app.update(cx, |this, _| this.select_track(singer));
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        let page = this.pipeline_inspector.model.page().unwrap();
        let guide = page
            .nodes
            .iter()
            .find(|node| {
                matches!(
                    &node.data,
                    PipelineNodeData::Instrument {
                        singer_guide: true,
                        ..
                    }
                )
            })
            .expect("singer guide is represented");
        assert_eq!(
            this.pipeline_node_text(guide).detail,
            this.t(Key::PipelineGuideFallback)
        );
        let mut audition = guide.clone();
        if let PipelineNodeData::Instrument { audition_only, .. } = &mut audition.data {
            *audition_only = true;
        }
        assert_eq!(
            this.pipeline_node_text(&audition).detail,
            this.t(Key::PipelineGuideAudition)
        );
        let mut take = guide.clone();
        take.data = PipelineNodeData::SingerTake {
            source: 999,
            freshness_checked: false,
        };
        assert_eq!(
            this.pipeline_node_text(&take).detail,
            this.t(Key::PipelineTakeUnchecked)
        );
    });
}

#[gpui::test]
fn pipeline_details_fit_the_inspector_across_the_ui_matrix(cx: &mut TestAppContext) {
    let (app, cx, _, clip) = with_selected_clip(cx);
    app.update(cx, |this, _| {
        this.panels.set_detached(Panel::Inspector, false);
        this.panels.move_to(Panel::Inspector, Dock::Right);
        this.panels.set_size(Dock::Right, px(220.));
        this.panels.show(Panel::Inspector);
    });
    for (scheme, language, font_size) in [
        ("daylight", auris_i18n::Language::English, 13.0),
        ("midnight", auris_i18n::Language::English, 16.0),
        ("daylight", auris_i18n::Language::Japanese, 13.0),
        ("midnight", auris_i18n::Language::Japanese, 16.0),
    ] {
        app.update(cx, |this, _| {
            this.theme = Theme::named(scheme);
            this.language = language;
        });
        cx.update(|window, _| window.set_rem_size(px(font_size)));
        paint(&app, cx);
        click("pipeline-disclosure", cx);
        paint(&app, cx);
        let body = app.read_with(cx, |this, _| this.inspector_scroll.bounds());
        let disclosure = cx.debug_bounds("pipeline-disclosure").unwrap();
        assert!(disclosure.left() >= body.left());
        assert!(disclosure.right() <= body.right());
        let row = cx
            .debug_bounds(selector(format!("pipeline-node-clip:{}", clip.0)))
            .unwrap();
        assert!(row.left() >= body.left());
        assert!(row.right() <= body.right());
        app.read_with(cx, |this, _| assert!(this.pipeline_inspector.expanded));
        click("pipeline-disclosure", cx);
        paint(&app, cx);
    }
}
