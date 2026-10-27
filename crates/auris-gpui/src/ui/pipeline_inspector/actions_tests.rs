use super::*;

use crate::dock::{Dock, Panel};
use crate::harness::{choose, click, paint, with_a_clip};
use crate::theme::Theme;
use crate::ui::context_menu::MenuCommand;
use crate::ui::plugin_window::PluginSubject;
use gpui::{TestAppContext, px};

fn selector(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

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

fn click_visible_action(
    app: &gpui::Entity<AurisApp>,
    cx: &mut gpui::VisualTestContext,
    id: &'static str,
) {
    let action = cx.debug_bounds(id).expect("pipeline action is laid out");
    app.update(cx, |this, _| {
        let body = this.inspector_scroll.bounds();
        let mut offset = this.inspector_scroll.offset();
        offset.y += body.center().y - action.center().y;
        this.inspector_scroll.set_offset(offset);
    });
    paint(app, cx);
    let action = cx.debug_bounds(id).unwrap();
    app.read_with(cx, |this, _| {
        assert!(
            this.inspector_scroll.bounds().contains(&action.center()),
            "action must be inside the scrolling viewport"
        )
    });
    click(id, cx);
}

#[gpui::test]
fn stage_menu_reorders_and_removes_the_exact_duplicate_and_undo_restores_notes(
    cx: &mut TestAppContext,
) {
    let (app, cx, _, clip) = with_a_clip(cx);
    let initial = app.update(cx, |this, _| {
        this.session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        let notes = this
            .session
            .project()
            .midi_clip(clip)
            .unwrap()
            .1
            .notes
            .clone();
        this.session
            .set_clip_transforms(
                clip,
                vec![
                    NoteTransform::Humanize {
                        amount: 0.2,
                        seed: 7,
                    },
                    NoteTransform::Gate { amount: 0.5 },
                    NoteTransform::Gate { amount: 0.8 },
                ],
            )
            .unwrap();
        notes
    });
    app.update(cx, |this, _| this.select_clip(Some(clip)));
    paint(&app, cx);
    show_details(&app, cx);

    let revision = app.read_with(cx, |this, _| this.session.revision());
    click(selector(format!("pipeline-stage-actions-{}-1", clip.0)), cx);
    cx.simulate_keystrokes("escape");
    paint(&app, cx);
    app.read_with(cx, |this, _| assert!(this.menu.is_none()));
    click(selector(format!("pipeline-stage-actions-{}-1", clip.0)), cx);
    let move_later = MenuCommand::PipelineStage {
        clip,
        revision,
        edit: ClipTransformEdit::Move { index: 1, delta: 1 },
    };
    choose(&app, cx, &move_later);
    paint(&app, cx);

    let moved = app.read_with(cx, |this, _| {
        this.session.clip_transforms(clip).unwrap().to_vec()
    });
    assert!(matches!(moved[1], NoteTransform::Gate { amount } if amount == 0.8));
    assert!(matches!(moved[2], NoteTransform::Gate { amount } if amount == 0.5));

    let revision = app.read_with(cx, |this, _| this.session.revision());
    click(selector(format!("pipeline-stage-actions-{}-2", clip.0)), cx);
    choose(
        &app,
        cx,
        &MenuCommand::PipelineStage {
            clip,
            revision,
            edit: ClipTransformEdit::Remove { index: 2 },
        },
    );
    paint(&app, cx);
    assert_eq!(
        app.read_with(cx, |this, _| this
            .session
            .clip_transforms(clip)
            .unwrap()
            .len()),
        2
    );

    app.update(cx, |this, _| {
        assert!(this.session.undo().is_some());
    });
    let restored = app.read_with(cx, |this, _| {
        (
            this.session.clip_transforms(clip).unwrap().to_vec(),
            this.session
                .project()
                .midi_clip(clip)
                .unwrap()
                .1
                .notes
                .clone(),
        )
    });
    assert_eq!(restored.0, moved);
    assert_eq!(restored.1, initial);
}

#[gpui::test]
fn stale_stage_menu_command_is_rejected_without_a_second_history_entry(cx: &mut TestAppContext) {
    let (app, cx, _, clip) = with_a_clip(cx);
    app.update(cx, |this, _| {
        this.session
            .set_clip_transforms(
                clip,
                vec![
                    NoteTransform::Gate { amount: 0.4 },
                    NoteTransform::Gate { amount: 0.8 },
                ],
            )
            .unwrap();
        this.select_clip(Some(clip));
    });
    paint(&app, cx);
    show_details(&app, cx);
    let revision = app.read_with(cx, |this, _| this.session.revision());
    click(selector(format!("pipeline-stage-actions-{}-0", clip.0)), cx);

    let post_mutation = app.update(cx, |this, _| {
        let revision = this.session.revision();
        this.session
            .edit_clip_transform(clip, revision, ClipTransformEdit::Remove { index: 1 })
            .unwrap();
        this.session.clip_transforms(clip).unwrap().to_vec()
    });
    choose(
        &app,
        cx,
        &MenuCommand::PipelineStage {
            clip,
            revision,
            edit: ClipTransformEdit::Move { index: 0, delta: 1 },
        },
    );
    assert_eq!(
        app.read_with(cx, |this, _| this
            .session
            .clip_transforms(clip)
            .unwrap()
            .to_vec()),
        post_mutation
    );
    app.update(cx, |this, _| {
        assert!(this.session.undo().is_some());
        assert_eq!(this.session.clip_transforms(clip).unwrap().len(), 2);
    });
}

#[gpui::test]
fn effect_actions_open_the_stable_slot_and_route_more_menu_edits(cx: &mut TestAppContext) {
    let (app, cx, track, _) = with_a_clip(cx);
    let (first, second) = app.update(cx, |this, _| {
        let first = this
            .session
            .add_effect(Some(track), "auris.fx.gain")
            .unwrap();
        let second = this
            .session
            .add_effect(Some(track), "auris.fx.delay")
            .unwrap();
        this.session.set_automation_point(
            ParamTarget::Effect {
                track: Some(track),
                slot: first,
                param: ParamId(0),
            },
            Ticks::ZERO,
            0.25,
        );
        this.select_track(track);
        (first, second)
    });
    show_details(&app, cx);
    click("pipeline-scope-track", cx);
    paint(&app, cx);
    click_visible_action(
        &app,
        cx,
        selector(format!("pipeline-parameters-{}", first.0)),
    );
    paint(&app, cx);
    assert_eq!(
        app.read_with(cx, |this, _| this.plugin_window.unwrap().subject),
        PluginSubject::Insert {
            track: Some(track),
            slot: first,
        }
    );

    click_visible_action(
        &app,
        cx,
        selector(format!("pipeline-effect-menu-{}", first.0)),
    );
    choose(
        &app,
        cx,
        &MenuCommand::MoveEffect {
            track: Some(track),
            slot: first,
            delta: 1,
        },
    );
    paint(&app, cx);
    assert_eq!(
        app.read_with(cx, |this, _| {
            this.session.project().track(track).unwrap().mixer.effects[1].id
        }),
        first
    );

    app.read_with(cx, |this, _| {
        assert!(
            this.session
                .automation()
                .lane(ParamTarget::Effect {
                    track: Some(track),
                    slot: first,
                    param: ParamId(0)
                })
                .is_some()
        );
    });
    click_visible_action(
        &app,
        cx,
        selector(format!("pipeline-effect-menu-{}", first.0)),
    );
    choose(
        &app,
        cx,
        &MenuCommand::ToggleEffect {
            track: Some(track),
            slot: first,
        },
    );
    assert_eq!(
        app.read_with(cx, |this, _| this
            .session
            .effect_enabled(Some(track), first)),
        Some(false)
    );

    paint(&app, cx);
    click_visible_action(
        &app,
        cx,
        selector(format!("pipeline-effect-menu-{}", first.0)),
    );
    choose(&app, cx, &MenuCommand::RemoveEffect(first));
    assert_eq!(
        app.read_with(cx, |this, _| this
            .session
            .effect_enabled(Some(track), second)),
        Some(true)
    );
}

#[gpui::test]
fn automation_action_reveals_the_track_and_is_idempotent_without_project_edits(
    cx: &mut TestAppContext,
) {
    let (app, cx, track, _) = with_a_clip(cx);
    let target = ParamTarget::TrackGain(track);
    let before = app.update(cx, |this, _| {
        this.session.set_automation_point(target, Ticks::ZERO, -6.0);
        this.select_track(track);
        serde_json::to_value(this.project()).unwrap()
    });
    show_details(&app, cx);
    click("pipeline-scope-track", cx);
    paint(&app, cx);
    let node_id = app.read_with(cx, |this, _| {
        this.pipeline_inspector
            .model
            .nodes()
            .find_map(|node| {
                matches!(
                    &node.data,
                    PipelineNodeData::Automation {
                        target: node_target,
                        ..
                    } if *node_target == target
                )
                .then(|| node.id.clone())
            })
            .expect("track automation is represented in the pipeline")
    });
    let action = selector(format!("pipeline-edit-{node_id}"));
    click(action, cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert_eq!(this.automation_lanes.get(&track), Some(&target));
        assert!(!this.track_grid.open);
    });
    click(action, cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert_eq!(this.automation_lanes.get(&track), Some(&target));
        assert_eq!(serde_json::to_value(this.project()).unwrap(), before);
    });
}

#[gpui::test]
fn pipeline_actions_fit_a_narrow_inspector_in_both_languages(cx: &mut TestAppContext) {
    let (app, cx, _, clip) = with_a_clip(cx);
    app.update(cx, |this, _| {
        this.session
            .set_clip_transforms(
                clip,
                vec![NoteTransform::Humanize {
                    amount: 0.2,
                    seed: 11,
                }],
            )
            .unwrap();
        this.panels.set_detached(Panel::Inspector, false);
        this.panels.move_to(Panel::Inspector, Dock::Right);
        this.panels.set_size(Dock::Right, px(220.));
        this.panels.show(Panel::Inspector);
        this.select_clip(Some(clip));
    });
    for (language, scheme) in [
        (auris_i18n::Language::English, "midnight"),
        (auris_i18n::Language::Japanese, "daylight"),
    ] {
        app.update(cx, |this, _| {
            this.language = language;
            this.theme = Theme::named(scheme);
            this.pipeline_inspector.expanded = false;
        });
        paint(&app, cx);
        click("pipeline-disclosure", cx);
        paint(&app, cx);
        let body = app.read_with(cx, |this, _| this.inspector_scroll.bounds());
        let action = cx
            .debug_bounds(selector(format!("pipeline-stage-actions-{}-0", clip.0)))
            .expect("stage action is visible in the narrow inspector");
        assert!(action.left() >= body.left());
        assert!(action.right() <= body.right());
    }
}
