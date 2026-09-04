//! The settings window: audio device selection and key bindings.
//!
//! A separate gpui window rather than a panel, because settings are not part of editing a
//! project and should not compete with it for space. It holds a weak handle to the main view
//! and applies every change through that, so there is still exactly one owner of the session.

use auris_i18n::{Key, Language, messages};
use auris_session::prelude::*;
use auris_session::session::AudioStatus;
use gpui::{
    AnyElement, App, Context, FocusHandle, Focusable, IntoElement, KeyDownEvent, Render,
    WeakEntity, Window, div, prelude::*, px,
};

use crate::actions::{BINDABLE, Bindable};
use crate::app::AurisApp;
use crate::gestures::{PointerGesture, PointerGestures};
use crate::keymap::Keymap;
use crate::theme::{Metrics, SCHEMES, Theme, scheme_or_default};
use crate::ui::icons::Icon;
use crate::ui::palette;
use crate::ui::text_field::{HasTextField, KeyEffect, TextField};
use crate::ui::widgets::{ButtonStyle, button, chain_button, divider};

/// Which page the settings window is showing.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SettingsTab {
    /// Interface language and anything else that is not audio or keys.
    General,
    /// Output device, input device, sample rate and buffer size.
    Audio,
    /// Key bindings.
    Keys,
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
    tab: SettingsTab,
    /// What the host can see, read once when the window opens.
    devices: AudioDevices,
    audio: AudioPreferences,
    keymap: Keymap,
    /// Stored language preference; `None` follows the system.
    language_preference: Option<Language>,
    /// Language this window is drawn in, which is the resolved preference.
    language: Language,
    /// Whether the document is written back over itself as it changes.
    autosave: bool,
    /// The dictionary folder kanji lyrics are read through. `None` on most machines.
    japanese_dictionary: Option<std::path::PathBuf>,
    /// Where singer voices run their inference.
    singer_acceleration: Acceleration,
    /// What a click creates and what deletes.
    /// How a bounce is written.
    export: ExportPreferences,
    pointer: PointerGestures,
    /// What the audio backend is actually doing.
    ///
    /// Cached rather than read during render: the window is opened from inside the main
    /// view's update, and reading an entity that is already being updated panics.
    live: Option<AudioStatus>,
    /// Which slot the next key press is being captured into, if any.
    capturing: Option<Capture>,
    /// What the key list is being filtered by.
    ///
    /// A real text field rather than a string built from key events, because the labels are
    /// translated: a Japanese user filtering on 「トラック」 needs the IME, and a field that reads
    /// key events never sees a composition at all.
    search: TextField,
    status: String,
    focus: FocusHandle,
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
    pub(crate) fn sync_appearance(&mut self, theme: Theme, language_preference: Option<Language>) {
        self.theme = theme;
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
        theme: Theme,
        devices: AudioDevices,
        audio: AudioPreferences,
        live: AudioStatus,
        keymap: Keymap,
        language_preference: Option<Language>,
        pointer: PointerGestures,
        autosave: bool,
        japanese_dictionary: Option<std::path::PathBuf>,
        singer_acceleration: Acceleration,
        export: ExportPreferences,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            app,
            theme,
            tab: SettingsTab::General,
            devices,
            audio,
            live: Some(live),
            keymap,
            language_preference,
            language: Language::resolve(language_preference),
            autosave,
            japanese_dictionary,
            singer_acceleration,
            export,
            pointer,
            capturing: None,
            search: TextField::new(String::new()),
            status: String::new(),
            focus: cx.focus_handle(),
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

    /// A row of gesture buttons for one of the two actions.
    ///
    /// `offered` is what the row lists, which is not the same for both: the bare click may create
    /// and may not delete, and a button that swapped the two into an arrangement
    /// [`PointerGestures::set_delete`] refuses would look like the panel ignoring a click.
    fn gesture_row(
        &self,
        id: &'static str,
        label: Key,
        current: PointerGesture,
        offered: fn(PointerGesture) -> bool,
        assign: fn(&mut PointerGestures, PointerGesture),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
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
            .children(
                PointerGesture::ALL
                    .into_iter()
                    .filter(|gesture| offered(*gesture))
                    .enumerate()
                    .map(|(index, gesture)| {
                        button(
                            (id, index),
                            self.t(gesture.label()),
                            ButtonStyle::Normal,
                            current == gesture,
                            theme.accent,
                            &theme,
                            cx.listener(move |this, _, _, cx| {
                                let mut pointer = this.pointer;
                                assign(&mut pointer, gesture);
                                this.apply_pointer(pointer, cx);
                            }),
                        )
                    }),
            )
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

    /// Repaints both windows in another colour scheme.
    ///
    /// This window holds its own copy of the palette rather than reading the application's, so it
    /// has to be told as well — otherwise the change would be visible everywhere except in the
    /// window where it was made.
    fn apply_scheme(&mut self, id: &'static str, cx: &mut Context<Self>) {
        self.theme = Theme::named(id);
        let _ = self.app.update(cx, |app, cx| app.apply_scheme(id, cx));
        self.status = messages::scheme_changed(self.language, scheme_or_default(id).name);
        cx.notify();
    }

    /// Hands new audio preferences to the session and reports what happened.
    fn apply_audio(&mut self, audio: AudioPreferences, cx: &mut Context<Self>) {
        let requested = audio.clone();
        let Ok(outcome) = self
            .app
            .update(cx, |app, _| app.apply_audio_preferences(audio))
        else {
            return;
        };
        self.status = match outcome {
            Ok(status) => {
                self.audio = requested;
                status
            }
            Err(error) => crate::i18n::error_text(&error, self.language),
        };
        // The previous update has finished, so reading back is safe here.
        self.live = self
            .app
            .read_with(cx, |app, _| app.session.audio_status())
            .ok();
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
            .gap_1()
            .p_2()
            .bg(theme.surface_raised)
            .border_b_1()
            .border_color(theme.border)
            .child(button(
                "tab-general",
                self.t(Key::TabGeneral),
                ButtonStyle::Normal,
                tab == SettingsTab::General,
                theme.accent,
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.tab = SettingsTab::General;
                    this.capturing = None;
                    cx.notify();
                }),
            ))
            .child(button(
                "tab-audio",
                self.t(Key::TabAudio),
                ButtonStyle::Normal,
                tab == SettingsTab::Audio,
                theme.accent,
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.tab = SettingsTab::Audio;
                    this.capturing = None;
                    cx.notify();
                }),
            ))
            .child(button(
                "tab-keys",
                self.t(Key::TabKeys),
                ButtonStyle::Normal,
                tab == SettingsTab::Keys,
                theme.accent,
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.tab = SettingsTab::Keys;
                    cx.notify();
                }),
            ))
    }

    /// The General page: for now, the interface language.
    fn render_general(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let current = self.language_preference;

        // "System" first, then each language in its own name — a picker written in a language
        // you cannot read is no use to the person who needs it.
        let mut choices: Vec<(Option<Language>, &'static str)> =
            vec![(None, self.t(Key::LanguageFollowSystem))];
        choices.extend(Language::ALL.map(|language| (Some(language), language.endonym())));

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(section_title(self.t(Key::AppearanceHeading), &theme))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .children(SCHEMES.iter().enumerate().map(|(index, scheme)| {
                        button(
                            ("scheme", index),
                            scheme.name,
                            ButtonStyle::Normal,
                            theme.scheme == scheme.id,
                            theme.accent,
                            &theme,
                            cx.listener(move |this, _, _, cx| this.apply_scheme(scheme.id, cx)),
                        )
                    })),
            )
            .child(divider(&theme))
            .child(section_title(self.t(Key::LanguageHeading), &theme))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .children(
                        choices
                            .into_iter()
                            .enumerate()
                            .map(|(index, (choice, label))| {
                                button(
                                    ("language", index),
                                    label,
                                    ButtonStyle::Normal,
                                    current == choice,
                                    theme.accent,
                                    &theme,
                                    cx.listener(move |this, _, _, cx| {
                                        this.apply_language(choice, cx)
                                    }),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::LanguageNote)),
            )
            .child(divider(&theme))
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
            .child(divider(&theme))
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
            .child(divider(&theme))
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
                                None => match auris_session::library::installed_dictionary() {
                                    Some(_) => self.t(Key::ValueShippedDictionary).to_string(),
                                    None => self.t(Key::ValueNotSet).to_string(),
                                },
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
            .child(divider(&theme))
            .child(section_title(self.t(Key::SingerComputeHeading), &theme))
            .child(
                div().flex().gap_1().children(
                    [
                        (Acceleration::Auto, self.t(Key::SingerComputeAuto)),
                        // Initialisms, not words: the same three letters in every language.
                        (Acceleration::Gpu, "GPU"),
                        (Acceleration::Cpu, "CPU"),
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(index, (choice, label))| {
                        button(
                            ("singer-compute", index),
                            label,
                            ButtonStyle::Normal,
                            self.singer_acceleration == choice,
                            theme.accent,
                            &theme,
                            cx.listener(move |this, _, _, cx| {
                                this.apply_singer_acceleration(choice, cx)
                            }),
                        )
                    }),
                ),
            )
            .child(note(self.t(Key::SingerComputeNote), &theme))
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

        rows.push(section_title(self.t(Key::OutputDevice), &theme));
        rows.push(self.device_row(
            "device-default",
            self.t(Key::SystemDefaultDevice),
            self.t(Key::SystemDefaultDeviceDetail),
            None,
            DeviceSlot::Output,
            cx,
        ));
        for (index, device) in self.devices.output.clone().into_iter().enumerate() {
            let detail = describe(&device, self.language);
            rows.push(self.device_row(
                ("device", index),
                &device.name.clone(),
                &detail,
                Some(device.name),
                DeviceSlot::Output,
                cx,
            ));
        }

        rows.push(divider(&theme).into_any_element());
        rows.push(section_title(self.t(Key::InputDevice), &theme));
        rows.push(note(self.t(Key::InputDeviceNote), &theme));
        rows.push(self.device_row(
            "input-default",
            self.t(Key::SystemDefaultDevice),
            self.t(Key::SystemDefaultDeviceDetail),
            None,
            DeviceSlot::Input,
            cx,
        ));
        for (index, device) in self.devices.input.clone().into_iter().enumerate() {
            let detail = describe(&device, self.language);
            rows.push(self.device_row(
                ("input", index),
                &device.name.clone(),
                &detail,
                Some(device.name),
                DeviceSlot::Input,
                cx,
            ));
        }

        rows.push(divider(&theme).into_any_element());
        rows.push(section_title(self.t(Key::SampleRate), &theme));
        rows.push(
            div()
                .flex()
                .flex_wrap()
                .gap_1()
                .child(self.rate_button("rate-auto", self.t(Key::DeviceDefaultRate), None, cx))
                .children(
                    self.rate_choices()
                        .into_iter()
                        .enumerate()
                        .map(|(index, rate)| {
                            self.rate_button(
                                ("rate", index),
                                messages::rate_single(self.language, rate as f64 / 1000.0),
                                Some(rate),
                                cx,
                            )
                        }),
                )
                .into_any_element(),
        );

        rows.push(divider(&theme).into_any_element());
        rows.push(section_title(self.t(Key::BufferSize), &theme));
        rows.push(
            div()
                .flex()
                .flex_wrap()
                .gap_1()
                .children(AudioPreferences::BLOCK_CHOICES.into_iter().enumerate().map(
                    |(index, frames)| {
                        let selected = audio.block_frames == frames;
                        let rate = live.as_ref().map_or(48_000.0, |status| status.sample_rate);
                        let latency = frames as f64 / rate.max(1.0) * 1000.0;
                        let label = messages::buffer_choice(self.language, frames, latency);
                        button(
                            ("block", index),
                            label,
                            ButtonStyle::Normal,
                            selected,
                            theme.accent,
                            &theme,
                            cx.listener(move |this, _, _, cx| {
                                let audio = AudioPreferences {
                                    block_frames: frames,
                                    ..this.audio.clone()
                                };
                                this.apply_audio(audio, cx);
                            }),
                        )
                    },
                ))
                .into_any_element(),
        );

        // How a bounce is written. Here rather than in a dialog in front of the save sheet,
        // because it is a fact about the person and their delivery rather than about the song:
        // an export that asks three questions every time is one people stop using for a quick
        // listen. On the Audio page rather than a page of its own — these are the same three
        // numbers as the ones above them, aimed at a file instead of at a device.
        rows.push(divider(&theme).into_any_element());
        rows.push(section_title(self.t(Key::ExportFormat), &theme));
        rows.push(
            div()
                .flex()
                .flex_wrap()
                .gap_1()
                .children(
                    [WavBitDepth::Int16, WavBitDepth::Int24, WavBitDepth::Float32]
                        .into_iter()
                        .enumerate()
                        .map(|(index, depth)| {
                            button(
                                ("depth", index),
                                depth.label(),
                                ButtonStyle::Normal,
                                export.bit_depth == depth,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, _, _, cx| {
                                    let export = ExportPreferences {
                                        bit_depth: depth,
                                        ..this.export
                                    };
                                    this.apply_export(export, cx);
                                }),
                            )
                        }),
                )
                .into_any_element(),
        );

        rows.push(section_title(self.t(Key::ExportRate), &theme));
        rows.push(
            div()
                .flex()
                .flex_wrap()
                .gap_1()
                // The project's own rate first, and the default: an export at any other rate
                // resamples the whole mix, which is a thing to ask for rather than to inherit
                // from whatever the output device happens to be running at.
                .child(self.export_rate_button(
                    "export-rate-project",
                    self.t(Key::ProjectRate),
                    None,
                    cx,
                ))
                .children(AudioPreferences::RATE_CHOICES.into_iter().enumerate().map(
                    |(index, rate)| {
                        self.export_rate_button(
                            ("export-rate", index),
                            messages::rate_single(self.language, rate as f64 / 1000.0),
                            Some(rate),
                            cx,
                        )
                    },
                ))
                .into_any_element(),
        );

        rows.push(section_title(self.t(Key::ExportDither), &theme));
        // Off and unusable at a float depth rather than hidden: a control that disappears when
        // a neighbour moves reads as a bug in the window. There is nothing to dither *to* when
        // the file stores what the render produced.
        let dithers = export.dither_applies();
        let on = export.dither && dithers;
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
                    cx.listener(move |this, _, _, cx| {
                        if !this.export.dither_applies() {
                            return;
                        }
                        let export = ExportPreferences {
                            dither: !this.export.dither,
                            ..this.export
                        };
                        this.apply_export(export, cx);
                    }),
                ))
                .into_any_element(),
        );
        rows.push(note(self.t(Key::ExportDitherNote), &theme));

        if let Some(status) = live {
            rows.push(divider(&theme).into_any_element());
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

    /// Sample rates worth offering: what the chosen device advertises, or a sensible list.
    fn rate_choices(&self) -> Vec<u32> {
        let chosen = self.audio.device.as_deref().and_then(|name| {
            self.devices
                .output
                .iter()
                .find(|device| device.name == name)
                .filter(|device| !device.sample_rates.is_empty())
        });
        match chosen {
            Some(device) => device.sample_rates.clone(),
            None => AudioPreferences::RATE_CHOICES.to_vec(),
        }
    }

    /// One device in one of the two lists. `device` of `None` is the system-default row.
    ///
    /// Whether it reads as chosen is worked out here rather than handed in, because it is exactly
    /// the same question the click answers: the row that writes `Some("Scarlett")` into the input
    /// slot is the row that is lit when the input slot holds it, and two callers deciding that
    /// separately is two chances to disagree.
    fn device_row(
        &self,
        id: impl Into<gpui::ElementId>,
        name: &str,
        detail: &str,
        device: Option<String>,
        slot: DeviceSlot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
        let selected = match slot {
            DeviceSlot::Output => self.audio.device == device,
            DeviceSlot::Input => self.audio.input_device == device,
        };
        div()
            .id(id.into())
            .flex()
            .items_center()
            .gap_2()
            .p_2()
            .rounded(Metrics::RADIUS_SM)
            .bg(if selected {
                theme.accent_soft
            } else {
                theme.surface_sunken
            })
            .border_1()
            .border_color(if selected {
                theme.accent
            } else {
                theme.border_subtle
            })
            .cursor_pointer()
            .hover(|this| this.border_color(theme.border))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text)
                            .truncate()
                            .child(name.to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .truncate()
                            .child(detail.to_string()),
                    ),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                let audio = match slot {
                    DeviceSlot::Output => output_device_preferences(&this.audio, device.clone()),
                    // The rate is left alone: it belongs to the output, and a take asks for the
                    // project's rate rather than for whatever is set here.
                    DeviceSlot::Input => AudioPreferences {
                        input_device: device.clone(),
                        ..this.audio.clone()
                    },
                };
                this.apply_audio(audio, cx);
            }))
            .into_any_element()
    }

    fn rate_button(
        &self,
        id: impl Into<gpui::ElementId>,
        label: impl Into<gpui::SharedString>,
        rate: Option<u32>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
        button(
            id.into(),
            label.into(),
            ButtonStyle::Normal,
            self.audio.sample_rate == rate,
            theme.accent,
            &theme,
            cx.listener(move |this, _, _, cx| {
                let audio = AudioPreferences {
                    sample_rate: rate,
                    ..this.audio.clone()
                };
                this.apply_audio(audio, cx);
            }),
        )
        .into_any_element()
    }

    /// One choice of rate to render an export at.
    ///
    /// Deliberately not [`Self::rate_button`]: that one drives the *device*, and a render is not
    /// a device. The lists even differ — an export can be asked for a rate no output here can
    /// play, which is the ordinary case for delivering 44.1 kHz from a 48 kHz rig.
    fn export_rate_button(
        &self,
        id: impl Into<gpui::ElementId>,
        label: impl Into<gpui::SharedString>,
        rate: Option<u32>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
        button(
            id.into(),
            label.into(),
            ButtonStyle::Normal,
            self.export.sample_rate == rate,
            theme.accent,
            &theme,
            cx.listener(move |this, _, _, cx| {
                let export = ExportPreferences {
                    sample_rate: rate,
                    ..this.export
                };
                this.apply_export(export, cx);
            }),
        )
        .into_any_element()
    }

    /// The commands the search box is showing.
    ///
    /// Matched on the group and the name together, in this language and in English, by the same
    /// scoring the command palette uses — so a query means the same thing in both lists. Filtered
    /// but *not* reordered: this list is arranged under section headings, and sorting by score
    /// would scatter the sections.
    fn found_commands(&self) -> Vec<&'static Bindable> {
        let query = self.search.content();
        BINDABLE
            .iter()
            .filter(|command| {
                let drawn = format!("{} {}", self.t(command.group), self.t(command.label));
                let english = palette::english_haystack(
                    self.language,
                    command.group,
                    command.label.get(Language::English),
                );
                palette::best_score(query, &drawn, english.as_deref()).is_some()
            })
            .collect()
    }

    fn render_keys(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let found = self.found_commands();
        let search = self.render_search(cx);

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
            .child(search)
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
                            .child(self.t(Key::SearchCommands))
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
                            self.focus.clone(),
                            cx.entity(),
                            theme.clone(),
                        )
                        .into_any_element()
                    }),
            )
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.capturing.is_some() {
            self.capture_key(event, cx);
            return true;
        }
        if self.tab != SettingsTab::Keys {
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
            "escape" if !self.search.content().is_empty() => {
                self.search = TextField::new(String::new());
                true
            }
            // Backspace, the caret, Select All. Shared with every field in the main window
            // rather than written out again here — this box was the fourth copy of the same
            // table, and the four had already drifted apart.
            key => self.search.apply_key(key, shift, secondary) != KeyEffect::Ignored,
        };
        if claimed {
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
    fn field(&mut self) -> Option<&mut TextField> {
        // Only while the list is on screen and no row is waiting for a key press — the same two
        // conditions under which the box is drawn as an editable one.
        (self.tab == SettingsTab::Keys && self.capturing.is_none()).then_some(&mut self.search)
    }

    fn readable_field(&self) -> Option<&TextField> {
        (self.tab == SettingsTab::Keys && self.capturing.is_none()).then_some(&self.search)
    }
}

crate::entity_input_handler!(SettingsWindow);

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // This window has one focusable thing in it, and both the key capture and the search box
        // need it: a text field only registers itself as the window's input handler while its own
        // handle is focused, and a key press only reaches `on_key` through the focused element.
        if !self.focus.is_focused(window) {
            window.focus(&self.focus);
        }

        let theme = self.theme.clone();
        let tabs = self.render_tabs(cx);
        let body = match self.tab {
            SettingsTab::General => self.render_general(cx),
            SettingsTab::Audio => self.render_audio(cx),
            SettingsTab::Keys => self.render_keys(cx),
        };
        let status = self.status.clone();

        div()
            .id("settings-root")
            .key_context("AurisSettings")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .text_color(theme.text)
            .font(crate::theme::ui_font())
            .text_sm()
            // Swallowed only when it was wanted: a key that fills in a binding or edits the
            // search box must not also fire whatever is bound to it, and every other key should
            // carry on to wherever it was going.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.on_key(event, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(tabs)
            .child(
                div()
                    .id("settings-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .child(body),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(Metrics::STATUS_HEIGHT)
                    .px_3()
                    .bg(theme.surface_raised)
                    .border_t_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(status),
            )
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
