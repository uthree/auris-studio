//! The settings window: audio device selection and key bindings.
//!
//! A separate gpui window rather than a panel, because settings are not part of editing a
//! project and should not compete with it for space. It holds a weak handle to the main view
//! and applies every change through that, so there is still exactly one owner of the session.

use auris_session::prelude::*;
use auris_session::session::AudioStatus;
use gpui::{
    AnyElement, App, Context, FocusHandle, Focusable, IntoElement, KeyDownEvent, Render,
    WeakEntity, Window, div, prelude::*, px,
};

use crate::actions::{BINDABLE, Bindable};
use crate::app::AurisApp;
use crate::keymap::Keymap;
use crate::theme::{Metrics, Theme};
use crate::ui::icons::Icon;
use crate::ui::widgets::{ButtonStyle, button, chain_button, divider};

/// Which page the settings window is showing.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SettingsTab {
    /// Output device, sample rate and buffer size.
    Audio,
    /// Key bindings.
    Keys,
}

/// The settings window's view.
pub struct SettingsWindow {
    app: WeakEntity<AurisApp>,
    theme: Theme,
    tab: SettingsTab,
    /// Output devices, read once when the window opens.
    devices: Vec<OutputDeviceInfo>,
    audio: AudioPreferences,
    keymap: Keymap,
    /// What the audio backend is actually doing.
    ///
    /// Cached rather than read during render: the window is opened from inside the main
    /// view's update, and reading an entity that is already being updated panics.
    live: Option<AudioStatus>,
    /// Command whose next keystroke is being captured, if any.
    capturing: Option<&'static Bindable>,
    status: String,
    focus: FocusHandle,
}

impl Focusable for SettingsWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl SettingsWindow {
    /// Builds the window's view.
    ///
    /// The state is handed in rather than read back through `app`: this runs inside the main
    /// view's own update, and reading an entity that is already being updated panics.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        app: WeakEntity<AurisApp>,
        theme: Theme,
        devices: Vec<OutputDeviceInfo>,
        audio: AudioPreferences,
        live: AudioStatus,
        keymap: Keymap,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            app,
            theme,
            tab: SettingsTab::Audio,
            devices,
            audio,
            live: Some(live),
            keymap,
            capturing: None,
            status: String::new(),
            focus: cx.focus_handle(),
        }
    }

    /// Hands new audio preferences to the session and reports what happened.
    fn apply_audio(&mut self, audio: AudioPreferences, cx: &mut Context<Self>) {
        self.audio = audio.clone();
        let outcome = self
            .app
            .update(cx, |app, _| app.apply_audio_preferences(audio))
            .unwrap_or_else(|_| Err("the main window has closed".to_string()));
        self.status = match outcome {
            Ok(status) => status,
            Err(error) => format!("Could not switch: {error}"),
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
                "tab-audio",
                "Audio",
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
                "Key Bindings",
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

    fn render_audio(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let audio = self.audio.clone();
        let live = self.live.clone();

        let mut rows: Vec<AnyElement> = Vec::new();

        rows.push(section_title("Output Device", &theme));
        rows.push(self.device_row(
            "device-default",
            "System Default",
            "Follows whatever macOS is set to",
            audio.device.is_none(),
            None,
            cx,
        ));
        for (index, device) in self.devices.clone().into_iter().enumerate() {
            let detail = describe(&device);
            let selected = audio.device.as_deref() == Some(device.name.as_str());
            rows.push(self.device_row(
                ("device", index),
                &device.name.clone(),
                &detail,
                selected,
                Some(device.name),
                cx,
            ));
        }

        rows.push(divider(&theme).into_any_element());
        rows.push(section_title("Sample Rate", &theme));
        rows.push(
            div()
                .flex()
                .flex_wrap()
                .gap_1()
                .child(self.rate_button("rate-auto", "Device Default", None, cx))
                .children(
                    self.rate_choices()
                        .into_iter()
                        .enumerate()
                        .map(|(index, rate)| {
                            self.rate_button(
                                ("rate", index),
                                format!("{:.1} kHz", rate as f64 / 1000.0),
                                Some(rate),
                                cx,
                            )
                        }),
                )
                .into_any_element(),
        );

        rows.push(divider(&theme).into_any_element());
        rows.push(section_title("Buffer Size", &theme));
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
                        button(
                            ("block", index),
                            format!("{frames} · {latency:.1} ms"),
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
                    .child(format!(
                        "Running: {} · {:.0} Hz · {} ch{}",
                        status.device,
                        status.sample_rate,
                        status.channels,
                        if status.running { "" } else { " (silent)" }
                    ))
                    .children(status.gpu.map(|gpu| div().child(format!("GPU: {gpu}"))))
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
                .iter()
                .find(|device| device.name == name)
                .filter(|device| !device.sample_rates.is_empty())
        });
        match chosen {
            Some(device) => device.sample_rates.clone(),
            None => AudioPreferences::RATE_CHOICES.to_vec(),
        }
    }

    fn device_row(
        &self,
        id: impl Into<gpui::ElementId>,
        name: &str,
        detail: &str,
        selected: bool,
        device: Option<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
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
                let audio = AudioPreferences {
                    device: device.clone(),
                    // The old rate may not exist on the new device, so let it choose.
                    sample_rate: None,
                    ..this.audio.clone()
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

    fn render_keys(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let capturing = self.capturing.map(|command| command.id);

        let mut rows: Vec<AnyElement> = Vec::new();
        let mut group = "";
        for (index, command) in BINDABLE.iter().enumerate() {
            if command.group != group {
                group = command.group;
                rows.push(section_title(group, &theme));
            }

            let keystroke = self.keymap.keystroke(command).to_string();
            let is_capturing = capturing == Some(command.id);
            let overridden = self.keymap.is_overridden(command);
            let conflicts = self.keymap.conflicts(&keystroke, command);

            rows.push(
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
                            .child(command.label),
                    )
                    .children((!conflicts.is_empty() && !is_capturing).then(|| {
                        div()
                            .text_xs()
                            .text_color(theme.mute)
                            .child(format!("also {}", conflicts[0].label))
                    }))
                    .child(div().w(px(128.0)).child(button(
                        ("bind", index),
                        if is_capturing {
                            "Press a key…".to_string()
                        } else {
                            keystroke
                        },
                        ButtonStyle::Normal,
                        is_capturing,
                        theme.accent,
                        &theme,
                        cx.listener(move |this, _, window, cx| {
                            this.capturing = Some(command);
                            // The capture reads key events, so the window must hold focus.
                            window.focus(&this.focus);
                            cx.notify();
                        }),
                    )))
                    .child(div().w(px(20.0)).child(if overridden {
                        chain_button(
                            ("reset", index),
                            Icon::Cross,
                            &theme,
                            cx.listener(move |this, _, _, cx| {
                                this.keymap.clear(command);
                                this.apply_keymap(cx);
                            }),
                        )
                        .into_any_element()
                    } else {
                        div().into_any_element()
                    }))
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
                "Restore Defaults",
                ButtonStyle::Normal,
                false,
                theme.accent,
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.keymap.reset();
                    this.capturing = None;
                    this.status = "Key bindings restored to defaults".to_string();
                    this.apply_keymap(cx);
                }),
            )))
            .into_any_element()
    }

    /// Turns a captured key press into a binding.
    fn on_key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(command) = self.capturing else {
            return;
        };
        // Escape abandons the capture rather than being bound: otherwise there would be no way
        // to back out once a row is armed.
        if event.keystroke.key == "escape" && !event.keystroke.modifiers.modified() {
            self.capturing = None;
            self.status = "Cancelled".to_string();
            cx.notify();
            return;
        }

        let keystroke = event.keystroke.unparse();
        self.capturing = None;
        if self.keymap.set(command, &keystroke) {
            let clash = self.keymap.conflicts(&keystroke, command);
            self.status = match clash.first() {
                Some(other) => format!(
                    "{} is now {keystroke} — also bound to {}",
                    command.label, other.label
                ),
                None => format!("{} is now {keystroke}", command.label),
            };
            self.apply_keymap(cx);
        } else {
            self.status = format!("{keystroke} cannot be used as a binding");
            cx.notify();
        }
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let tabs = self.render_tabs(cx);
        let body = match self.tab {
            SettingsTab::Audio => self.render_audio(cx),
            SettingsTab::Keys => self.render_keys(cx),
        };
        let status = self.status.clone();
        let capturing = self.capturing.is_some();

        div()
            .id("settings-root")
            .key_context("AurisSettings")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .text_color(theme.text)
            .font_family("Helvetica")
            .text_sm()
            // While a row is armed, swallow the key so it configures the binding instead of
            // firing whatever is currently bound to it.
            .when(capturing, |this| {
                this.on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.on_key(event, window, cx);
                }))
            })
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

fn section_title(title: &str, theme: &Theme) -> AnyElement {
    div()
        .pt_2()
        .text_xs()
        .text_color(theme.text_faint)
        .child(title.to_string())
        .into_any_element()
}

/// One line describing what a device can do.
fn describe(device: &OutputDeviceInfo) -> String {
    let rates = match (device.sample_rates.first(), device.sample_rates.last()) {
        (Some(low), Some(high)) if low != high => {
            format!(
                "{:.1}–{:.1} kHz",
                *low as f64 / 1000.0,
                *high as f64 / 1000.0
            )
        }
        (Some(rate), _) => format!("{:.1} kHz", *rate as f64 / 1000.0),
        _ => "rate unknown".to_string(),
    };
    let default = if device.is_default { " · default" } else { "" };
    format!("{} ch · {rates}{default}", device.max_channels)
}
