//! Connection and generation preferences in the application settings window.
use super::*;

pub(super) struct AgentSettings {
    prefs: AgentPreferences,
    pub(super) fields: [TextField; 2],
    pub(super) focus: [FocusHandle; 2],
    pub(super) active: Option<usize>,
}

impl AgentSettings {
    pub(super) fn new(prefs: AgentPreferences, cx: &mut Context<SettingsWindow>) -> Self {
        Self {
            fields: [
                TextField::new(prefs.url.clone()),
                TextField::new(prefs.api_key_env.clone()),
            ],
            focus: [
                cx.focus_handle().tab_stop(true),
                cx.focus_handle().tab_stop(true),
            ],
            active: None,
            prefs,
        }
    }
}

impl SettingsWindow {
    fn apply_agent(&mut self, cx: &mut Context<Self>) {
        self.agent.prefs.url = self.agent.fields[0].content().trim().to_owned();
        self.agent.prefs.api_key_env = self.agent.fields[1].content().trim().to_owned();
        let draft = self.agent.prefs.clone();
        let result = self.app.update(cx, |app, cx| {
            // Model and live permission controls belong to the panel and may have changed
            // since this settings window opened. Never overwrite them with this draft.
            let mut prefs = app.settings.agent.clone();
            prefs.provider = draft.provider;
            prefs.url = draft.url;
            prefs.api_key_env = draft.api_key_env;
            prefs.context_tokens = draft.context_tokens;
            prefs.output_tokens = draft.output_tokens;
            prefs.thinking = draft.thinking;
            app.agent_chat.load_preferences(&prefs);
            app.agent_apply_settings();
            app.agent_chat.models.clear();
            app.agent_chat.models_error = None;
            app.agent_chat.models_rx = None;
            app.agent_chat.fetching_models = false;
            cx.notify();
        });
        self.status = match result {
            Ok(()) => self.t(Key::AgentSettingsApplied).to_owned(),
            Err(error) => error.to_string(),
        };
        cx.notify();
    }

    pub(super) fn agent_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let Some(index) = self.agent.active else {
            return false;
        };
        if event.keystroke.key == "enter" && self.agent.fields[index].marked().is_none() {
            self.apply_agent(cx);
            return true;
        }
        self.agent.fields[index].apply_key_with_clipboard(
            &event.keystroke.key,
            event.keystroke.modifiers.shift,
            event.keystroke.modifiers.secondary(),
            false,
            cx,
        ) != KeyEffect::Ignored
    }

    fn agent_field(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let id = ["agent-url", "agent-key-env"][index];
        let focus = self.agent.focus[index].clone();
        let field = &self.agent.fields[index];
        div()
            .id(id)
            .debug_selector(move || id.to_owned())
            .track_focus(&focus)
            .tab_index(0)
            .w_full()
            .min_w_0()
            .h(px(30.0))
            .bg(self.theme.surface_sunken)
            .border_1()
            .rounded(Metrics::RADIUS_SM)
            .border_color(self.theme.border)
            .focus(|el| el.border_color(self.theme.accent))
            .child(crate::ui::prompt::editable_text(
                field.content().to_owned().into(),
                field.selection(),
                field.marked(),
                focus.clone(),
                cx.entity(),
                self.theme.clone(),
            ))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.agent.active = Some(index);
                    this.editing_search = false;
                    window.focus(&focus);
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    pub(super) fn render_agent(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let provider = self.agent.prefs.provider.clone();
        let provider = if provider.is_empty() {
            "ollama".to_owned()
        } else {
            provider
        };
        let provider_control = self.dropdown(
            "agent-provider",
            vec![
                ("ollama".to_owned(), "Ollama".to_owned(), String::new()),
                (
                    "openai".to_owned(),
                    "OpenAI compatible".to_owned(),
                    String::new(),
                ),
            ],
            &provider,
            |this, value, cx| {
                this.agent.prefs.provider = value;
                cx.notify();
            },
            cx,
        );
        let mut rows = vec![
            section_title(self.t(Key::AgentProviderLabel), &theme),
            provider_control,
            section_title(self.t(Key::AgentUrlLabel), &theme),
            self.agent_field(0, cx),
            section_title(self.t(Key::AgentKeyEnvLabel), &theme),
            self.agent_field(1, cx),
            note(self.t(Key::AgentModelInPanel), &theme),
        ];
        if provider == "ollama" {
            let context = self.agent.prefs.context_tokens.unwrap_or(32768);
            let output = self.agent.prefs.output_tokens.unwrap_or(4096);
            rows.push(section_title(self.t(Key::AgentContextTokens), &theme));
            rows.push(
                self.dropdown(
                    "agent-context-tokens",
                    [32768, 65536, 131072, 262144]
                        .into_iter()
                        .map(|n| (n, format!("{}K", n / 1024), String::new()))
                        .collect(),
                    &context,
                    |this, value, cx| {
                        this.agent.prefs.context_tokens = Some(value);
                        this.agent.prefs.output_tokens = Some(
                            this.agent
                                .prefs
                                .output_tokens
                                .unwrap_or(4096)
                                .min(value / 4),
                        );
                        cx.notify();
                    },
                    cx,
                ),
            );
            rows.push(section_title(self.t(Key::AgentOutputTokens), &theme));
            rows.push(
                self.dropdown(
                    "agent-output-tokens",
                    [4096, 8192, 16384, 32768, 65536]
                        .into_iter()
                        .map(|n| (n, format!("{}K", n / 1024), String::new()))
                        .collect(),
                    &output,
                    |this, value, cx| {
                        this.agent.prefs.output_tokens = Some(value);
                        if value >= this.agent.prefs.context_tokens.unwrap_or(32768) / 2 {
                            this.agent.prefs.context_tokens = Some(value * 4);
                        }
                        cx.notify();
                    },
                    cx,
                ),
            );
            rows.push(note(self.t(Key::AgentOutputTokensHelp), &theme));
            rows.push(section_title(self.t(Key::AgentThinking), &theme));
            let thinking = self.agent.prefs.thinking;
            let choices = [
                (None, Key::AgentThinkingAuto),
                (Some(false), Key::AgentThinkingOff),
                (Some(true), Key::AgentThinkingOn),
            ]
            .into_iter()
            .map(|(value, key)| (value, self.t(key).to_owned(), String::new()))
            .collect();
            rows.push(self.dropdown(
                "agent-thinking",
                choices,
                &thinking,
                |this, value, cx| {
                    this.agent.prefs.thinking = value;
                    cx.notify();
                },
                cx,
            ));
        }
        rows.push(
            button(
                "agent-apply",
                self.t(Key::AgentApply),
                ButtonStyle::Normal,
                false,
                theme.accent,
                &theme,
                cx.listener(|this, _, _, cx| this.apply_agent(cx)),
            )
            .into_any_element(),
        );
        div()
            .flex()
            .flex_col()
            .gap_2()
            .children(rows)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    #[gpui::test]
    fn connection_settings_move_out_of_the_panel_and_preserve_live_model_changes(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.settings.agent = AgentPreferences {
                model: "first".into(),
                url: "http://127.0.0.1:1".into(),
                ..Default::default()
            };
            this.agent_chat
                .load_preferences(&this.settings.agent.clone());
            this.agent_chat.models_error = Some("offline fixture".into());
            this.panels.show(crate::dock::Panel::Agent);
            this.open_settings(cx);
        });
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("agent-model").is_some());
        assert!(cx.debug_bounds("agent-url").is_none());
        assert!(cx.debug_bounds("agent-provider").is_none());
        let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
        let cx = &mut VisualTestContext::from_window(handle.into(), cx);
        cx.simulate_resize(gpui::size(px(760.0), px(1200.0)));
        cx.run_until_parked();
        crate::harness::click("tab-agent", cx);
        crate::harness::click("agent-url", cx);
        cx.update(|_, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string("http://127.0.0.1:2".into()))
        });
        cx.simulate_keystrokes("secondary-a secondary-v");
        crate::harness::click("agent-key-env", cx);
        cx.simulate_input("OLLAMA_KEY");
        crate::harness::click("agent-output-tokens", cx);
        cx.simulate_keystrokes("down enter");
        app.update(cx, |this, _| {
            this.settings.agent.model = "second".into();
            this.settings.agent.policy.mode = auris_session::agent_policy::Mode::Plan;
            this.agent_chat.busy = true;
        });
        crate::harness::click("agent-apply", cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.settings.agent.model, "second");
            assert_eq!(this.settings.agent.url, "http://127.0.0.1:2");
            assert_eq!(this.settings.agent.api_key_env, "OLLAMA_KEY");
            assert_eq!(this.settings.agent.output_tokens, Some(8192));
            assert_eq!(
                this.settings.agent.policy.mode,
                auris_session::agent_policy::Mode::Plan
            );
            assert!(
                this.agent_chat.busy,
                "applying settings keeps the current turn running"
            );
            assert_eq!(this.agent_chat.chosen_model, "second");
        });
        crate::harness::click("settings-search", cx);
        cx.simulate_input("出力トークン");
        cx.run_until_parked();
        assert!(cx.debug_bounds("agent-output-tokens").is_some());
        assert!(cx.debug_bounds("agent-model").is_none());
        handle
            .update(cx, |this, _, _| {
                assert_eq!(this.search.content(), "出力トークン")
            })
            .unwrap();
    }
}
