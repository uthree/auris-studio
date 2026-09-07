//! Searchable settings sections, shared by filtering and the cross-page results.

use super::*;
use crate::dock::{Dock, Panel};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Section {
    Appearance,
    Language,
    Pointer,
    Autosave,
    Snap,
    Dictionary,
    Singer,
    Panels,
    Host,
    Output,
    Input,
    Rate,
    Buffer,
    Export,
}

impl Section {
    const ALL: [Self; 14] = [
        Self::Appearance,
        Self::Language,
        Self::Pointer,
        Self::Autosave,
        Self::Snap,
        Self::Dictionary,
        Self::Singer,
        Self::Panels,
        Self::Host,
        Self::Output,
        Self::Input,
        Self::Rate,
        Self::Buffer,
        Self::Export,
    ];

    fn tab(self) -> SettingsTab {
        match self {
            Self::Host | Self::Output | Self::Input | Self::Rate | Self::Buffer | Self::Export => {
                SettingsTab::Audio
            }
            _ => SettingsTab::General,
        }
    }

    fn keys(self) -> &'static [Key] {
        match self {
            Self::Appearance => &[
                Key::AppearanceHeading,
                Key::UiFont,
                Key::UiFontNote,
                Key::ThemeName,
                Key::ThemeBase,
                Key::ThemeAccent,
            ],
            Self::Language => &[
                Key::LanguageHeading,
                Key::LanguageNote,
                Key::LanguageFollowSystem,
            ],
            Self::Pointer => &[
                Key::PointerHeading,
                Key::PointerCreate,
                Key::PointerDelete,
                Key::PointerNote,
            ],
            Self::Autosave => &[Key::Autosave, Key::AutosaveNote],
            Self::Snap => &[Key::SnapNoteLengths, Key::SnapNoteLengthsNote],
            Self::Dictionary => &[Key::JapaneseDictionaryHeading, Key::JapaneseDictionaryNote],
            Self::Singer => &[
                Key::SingerComputeHeading,
                Key::SingerComputeNote,
                Key::SingerComputeAuto,
            ],
            Self::Panels => &[
                Key::PanelPositions,
                Key::PanelPositionsNote,
                Key::Library,
                Key::PianoRoll,
                Key::Mixer,
                Key::Inspector,
                Key::LogPanel,
                Key::AgentPanel,
            ],
            Self::Host => &[Key::AudioHost, Key::RefreshAudioDevices],
            Self::Output => &[Key::OutputDevice],
            Self::Input => &[Key::InputDevice, Key::InputDeviceNote, Key::AsioInputNote],
            Self::Rate => &[Key::SampleRate, Key::DeviceDefaultRate],
            Self::Buffer => &[Key::BufferSize, Key::RequestedBufferNote],
            Self::Export => &[
                Key::ExportFormat,
                Key::ExportRate,
                Key::ExportDither,
                Key::ExportDitherNote,
            ],
        }
    }
}

fn matches(query: &str, keys: &[Key]) -> bool {
    let haystack = keys
        .iter()
        .flat_map(|key| Language::ALL.map(|language| key.get(language)))
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    query
        .split_whitespace()
        .all(|word| haystack.contains(&word.to_lowercase()))
}

impl SettingsWindow {
    pub(super) fn searching(&self) -> bool {
        !self.search.content().trim().is_empty()
    }

    pub(super) fn matches_section(&self, section: Section) -> bool {
        if section == Section::Panels {
            Panel::ALL
                .into_iter()
                .any(|panel| self.matches_panel(panel))
        } else {
            matches(self.search.content(), section.keys())
        }
    }

    fn matches_panel(&self, panel: Panel) -> bool {
        matches(
            self.search.content(),
            &[Key::PanelPositions, Key::PanelPositionsNote, panel.label()],
        )
    }

    fn found_sections(&self, tab: SettingsTab) -> bool {
        Section::ALL
            .into_iter()
            .any(|section| section.tab() == tab && self.matches_section(section))
    }

    pub(super) fn render_results(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let mut rows = Vec::new();
        for tab in [SettingsTab::General, SettingsTab::Audio, SettingsTab::Keys] {
            let found = if tab == SettingsTab::Keys {
                !self.found_commands().is_empty()
            } else {
                self.found_sections(tab)
            };
            if !found {
                continue;
            }
            let title = match tab {
                SettingsTab::General => Key::TabGeneral,
                SettingsTab::Audio => Key::TabAudio,
                SettingsTab::Keys => Key::TabKeys,
            };
            rows.push(section_title(self.t(title), &self.theme));
            rows.push(match tab {
                SettingsTab::General => self.render_general(cx),
                SettingsTab::Audio => self.render_audio(cx),
                SettingsTab::Keys => self.render_keys(cx),
            });
        }
        if rows.is_empty() {
            rows.push(
                div()
                    .debug_selector(|| "settings-no-results".to_owned())
                    .child(note(self.t(Key::NoSettingsMatch), &self.theme))
                    .into_any_element(),
            );
        }
        div()
            .id("settings-results")
            .flex()
            .flex_col()
            .gap_2()
            .children(rows)
            .into_any_element()
    }

    pub(super) fn render_panel_positions(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let mut rows = vec![
            section_title(self.t(Key::PanelPositions), &theme),
            note(self.t(Key::PanelPositionsNote), &theme),
        ];
        for panel in Panel::ALL {
            if !self.matches_panel(panel) {
                continue;
            }
            // Read the current placement so changes made through the main window's panel menu
            // also appear here. Moving a panel uses the same persistence path as that menu.
            let current = self.panels.dock(panel);
            let choices = Dock::ALL
                .into_iter()
                .map(|dock| (dock, self.t(dock.label()).to_owned(), String::new()))
                .collect();
            let control = self.dropdown(
                panel.command(),
                choices,
                &current,
                move |this, dock, cx| {
                    let _ = this.app.update(cx, |app, cx| {
                        if app.panels.dock(panel) != dock {
                            app.dock_panel(panel, dock);
                            cx.notify();
                        }
                    });
                    cx.notify();
                },
                cx,
            );
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(160.0))
                            .flex_shrink_0()
                            .text_xs()
                            .child(self.t(panel.label())),
                    )
                    .child(div().flex_1().min_w_0().child(control))
                    .into_any_element(),
            );
        }
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
    use gpui::{Entity, EntityInputHandler, TestAppContext, VisualTestContext, WindowHandle};

    fn open_settings(
        cx: &mut TestAppContext,
    ) -> (
        Entity<AurisApp>,
        WindowHandle<SettingsWindow>,
        VisualTestContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.panels = PanelLayout::default();
            this.keymap = Keymap::default();
            this.open_settings(cx);
        });
        cx.run_until_parked();
        let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
        let cx = VisualTestContext::from_window(handle.into(), cx);
        cx.simulate_resize(gpui::size(px(800.0), px(900.0)));
        cx.run_until_parked();
        (app, handle, cx)
    }

    struct RestoreLayout(PanelLayout);

    impl Drop for RestoreLayout {
        fn drop(&mut self) {
            self.0.save().expect("restore isolated panel preferences");
        }
    }

    #[gpui::test]
    fn search_uses_ime_and_finds_controls_on_other_tabs(cx: &mut TestAppContext) {
        let (_, handle, mut cx) = open_settings(cx);
        let cx = &mut cx;
        crate::harness::click("settings-search", cx);
        handle
            .update(cx, |this, window, cx| {
                this.replace_and_mark_text_in_range(None, "サンプル", Some(4..4), window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        cx.simulate_input("サンプル");
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("sample-rate").is_some(),
            "audio setting appears on the general tab"
        );
        handle
            .update(cx, |this, _, _| {
                assert_eq!(this.search.content(), "サンプル");
                assert_eq!(this.tab, SettingsTab::General);
                assert!(!this.matches_section(Section::Language));
            })
            .unwrap();

        cx.simulate_keystrokes("secondary-a");
        cx.simulate_input("unfindable-setting-987654");
        cx.run_until_parked();
        assert!(cx.debug_bounds("settings-no-results").is_some());
        handle
            .update(cx, |this, _, _| {
                assert!(!this.found_sections(SettingsTab::General));
                assert!(!this.found_sections(SettingsTab::Audio));
                assert!(this.found_commands().is_empty());
            })
            .unwrap();
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(cx.debug_bounds("language").is_some());

        crate::harness::click("tab-audio", cx);
        cx.simulate_input("UI font");
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("create-theme").is_some(),
            "general settings appear on the audio tab"
        );
        crate::harness::click("clear-settings-search", cx);
        cx.run_until_parked();
        assert!(cx.debug_bounds("audio-host").is_some());
    }

    #[gpui::test]
    fn search_and_a_theme_draft_keep_separate_input(cx: &mut TestAppContext) {
        let (_, handle, mut cx) = open_settings(cx);
        let cx = &mut cx;
        crate::harness::click("tab-audio", cx);
        cx.simulate_input("font");
        cx.run_until_parked();
        crate::harness::click("create-theme", cx);
        cx.simulate_input("下書き");
        cx.run_until_parked();
        crate::harness::click("settings-search", cx);
        cx.simulate_keystrokes("secondary-a");
        cx.simulate_input("mixer");
        cx.run_until_parked();
        handle
            .update(cx, |this, _, _| {
                assert_eq!(this.search.content(), "mixer");
                assert_eq!(
                    this.appearance_editor
                        .as_ref()
                        .unwrap()
                        .readable_field()
                        .content(),
                    "下書き"
                );
            })
            .unwrap();
        assert!(cx.debug_bounds("view.mixer").is_some());
        handle
            .update(cx, |this, _, _| {
                assert!(this.matches_panel(Panel::Mixer));
                assert!(!this.matches_panel(Panel::Library));
            })
            .unwrap();
        crate::harness::click("tab-general", cx);
        crate::harness::click("theme-name", cx);
        cx.simulate_input("の続き");
        handle
            .update(cx, |this, _, _| {
                assert_eq!(this.search.content(), "");
                assert_eq!(
                    this.appearance_editor
                        .as_ref()
                        .unwrap()
                        .readable_field()
                        .content(),
                    "下書きの続き"
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn key_capture_in_search_results_does_not_edit_the_query(cx: &mut TestAppContext) {
        let (_, handle, mut cx) = open_settings(cx);
        let cx = &mut cx;
        crate::harness::click("settings-search", cx);
        cx.simulate_input("Play / Stop");
        cx.run_until_parked();
        crate::harness::click("bind:transport.play:0", cx);
        handle
            .update(cx, |this, _, _| {
                assert!(this.capturing.is_some());
                assert!(this.field().is_none());
            })
            .unwrap();
        cx.simulate_keystrokes("escape");
        handle
            .update(cx, |this, _, _| {
                assert!(this.capturing.is_none());
                assert_eq!(this.search.content(), "Play / Stop");
            })
            .unwrap();
    }

    #[gpui::test]
    fn every_panel_can_move_from_search_and_keeps_its_position(cx: &mut TestAppContext) {
        let (app, handle, mut cx) = open_settings(cx);
        let _restore = RestoreLayout(PanelLayout::load());
        let cx = &mut cx;
        crate::harness::click("settings-search", cx);
        cx.simulate_input("panel positions");
        cx.run_until_parked();
        for panel in Panel::ALL {
            for dock in Dock::ALL {
                crate::harness::click(panel.command(), cx);
                cx.simulate_keystrokes(match dock {
                    Dock::Left => "home enter",
                    Dock::Bottom => "home down enter",
                    Dock::Right => "end enter",
                });
                cx.run_until_parked();
                app.read_with(cx, |this, _| assert_eq!(this.panels.dock(panel), dock));
                handle
                    .update(cx, |this, _, _| assert_eq!(this.panels.dock(panel), dock))
                    .unwrap();
                // The first choice may already be selected, in which case it need not write.
                if dock != Dock::Left {
                    assert_eq!(PanelLayout::load().dock(panel), dock);
                }
            }
        }
        app.update(cx, |this, cx| {
            this.dock_panel(Panel::Mixer, Dock::Bottom);
            cx.notify();
        });
        cx.run_until_parked();
        handle
            .update(cx, |this, _, _| {
                assert_eq!(this.panels.dock(Panel::Mixer), Dock::Bottom)
            })
            .unwrap();
    }

    #[test]
    fn settings_search_accepts_both_languages_case_and_multiple_words() {
        assert!(matches("  SAMPLE rate ", Section::Rate.keys()));
        assert!(matches("サンプル", Section::Rate.keys()));
        assert!(matches("ミキサー", Section::Panels.keys()));
        assert!(matches("UI フォント", Section::Appearance.keys()));
        assert!(matches(" \t", Section::Autosave.keys()));
        assert!(!matches("sample autosave", Section::Rate.keys()));
        assert!(!matches("unfindable-setting", Section::Panels.keys()));
    }
}
