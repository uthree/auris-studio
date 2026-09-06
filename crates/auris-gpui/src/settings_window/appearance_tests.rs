//! Appearance changes exercised through the real settings window and its input handler.

use gpui::{Entity, EntityInputHandler, TestAppContext, VisualTestContext, WindowHandle, size};

use super::*;

struct RestoreAppearance(Appearance);

impl Drop for RestoreAppearance {
    fn drop(&mut self) {
        self.0
            .save()
            .expect("restore isolated appearance after the test");
    }
}

fn open_appearance_settings(
    cx: &mut TestAppContext,
) -> (
    Entity<AurisApp>,
    WindowHandle<SettingsWindow>,
    VisualTestContext,
) {
    let (app, cx) = crate::harness::open(cx);
    app.update(cx, |this, cx| {
        // Every test starts from explicit preferences, regardless of what another window test
        // has saved in the harness's isolated configuration directory.
        this.appearance = Appearance {
            font_family: Some("Test Interface".to_owned()),
            ..Appearance::default()
        };
        this.theme = this.appearance.theme();
        cx.set_global(this.theme.clone());
        this.open_settings(cx);
    });
    cx.run_until_parked();
    let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
    let cx = VisualTestContext::from_window(handle.into(), cx);
    // Keep the draft and its actions in view; text metrics on the test platform are synthetic.
    cx.simulate_resize(size(px(760.0), px(1040.0)));
    cx.run_until_parked();
    (app, handle, cx)
}

#[gpui::test]
fn a_japanese_theme_is_committed_then_edited_without_losing_its_font(cx: &mut TestAppContext) {
    let (app, handle, mut cx) = open_appearance_settings(cx);
    let _restore = RestoreAppearance(Appearance::load());
    let cx = &mut cx;
    let before = app.read_with(cx, |this, _| this.appearance.clone());
    crate::harness::click("create-theme", cx);
    crate::harness::click("theme-name", cx);
    handle
        .update(cx, |this, window, cx| {
            // Exercise the platform's UTF-16 input boundary, not only direct field mutation.
            this.replace_and_mark_text_in_range(None, "宵の青", Some(3..3), window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    handle
        .update(cx, |this, _, _| {
            assert!(
                this.appearance_editor.is_some(),
                "IME confirmation does not save the draft"
            );
            assert_eq!(this.appearance, before);
        })
        .unwrap();
    cx.simulate_input("宵の青");
    cx.simulate_keystrokes("tab tab");
    handle
        .update(cx, |this, _, _| {
            assert!(this.readable_field().unwrap().content().starts_with('#'));
        })
        .unwrap();
    cx.simulate_keystrokes("secondary-a");
    cx.simulate_input("#60A5FA");
    crate::harness::click("save-theme", cx);
    cx.run_until_parked();

    let created = handle
        .update(cx, |this, _, _| {
            assert!(this.appearance_editor.is_none());
            assert_eq!(this.appearance.custom_schemes.len(), 1);
            let custom = &this.appearance.custom_schemes[0];
            assert_eq!(custom.name, "宵の青");
            assert_eq!(custom.accent, 0x60a5fa);
            assert_eq!(this.appearance.scheme, custom.id);
            assert_eq!(this.appearance.font_family, before.font_family);
            assert_eq!(this.theme.font.family.as_ref(), "Test Interface");
            this.appearance.clone()
        })
        .unwrap();
    app.read_with(cx, |this, cx| {
        assert_eq!(this.appearance, created);
        assert_eq!(this.theme.scheme, created.scheme);
        assert_eq!(cx.global::<Theme>().font.family.as_ref(), "Test Interface");
        assert_eq!(cx.global::<Theme>().accent, this.theme.accent);
    });

    crate::harness::click("edit-theme", cx);
    crate::harness::click("theme-name", cx);
    cx.simulate_keystrokes("secondary-a");
    cx.simulate_input("宵の琥珀");
    crate::harness::click("theme-accent", cx);
    cx.simulate_keystrokes("secondary-a");
    cx.simulate_input("#E09040");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let edited = app.read_with(cx, |this, _| this.appearance.clone());
    assert_eq!(
        edited.custom_schemes.len(),
        1,
        "editing replaces the existing theme"
    );
    assert_eq!(
        edited.scheme, created.scheme,
        "renaming keeps the stable identifier"
    );
    assert_eq!(edited.custom_schemes[0].name, "宵の琥珀");
    assert_eq!(edited.custom_schemes[0].accent, 0xe09040);
    assert_eq!(edited.font_family, before.font_family);
    handle
        .update(cx, |this, _, _| {
            assert!(this.appearance_editor.is_none());
            assert_eq!(this.appearance, edited);
            assert_eq!(this.theme.font.family.as_ref(), "Test Interface");
        })
        .unwrap();
    assert_eq!(
        Appearance::load(),
        edited,
        "the edited theme and font survive a reload"
    );
    handle
        .update(cx, |this, _, cx| {
            this.font_families = vec!["Another Interface".to_owned(), "Test Interface".to_owned()];
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    crate::harness::click("ui-font", cx);
    cx.simulate_keystrokes("home down enter");
    app.read_with(cx, |this, _| {
        assert_eq!(
            this.appearance.font_family.as_deref(),
            Some("Another Interface")
        );
        assert_eq!(this.appearance.custom_schemes, edited.custom_schemes);
        assert_eq!(this.theme.font.family.as_ref(), "Another Interface");
    });
    crate::harness::click("ui-font", cx);
    cx.simulate_keystrokes("home enter");
    assert_eq!(Appearance::load().font_family, None);
    app.read_with(cx, |this, _| {
        assert_eq!(this.theme.font.family, crate::theme::ui_font().family);
        assert_eq!(this.appearance.scheme, edited.scheme);
    });
}

#[gpui::test]
fn cancelling_a_theme_draft_preserves_the_applied_appearance(cx: &mut TestAppContext) {
    let (app, handle, mut cx) = open_appearance_settings(cx);
    let cx = &mut cx;
    let before = app.read_with(cx, |this, _| this.appearance.clone());
    for cancel in ["button", "escape"] {
        crate::harness::click("create-theme", cx);
        cx.simulate_input("保存しないテーマ");
        crate::harness::click("theme-accent", cx);
        cx.simulate_keystrokes("secondary-a");
        cx.simulate_input("#D07040");
        app.read_with(cx, |this, _| assert_eq!(this.appearance, before));
        if cancel == "button" {
            crate::harness::click("cancel-theme", cx);
        } else {
            cx.simulate_keystrokes("escape");
        }
        handle
            .update(cx, |this, _, _| {
                assert!(this.appearance_editor.is_none());
                assert_eq!(this.appearance, before);
            })
            .unwrap();
        app.read_with(cx, |this, _| assert_eq!(this.appearance, before));
    }
}

#[gpui::test]
fn invalid_theme_input_stays_open_for_correction(cx: &mut TestAppContext) {
    let (app, handle, mut cx) = open_appearance_settings(cx);
    let cx = &mut cx;
    let before = app.read_with(cx, |this, _| this.appearance.clone());
    crate::harness::click("create-theme", cx);
    cx.simulate_input("最初の入力");
    handle
        .update(cx, |this, _, _| {
            assert_eq!(this.readable_field().unwrap().content(), "最初の入力");
        })
        .unwrap();
    cx.simulate_keystrokes("secondary-a");
    handle
        .update(cx, |this, window, cx| {
            assert!(
                this.sync_editor_focus(window),
                "opening the editor focuses its name field"
            );
            // A paste arrives as one platform text replacement, including its line breaks.
            this.replace_text_in_range(None, "入力の\n確認", window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |this, _, _| {
            assert_eq!(this.readable_field().unwrap().content(), "入力の 確認");
        })
        .unwrap();
    crate::harness::click("theme-accent", cx);
    cx.simulate_keystrokes("secondary-a");
    cx.simulate_input("#12");
    crate::harness::click("save-theme", cx);
    handle
        .update(cx, |this, _, _| {
            assert!(this.appearance_editor.is_some());
            assert_eq!(this.appearance, before);
            assert_eq!(this.status, this.t(Key::ThemeAccentInvalid));
            assert_eq!(this.readable_field().unwrap().content(), "#12");
        })
        .unwrap();
    crate::harness::click("theme-accent", cx);
    cx.simulate_keystrokes("secondary-a");
    cx.simulate_input("#60A5FA");
    crate::harness::click("theme-name", cx);
    cx.simulate_keystrokes("secondary-a");
    cx.simulate_input(SCHEMES[0].name);
    crate::harness::click("save-theme", cx);
    handle
        .update(cx, |this, _, _| {
            assert!(this.appearance_editor.is_some());
            assert_eq!(this.appearance, before);
            assert_eq!(this.status, this.t(Key::ThemeNameExists));
        })
        .unwrap();
    app.read_with(cx, |this, _| assert_eq!(this.appearance, before));
}
