//! Optional first-run guidance that opens the existing configuration windows.

use auris_i18n::{Key, Language};
use gpui::{
    Bounds, Context, FocusHandle, IntoElement, Render, WeakEntity, Window, WindowBounds,
    WindowHandle, WindowOptions, div, prelude::*, px, size,
};

use crate::ui::widgets::{ButtonState, ButtonStyle, Latch, button, button_enabled};
use crate::{
    app::AurisApp, settings_window::SettingsTab, theme::Theme, titlebar,
    voice_setup_window::VoiceSetupTab,
};

mod adapters;
use adapters::AdapterSetup;

const STEPS: &[Key] = &[
    Key::SetupAudio,
    Key::SetupAgent,
    Key::SetupInput,
    Key::SetupSinging,
    Key::SetupAppearance,
];

#[derive(Clone, Copy)]
enum Destination {
    Settings(SettingsTab),
    Voice(VoiceSetupTab),
}

/// A machine-setup guide independent of the open project and its undo history.
pub(crate) struct SetupGuide {
    app: WeakEntity<AurisApp>,
    theme: Theme,
    language: Language,
    selected_model: Option<String>,
    focus: FocusHandle,
    step: usize,
    error: Option<String>,
    adapters: AdapterSetup,
}

impl SetupGuide {
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.adapters.poll();
        if let Ok((theme, language, model)) = self.app.read_with(cx, |app, _| {
            (
                app.theme.clone(),
                app.language(),
                app.settings
                    .agent
                    .is_configured()
                    .then(|| app.settings.agent.model.clone()),
            )
        }) {
            self.theme = theme;
            self.language = language;
            self.selected_model = model;
            cx.notify();
        }
    }
    fn t(&self, key: Key) -> &'static str {
        key.get(self.language)
    }

    fn open(&mut self, destination: Destination, cx: &mut Context<Self>) {
        let _ = self.app.update(cx, |app, cx| match destination {
            Destination::Settings(tab) => app.open_settings_tab(tab, cx),
            Destination::Voice(tab) => app.open_voice_setup(tab, cx),
        });
    }

    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.remember_dismissal(cx) {
            window.remove_window();
        }
    }

    fn remember_dismissal(&mut self, cx: &mut Context<Self>) -> bool {
        match self.app.update(cx, |app, _| {
            let previous = app.settings.setup_guide_seen;
            app.settings.setup_guide_seen = true;
            if let Err(error) = app.save_settings() {
                app.settings.setup_guide_seen = previous;
                return Err(error.to_string());
            }
            Ok(())
        }) {
            Ok(Ok(())) => {
                self.adapters.cancel_all();
                true
            }
            Ok(Err(error)) => {
                self.error = Some(error);
                cx.notify();
                false
            }
            Err(_) => true,
        }
    }

    fn action(
        &self,
        id: &'static str,
        label: Key,
        destination: Destination,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        button(
            id,
            self.t(label),
            ButtonStyle::Normal,
            false,
            self.theme.accent,
            &self.theme,
            cx.listener(move |this, _, _, cx| this.open(destination, cx)),
        )
    }
}

impl Render for SetupGuide {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let mut sidebar = div()
            .w(px(190.))
            .flex_shrink_0()
            .p_4()
            .flex()
            .flex_col()
            .gap_2()
            .bg(theme.surface);
        for (index, key) in STEPS.iter().enumerate() {
            sidebar = sidebar.child(button(
                ("setup-step", index),
                format!("{}. {}", index + 1, self.t(*key)),
                ButtonStyle::Normal,
                self.step == index,
                theme.accent_soft,
                &theme,
                cx.listener(move |this, _, _, cx| {
                    this.step = index;
                    cx.notify();
                }),
            ));
        }
        let mut content = div()
            .id("setup-content")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .p_6()
            .flex()
            .flex_col()
            .gap_4()
            .child(div().text_lg().child(self.t(STEPS[self.step])))
            .when(self.step != 1, |this| {
                this.child(
                    div()
                        .text_color(theme.text_muted)
                        .child(self.t(Key::SetupOptional)),
                )
            });
        match self.step {
            0 => {
                content = content
                    .child(self.t(Key::SetupAudioHelp))
                    .child(self.action(
                        "setup-audio",
                        Key::SetupOpenAudio,
                        Destination::Settings(SettingsTab::Audio),
                        cx,
                    ));
            }
            1 => {
                content = content
                    .child(self.t(Key::SetupAgentHelp))
                    .child(self.adapter_controls(cx));
                if let Some(model) = &self.selected_model {
                    content = content.child(div().text_color(theme.text_muted).child(format!(
                        "{}: {}",
                        self.t(Key::SetupSelectedModel),
                        model
                    )));
                }
                content = content.child(self.action(
                    "setup-agent",
                    Key::SetupOpenAgent,
                    Destination::Settings(SettingsTab::Agent),
                    cx,
                ));
            }
            2 => {
                content = content
                    .child(self.t(Key::SetupInputNotes))
                    .child(self.t(Key::SetupInputTyping))
                    .child(self.t(Key::SetupInputFiles))
                    .child(self.action(
                        "setup-input-keys",
                        Key::SetupOpenKeys,
                        Destination::Settings(SettingsTab::Keys),
                        cx,
                    ));
            }
            3 => {
                content = content
                    .child(self.t(Key::SetupSingingHelp))
                    .child(self.action(
                        "setup-voicevox",
                        Key::BrowserSetupVoicevox,
                        Destination::Voice(VoiceSetupTab::Voicevox),
                        cx,
                    ))
                    .child(self.action(
                        "setup-diffsinger",
                        Key::BrowserSetupDiffSinger,
                        Destination::Voice(VoiceSetupTab::DiffSinger),
                        cx,
                    ))
                    .child(self.action(
                        "setup-leapsinger",
                        Key::BrowserSetupLeapSinger,
                        Destination::Voice(VoiceSetupTab::LeapSinger),
                        cx,
                    ));
            }
            _ => {
                content = content
                    .child(self.t(Key::SetupAppearanceHelp))
                    .child(self.action(
                        "setup-appearance",
                        Key::SetupOpenAppearance,
                        Destination::Settings(SettingsTab::General),
                        cx,
                    ))
                    .child(self.action(
                        "setup-keys",
                        Key::SetupOpenKeys,
                        Destination::Settings(SettingsTab::Keys),
                        cx,
                    ));
            }
        }
        content = content.child(
            div()
                .text_color(theme.text_muted)
                .child(self.t(Key::SetupReturnHint)),
        );
        if let Some(error) = &self.error {
            content = content.child(div().text_color(theme.danger).child(error.clone()));
        }
        let last = self.step == STEPS.len() - 1;
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.text)
            .font(theme.font.clone())
            .text_sm()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => this.finish(window, cx),
                    "tab" => {
                        if event.keystroke.modifiers.shift {
                            window.focus_prev();
                        } else {
                            window.focus_next();
                        }
                    }
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(
                titlebar::titlebar(window, &theme)
                    .child(
                        titlebar::drag_region("setup-title")
                            .flex_1()
                            .justify_center()
                            .child(self.t(Key::SetupGuide)),
                    )
                    .child(titlebar::controls(
                        window,
                        &theme,
                        cx.listener(|this, _, window, cx| this.finish(window, cx)),
                    )),
            )
            .child(
                div()
                    .p_4()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_lg().child(self.t(Key::SetupWelcome)))
                    .child(self.t(Key::SetupWelcomeHelp)),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(sidebar)
                    .child(content),
            )
            .child(
                div()
                    .p_4()
                    .border_t_1()
                    .border_color(theme.border)
                    .flex_shrink_0()
                    .flex()
                    .gap_2()
                    .child(button(
                        "setup-later",
                        self.t(Key::SetupLater),
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, window, cx| this.finish(window, cx)),
                    ))
                    .child(div().flex_1())
                    .child(button_enabled(
                        "setup-back",
                        self.t(Key::WorkshopBack),
                        ButtonStyle::Normal,
                        if self.step > 0 {
                            ButtonState::Enabled(Latch::Off)
                        } else {
                            ButtonState::Disabled
                        },
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.step = this.step.saturating_sub(1);
                            cx.notify();
                        }),
                    ))
                    .child(button(
                        "setup-next",
                        self.t(if last { Key::SetupDone } else { Key::SetupNext }),
                        ButtonStyle::Primary,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(move |this, _, window, cx| {
                            if last {
                                this.finish(window, cx);
                            } else {
                                this.step += 1;
                                cx.notify();
                            }
                        }),
                    )),
            )
            .children(titlebar::resize_handles(window))
    }
}

impl AurisApp {
    pub(crate) fn offer_initial_setup(&mut self, cx: &mut Context<Self>) {
        if !self.settings.setup_guide_seen {
            self.open_setup_guide(cx);
        }
    }

    pub(crate) fn open_setup_guide(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.setup_guide_window
            && handle
                .update(cx, |_, window, cx| {
                    window.activate_window();
                    cx.notify();
                })
                .is_ok()
        {
            return;
        }
        let app = cx.entity().downgrade();
        let theme = self.theme.clone();
        let language = self.language();
        let selected_model = self
            .settings
            .agent
            .is_configured()
            .then(|| self.settings.agent.model.clone());
        let bounds = Bounds::centered(None, size(px(760.), px(580.)), cx);
        let opened: Result<WindowHandle<SetupGuide>, _> = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(560.), px(420.))),
                focus: true,
                ..titlebar::options(self.t(Key::SetupGuide))
            },
            |window, cx| {
                let view = cx.new(|cx: &mut Context<SetupGuide>| {
                    let focus = cx.focus_handle();
                    focus.focus(window);
                    // Opening a native window renders synchronously while its owner is borrowed.
                    // Refresh the initial snapshots only after that outer update has returned.
                    cx.spawn(async move |this, cx| {
                        loop {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(250))
                                .await;
                            if this.update(cx, |guide, cx| guide.refresh(cx)).is_err() {
                                break;
                            }
                        }
                    })
                    .detach();
                    SetupGuide {
                        app,
                        theme,
                        language,
                        selected_model,
                        focus,
                        step: 0,
                        error: None,
                        adapters: AdapterSetup::new(),
                    }
                });
                let weak = view.downgrade();
                window.on_window_should_close(cx, move |_, cx| {
                    weak.update(cx, |guide, cx| guide.remember_dismissal(cx))
                        .unwrap_or(true)
                });
                view
            },
        );
        match opened {
            Ok(handle) => self.setup_guide_window = Some(handle),
            Err(error) => self.set_status(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {

    #[gpui::test]
    fn guide_routes_to_existing_settings_and_can_be_reopened_after_dismissal(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        let original_seen = app.read_with(cx, |app, _| app.settings.setup_guide_seen);
        let before = app.read_with(cx, |app, _| app.session.project().clone());
        app.update(cx, |app, cx| {
            app.settings.setup_guide_seen = false;
            // Model a first-run window even if another fixture has already dismissed its guide.
            app.settings_baseline.borrow_mut().setup_guide_seen = false;
            app.offer_initial_setup(cx);
        });
        let handle = app.read_with(cx, |app, _| app.setup_guide_window.unwrap());
        cx.run_until_parked();
        let guide_cx = &mut gpui::VisualTestContext::from_window(handle.into(), cx);
        guide_cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(1400.)));
        guide_cx.run_until_parked();
        crate::harness::click("setup-next", guide_cx);
        guide_cx.run_until_parked();
        crate::harness::click("setup-agent", guide_cx);
        app.read_with(guide_cx, |app, _| {
            assert!(app.settings_window.is_some());
            assert!(!app.settings.setup_guide_seen);
            assert_eq!(app.session.project(), &before);
        });
        handle
            .update(guide_cx, |guide, _, cx| {
                guide.step = 3;
                cx.notify();
            })
            .unwrap();
        guide_cx.run_until_parked();
        crate::harness::click("setup-voicevox", guide_cx);
        app.read_with(guide_cx, |app, _| assert!(app.voice_setup_window.is_some()));
        crate::harness::click("setup-later", guide_cx);
        guide_cx.run_until_parked();
        app.read_with(guide_cx, |app, _| {
            assert!(app.settings.setup_guide_seen);
            // A successful disk write advances this window's baseline. A separate read of the
            // shared settings file can instead see a later write from another window fixture.
            assert!(app.settings_baseline.borrow().setup_guide_seen);
            assert_eq!(app.session.project(), &before);
        });
        app.update(guide_cx, |app, cx| {
            app.setup_guide_window = None;
            app.offer_initial_setup(cx);
            assert!(app.setup_guide_window.is_none());
            app.open_setup_guide(cx);
            assert!(app.setup_guide_window.is_some());
            app.settings.setup_guide_seen = original_seen;
            app.save_settings().unwrap();
        });
    }
}
