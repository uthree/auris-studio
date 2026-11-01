//! Local song-choice evidence, edited independently from model connection settings.
use super::*;
use atelier::Divider;
use auris_session::workshop::{self, Preferences, Selection};

#[derive(Default)]
pub(super) struct Personalization {
    preferences: Option<Preferences>,
    pub(super) busy: bool,
    confirming: bool,
    error: Option<String>,
    original: Option<Selection>,
    chosen: usize,
    pub(super) viewport: std::rc::Rc<std::cell::RefCell<crate::ui::text_area::ProseViewport>>,
}

impl SettingsWindow {
    pub(super) fn cancel_preference(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.personalization.original = None;
        self.personalization.error = None;
        self.agent.active = None;
        self.editing_search = true;
        window.focus(&self.search_focus);
        cx.notify();
    }
    fn preference_operation(
        &mut self,
        save: bool,
        operation: impl FnOnce() -> Result<Preferences, String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.personalization.busy || self.personalization.confirming {
            return;
        }
        self.personalization.busy = true;
        self.personalization.error = None;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { operation() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.personalization.busy = false;
                match result {
                    Ok(preferences) => {
                        this.personalization.preferences = Some(preferences);
                        this.personalization.original = None;
                        if save {
                            this.status = this.t(Key::PreferenceSaved).to_owned();
                            let _ = this.app.update(cx, |app, cx| {
                                if let Some(workshop) = &app.song_workshop {
                                    workshop
                                        .send(auris_agent::workshop::Command::ReloadPreferences);
                                }
                                cx.notify();
                            });
                        }
                    }
                    Err(error) => this.personalization.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn save_preference(&mut self, cx: &mut Context<Self>) {
        let Some(original) = self.personalization.original.clone() else {
            return;
        };
        let context = self.agent.fields[agent::AgentSettings::PREFERENCE_FIELD]
            .content()
            .to_owned();
        let chosen = self.personalization.chosen;
        if context.trim().is_empty() || chosen >= original.alternatives.len() {
            self.personalization.error = Some(self.t(Key::PreferenceInvalid).to_owned());
            cx.notify();
            return;
        }
        self.preference_operation(
            true,
            move || workshop::revise_preference(&original, Some((context, chosen))),
            cx,
        );
    }

    fn confirm_delete_preference(
        &mut self,
        selection: Option<Selection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.personalization.busy || self.personalization.confirming {
            return;
        }
        let title = self.t(if selection.is_some() {
            Key::PreferenceDeleteTitle
        } else {
            Key::PreferenceResetTitle
        });
        let detail = match &selection {
            Some(selection) => format!(
                "{}\n\n{}",
                selection.context,
                self.t(Key::PreferenceDeleteNote)
            ),
            None => self.t(Key::PreferenceResetQuestion).to_owned(),
        };
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            title,
            Some(&detail),
            &[
                gpui::PromptButton::cancel(self.t(Key::Cancel)),
                gpui::PromptButton::new(self.t(Key::MenuDelete)),
            ],
            cx,
        );
        self.personalization.confirming = true;
        cx.spawn(async move |this, cx| {
            let confirmed = matches!(answer.await, Ok(1));
            let _ = this.update(cx, |this, cx| {
                this.personalization.confirming = false;
                if confirmed {
                    this.preference_operation(
                        true,
                        move || match selection {
                            Some(original) => workshop::revise_preference(&original, None),
                            None => workshop::update_preferences(|p| p.selections.clear()),
                        },
                        cx,
                    );
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_personalization(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.personalization.preferences.is_none()
            && !self.personalization.busy
            && self.personalization.error.is_none()
        {
            self.preference_operation(false, workshop::load_preferences, cx);
        }
        let busy = self.personalization.busy || self.personalization.confirming;
        let editing = self.personalization.original.is_some();
        let spacing = self.theme.atelier.spacing;
        let mut body = div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .gap(spacing.xl)
            .child(
                SectionHeader::new(self.t(Key::PreferenceHeading))
                    .description(self.t(Key::PreferenceHelp))
                    .action(
                        Button::new("preferences-refresh", self.t(Key::PreferenceRefresh))
                            .ghost()
                            .disabled(busy || editing)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.preference_operation(false, workshop::load_preferences, cx)
                            })),
                    ),
            );
        if self.personalization.busy {
            body = body.child(note(
                self.t(if self.personalization.preferences.is_some() {
                    Key::PreferenceSaving
                } else {
                    Key::PreferenceLoading
                }),
                &self.theme,
            ));
        }
        if let Some(error) = &self.personalization.error {
            body = body.child(div().text_color(self.theme.danger).child(error.clone()));
        }
        let Some(preferences) = self.personalization.preferences.clone() else {
            return body.into_any_element();
        };
        body = body.child(
            FormGroup::untitled().child(
                FormField::new(self.t(Key::WorkshopMemory)).beside().child(
                    Switch::new("preferences-enabled", preferences.enabled)
                        .disabled(busy || editing)
                        .on_change(cx.listener(move |this, on: &bool, _, cx| {
                            let enabled = *on;
                            this.preference_operation(
                                true,
                                move || workshop::update_preferences(|p| p.enabled = enabled),
                                cx,
                            );
                        })),
                ),
            ),
        );
        if let Some(original) = self.personalization.original.clone() {
            body = body.child(self.render_preference_editor(&original, busy, cx));
        } else {
            body = body.child(self.render_preference_history(&preferences, busy, cx));
        }
        // The page's foot: the reset, a ghost at the leading end.
        body = body.child(
            div().flex().child(
                Button::new("preferences-reset", self.t(Key::WorkshopForget))
                    .ghost()
                    .disabled(busy || preferences.selections.is_empty() || editing)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.confirm_delete_preference(None, window, cx);
                    })),
            ),
        );
        body.into_any_element()
    }

    /// The correction form for one recorded choice: the request above, the alternative below,
    /// and Cancel and Save at the trailing end.
    fn render_preference_editor(
        &mut self,
        original: &Selection,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let choices = original
            .alternatives
            .iter()
            .enumerate()
            .map(|(i, d)| (i, d.title.clone(), d.description.clone()))
            .collect();
        let chosen = self.personalization.chosen;
        let dropdown = self.dropdown(
            "preference-chosen",
            choices,
            &chosen,
            |this, chosen, cx| {
                if !this.personalization.busy {
                    this.personalization.chosen = chosen;
                }
                cx.notify();
            },
            cx,
        );
        let prompt = self.agent_prompt(agent::AgentSettings::PREFERENCE_FIELD, cx);
        div()
            .flex()
            .flex_col()
            .gap(self.theme.atelier.spacing.md)
            .child(
                FormGroup::new(self.t(Key::PreferenceEdit))
                    .child(FormField::new(self.t(Key::PreferenceRequest)).child(prompt))
                    .child(FormField::new(self.t(Key::PreferenceChosen)).child(dropdown)),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(self.theme.atelier.spacing.sm)
                    .child(
                        Button::new("preference-cancel", self.t(Key::Cancel))
                            .ghost()
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.cancel_preference(window, cx)
                            })),
                    )
                    .child(
                        Button::new("preference-save", self.t(Key::PreferenceSave))
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_preference(cx);
                                if this.personalization.busy {
                                    window.focus(&this.search_focus);
                                }
                            })),
                    ),
            )
            .into_any_element()
    }

    /// The recorded choices, newest first, on one surface with a rule between entries.
    ///
    /// Each entry is a block rather than a `ListItem`: a request can run to several lines, and
    /// a list row has a fixed height, so a long one would spill over the entry below it.
    fn render_preference_history(
        &mut self,
        preferences: &Preferences,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &self.theme;
        let spacing = theme.atelier.spacing;
        let typography = &theme.atelier.typography;
        let mut group = FormGroup::new(self.t(Key::WorkshopMemoryView));
        if preferences.selections.is_empty() {
            return group
                .child(note(self.t(Key::PreferenceEmpty), theme))
                .into_any_element();
        }
        for (position, (index, selection)) in
            preferences.selections.iter().enumerate().rev().enumerate()
        {
            let chosen = selection.alternatives.get(selection.chosen);
            let original = selection.clone();
            let remove = original.clone();
            if position > 0 {
                group = group.child(Divider::horizontal());
            }
            let title = chosen
                .map(|d| d.title.clone())
                .unwrap_or_else(|| self.t(Key::PreferenceUnavailable).to_owned());
            let entry = div()
                .id(("preference-row", index))
                .flex()
                .flex_col()
                .min_w_0()
                .gap(spacing.xs)
                .child(
                    div()
                        .text_size(typography.size_md)
                        .font_weight(typography.label_weight)
                        .text_color(theme.text)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(typography.size_sm)
                        .text_color(theme.text_muted)
                        .child(selection.context.trim().to_owned()),
                )
                .children(
                    chosen
                        .filter(|d| !d.description.trim().is_empty())
                        .map(|d| {
                            div()
                                .text_size(typography.size_sm)
                                .text_color(theme.text_faint)
                                .child(d.description.trim().to_owned())
                        }),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(spacing.xs)
                        .pt(spacing.xxs)
                        .child(
                            Button::new(("preference-edit", index), self.t(Key::PreferenceEdit))
                                .ghost()
                                .size(ControlSize::Small)
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.agent.fields[agent::AgentSettings::PREFERENCE_FIELD] =
                                        TextField::new(original.context.clone());
                                    this.personalization.chosen = original.chosen;
                                    this.personalization.original = Some(original.clone());
                                    this.agent.active =
                                        Some(agent::AgentSettings::PREFERENCE_FIELD);
                                    this.editing_search = false;
                                    this.personalization.viewport.borrow_mut().follow_caret();
                                    window.focus(
                                        &this.agent.focus[agent::AgentSettings::PREFERENCE_FIELD],
                                    );
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new(
                                ("preference-remove", index),
                                self.t(Key::WorkshopForgetChoice),
                            )
                            .ghost()
                            .size(ControlSize::Small)
                            .disabled(busy)
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.confirm_delete_preference(
                                        Some(remove.clone()),
                                        window,
                                        cx,
                                    );
                                },
                            )),
                        ),
                );
            group = group.child(entry);
        }
        group.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    #[gpui::test]
    fn preference_history_can_be_corrected_removed_and_reset(cx: &mut TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        let original = Selection {
            conversation: "settings-test".into(),
            candidate: 1,
            round: 1,
            context: "A calm theme".into(),
            chosen: 0,
            alternatives: vec![
                workshop::Direction {
                    title: "Piano".into(),
                    description: "Sparse piano".into(),
                    instructions: "Compose piano".into(),
                    protected_tracks: vec![],
                },
                workshop::Direction {
                    title: "Strings".into(),
                    description: "Warm strings".into(),
                    instructions: "Compose strings".into(),
                    protected_tracks: vec![],
                },
            ],
        };
        workshop::update_preferences(|p| {
            *p = Preferences::default();
            p.record(original.clone());
        })
        .unwrap();
        app.update(cx, |this, cx| this.open_settings(cx));
        let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
        let cx = &mut VisualTestContext::from_window(handle.into(), cx);
        cx.simulate_resize(gpui::size(px(1200.0), px(1800.0)));
        crate::harness::click("tab-personalization", cx);
        cx.run_until_parked();
        crate::harness::click("preference-edit-0", cx);
        cx.simulate_keystrokes("secondary-a");
        cx.simulate_input("Discard this correction");
        cx.simulate_keystrokes("escape");
        handle
            .read_with(cx, |this, _| {
                assert!(this.personalization.original.is_none())
            })
            .unwrap();
        assert_eq!(
            workshop::load_preferences().unwrap().selections[0],
            original
        );
        crate::harness::click("preference-edit-0", cx);
        cx.simulate_keystrokes("secondary-a");
        cx.simulate_input("弦楽器のテーマ");
        cx.simulate_keystrokes("enter");
        cx.simulate_input("Keep it warm.");
        handle
            .update(cx, |this, _, cx| {
                this.preference_operation(true, || Err("Cannot write preferences".into()), cx);
            })
            .unwrap();
        cx.run_until_parked();
        handle
            .read_with(cx, |this, _| {
                assert!(this.personalization.original.is_some());
                assert!(this.personalization.error.is_some());
                assert_eq!(
                    this.agent.fields[agent::AgentSettings::PREFERENCE_FIELD].content(),
                    "弦楽器のテーマ\nKeep it warm."
                );
            })
            .unwrap();
        assert_eq!(
            workshop::load_preferences().unwrap().selections[0],
            original
        );
        crate::harness::click("preference-chosen", cx);
        cx.simulate_keystrokes("down enter");
        crate::harness::click("preference-save", cx);
        cx.run_until_parked();
        let preferences = workshop::load_preferences().unwrap();
        assert_eq!(
            preferences.selections[0].context,
            "弦楽器のテーマ\nKeep it warm."
        );
        assert_eq!(preferences.selections[0].chosen, 1);
        assert_eq!(
            preferences.selections[0].alternatives,
            original.alternatives
        );
        crate::harness::click("preferences-enabled", cx);
        cx.run_until_parked();
        assert!(!workshop::load_preferences().unwrap().enabled);
        let language = handle.read_with(cx, |this, _| this.language).unwrap();
        crate::harness::click("preferences-reset", cx);
        assert!(cx.has_pending_prompt());
        assert_eq!(workshop::load_preferences().unwrap().selections.len(), 1);
        cx.simulate_prompt_answer(Key::Cancel.get(language));
        cx.run_until_parked();
        assert_eq!(workshop::load_preferences().unwrap().selections.len(), 1);
        crate::harness::click("preferences-reset", cx);
        cx.simulate_prompt_answer(Key::MenuDelete.get(language));
        cx.run_until_parked();
        let preferences = workshop::load_preferences().unwrap();
        assert!(preferences.selections.is_empty());
        assert!(!preferences.enabled);
        // Reload a choice written by another surface and remove it individually.
        workshop::update_preferences(|p| p.selections.push(original.clone())).unwrap();
        crate::harness::click("preferences-refresh", cx);
        cx.run_until_parked();
        crate::harness::click("preference-remove-0", cx);
        assert!(cx.has_pending_prompt());
        assert!(cx.pending_prompt().unwrap().1.contains(&original.context));
        assert_eq!(
            workshop::load_preferences().unwrap().selections,
            vec![original.clone()]
        );
        cx.simulate_prompt_answer(Key::Cancel.get(language));
        cx.run_until_parked();
        assert_eq!(
            workshop::load_preferences().unwrap().selections,
            vec![original.clone()]
        );
        crate::harness::click("preference-remove-0", cx);
        cx.simulate_prompt_answer(Key::MenuDelete.get(language));
        cx.run_until_parked();
        assert!(workshop::load_preferences().unwrap().selections.is_empty());
        handle
            .read_with(cx, |this, _| assert!(this.personalization.error.is_none()))
            .unwrap();
    }
}
