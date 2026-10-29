//! Optional first-run guidance, drawn as a page of the start screen.
//!
//! The guide used to be a window of its own. It is now the chooser's third page, beside "New
//! project" and "Recent projects", so first launch shows one window rather than two and the
//! guide reads as part of starting rather than as a dialog in front of it. Each step still opens
//! the existing configuration windows, which close back onto the page.

use atelier::{Button, ButtonVariant, ControlSize, IconName, Label, SectionHeader, TextColor};
use auris_i18n::Key;
use gpui::{AnyElement, Context, IntoElement, Window, div, prelude::*};

use crate::app::AurisApp;
use crate::settings_window::SettingsTab;
use crate::ui::project_chooser::ChooserPage;
use crate::voice_setup_window::VoiceSetupTab;

/// The guide's steps, in order.
pub(crate) const STEPS: &[Key] = &[
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

impl AurisApp {
    /// Shows the guide on a first run, in place of the start screen's first page.
    pub(crate) fn offer_initial_setup(&mut self, cx: &mut Context<Self>) {
        if !self.settings.setup_guide_seen
            && let Some(chooser) = &mut self.project_chooser
        {
            chooser.page = ChooserPage::Setup;
            cx.notify();
        }
    }

    /// Brings the guide forward wherever the start screen is, opening one if none is.
    ///
    /// The settings window asks for it from an editor as well, where there is no start screen to
    /// switch: a project window that is showing one is reused, and otherwise a fresh start
    /// screen opens on the guide — the same screen a launch shows, never a separate guide window.
    pub(crate) fn open_setup_guide(&mut self, cx: &mut Context<Self>) {
        if self.song_workshop.is_none()
            && let Some(chooser) = &mut self.project_chooser
        {
            chooser.page = ChooserPage::Setup;
            let id = self.project_window;
            cx.notify();
            cx.defer(move |cx| {
                if let Some(handle) = cx.windows().into_iter().find(|h| Some(h.window_id()) == id) {
                    let _ = handle.update(cx, |_, window, _| window.activate_window());
                }
            });
            return;
        }
        cx.defer(crate::project_windows::open_setup_guide);
    }

    /// Leaves the guide for the first page and remembers that it was seen.
    pub(crate) fn dismiss_setup_guide(&mut self, cx: &mut Context<Self>) {
        let previous = self.settings.setup_guide_seen;
        self.settings.setup_guide_seen = true;
        let error = self.save_settings().err().map(|error| error.to_string());
        if let Some(chooser) = &mut self.project_chooser {
            match error {
                Some(error) => {
                    self.settings.setup_guide_seen = previous;
                    chooser.setup_error = Some(error);
                }
                None => {
                    chooser.setup_error = None;
                    chooser.page = ChooserPage::New;
                    chooser.setup_step = 0;
                }
            }
        }
        cx.notify();
    }

    /// Moves the guide one step back or forward; forward from the last step finishes it.
    pub(crate) fn step_setup_guide(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(chooser) = &mut self.project_chooser else {
            return;
        };
        if !forward {
            chooser.setup_step = chooser.setup_step.saturating_sub(1);
        } else if chooser.setup_step + 1 < STEPS.len() {
            chooser.setup_step += 1;
        } else {
            self.dismiss_setup_guide(cx);
            return;
        }
        cx.notify();
    }

    fn open_setup_destination(&mut self, destination: Destination, cx: &mut Context<Self>) {
        match destination {
            Destination::Settings(tab) => self.open_settings_tab(tab, cx),
            Destination::Voice(tab) => self.open_voice_setup(tab, cx),
        }
    }

    fn setup_action(
        &self,
        id: &'static str,
        label: Key,
        destination: Destination,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        div()
            .debug_selector(move || id.into())
            .child(Button::new(id, self.t(label)).on_click(
                cx.listener(move |this, _, _, cx| this.open_setup_destination(destination, cx)),
            ))
    }

    /// The guide's page: a heading, the row of steps, and the current step's advice.
    pub(crate) fn render_setup_page(&self, step: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let spacing = theme.atelier.spacing;
        let steps =
            div()
                .flex()
                .flex_wrap()
                .gap(spacing.xs)
                .children(STEPS.iter().enumerate().map(|(index, key)| {
                    div()
                        .debug_selector(move || format!("setup-step-{index}"))
                        .child(
                            Button::new(
                                ("setup-step", index),
                                format!("{}. {}", index + 1, self.t(*key)),
                            )
                            .variant(ButtonVariant::Ghost)
                            .size(ControlSize::Small)
                            .selected(step == index)
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if let Some(chooser) = &mut this.project_chooser {
                                        chooser.setup_step = index;
                                    }
                                    cx.notify();
                                },
                            )),
                        )
                }));
        let paragraph = |key: Key| {
            Label::new(self.t(key))
                .color(TextColor::Muted)
                .into_any_element()
        };
        let mut detail = div().flex().flex_col().gap(spacing.md).child(
            Label::new(self.t(STEPS[step]))
                .size(atelier::TextSize::Large)
                .heading_weight(),
        );
        let mut actions = div().flex().flex_wrap().gap(spacing.sm);
        match step {
            0 => {
                detail = detail.child(paragraph(Key::SetupAudioHelp));
                actions = actions.child(self.setup_action(
                    "setup-audio",
                    Key::SetupOpenAudio,
                    Destination::Settings(SettingsTab::Audio),
                    cx,
                ));
            }
            1 => {
                detail = detail.child(paragraph(Key::SetupAgentHelp));
                if self.settings.agent.is_configured() {
                    detail = detail.child(
                        Label::new(format!(
                            "{}: {}",
                            self.t(Key::SetupSelectedModel),
                            self.settings.agent.model
                        ))
                        .size(atelier::TextSize::Small)
                        .color(TextColor::Faint),
                    );
                }
                actions = actions.child(self.setup_action(
                    "setup-agent",
                    Key::SetupOpenAgent,
                    Destination::Settings(SettingsTab::Agent),
                    cx,
                ));
            }
            2 => {
                detail = detail
                    .child(paragraph(Key::SetupInputNotes))
                    .child(paragraph(Key::SetupInputTyping))
                    .child(paragraph(Key::SetupInputFiles));
                actions = actions.child(self.setup_action(
                    "setup-input-keys",
                    Key::SetupOpenKeys,
                    Destination::Settings(SettingsTab::Keys),
                    cx,
                ));
            }
            3 => {
                detail = detail.child(paragraph(Key::SetupSingingHelp));
                actions = actions
                    .child(self.setup_action(
                        "setup-voicevox",
                        Key::BrowserSetupVoicevox,
                        Destination::Voice(VoiceSetupTab::Voicevox),
                        cx,
                    ))
                    .child(self.setup_action(
                        "setup-diffsinger",
                        Key::BrowserSetupDiffSinger,
                        Destination::Voice(VoiceSetupTab::DiffSinger),
                        cx,
                    ))
                    .child(self.setup_action(
                        "setup-leapsinger",
                        Key::BrowserSetupLeapSinger,
                        Destination::Voice(VoiceSetupTab::LeapSinger),
                        cx,
                    ));
            }
            _ => {
                detail = detail.child(paragraph(Key::SetupAppearanceHelp));
                actions = actions
                    .child(self.setup_action(
                        "setup-appearance",
                        Key::SetupOpenAppearance,
                        Destination::Settings(SettingsTab::General),
                        cx,
                    ))
                    .child(self.setup_action(
                        "setup-keys",
                        Key::SetupOpenKeys,
                        Destination::Settings(SettingsTab::Keys),
                        cx,
                    ));
            }
        }
        let error = self
            .project_chooser
            .as_ref()
            .and_then(|chooser| chooser.setup_error.clone());
        div()
            .flex()
            .flex_col()
            .gap(spacing.xl)
            .child(
                SectionHeader::new(self.t(Key::SetupWelcome))
                    .description(self.t(Key::SetupWelcomeHelp)),
            )
            .child(steps)
            .child(detail.child(actions))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(spacing.xs)
                    .child(
                        Label::new(self.t(Key::SetupOptional))
                            .size(atelier::TextSize::Small)
                            .color(TextColor::Faint),
                    )
                    .child(
                        Label::new(self.t(Key::SetupReturnHint))
                            .size(atelier::TextSize::Small)
                            .color(TextColor::Faint),
                    ),
            )
            .children(error.map(|error| {
                div()
                    .flex()
                    .items_center()
                    .gap(spacing.xs)
                    .text_color(theme.danger)
                    .child(atelier::Icon::new(IconName::Alert).color(theme.danger))
                    .child(error)
            }))
            .into_any_element()
    }

    /// The footer commands while the guide is showing.
    pub(crate) fn render_setup_footer(
        &self,
        step: usize,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let last = step + 1 == STEPS.len();
        vec![
            div()
                .debug_selector(|| "setup-later".into())
                .child(
                    Button::new("setup-later", self.t(Key::SetupLater))
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| this.dismiss_setup_guide(cx))),
                )
                .into_any_element(),
            div().flex_1().into_any_element(),
            div()
                .debug_selector(|| "setup-back".into())
                .child(
                    Button::new("setup-back", self.t(Key::WorkshopBack))
                        .icon(IconName::ChevronLeft)
                        .disabled(step == 0)
                        .on_click(cx.listener(|this, _, _, cx| this.step_setup_guide(false, cx))),
                )
                .into_any_element(),
            div()
                .debug_selector(|| "setup-next".into())
                .child(
                    Button::new(
                        "setup-next",
                        self.t(if last { Key::SetupDone } else { Key::SetupNext }),
                    )
                    .primary()
                    .when(!last, |button| button.icon_after(IconName::ChevronRight))
                    .on_click(cx.listener(|this, _, _, cx| this.step_setup_guide(true, cx))),
                )
                .into_any_element(),
        ]
    }

    /// Escape on the guide: leave it as Set up later would.
    pub(crate) fn setup_escape(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_setup_guide(cx);
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::project_chooser::ChooserPage;

    #[gpui::test]
    fn guide_is_a_page_of_the_start_screen_and_routes_to_existing_settings(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        let original_seen = app.read_with(cx, |app, _| app.settings.setup_guide_seen);
        let before = app.read_with(cx, |app, _| app.session.project().clone());
        let windows = cx.update(|_, cx| cx.windows().len());
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.settings.setup_guide_seen = false;
                // Model a first-run window even if another fixture has already dismissed it.
                app.settings_baseline.borrow_mut().setup_guide_seen = false;
                app.show_project_chooser(window, cx);
                app.offer_initial_setup(cx);
            })
        });
        crate::harness::paint(&app, cx);
        app.read_with(cx, |app, _| {
            assert_eq!(
                app.project_chooser.as_ref().unwrap().page,
                ChooserPage::Setup
            );
        });
        assert_eq!(
            cx.update(|_, cx| cx.windows().len()),
            windows,
            "the guide opens no window of its own"
        );

        crate::harness::click("setup-next", cx);
        crate::harness::paint(&app, cx);
        crate::harness::click("setup-agent", cx);
        app.read_with(cx, |app, _| {
            assert!(app.settings_window.is_some());
            assert!(!app.settings.setup_guide_seen);
            assert_eq!(app.session.project(), &before);
        });
        app.update(cx, |app, cx| {
            app.project_chooser.as_mut().unwrap().setup_step = 3;
            cx.notify();
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("setup-voicevox", cx);
        app.read_with(cx, |app, _| assert!(app.voice_setup_window.is_some()));

        crate::harness::click("setup-later", cx);
        crate::harness::paint(&app, cx);
        app.read_with(cx, |app, _| {
            assert!(app.settings.setup_guide_seen);
            assert!(app.settings_baseline.borrow().setup_guide_seen);
            assert_eq!(app.project_chooser.as_ref().unwrap().page, ChooserPage::New);
            assert_eq!(app.session.project(), &before);
        });

        // Seen once, it is not offered again, but the sidebar still reaches it.
        app.update(cx, |app, cx| app.offer_initial_setup(cx));
        app.read_with(cx, |app, _| {
            assert_eq!(app.project_chooser.as_ref().unwrap().page, ChooserPage::New);
        });
        crate::harness::click("chooser-setup", cx);
        app.update(cx, |app, _| {
            assert_eq!(
                app.project_chooser.as_ref().unwrap().page,
                ChooserPage::Setup
            );
            app.settings.setup_guide_seen = original_seen;
            app.save_settings().unwrap();
        });
    }
}
