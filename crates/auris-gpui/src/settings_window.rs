//! The settings window: appearance, editing, audio devices and key bindings.
//!
//! A separate gpui window rather than a panel, because settings are not part of editing a
//! project and should not compete with it for space. It holds a weak handle to the main view
//! and applies every change through that, so there is still exactly one owner of the session.

use auris_i18n::{Key, Language, messages};
use auris_session::AgentPreferences;
use auris_session::prelude::*;
use auris_session::session::AudioStatus;
use gpui::{
    AnyElement, App, Context, FocusHandle, Focusable, IntoElement, KeyDownEvent, Render,
    WeakEntity, Window, div, prelude::*, px,
};

use crate::actions::{BINDABLE, Bindable};
use crate::app::AurisApp;
use crate::appearance::Appearance;
use crate::gestures::{PointerGesture, PointerGestures};
use crate::keymap::Keymap;
use crate::theme::{Metrics, SCHEMES, Theme};
use crate::titlebar;
use crate::ui::icons::Icon;
use crate::ui::palette;
use crate::ui::text_field::{HasTextField, KeyEffect, TextField};
use crate::ui::widgets::{ButtonStyle, button, chain_button, divider};

mod agent;
mod appearance_editor;
#[cfg(test)]
mod appearance_tests;
mod dropdown;
mod search;

use crate::dock::PanelLayout;
use search::Section;

/// Which page the settings window is showing.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SettingsTab {
    /// Interface language and anything else that is not audio or keys.
    General,
    /// Output device, input device, sample rate and buffer size.
    Audio,
    /// Key bindings.
    Keys,
    /// Language-model connection and generation settings.
    Agent,
}

/// The devices the host could see when the window opened.
///
/// Both lists together, because enumerating them talks to the OS audio server and the window is
/// built from a snapshot rather than by asking again on every frame.
#[derive(Clone, Debug, Default)]
pub struct AudioDevices {
    /// Everything that can play.
    pub output: Vec<AudioDeviceInfo>,
    /// Everything that can record.
    pub input: Vec<AudioDeviceInfo>,
}

/// Which of a project's two device slots a row writes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum DeviceSlot {
    Output,
    Input,
}

/// The settings window's view.
pub struct SettingsWindow {
    app: WeakEntity<AurisApp>,
    theme: Theme,
    appearance: Appearance,
    appearance_editor: Option<appearance_editor::AppearanceEditor>,
    font_families: Vec<String>,
    tab: SettingsTab,
    agent: agent::AgentSettings,
    /// What the host can see, refreshed on request or when the audio host changes.
    devices: AudioDevices,
    hosts: Vec<String>,
    audio: AudioPreferences,
    keymap: Keymap,
    /// Stored language preference; `None` follows the system.
    language_preference: Option<Language>,
    /// Language this window is drawn in, which is the resolved preference.
    language: Language,
    /// Whether the document is written back over itself as it changes.
    autosave: bool,
    /// Whether dragging a note's right edge rounds its duration to the editing grid.
    snap_note_lengths: bool,
    /// The dictionary folder kanji lyrics are read through. `None` on most machines.
    japanese_dictionary: Option<std::path::PathBuf>,
    /// Where singer voices run their inference.
    singer_acceleration: Acceleration,
    /// How a bounce is written.
    export: ExportPreferences,
    /// What a click creates and what deletes.
    pointer: PointerGestures,
    /// What the audio backend is actually doing.
    ///
    /// Cached rather than read during render: the window is opened from inside the main
    /// view's update, and reading an entity that is already being updated panics.
    live: Option<AudioStatus>,
    /// Which slot the next key press is being captured into, if any.
    capturing: Option<Capture>,
    /// What settings and key bindings are being filtered by.
    ///
    /// A real text field rather than a string built from key events, because the labels are
    /// translated: a Japanese user filtering on 「トラック」 needs the IME, and a field that reads
    /// key events never sees a composition at all.
    search: TextField,
    search_focus: FocusHandle,
    editing_search: bool,
    panels: PanelLayout,
    body_scroll: gpui::ScrollHandle,
    status: String,
    focus: FocusHandle,
    dropdown_menu: Option<dropdown::DropdownMenu>,
    dropdown_focus: std::collections::BTreeMap<&'static str, FocusHandle>,
}

/// A row of the key list, armed and waiting for a key press.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Capture {
    /// The command being bound.
    command: &'static Bindable,
    /// Which of its keystrokes is being replaced. Past the end appends another.
    slot: usize,
}

impl Focusable for SettingsWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl SettingsWindow {
    /// Refreshes appearance values changed from another application surface.
    pub(crate) fn sync_appearance(
        &mut self,
        appearance: Appearance,
        language_preference: Option<Language>,
    ) {
        self.theme = appearance.theme();
        self.appearance = appearance;
        self.language_preference = language_preference;
        self.language = Language::resolve(language_preference);
    }

    /// Builds the window's view.
    ///
    /// The state is handed in rather than read back through `app`: this runs inside the main
    /// view's own update, and reading an entity that is already being updated panics.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        app: WeakEntity<AurisApp>,
        appearance: Appearance,
        devices: AudioDevices,
        audio: AudioPreferences,
        live: AudioStatus,
        keymap: Keymap,
        language_preference: Option<Language>,
        pointer: PointerGestures,
        autosave: bool,
        snap_note_lengths: bool,
        japanese_dictionary: Option<std::path::PathBuf>,
        singer_acceleration: Acceleration,
        export: ExportPreferences,
        panels: PanelLayout,
        agent: AgentPreferences,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut font_families = cx.text_system().all_font_names();
        font_families.sort_by_key(|name| name.to_lowercase());
        font_families.dedup();
        if let Some(main) = app.upgrade() {
            cx.observe(&main, |this, main, cx| {
                let panels = main.read(cx).panels.clone();
                if this.panels != panels {
                    this.panels = panels;
                    cx.notify();
                }
            })
            .detach();
        }
        Self {
            app,
            theme: appearance.theme(),
            appearance,
            appearance_editor: None,
            font_families,
            tab: SettingsTab::General,
            agent: agent::AgentSettings::new(agent, cx),
            devices,
            hosts: Session::audio_hosts(),
            audio,
            live: Some(live),
            keymap,
            language_preference,
            language: Language::resolve(language_preference),
            autosave,
            snap_note_lengths,
            japanese_dictionary,
            singer_acceleration,
            export,
            pointer,
            capturing: None,
            search: TextField::new(String::new()),
            search_focus: cx.focus_handle().tab_stop(true),
            editing_search: true,
            panels,
            body_scroll: gpui::ScrollHandle::new(),
            status: String::new(),
            focus: cx.focus_handle(),
            dropdown_menu: None,
            dropdown_focus: std::collections::BTreeMap::new(),
        }
    }

    /// A fixed string in the language this window is drawn in.
    fn t(&self, key: Key) -> &'static str {
        key.get(self.language)
    }

    /// Hands edited pointer gestures to the application, which saves them.
    fn apply_pointer(&mut self, pointer: PointerGestures, cx: &mut Context<Self>) {
        self.pointer = pointer;
        let _ = self
            .app
            .update(cx, |app, _| app.apply_pointer_gestures(pointer));
        cx.notify();
    }

    /// A gesture dropdown for one of the two actions.
    ///
    /// `offered` is what the row lists, which is not the same for both: the bare click may create
    /// and may not delete, and a button that swapped the two into an arrangement
    /// [`PointerGestures::set_delete`] refuses would look like the panel ignoring a click.
    fn gesture_row(
        &mut self,
        id: &'static str,
        label: Key,
        current: PointerGesture,
        offered: fn(PointerGesture) -> bool,
        assign: fn(&mut PointerGestures, PointerGesture),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
        let choices = PointerGesture::ALL
            .into_iter()
            .filter(|gesture| offered(*gesture))
            .map(|gesture| (gesture, self.t(gesture.label()).to_owned(), String::new()))
            .collect();
        let control = self.dropdown(
            id,
            choices,
            &current,
            move |this, gesture, cx| {
                let mut pointer = this.pointer;
                assign(&mut pointer, gesture);
                this.apply_pointer(pointer, cx);
            },
            cx,
        );
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .w(px(200.0))
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(label)),
            )
            .child(div().flex_1().min_w_0().child(control))
            .into_any_element()
    }

    /// Hands a language choice to the application, which installs and saves it.
    fn apply_language(&mut self, preference: Option<Language>, cx: &mut Context<Self>) {
        self.language_preference = preference;
        self.language = Language::resolve(preference);
        let _ = self.app.update(cx, |app, cx| {
            let cx: &mut App = cx;
            app.apply_language(preference, cx);
        });
        self.status = messages::language_changed(self.language, self.language.endonym());
        cx.notify();
    }

    /// Hands new audio preferences to the session and reports what happened.
    fn apply_audio(&mut self, audio: AudioPreferences, cx: &mut Context<Self>) {
        let refresh_devices = self.audio.host != audio.host || self.audio.uses_asio();
        let requested = audio.clone();
        let Ok(outcome) = self
            .app
            .update(cx, |app, _| app.apply_audio_preferences(audio))
        else {
            return;
        };
        self.status = match outcome {
            Ok(status) => {
                self.audio = self
                    .app
                    .read_with(cx, |app, _| app.session.audio_preferences().clone())
                    .unwrap_or(requested);
                status
            }
            Err(error) => crate::i18n::error_text(&error, self.language),
        };
        // The previous update has finished, so reading back is safe here.
        self.live = self
            .app
            .read_with(cx, |app, _| app.session.audio_status())
            .ok();
        if refresh_devices
            && let Ok(devices) = self.app.read_with(cx, |app, _| AudioDevices {
                output: app.session.output_devices(),
                input: app.session.input_devices(),
            })
        {
            self.devices = devices;
        }
        cx.notify();
    }

    /// Hands the edited keymap to the application, which installs and saves it.
    fn apply_keymap(&mut self, cx: &mut Context<Self>) {
        let keymap = self.keymap.clone();
        let _ = self.app.update(cx, |app, cx| {
            let cx: &mut App = cx;
            app.apply_keymap(keymap, cx);
        });
        cx.notify();
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let tab = self.tab;
        div()
            .flex()
            .items_center()
            .flex_shrink_0()
            .gap_1()
            .pr_2()
            .child(button(
                "tab-general",
                self.t(Key::TabGeneral),
                ButtonStyle::Normal,
                !self.searching() && tab == SettingsTab::General,
                theme.accent,
                &theme,
                cx.listener(|this, _, window, cx| {
                    this.select_tab(SettingsTab::General, window, cx);
                }),
            ))
            .child(button(
                "tab-audio",
                self.t(Key::TabAudio),
                ButtonStyle::Normal,
                !self.searching() && tab == SettingsTab::Audio,
                theme.accent,
                &theme,
                cx.listener(|this, _, window, cx| {
                    this.select_tab(SettingsTab::Audio, window, cx);
                }),
            ))
            .child(button(
                "tab-agent",
                self.t(Key::AgentPanel),
                ButtonStyle::Normal,
                !self.searching() && tab == SettingsTab::Agent,
                theme.accent,
                &theme,
                cx.listener(|this, _, window, cx| this.select_tab(SettingsTab::Agent, window, cx)),
            ))
            .child(button(
                "tab-keys",
                self.t(Key::TabKeys),
                ButtonStyle::Normal,
                !self.searching() && tab == SettingsTab::Keys,
                theme.accent,
                &theme,
                cx.listener(|this, _, window, cx| {
                    this.select_tab(SettingsTab::Keys, window, cx);
                }),
            ))
    }

    fn select_tab(&mut self, tab: SettingsTab, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        self.capturing = None;
        self.editing_search = true;
        self.search = TextField::new(String::new());
        self.text_changed();
        window.focus(&self.search_focus);
        cx.notify();
    }

    /// Keeps the last text field active while a button or dropdown holds focus.
    fn sync_text_focus(&mut self, window: &Window) -> bool {
        if let Some(index) = self
            .agent
            .focus
            .iter()
            .position(|focus| focus.is_focused(window))
        {
            self.agent.active = Some(index);
            self.editing_search = false;
            return true;
        }
        self.agent.active = None;
        let editor_focused = self.sync_editor_focus(window);
        if editor_focused {
            self.editing_search = false;
        } else if self.search_focus.is_focused(window) || self.appearance_editor.is_none() {
            self.editing_search = true;
        }
        editor_focused
    }

    /// Appearance, language, editing behaviour and singer preferences.
    fn render_general(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let current = self.language_preference;

        // "System" first, then each language in its own name — a picker written in a language
        // you cannot read is no use to the person who needs it.
        let mut choices: Vec<(Option<Language>, &'static str)> =
            vec![(None, self.t(Key::LanguageFollowSystem))];
        choices.extend(Language::ALL.map(|language| (Some(language), language.endonym())));

        let appearance_control = self.render_appearance(cx);
        let language_choices = choices
            .into_iter()
            .map(|(value, label)| (value, label.to_owned(), String::new()))
            .collect();
        let language_control = self.dropdown(
            "language",
            language_choices,
            &current,
            |this, value, cx| this.apply_language(value, cx),
            cx,
        );
        let acceleration = self.singer_acceleration;
        let acceleration_choices = vec![
            (
                Acceleration::Auto,
                self.t(Key::SingerComputeAuto).to_owned(),
                String::new(),
            ),
            (Acceleration::Gpu, "GPU".to_owned(), String::new()),
            (Acceleration::Cpu, "CPU".to_owned(), String::new()),
        ];
        let acceleration_control = self.dropdown(
            "singer-compute",
            acceleration_choices,
            &acceleration,
            |this, value, cx| this.apply_singer_acceleration(value, cx),
            cx,
        );
        div()
            .flex()
            .flex_col()
            .gap_2()
            .when(self.matches_section(Section::Appearance), |view| {
                view.child(appearance_control)
            })
            .when(self.matches_section(Section::Language), |view| {
                view.child(divider(&theme))
                    .child(section_title(self.t(Key::LanguageHeading), &theme))
                    .child(language_control)
                    .child(note(self.t(Key::LanguageNote), &theme))
            })
            .when(self.matches_section(Section::Pointer), |view| {
                view.child(divider(&theme))
                    .child(section_title(self.t(Key::PointerHeading), &theme))
                    .child(self.gesture_row(
                        "pointer-create",
                        Key::PointerCreate,
                        self.pointer.create,
                        |_| true,
                        PointerGestures::set_create,
                        cx,
                    ))
                    .child(self.gesture_row(
                        "pointer-delete",
                        Key::PointerDelete,
                        self.pointer.delete,
                        PointerGesture::may_delete,
                        PointerGestures::set_delete,
                        cx,
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child(self.t(Key::PointerNote)),
                    )
                    // Only while it applies. A standing warning about a setting nobody has chosen is a
                    // line every user reads once and no user acts on.
                    .when(self.pointer.create == PointerGesture::Click, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child(self.t(Key::PointerClickNote)),
                        )
                    })
            })
            .when(self.matches_section(Section::Autosave), |view| {
                view.child(divider(&theme))
                    .child(section_title(self.t(Key::Autosave), &theme))
                    .child(div().flex().gap_1().child(button(
                        "autosave",
                        self.t(if self.autosave {
                            Key::ValueOn
                        } else {
                            Key::ValueOff
                        }),
                        ButtonStyle::Normal,
                        self.autosave,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.apply_autosave(!this.autosave, cx);
                        }),
                    )))
                    .child(note(self.t(Key::AutosaveNote), &theme))
            })
            .when(self.matches_section(Section::Snap), |view| {
                view.child(divider(&theme))
                    .child(section_title(self.t(Key::SnapNoteLengths), &theme))
                    .child(div().flex().gap_1().child(button(
                        "snap-note-lengths",
                        self.t(if self.snap_note_lengths {
                            Key::ValueOn
                        } else {
                            Key::ValueOff
                        }),
                        ButtonStyle::Normal,
                        self.snap_note_lengths,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.apply_snap_note_lengths(!this.snap_note_lengths, cx);
                        }),
                    )))
                    .child(note(self.t(Key::SnapNoteLengthsNote), &theme))
            })
            .when(self.matches_section(Section::Dictionary), |view| {
                view.child(divider(&theme))
                    .child(section_title(
                        self.t(Key::JapaneseDictionaryHeading),
                        &theme,
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .text_color(
                                        match (
                                            &self.japanese_dictionary,
                                            auris_session::library::installed_dictionary(),
                                        ) {
                                            (None, None) => theme.text_muted,
                                            _ => theme.text,
                                        },
                                    )
                                    .truncate()
                                    // An empty setting is not an empty state: the shipped dictionary
                                    // stands in, and the row should say which one is answering.
                                    .child(match &self.japanese_dictionary {
                                        Some(folder) => folder.display().to_string(),
                                        None => {
                                            match auris_session::library::installed_dictionary() {
                                                Some(_) => {
                                                    self.t(Key::ValueShippedDictionary).to_string()
                                                }
                                                None => self.t(Key::ValueNotSet).to_string(),
                                            }
                                        }
                                    }),
                            )
                            .child(button(
                                "dictionary-choose",
                                self.t(Key::MenuChoose),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(|this, _, _, cx| this.choose_japanese_dictionary(cx)),
                            ))
                            .child(button(
                                "dictionary-clear",
                                self.t(Key::MenuClear),
                                ButtonStyle::Ghost,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(|this, _, _, cx| {
                                    this.apply_japanese_dictionary(None, cx);
                                }),
                            )),
                    )
                    .child(note(self.t(Key::JapaneseDictionaryNote), &theme))
            })
            .when(self.matches_section(Section::Singer), |view| {
                view.child(divider(&theme))
                    .child(section_title(self.t(Key::SingerComputeHeading), &theme))
                    .child(acceleration_control)
                    .child(note(self.t(Key::SingerComputeNote), &theme))
            })
            .when(self.matches_section(Section::Panels), |view| {
                view.child(divider(&theme))
                    .child(self.render_panel_positions(cx))
            })
            .into_any_element()
    }

    /// Hands the acceleration choice to the application, which installs and saves it.
    fn apply_singer_acceleration(&mut self, acceleration: Acceleration, cx: &mut Context<Self>) {
        self.singer_acceleration = acceleration;
        let _ = self
            .app
            .update(cx, |app, _| app.apply_singer_acceleration(acceleration));
        cx.notify();
    }

    /// Hands an autosave choice to the application, which installs and saves it.
    fn apply_autosave(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.autosave = enabled;
        let _ = self.app.update(cx, |app, _| app.apply_autosave(enabled));
        cx.notify();
    }

    /// Hands the note-length snapping choice to the application and remembers it.
    fn apply_snap_note_lengths(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.snap_note_lengths = enabled;
        let _ = self
            .app
            .update(cx, |app, _| app.apply_snap_note_lengths(enabled));
        cx.notify();
    }

    /// Asks for the dictionary folder, then hands it to the application.
    fn choose_japanese_dictionary(&mut self, cx: &mut Context<Self>) {
        let language = self.language;
        cx.spawn(async move |this, cx| {
            let handle = rfd::AsyncFileDialog::new()
                .set_title(Key::DialogDictionaryFolder.get(language))
                .pick_folder()
                .await;
            let Some(handle) = handle else { return };
            let folder = handle.path().to_path_buf();
            let _ = this.update(cx, |this, cx| {
                this.apply_japanese_dictionary(Some(folder), cx);
            });
        })
        .detach();
    }

    /// Hands a dictionary choice to the application, which loads, installs and saves it.
    ///
    /// A folder that fails to load leaves the setting as it was and puts the loader's words in
    /// this window's own status line, which is the screen the person is looking at.
    fn apply_japanese_dictionary(
        &mut self,
        folder: Option<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let applied = self
            .app
            .update(cx, |app, _| app.apply_japanese_dictionary(folder.clone()));
        match applied {
            Ok(Ok(())) => {
                self.japanese_dictionary = folder;
                self.status.clear();
            }
            Ok(Err(message)) => self.status = message,
            Err(_) => {}
        }
        cx.notify();
    }

    /// Hands an export choice to the application, which saves it.
    fn apply_export(&mut self, export: ExportPreferences, cx: &mut Context<Self>) {
        self.export = export;
        let _ = self.app.update(cx, |app, _| app.apply_export(export));
        cx.notify();
    }

    fn render_audio(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let audio = self.audio.clone();
        let live = self.live.clone();
        let export = self.export;
        let mut rows: Vec<AnyElement> = Vec::new();

        if self.matches_section(Section::Host) {
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(section_title(self.t(Key::AudioHost), &theme))
                    .child(button(
                        "refresh-audio-devices",
                        self.t(Key::RefreshAudioDevices),
                        ButtonStyle::Ghost,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.hosts = Session::audio_hosts();
                            if let Ok(devices) = this.app.read_with(cx, |app, _| AudioDevices {
                                output: app.session.output_devices(),
                                input: app.session.input_devices(),
                            }) {
                                this.devices = devices;
                            }
                            cx.notify();
                        }),
                    ))
                    .into_any_element(),
            );
            let mut hosts = vec![(
                None,
                self.t(Key::SystemDefaultDevice).to_owned(),
                String::new(),
            )];
            hosts.extend(
                self.hosts
                    .iter()
                    .map(|host| (Some(host.clone()), host.clone(), String::new())),
            );
            if let Some(host) = &audio.host
                && !self.hosts.contains(host)
            {
                hosts.push((
                    Some(host.clone()),
                    host.clone(),
                    self.t(Key::SettingUnavailable).to_owned(),
                ));
            }
            rows.push(self.dropdown(
                "audio-host",
                hosts,
                &audio.host,
                |this, host, cx| {
                    if this.audio.host != host {
                        this.apply_audio(
                            AudioPreferences {
                                host,
                                device: None,
                                input_device: None,
                                sample_rate: None,
                                ..this.audio.clone()
                            },
                            cx,
                        );
                    }
                },
                cx,
            ));
        }
        if self.matches_section(Section::Output) {
            if !rows.is_empty() {
                rows.push(divider(&theme).into_any_element());
            }
            rows.push(section_title(self.t(Key::OutputDevice), &theme));
            rows.push(self.device_dropdown(DeviceSlot::Output, cx));
        }
        if self.matches_section(Section::Input) {
            if !rows.is_empty() {
                rows.push(divider(&theme).into_any_element());
            }
            rows.push(section_title(self.t(Key::InputDevice), &theme));
            if audio.uses_asio() {
                rows.push(note(self.t(Key::AsioInputNote), &theme));
            } else {
                rows.push(self.device_dropdown(DeviceSlot::Input, cx));
                rows.push(note(self.t(Key::InputDeviceNote), &theme));
            }
        }
        if self.matches_section(Section::Rate) {
            if !rows.is_empty() {
                rows.push(divider(&theme).into_any_element());
            }
            rows.push(section_title(self.t(Key::SampleRate), &theme));
            let rates = self.rate_choices();
            let mut rate_choices = vec![(
                None,
                self.t(Key::DeviceDefaultRate).to_owned(),
                String::new(),
            )];
            rate_choices.extend(rates.iter().map(|rate| {
                (
                    Some(*rate),
                    messages::rate_single(self.language, f64::from(*rate) / 1000.0),
                    String::new(),
                )
            }));
            if let Some(rate) = audio.sample_rate
                && !rates.contains(&rate)
            {
                rate_choices.push((
                    Some(rate),
                    messages::rate_single(self.language, f64::from(rate) / 1000.0),
                    self.t(Key::SettingUnavailable).to_owned(),
                ));
            }
            rows.push(self.dropdown(
                "sample-rate",
                rate_choices,
                &audio.sample_rate,
                |this, sample_rate, cx| {
                    this.apply_audio(
                        AudioPreferences {
                            sample_rate,
                            ..this.audio.clone()
                        },
                        cx,
                    );
                },
                cx,
            ));
        }
        if self.matches_section(Section::Buffer) {
            if !rows.is_empty() {
                rows.push(divider(&theme).into_any_element());
            }
            rows.push(section_title(self.t(Key::BufferSize), &theme));
            let mut block_sizes = AudioPreferences::BLOCK_CHOICES.to_vec();
            if !block_sizes.contains(&audio.block_frames) {
                block_sizes.push(audio.block_frames);
            }
            let rate = live.as_ref().map_or(48_000.0, |status| status.sample_rate);
            let blocks = block_sizes
                .into_iter()
                .map(|frames| {
                    let latency = frames as f64 / rate.max(1.0) * 1000.0;
                    (
                        frames,
                        messages::buffer_choice(self.language, frames, latency),
                        String::new(),
                    )
                })
                .collect();
            rows.push(self.dropdown(
                "block",
                blocks,
                &audio.block_frames,
                |this, block_frames, cx| {
                    this.apply_audio(
                        AudioPreferences {
                            block_frames,
                            ..this.audio.clone()
                        },
                        cx,
                    );
                },
                cx,
            ));
            rows.push(note(self.t(Key::RequestedBufferNote), &theme));
            let actual = live
                .as_ref()
                .and_then(|status| {
                    let frames = status.buffer_frames?;
                    Some(messages::actual_buffer(
                        self.language,
                        status.host.as_deref().unwrap_or(""),
                        frames,
                        f64::from(frames) / status.sample_rate.max(1.0) * 1000.0,
                    ))
                })
                .unwrap_or_else(|| self.t(Key::ActualBufferUnknown).to_owned());
            rows.push(note(&actual, &theme));
        }
        if self.matches_section(Section::Export) {
            if !rows.is_empty() {
                rows.push(divider(&theme).into_any_element());
            }
            // Export preferences describe the file, independently of the output device's settings.

            rows.push(section_title(self.t(Key::ExportFormat), &theme));
            let formats = [
                AudioExportFormat::Wav,
                AudioExportFormat::Flac,
                AudioExportFormat::Mp3,
            ]
            .into_iter()
            .map(|format| {
                (
                    format,
                    crate::i18n::audio_export_format_key(format)
                        .get(self.language)
                        .to_owned(),
                    String::new(),
                )
            })
            .collect();
            rows.push(self.dropdown(
                "export-format",
                formats,
                &export.format,
                |this, format, cx| {
                    let mut export = ExportPreferences {
                        format,
                        ..this.export
                    };
                    if matches!(format, AudioExportFormat::Flac)
                        && matches!(export.bit_depth, WavBitDepth::Float32)
                    {
                        export.bit_depth = WavBitDepth::Int24;
                    }
                    if let Some(rate) = export.sample_rate
                        && !format.supports_sample_rate(rate)
                    {
                        export.sample_rate = Some(44_100);
                    }
                    this.apply_export(export, cx);
                },
                cx,
            ));

            if !matches!(export.format, AudioExportFormat::Mp3) {
                rows.push(section_title(self.t(Key::ExportBitDepth), &theme));
                let depth_choices: &[WavBitDepth] = match export.format {
                    AudioExportFormat::Wav => {
                        &[WavBitDepth::Int16, WavBitDepth::Int24, WavBitDepth::Float32]
                    }
                    AudioExportFormat::Flac => &[WavBitDepth::Int16, WavBitDepth::Int24],
                    AudioExportFormat::Mp3 => &[],
                };
                let depths = depth_choices
                    .iter()
                    .copied()
                    .map(|depth| {
                        (
                            depth,
                            crate::i18n::wav_bit_depth_key(depth)
                                .get(self.language)
                                .to_owned(),
                            String::new(),
                        )
                    })
                    .collect();
                rows.push(self.dropdown(
                    "depth",
                    depths,
                    &export.bit_depth,
                    |this, bit_depth, cx| {
                        this.apply_export(
                            ExportPreferences {
                                bit_depth,
                                ..this.export
                            },
                            cx,
                        );
                    },
                    cx,
                ));
            }
            rows.push(section_title(self.t(Key::ExportRate), &theme));
            let mut export_rates = vec![(None, self.t(Key::ProjectRate).to_owned(), String::new())];
            let rate_choices: &[u32] = if matches!(export.format, AudioExportFormat::Mp3) {
                &ExportPreferences::MP3_RATE_CHOICES
            } else {
                &AudioPreferences::RATE_CHOICES
            };
            export_rates.extend(rate_choices.iter().copied().map(|rate| {
                (
                    Some(rate),
                    messages::rate_single(self.language, f64::from(rate) / 1000.0),
                    String::new(),
                )
            }));
            if let Some(rate) = export.sample_rate
                && export.format.supports_sample_rate(rate)
                && !rate_choices.contains(&rate)
            {
                export_rates.push((
                    Some(rate),
                    messages::rate_single(self.language, f64::from(rate) / 1000.0),
                    String::new(),
                ));
            }
            rows.push(self.dropdown(
                "export-rate",
                export_rates,
                &export.sample_rate,
                |this, sample_rate, cx| {
                    this.apply_export(
                        ExportPreferences {
                            sample_rate,
                            ..this.export
                        },
                        cx,
                    );
                },
                cx,
            ));
            if matches!(export.format, AudioExportFormat::Mp3) {
                rows.push(section_title(self.t(Key::ExportBitrate), &theme));
                let bitrates = [
                    Mp3Bitrate::Kbps128,
                    Mp3Bitrate::Kbps192,
                    Mp3Bitrate::Kbps256,
                    Mp3Bitrate::Kbps320,
                ]
                .into_iter()
                .map(|bitrate| (bitrate, format!("{} kbps", bitrate.kbps()), String::new()))
                .collect();
                rows.push(self.dropdown(
                    "export-bitrate",
                    bitrates,
                    &export.mp3_bitrate,
                    |this, mp3_bitrate, cx| {
                        this.apply_export(
                            ExportPreferences {
                                mp3_bitrate,
                                ..this.export
                            },
                            cx,
                        );
                    },
                    cx,
                ));
            } else {
                rows.push(section_title(self.t(Key::ExportDither), &theme));
                let dithers = export.dither_applies();
                let on = export.dither && dithers;
                if dithers {
                    rows.push(
                        div()
                            .flex()
                            .gap_1()
                            .child(button(
                                "export-dither",
                                self.t(if on { Key::ValueOn } else { Key::ValueOff }),
                                ButtonStyle::Normal,
                                on,
                                theme.accent,
                                &theme,
                                cx.listener(|this, _, _, cx| {
                                    this.apply_export(
                                        ExportPreferences {
                                            dither: !this.export.dither,
                                            ..this.export
                                        },
                                        cx,
                                    );
                                }),
                            ))
                            .into_any_element(),
                    );
                } else {
                    rows.push(
                        div()
                            .id("export-dither-disabled")
                            .text_xs()
                            .text_color(theme.text_faint)
                            .child(self.t(Key::ValueOff))
                            .into_any_element(),
                    );
                }
                rows.push(note(self.t(Key::ExportDitherNote), &theme));
            }
        }
        if !self.searching()
            && let Some(status) = live
        {
            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .pt_1()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child({
                        let suffix = if status.running {
                            String::new()
                        } else {
                            messages::silent_suffix(self.language)
                        };
                        messages::running_device(
                            self.language,
                            &status.device,
                            status.sample_rate,
                            status.channels,
                            &suffix,
                        )
                    })
                    .children(
                        status
                            .gpu
                            .map(|gpu| div().child(messages::gpu_in_use(self.language, &gpu))),
                    )
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

    /// Sample rates advertised by the chosen device, or a sensible list when unknown.
    fn rate_choices(&self) -> Vec<u32> {
        let chosen = self
            .devices
            .output
            .iter()
            .find(|device| match self.audio.device.as_deref() {
                Some(name) => device.name == name,
                None => device.is_default,
            })
            .filter(|device| !device.sample_rates.is_empty());
        match chosen {
            Some(device) => device.sample_rates.clone(),
            None => AudioPreferences::RATE_CHOICES.to_vec(),
        }
    }

    /// Device names retain their channel/rate details, including a saved missing device.
    fn device_dropdown(&mut self, slot: DeviceSlot, cx: &mut Context<Self>) -> AnyElement {
        let (id, current, devices) = match slot {
            DeviceSlot::Output => (
                "output-device",
                self.audio.device.clone(),
                &self.devices.output,
            ),
            DeviceSlot::Input => (
                "input-device",
                self.audio.input_device.clone(),
                &self.devices.input,
            ),
        };
        let asio = slot == DeviceSlot::Output && self.audio.uses_asio();
        let mut choices = vec![(
            None,
            self.t(if asio {
                Key::FirstAsioDevice
            } else {
                Key::SystemDefaultDevice
            })
            .to_owned(),
            self.t(if asio {
                Key::FirstAsioDeviceDetail
            } else {
                Key::SystemDefaultDeviceDetail
            })
            .to_owned(),
        )];
        choices.extend(devices.iter().map(|device| {
            (
                Some(device.name.clone()),
                device.name.clone(),
                describe(device, self.language),
            )
        }));
        if let Some(name) = &current
            && !devices.iter().any(|device| device.name == *name)
        {
            choices.push((
                Some(name.clone()),
                name.clone(),
                self.t(Key::SettingUnavailable).to_owned(),
            ));
        }
        self.dropdown(
            id,
            choices,
            &current,
            move |this, device, cx| {
                let audio = match slot {
                    DeviceSlot::Output => output_device_preferences(&this.audio, device),
                    DeviceSlot::Input => AudioPreferences {
                        input_device: device,
                        ..this.audio.clone()
                    },
                };
                this.apply_audio(audio, cx);
            },
            cx,
        )
    }
    /// The commands the search box is showing.
    ///
    /// Matched on the group and the name together, in both languages, by the same
    /// scoring the command palette uses — so a query means the same thing in both lists. Filtered
    /// but *not* reordered: this list is arranged under section headings, and sorting by score
    /// would scatter the sections.
    fn found_commands(&self) -> Vec<&'static Bindable> {
        let query = self.search.content();
        BINDABLE
            .iter()
            .filter(|command| {
                Language::ALL.into_iter().any(|language| {
                    let labels = format!(
                        "{} {}",
                        command.group.get(language),
                        command.label.get(language)
                    );
                    palette::best_score(query, &labels, None).is_some()
                })
            })
            .collect()
    }

    fn render_keys(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let found = self.found_commands();

        let mut rows: Vec<AnyElement> = Vec::new();
        let mut group: Option<Key> = None;
        for command in found.iter().copied() {
            if group != Some(command.group) {
                group = Some(command.group);
                rows.push(self.render_group_heading(command.group, cx));
            }
            rows.push(self.render_key_row(command, cx));
        }
        if rows.is_empty() {
            rows.push(
                div()
                    .h(px(26.0))
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::NothingMatchesSearch))
                    .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_1()
            .children(rows)
            .child(div().flex().justify_end().pt_3().child(button(
                "reset-all",
                self.t(Key::RestoreDefaults),
                ButtonStyle::Normal,
                false,
                theme.accent,
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.keymap.reset();
                    this.capturing = None;
                    this.status = this.t(Key::BindingsRestored).to_string();
                    this.apply_keymap(cx);
                }),
            )))
            .into_any_element()
    }

    /// The box that narrows the list.
    ///
    /// Only editable while no row is armed. The field registers itself as the window's text input
    /// handler when it paints, and a handler registered while a row was waiting for a key press
    /// would swallow the press into the search box instead of binding it.
    fn render_search(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let armed = self.capturing.is_some();
        let empty = self.search.content().is_empty();
        div()
            .id("settings-search")
            .debug_selector(|| "settings-search".to_owned())
            .track_focus(&self.search_focus)
            .tab_index(0)
            .flex()
            .items_center()
            .h(px(28.0))
            .mb_2()
            .rounded(Metrics::RADIUS_SM)
            .bg(theme.surface_sunken)
            .border_1()
            .border_color(if armed { theme.border } else { theme.accent })
            .child(
                div()
                    .relative()
                    .flex_1()
                    .h_full()
                    // Behind the field, never instead of it. Swapping the box for a label while
                    // it was empty meant the field never painted, so it never registered itself
                    // as the window's input handler — and a box that cannot be typed into cannot
                    // stop being empty. Nothing could be searched for at all.
                    .children(empty.then(|| {
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            // The same inset and size the field draws its own text at, so the
                            // first character typed lands where the placeholder was.
                            .pl(crate::ui::prompt::FIELD_PADDING)
                            .text_size(crate::ui::prompt::TEXT_SIZE)
                            .text_color(theme.text_faint)
                            .child(self.t(Key::SearchSettings))
                    }))
                    .child(if armed {
                        // A row is waiting for a key press, and the field must not take it. Drawn
                        // rather than editable for exactly as long as that is true.
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .pl(crate::ui::prompt::FIELD_PADDING)
                            .text_size(crate::ui::prompt::TEXT_SIZE)
                            .text_color(theme.text)
                            .child(self.search.content().to_string())
                            .into_any_element()
                    } else {
                        crate::ui::prompt::editable_text(
                            self.search.content().to_string().into(),
                            self.search.selection(),
                            self.search.marked(),
                            self.search_focus.clone(),
                            cx.entity(),
                            theme.clone(),
                        )
                        .into_any_element()
                    }),
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.capturing = None;
                    this.dropdown_menu = None;
                    this.editing_search = true;
                    window.focus(&this.search_focus);
                    cx.notify();
                }),
            )
            .when(!empty, |view| {
                view.child(button(
                    "clear-settings-search",
                    self.t(Key::MenuClear),
                    ButtonStyle::Ghost,
                    false,
                    theme.accent,
                    &theme,
                    cx.listener(|this, _, window, cx| {
                        this.select_tab(this.tab, window, cx);
                    }),
                ))
            })
            .into_any_element()
    }

    /// A section heading, with the button that puts its whole group back.
    fn render_group_heading(&self, group: Key, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let changed = BINDABLE
            .iter()
            .any(|command| command.group == group && self.keymap.is_overridden(command));
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(section_title(self.t(group), &theme))
            .child(div().flex_1())
            // Only once something in the group has been changed: a row of buttons that would all
            // do nothing is a row of buttons that teaches the user to ignore them.
            .children(changed.then(|| {
                chain_button(
                    ("reset-group", group as usize),
                    Icon::Cross,
                    &theme,
                    cx.listener(move |this, _, _, cx| {
                        this.keymap.reset_group(group);
                        this.capturing = None;
                        this.apply_keymap(cx);
                    }),
                )
            }))
            .into_any_element()
    }

    /// One command's row: what it is, where it reaches, and every key that runs it.
    fn render_key_row(&self, command: &'static Bindable, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let id = command.id;
        let keystrokes = self.keymap.displays(command);
        let overridden = self.keymap.is_overridden(command);
        let unbound = self.keymap.is_unbound(command);
        // Every command a key would also run, not just the first. Two clashes on one keystroke
        // used to look exactly like one.
        let clashes: Vec<&'static str> = keystrokes
            .iter()
            .flat_map(|keystroke| self.keymap.conflicts(keystroke, command))
            .map(|other| self.t(other.label))
            .collect();

        let chips: Vec<AnyElement> = keystrokes
            .iter()
            .enumerate()
            .map(|(slot, keystroke)| {
                let armed = self.capturing == Some(Capture { command, slot });
                div()
                    .flex()
                    .items_center()
                    .child(button(
                        gpui::SharedString::from(format!("bind:{id}:{slot}")),
                        if armed {
                            self.t(Key::PressAKey).to_string()
                        } else {
                            // Shown prettified but matched raw: `⌘S` is what belongs on the
                            // button, and `conflicts` compares against what gpui can parse.
                            crate::actions::menu_keystroke(keystroke)
                        },
                        ButtonStyle::Normal,
                        armed,
                        theme.accent,
                        &theme,
                        cx.listener(move |this, _, window, cx| this.arm(command, slot, window, cx)),
                    ))
                    // Only once there is more than one. A cross beside the single key every
                    // command starts with would be a second way to unbind it, in the row where
                    // that is least often what anyone means.
                    .children((keystrokes.len() > 1).then(|| {
                        chain_button(
                            gpui::SharedString::from(format!("drop:{id}:{slot}")),
                            Icon::Cross,
                            &theme,
                            cx.listener(move |this, _, _, cx| {
                                this.keymap.remove_at(command, slot);
                                this.capturing = None;
                                this.apply_keymap(cx);
                            }),
                        )
                    }))
                    .into_any_element()
            })
            .collect();
        // An unbound command still needs somewhere to press, or there would be no way back.
        let adding = self.capturing
            == Some(Capture {
                command,
                slot: keystrokes.len(),
            });
        let placeholder = (keystrokes.is_empty() && !adding).then(|| {
            button(
                gpui::SharedString::from(format!("bind:{id}:0")),
                self.t(Key::NoKeystroke).to_string(),
                ButtonStyle::Normal,
                false,
                theme.accent,
                &theme,
                cx.listener(move |this, _, window, cx| this.arm(command, 0, window, cx)),
            )
        });

        div()
            .flex()
            .items_center()
            .gap_2()
            .h(px(26.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .text_color(theme.text)
                    .truncate()
                    .child(self.t(command.label)),
            )
            // Where the binding reaches, for the commands that do not reach everywhere.
            .children(crate::actions::context_label(command.context).map(|scope| {
                div()
                    .text_xs()
                    .text_color(theme.text_faint)
                    .child(self.t(scope))
            }))
            .children(
                (!clashes.is_empty()
                    && self
                        .capturing
                        .is_none_or(|capture| capture.command != command))
                .then(|| {
                    div()
                        .text_xs()
                        .text_color(theme.mute)
                        .child(messages::also_bound_to(self.language, &clashes.join(", ")))
                }),
            )
            .children(chips)
            .children(placeholder)
            .children(adding.then(|| {
                button(
                    gpui::SharedString::from(format!("bind-new:{id}")),
                    self.t(Key::PressAKey).to_string(),
                    ButtonStyle::Normal,
                    true,
                    theme.accent,
                    &theme,
                    |_, _, _| {},
                )
            }))
            .child(chain_button(
                gpui::SharedString::from(format!("add:{id}")),
                Icon::Plus,
                &theme,
                cx.listener(move |this, _, window, cx| {
                    let slot = this.keymap.keystrokes(command).len();
                    this.arm(command, slot, window, cx);
                }),
            ))
            .child(div().w(px(24.0)).child(if unbound {
                div().into_any_element()
            } else {
                button(
                    gpui::SharedString::from(format!("unbind:{id}")),
                    self.t(Key::NoKeystroke).to_string(),
                    ButtonStyle::Ghost,
                    false,
                    theme.accent,
                    &theme,
                    cx.listener(move |this, _, _, cx| {
                        this.keymap.unbind(command);
                        this.capturing = None;
                        this.status =
                            messages::binding_unbound(this.language, this.t(command.label));
                        this.apply_keymap(cx);
                    }),
                )
                .into_any_element()
            }))
            .child(div().w(px(20.0)).child(if overridden {
                chain_button(
                    gpui::SharedString::from(format!("reset:{id}")),
                    Icon::Cross,
                    &theme,
                    cx.listener(move |this, _, _, cx| {
                        this.keymap.clear(command);
                        this.capturing = None;
                        this.apply_keymap(cx);
                    }),
                )
                .into_any_element()
            } else {
                div().into_any_element()
            }))
            .into_any_element()
    }

    /// Arms one slot of one command for the next key press.
    fn arm(
        &mut self,
        command: &'static Bindable,
        slot: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.capturing = Some(Capture { command, slot });
        // The capture reads key events, so the window must hold focus.
        window.focus(&self.focus);
        cx.notify();
    }

    /// Handles a key press, and says whether it was consumed.
    ///
    /// Two jobs, and which one it is doing depends on whether a row is armed: filling in a
    /// binding, or editing the search box. Characters never arrive here — those go through the
    /// platform's input handler, which is what lets an IME compose into the search box.
    fn on_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.dropdown_menu.is_some() && self.dropdown_key(event, cx) {
            return true;
        }
        self.sync_text_focus(window);
        if self.agent.active.is_some() && self.agent_key(event, cx) {
            return true;
        }
        if self.agent.active.is_none()
            && self.sync_text_focus(window)
            && self.appearance_editor_key(event, cx)
        {
            return true;
        }
        if self.capturing.is_some() {
            self.capture_key(event, cx);
            return true;
        }
        if event.keystroke.key == "tab" {
            if event.keystroke.modifiers.shift {
                window.focus_prev();
            } else {
                window.focus_next();
            }
            cx.notify();
            return true;
        }
        if !self.search_focus.is_focused(window) {
            return false;
        }
        self.search_key(event, cx)
    }

    /// Turns a captured key press into a binding.
    fn capture_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(Capture { command, slot }) = self.capturing else {
            return;
        };
        // Escape abandons the capture rather than being bound: otherwise there would be no way
        // to back out once a row is armed.
        if event.keystroke.key == "escape" && !event.keystroke.modifiers.modified() {
            self.capturing = None;
            self.status = self.t(Key::CaptureCancelled).to_string();
            cx.notify();
            return;
        }

        // Stored the way the file spells things, not the way this keyboard reported them, so a
        // keymap.json carried to the other platform still binds the modifier a user means.
        let keystroke = crate::actions::portable_keystroke(&event.keystroke.unparse());
        self.capturing = None;
        // Appending goes through `add`, which refuses a key the command already answers to:
        // pressing ＋ and then the key that is already there would otherwise list it twice.
        let bound = if slot >= self.keymap.keystrokes(command).len() {
            self.keymap.add(command, &keystroke)
        } else {
            self.keymap.set_at(command, slot, &keystroke)
        };
        let name = self.t(command.label);
        if bound {
            let clash = self.keymap.conflicts(&keystroke, command);
            let outcome = match clash.first() {
                Some(other) => Captured::Clashes(self.t(other.label)),
                None => Captured::Bound,
            };
            self.status = capture_status(self.language, name, &keystroke, outcome);
            self.apply_keymap(cx);
        } else {
            self.status = capture_status(self.language, name, &keystroke, Captured::Refused);
            cx.notify();
        }
    }

    /// The keys the search box claims: the ones the platform does not deliver as text.
    fn search_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let shift = event.keystroke.modifiers.shift;
        let secondary = event.keystroke.modifiers.secondary();
        let key = event.keystroke.key.as_str();
        let claimed = match key {
            // Escape clears the filter rather than closing the window: the list under a query is
            // a list with most of itself missing, and getting it back should not cost a trip
            // through the menu. This window's own, so it is answered before the shared list.
            "escape" if !self.search.content().is_empty() && self.search.marked().is_none() => {
                self.search = TextField::new(String::new());
                true
            }
            // Backspace, the caret, Select All. Shared with every field in the main window
            // rather than written out again here — this box was the fourth copy of the same
            // table, and the four had already drifted apart.
            key => {
                self.search
                    .apply_key_with_clipboard(key, shift, secondary, false, cx)
                    != KeyEffect::Ignored
            }
        };
        if claimed {
            self.text_changed();
            cx.notify();
        }
        claimed
    }
}

fn output_device_preferences(
    current: &AudioPreferences,
    device: Option<String>,
) -> AudioPreferences {
    AudioPreferences {
        // The old rate may not exist on a new device. Re-selecting the current device must keep
        // it, though, so an inert click cannot restart the audio stream at a different rate.
        sample_rate: (current.device == device)
            .then_some(current.sample_rate)
            .flatten(),
        device,
        ..current.clone()
    }
}

impl HasTextField for SettingsWindow {
    fn text_changed(&mut self) {
        if self.editing_search {
            self.dropdown_menu = None;
            self.body_scroll.set_offset(gpui::point(px(0.0), px(0.0)));
        }
        if let Some(field) = self.field()
            && field.content().contains(['\r', '\n'])
        {
            let text = field.content().replace(['\r', '\n'], " ");
            field.replace(0..field.content().len(), &text);
        }
    }

    fn field(&mut self) -> Option<&mut TextField> {
        if let Some(index) = self.agent.active {
            return Some(&mut self.agent.fields[index]);
        }
        if !self.editing_search {
            return self.appearance_editor.as_mut().map(|editor| editor.field());
        }
        // Only while the list is on screen and no row is waiting for a key press — the same two
        // conditions under which the box is drawn as an editable one.
        self.capturing.is_none().then_some(&mut self.search)
    }

    fn readable_field(&self) -> Option<&TextField> {
        if let Some(index) = self.agent.active {
            return Some(&self.agent.fields[index]);
        }
        if !self.editing_search {
            return self
                .appearance_editor
                .as_ref()
                .map(|editor| editor.readable_field());
        }
        self.capturing.is_none().then_some(&self.search)
    }
}

crate::entity_input_handler!(SettingsWindow);

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let editor_focused = self.sync_text_focus(window);
        if window.window_title() != self.t(Key::Settings) {
            window.set_window_title(self.t(Key::Settings));
        }
        // Preserve focus on child controls across renders while keeping the window's shortcuts
        // and text input available when the window first opens.
        if !editor_focused && !self.focus.contains_focused(window, cx) {
            self.editing_search = true;
            window.focus(&self.search_focus);
        }

        let theme = self.theme.clone();
        let titlebar = titlebar::titlebar(window, &theme)
            .child(
                titlebar::drag_region("settings-title")
                    .flex_1()
                    .min_w(px(40.0))
                    .px_3()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child(self.t(Key::Settings)),
                    ),
            )
            .child(self.render_tabs(cx))
            .child(titlebar::controls(window, &theme, |_, window, _| {
                window.remove_window();
            }));
        let body = if self.searching() {
            self.render_results(cx)
        } else {
            match self.tab {
                SettingsTab::General => self.render_general(cx),
                SettingsTab::Audio => self.render_audio(cx),
                SettingsTab::Keys => self.render_keys(cx),
                SettingsTab::Agent => self.render_agent(cx),
            }
        };
        let status = self.status.clone();
        let dropdown = self.render_dropdown(window, cx);

        div()
            .id("settings-root")
            .key_context("AurisSettings")
            .track_focus(&self.focus)
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .text_color(theme.text)
            .font(theme.font.clone())
            .text_sm()
            // Swallowed only when it was wanted: a key that fills in a binding or edits the
            // search box must not also fire whatever is bound to it, and every other key should
            // carry on to wherever it was going.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.on_key(event, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(titlebar)
            .child(
                div()
                    .px_3()
                    .pt_2()
                    .flex_shrink_0()
                    .child(self.render_search(cx)),
            )
            .child(
                div()
                    .id("settings-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.body_scroll)
                    .p_3()
                    .child(body),
            )
            .child(
                div()
                    .id("settings-status")
                    .flex()
                    .items_center()
                    .min_h(Metrics::STATUS_HEIGHT)
                    .max_h(px(84.0))
                    .flex_shrink_0()
                    .overflow_y_scroll()
                    .px_3()
                    .bg(theme.surface_raised)
                    .border_t_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(status),
            )
            .children(dropdown)
    }
}

/// A line of explanation under a section's heading.
fn note(text: &str, theme: &Theme) -> AnyElement {
    div()
        .text_xs()
        .text_color(theme.text_muted)
        .child(text.to_string())
        .into_any_element()
}

fn section_title(title: &str, theme: &Theme) -> AnyElement {
    div()
        .pt_2()
        .text_xs()
        .text_color(theme.text_faint)
        .child(title.to_string())
        .into_any_element()
}

/// How a captured key press turned out.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Captured<'a> {
    /// The command answers to it, and nothing else does.
    Bound,
    /// The command answers to it, and so does the command named here.
    Clashes(&'a str),
    /// The keymap would not take it.
    Refused,
}

/// What the footer says about a key press that has just been captured.
///
/// A free function rather than three lines inside the capture, because of the rule it turns on:
/// the keystroke a user sees is not the keystroke that is stored. `keystroke` arrives in the
/// portable spelling that goes into the file, and all three of these lines are read rather than
/// parsed — a footer saying "Save is now secondary-s" under a chip saying ⌘S is describing a
/// chord nobody pressed.
fn capture_status(
    language: Language,
    command: &str,
    keystroke: &str,
    outcome: Captured<'_>,
) -> String {
    let shown = crate::actions::menu_keystroke(keystroke);
    match outcome {
        Captured::Bound => messages::binding_set(language, command, &shown),
        Captured::Clashes(other) => {
            messages::binding_set_with_clash(language, command, &shown, other)
        }
        Captured::Refused => messages::binding_rejected(language, &shown),
    }
}

/// One line describing what a device can do.
fn describe(device: &AudioDeviceInfo, language: Language) -> String {
    let rates = match (device.sample_rates.first(), device.sample_rates.last()) {
        (Some(low), Some(high)) if low != high => {
            messages::rate_range(language, *low as f64 / 1000.0, *high as f64 / 1000.0)
        }
        (Some(rate), _) => messages::rate_single(language, *rate as f64 / 1000.0),
        _ => Key::RateUnknown.get(language).to_string(),
    };
    let detail = messages::device_detail(language, device.max_channels, &rates);
    if device.is_default {
        format!("{detail} · {}", Key::DeviceIsDefault.get(language))
    } else {
        detail
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn titlebar_tabs_remain_clickable_in_a_small_settings_window(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| this.open_settings(cx));
        cx.run_until_parked();
        let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
        handle
            .update(cx, |this, _, cx| {
                this.sync_appearance(
                    Appearance {
                        scheme: "daylight".into(),
                        ..Appearance::default()
                    },
                    Some(Language::Japanese),
                );
                cx.notify();
            })
            .unwrap();
        let cx = &mut gpui::VisualTestContext::from_window(handle.into(), cx);
        cx.simulate_resize(gpui::size(px(480.0), px(360.0)));
        cx.run_until_parked();
        let title = cx.debug_bounds("settings-title").unwrap();
        for (id, expected) in [
            ("tab-audio", SettingsTab::Audio),
            ("tab-keys", SettingsTab::Keys),
            ("tab-agent", SettingsTab::Agent),
            ("tab-general", SettingsTab::General),
        ] {
            let tab = cx.debug_bounds(id).unwrap();
            assert!(tab.top() >= title.top() && tab.bottom() <= title.bottom());
            assert!(tab.left() >= title.right() && tab.right() <= px(480.0));
            crate::harness::click(id, cx);
            handle
                .update(cx, |this, _, _| assert_eq!(this.tab, expected))
                .unwrap();
        }
        if !cfg!(target_os = "macos") {
            crate::harness::click("window-close", cx);
            assert!(handle.update(cx, |_, _, _| ()).is_err());
        }
    }

    #[test]
    fn the_footer_names_a_keystroke_the_way_the_platform_prints_it() {
        // `secondary-s` is the storage spelling and it stays that in the keymap; the chip in the
        // row already shows what the keyboard calls it. The footer two lines below was naming the
        // same key the other way, so the window said ⌘S and "Save is now secondary-s" at once.
        let shown = if cfg!(target_os = "macos") {
            "⌘S"
        } else {
            "Ctrl+S"
        };
        for line in [
            capture_status(Language::English, "Save", "secondary-s", Captured::Bound),
            capture_status(
                Language::English,
                "Save",
                "secondary-s",
                Captured::Clashes("Loop"),
            ),
            capture_status(Language::English, "Save", "secondary-s", Captured::Refused),
        ] {
            assert!(
                line.contains(shown),
                "`{line}` does not name the key this keyboard has"
            );
            assert!(
                !line.contains("secondary"),
                "`{line}` leaks the storage spelling into something a person reads"
            );
        }
    }

    #[test]
    fn reselecting_the_output_device_preserves_its_explicit_rate() {
        let current = AudioPreferences {
            device: Some("Studio Output".to_string()),
            sample_rate: Some(96_000),
            ..AudioPreferences::default()
        };

        assert_eq!(
            output_device_preferences(&current, Some("Studio Output".to_string())).sample_rate,
            Some(96_000)
        );
        assert_eq!(
            output_device_preferences(&current, Some("Other Output".to_string())).sample_rate,
            None,
            "a different device must renegotiate its supported rate"
        );
    }
}
