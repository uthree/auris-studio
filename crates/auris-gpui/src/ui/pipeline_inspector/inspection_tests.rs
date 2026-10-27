use super::*;

use crate::dock::{Dock, Panel};
use crate::harness::{click, paint, with_a_clip};
use crate::theme::Theme;
use auris_session::PipelineInspectionDetail;
use gpui::{TestAppContext, px};

fn show_details(app: &gpui::Entity<AurisApp>, cx: &mut gpui::VisualTestContext) {
    app.update(cx, |this, _| {
        this.panels.set_detached(Panel::Mixer, false);
        this.panels.move_to(Panel::Mixer, Dock::Bottom);
        this.panels.set_size(Dock::Bottom, px(520.));
        this.panels.show(Panel::Mixer);
    });
    paint(app, cx);
    click("mixer-pipeline-details", cx);
    paint(app, cx);
}

fn selector(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

fn stage_selector(clip: ClipId, stage: usize) -> &'static str {
    selector(format!("pipeline-inspect-clip:{}/stage:{stage}", clip.0))
}

fn state_selector(clip: ClipId, stage: usize) -> &'static str {
    selector(format!("pipeline-state-clip:{}/stage:{stage}", clip.0))
}

fn staged_clip(
    cx: &mut TestAppContext,
) -> (gpui::Entity<AurisApp>, &mut gpui::VisualTestContext, ClipId) {
    let (app, cx, _, clip) = with_a_clip(cx);
    app.update(cx, |this, _| {
        // The harness has no callback thread. Suspended sessions own the playhead directly.
        this.session.set_audio_suspended(true).unwrap();
        this.session
            .replace_notes(clip, vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)])
            .unwrap();
        this.session
            .set_clip_transforms(
                clip,
                vec![
                    NoteTransform::Transpose { semitones: 12 },
                    NoteTransform::Transpose { semitones: -5 },
                ],
            )
            .unwrap();
        this.select_clip(Some(clip));
    });
    paint(&app, cx);
    (app, cx, clip)
}

#[gpui::test]
fn intermediate_inspection_shows_each_stage_boundary_and_is_cached_until_refresh(
    cx: &mut TestAppContext,
) {
    let (app, cx, clip) = staged_clip(cx);
    show_details(&app, cx);
    let (before_project, before_graph, before_revision) = app.read_with(cx, |this, _| {
        (
            serde_json::to_value(this.project()).unwrap(),
            this.project().mixer_graph_view.clone(),
            this.session.revision(),
        )
    });

    click(stage_selector(clip, 0), cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        let snapshot = this.pipeline_inspector.inspection.as_ref().unwrap();
        let Some(PipelineInspectionDetail::Stage { state }) =
            snapshot.value.as_ref().map(|value| &value.detail)
        else {
            panic!("stage inspection did not return stage detail");
        };
        assert_eq!(state.stage_index, 0);
        assert_eq!(state.input[0].pitch, 60);
        assert_eq!(state.output[0].pitch, 72);
        assert_eq!(snapshot.value.as_ref().unwrap().at_tick, Ticks::ZERO);
    });

    click(stage_selector(clip, 1), cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        let snapshot = this.pipeline_inspector.inspection.as_ref().unwrap();
        let Some(PipelineInspectionDetail::Stage { state }) =
            snapshot.value.as_ref().map(|value| &value.detail)
        else {
            panic!("stage inspection did not return stage detail");
        };
        assert_eq!(state.stage_index, 1);
        assert_eq!(state.input[0].pitch, 72);
        assert_eq!(state.output[0].pitch, 67);
    });
    let state = state_selector(clip, 1);
    assert!(cx.debug_bounds(state).is_some());

    app.update(cx, |this, _| this.session.seek(Ticks::QUARTER));
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        let snapshot = this.pipeline_inspector.inspection.as_ref().unwrap();
        assert_eq!(snapshot.value.as_ref().unwrap().at_tick, Ticks::ZERO);
    });

    click(stage_selector(clip, 1), cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert_eq!(
            this.pipeline_inspector
                .inspection
                .as_ref()
                .unwrap()
                .value
                .as_ref()
                .unwrap()
                .at_tick,
            Ticks::QUARTER
        );
        assert_eq!(
            serde_json::to_value(this.project()).unwrap(),
            before_project
        );
        assert_eq!(this.project().mixer_graph_view, before_graph);
        assert_eq!(this.session.revision(), before_revision);
    });
}

#[gpui::test]
fn intermediate_inspection_is_invalidated_by_revision_and_undo_without_document_writes(
    cx: &mut TestAppContext,
) {
    let (app, cx, clip) = staged_clip(cx);
    show_details(&app, cx);
    click(stage_selector(clip, 0), cx);
    paint(&app, cx);

    let (before_project, before_graph, before_revision) = app.read_with(cx, |this, _| {
        (
            serde_json::to_value(this.project()).unwrap(),
            this.project().mixer_graph_view.clone(),
            this.session.revision(),
        )
    });
    app.update(cx, |this, _| {
        this.session
            .set_clip_transforms(clip, vec![NoteTransform::Transpose { semitones: 7 }])
            .unwrap();
    });
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert!(this.pipeline_inspector.inspection.is_none());
        assert_ne!(this.session.revision(), before_revision);
        assert_ne!(
            serde_json::to_value(this.project()).unwrap(),
            before_project
        );
        assert_eq!(this.project().mixer_graph_view, before_graph);
    });

    app.update(cx, |this, _| {
        assert!(this.session.undo().is_some());
    });
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert!(this.pipeline_inspector.inspection.is_none());
    });
}

#[gpui::test]
fn fader_inspection_reports_passive_parameter_values_without_loading_audio_state(
    cx: &mut TestAppContext,
) {
    let (app, cx, track, _) = with_a_clip(cx);
    app.update(cx, |this, _| this.select_track(track));
    show_details(&app, cx);
    click("pipeline-scope-track", cx);
    paint(&app, cx);
    click(
        selector(format!("pipeline-inspect-fader:track:{}", track.0)),
        cx,
    );
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        let snapshot = this.pipeline_inspector.inspection.as_ref().unwrap();
        let Some(PipelineInspectionDetail::Parameters { values }) =
            snapshot.value.as_ref().map(|value| &value.detail)
        else {
            panic!("fader inspection did not return parameter detail");
        };
        assert!(values.iter().any(|value| value.static_value == Some(0.0)));
        assert!(values.iter().all(|value| value.binding_checked));
    });
}

#[gpui::test]
fn intermediate_inspection_fits_narrow_english_and_japanese_inspector_without_pixels(
    cx: &mut TestAppContext,
) {
    let (app, cx, clip) = staged_clip(cx);
    app.update(cx, |this, _| {
        this.panels.set_detached(Panel::Inspector, false);
        this.panels.move_to(Panel::Inspector, Dock::Right);
        this.panels.set_size(Dock::Right, px(220.));
        this.panels.show(Panel::Inspector);
    });
    show_details(&app, cx);
    click(stage_selector(clip, 1), cx);
    paint(&app, cx);

    for (scheme, language, font_size) in [
        ("daylight", auris_i18n::Language::English, 13.0),
        ("midnight", auris_i18n::Language::Japanese, 16.0),
    ] {
        app.update(cx, |this, _| {
            this.theme = Theme::named(scheme);
            this.language = language;
        });
        cx.update(|window, _| window.set_rem_size(px(font_size)));
        paint(&app, cx);
        let body = app.read_with(cx, |this, _| this.inspector_scroll.bounds());
        let disclosure = cx.debug_bounds("pipeline-disclosure").unwrap();
        let state = cx.debug_bounds(state_selector(clip, 1)).unwrap();
        assert!(disclosure.left() >= body.left());
        assert!(disclosure.right() <= body.right());
        assert!(state.left() >= body.left());
        assert!(state.right() <= body.right());
    }
}
