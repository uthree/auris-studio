//! Editor entry points keep native windows, parameter access, and bypass separate.

use super::*;
use crate::auxiliary_window::Surface;
use crate::harness::{choose, click, paint, resize, with_a_clip};
use gpui::{TestAppContext, size};

fn selector(id: impl Into<gpui::ElementId>) -> &'static str {
    Box::leak(id.into().to_string().into_boxed_str())
}

#[gpui::test]
fn instrument_action_reuses_the_parameter_window_and_menu_supports_keyboard(
    cx: &mut TestAppContext,
) {
    let (app, cx, track, _) = with_a_clip(cx);
    app.update(cx, |this, _| {
        this.select_track(track);
        this.panels.show(crate::dock::Panel::Inspector);
    });
    paint(&app, cx);
    click("inst-open", cx);
    paint(&app, cx);
    let first = app.read_with(cx, |this, _| {
        assert_eq!(
            this.plugin_window.unwrap().subject,
            PluginSubject::Instrument(track)
        );
        this.auxiliary_windows[&Surface::Plugin].window_id()
    });
    click("inst-open", cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert_eq!(this.auxiliary_windows[&Surface::Plugin].window_id(), first);
    });
    click("inst-menu", cx);
    paint(&app, cx);
    cx.simulate_keystrokes("escape");
    paint(&app, cx);
    assert!(app.read_with(cx, |this, _| this.menu.is_none()));
    // Escape preserves the trigger's focus, so Enter can open the same menu again.
    cx.simulate_keystrokes("enter");
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("enter").unwrap(),
    });
    paint(&app, cx);
    assert!(app.read_with(cx, |this, _| this.menu.is_some()));
    // Enter on the open menu closes it. Its release must not press the restored trigger.
    cx.simulate_keystrokes("enter");
    paint(&app, cx);
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("enter").unwrap(),
    });
    paint(&app, cx);
    assert!(app.read_with(cx, |this, _| this.menu.is_none()));
    cx.simulate_keystrokes("enter");
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("enter").unwrap(),
    });
    paint(&app, cx);
    assert!(app.read_with(cx, |this, _| this.menu.is_some()));
    choose(
        &app,
        cx,
        &MenuCommand::ClosePluginEditor(PluginSubject::Instrument(track)),
    );
    paint(&app, cx);
    assert!(app.read_with(cx, |this, _| this.plugin_window.is_none()));
}

#[gpui::test]
fn closing_a_menu_preserves_an_explicit_move_to_another_pane(cx: &mut TestAppContext) {
    let (app, cx, track, _) = with_a_clip(cx);
    app.update(cx, |this, _| {
        this.select_track(track);
        this.panels.show(crate::dock::Panel::Inspector);
    });
    paint(&app, cx);
    click("inst-menu", cx);
    paint(&app, cx);
    assert!(app.read_with(cx, |this, _| this.menu.is_some()));

    // Menu commands can deliberately send the keyboard to an editor. Teardown must preserve
    // that destination instead of sending it back to the menu's trigger.
    cx.update(|window, cx| {
        app.update(cx, |this, cx| {
            this.close_menu();
            this.focus_pane(crate::app::Pane::Arrangement, window);
            cx.notify();
        });
    });
    paint(&app, cx);
    cx.update(|window, cx| {
        app.read_with(cx, |this, cx| {
            assert!(this.pane_focused(crate::app::Pane::Arrangement, window, cx));
        });
    });
}

#[gpui::test]
fn bypassed_effect_opens_from_inspector_and_mixer_without_changing_audio(cx: &mut TestAppContext) {
    let (app, cx, track, _) = with_a_clip(cx);
    let slot = app.update(cx, |this, _| {
        this.select_track(track);
        this.panels.show(crate::dock::Panel::Inspector);
        this.panels.show(crate::dock::Panel::Mixer);
        this.mixer_graph_visible = false;
        let slot = this
            .session
            .add_effect(Some(track), "auris.fx.reverb")
            .unwrap();
        this.session.set_effect_enabled(Some(track), slot, false);
        slot
    });
    let key = crate::ui::inspector::insert_element_key(Some(slot));
    let subject = PluginSubject::Insert {
        track: Some(track),
        slot,
    };
    paint(&app, cx);
    click(selector(("insert-open", key)), cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert_eq!(this.plugin_window.unwrap().subject, subject);
        assert_eq!(this.session.effect_enabled(Some(track), slot), Some(false));
    });
    click(selector(("mixer-fx", key)), cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert_eq!(this.plugin_window.unwrap().subject, subject);
        assert_eq!(this.session.effect_enabled(Some(track), slot), Some(false));
    });
    click(selector(("insert-menu", key)), cx);
    paint(&app, cx);
    choose(&app, cx, &MenuCommand::PluginParameters(subject));
    paint(&app, cx);
    assert_eq!(
        app.read_with(cx, |this, _| this.plugin_window.unwrap().subject),
        subject
    );
    click(selector(("fx-enabled", key)), cx);
    assert_eq!(
        app.read_with(cx, |this, _| this.session.effect_enabled(Some(track), slot)),
        Some(true)
    );
}

#[gpui::test]
fn editor_controls_fit_translations_themes_and_larger_type(cx: &mut TestAppContext) {
    let (app, cx, track, _) = with_a_clip(cx);
    let slot = app.update(cx, |this, _| {
        this.select_track(track);
        this.panels.show(crate::dock::Panel::Inspector);
        this.panels.show(crate::dock::Panel::Mixer);
        this.mixer_graph_visible = false;
        this.session
            .add_effect(Some(track), "auris.fx.reverb")
            .unwrap()
    });
    let key = crate::ui::inspector::insert_element_key(Some(slot));
    for (language, font_size, scheme) in [
        (
            auris_i18n::Language::Japanese,
            12.,
            &crate::theme::SCHEMES[0],
        ),
        (
            auris_i18n::Language::English,
            18.,
            crate::theme::SCHEMES.iter().find(|s| s.base > 0.5).unwrap(),
        ),
    ] {
        app.update(cx, |this, _| {
            this.language = language;
            this.theme = Theme::from_scheme(scheme);
        });
        cx.update(|window, _| window.set_rem_size(px(font_size)));
        resize(&app, cx, size(px(1280.), px(900.)));
        for (row, action, menu) in [
            ("instrument-editor-row", "inst-open", "inst-menu"),
            (
                selector(gpui::SharedString::from(format!("insert-editor-row-{key}"))),
                selector(("insert-open", key)),
                selector(("insert-menu", key)),
            ),
            (
                selector(gpui::SharedString::from(format!(
                    "{}-row",
                    gpui::ElementId::from(("mixer-fx", key))
                ))),
                selector(("mixer-fx", key)),
                selector(gpui::SharedString::from(format!(
                    "{}-menu",
                    gpui::ElementId::from(("mixer-fx", key))
                ))),
            ),
        ] {
            let row = cx.debug_bounds(row).unwrap();
            for control in [action, menu] {
                let bounds = cx.debug_bounds(control).unwrap();
                assert!(
                    bounds.left() >= row.left() && bounds.right() <= row.right() + px(1.),
                    "{control} escaped its row: {bounds:?} / {row:?}"
                );
            }
        }
    }
}

#[gpui::test]
#[ignore = "requires AURIS_VST3_SMOKE_PLUGIN with a native instrument editor"]
fn instrument_button_opens_native_editor_directly_and_parameters_remain_separate(
    cx: &mut TestAppContext,
) {
    let path = std::path::PathBuf::from(std::env::var_os("AURIS_VST3_SMOKE_PLUGIN").unwrap());
    let (app, cx, track, _) = with_a_clip(cx);
    app.update(cx, |this, _| {
        let info = this
            .session
            .vst3_plugins_in(&path)
            .unwrap()
            .into_iter()
            .find(|info| info.has_gui)
            .unwrap();
        this.session
            .set_vst3_instrument(track, &path, &info.class_id)
            .unwrap();
        this.session.poll();
        this.select_track(track);
    });
    paint(&app, cx);
    click("inst-open", cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert!(
            this.session
                .plugin_window_is_open(HostedWindow::Instrument(track))
        );
        assert!(
            this.plugin_window.is_none(),
            "the parameter window must not be an intermediate step"
        );
    });
    click("inst-open", cx);
    assert!(app.read_with(cx, |this, _| {
        this.session
            .plugin_window_is_open(HostedWindow::Instrument(track))
    }));
    click("inst-menu", cx);
    paint(&app, cx);
    choose(
        &app,
        cx,
        &MenuCommand::PluginParameters(PluginSubject::Instrument(track)),
    );
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert_eq!(
            this.plugin_window.unwrap().subject,
            PluginSubject::Instrument(track)
        );
        assert!(
            this.session
                .plugin_window_is_open(HostedWindow::Instrument(track))
        );
    });
    app.update(cx, |this, _| {
        this.close_plugin_editor(PluginSubject::Instrument(track));
        this.close_plugin_window();
    });
}

#[gpui::test]
#[ignore = "requires AURIS_CLAP_EFFECT_SMOKE_PLUGIN with a native effect editor"]
fn effect_button_opens_native_editor_without_changing_bypass(cx: &mut TestAppContext) {
    let path =
        std::path::PathBuf::from(std::env::var_os("AURIS_CLAP_EFFECT_SMOKE_PLUGIN").unwrap());
    let (app, cx, track, _) = with_a_clip(cx);
    let slot = app.update(cx, |this, _| {
        let info = this
            .session
            .hosted_plugins_in(&path)
            .unwrap()
            .into_iter()
            .find(|info| info.kind == PluginKind::Effect)
            .unwrap();
        let slot = this
            .session
            .add_hosted_effect(Some(track), &path, &info.clap_id)
            .unwrap();
        this.session.set_effect_enabled(Some(track), slot, false);
        this.session.poll();
        this.select_track(track);
        slot
    });
    let key = crate::ui::inspector::insert_element_key(Some(slot));
    paint(&app, cx);
    click(selector(("insert-open", key)), cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert!(
            this.session
                .plugin_window_is_open(HostedWindow::Effect(slot))
        );
        assert!(this.plugin_window.is_none());
        assert_eq!(this.session.effect_enabled(Some(track), slot), Some(false));
    });
    click(selector(("insert-open", key)), cx);
    app.read_with(cx, |this, _| {
        assert!(
            this.session
                .plugin_window_is_open(HostedWindow::Effect(slot))
        );
        assert_eq!(this.session.effect_enabled(Some(track), slot), Some(false));
    });
    app.update(cx, |this, _| {
        this.close_plugin_editor(PluginSubject::Insert {
            track: Some(track),
            slot,
        })
    });
}

#[gpui::test]
fn a_sample_loaded_from_the_window_puts_the_track_on_the_sampler(cx: &mut TestAppContext) {
    let (app, cx, track, _) = with_a_clip(cx);
    let rate = app.read_with(cx, |this, _| this.project().sample_rate);
    let tone: Vec<f32> = (0..4_800)
        .map(|n| (std::f64::consts::TAU * 440.0 * n as f64 / rate).sin() as f32)
        .collect();
    let buffer = AudioBuffer::from_planar(vec![tone], rate).unwrap();
    let line = app.update(cx, |this, _| {
        this.take_sample(
            track,
            std::path::Path::new("/samples/Pluck_C4.wav"),
            crate::ui::commands::Loaded::Audio(buffer),
        )
        .unwrap()
    });
    assert!(line.contains("Pluck_C4"), "{line}");
    app.read_with(cx, |this, _| {
        let inner = this
            .project()
            .track(track)
            .unwrap()
            .kind
            .as_instrument()
            .unwrap();
        assert_eq!(inner.instrument_id, AUDIO_SAMPLER_ID);
        assert!(this.session.track_sample_is_loaded(track));
        assert_eq!(
            this.session.track_sample_name(track).as_deref(),
            Some("Pluck_C4")
        );
    });
    // The sampler's window draws its sample row with the rest of its controls.
    app.update(cx, |this, _| {
        this.select_track(track);
        this.panels.show(crate::dock::Panel::Inspector);
    });
    paint(&app, cx);
    click("inst-open", cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert_eq!(
            this.plugin_window.unwrap().subject,
            PluginSubject::Instrument(track)
        );
    });
}
