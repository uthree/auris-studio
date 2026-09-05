//! The setup window for external singing backends.
//!
//! It edits small connection/deployment files, never a project. The actual validation, process
//! launch and file writing live in `auris-session`; this window only gathers and presents values.

use std::path::PathBuf;
use std::process::Child;

use auris_i18n::{Key, Language, messages};
use auris_session::{
    DiffSingerSetup, VoicevoxSetup, check_voicevox_connection, start_voicevox_engine,
    write_diffsinger_config, write_voicevox_connection,
};
use gpui::{
    AnyElement, App, Bounds, Context, FocusHandle, Focusable, IntoElement, KeyDownEvent, Render,
    TitlebarOptions, WeakEntity, Window, WindowBounds, WindowHandle, WindowOptions, div,
    prelude::*, px, size,
};

use crate::app::AurisApp;
use crate::theme::{Metrics, Theme};
use crate::ui::prompt::{editable_text, field_text};
use crate::ui::text_field::{HasTextField, KeyEffect, TextField};
use crate::ui::widgets::{ButtonStyle, button};

/// Which external backend is being configured.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VoiceSetupTab {
    /// A connection to VOICEVOX Engine.
    Voicevox,
    /// An OpenUtau-compatible DiffSinger deployment.
    DiffSinger,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Field {
    VoicevoxName,
    VoicevoxUrl,
    VoicevoxStyle,
    VoicevoxQuery,
    VoicevoxDecode,
    VoicevoxSampleRate,
    VoicevoxFrameRate,
    VoicevoxEngine,
    DiffFolder,
    DiffPhonemes,
    DiffAcoustic,
    DiffVocoder,
    DiffSampleRate,
    DiffHop,
    DiffMelBins,
    DiffMelBase,
}

impl Field {
    fn first(tab: VoiceSetupTab) -> Self {
        match tab {
            VoiceSetupTab::Voicevox => Self::VoicevoxName,
            VoiceSetupTab::DiffSinger => Self::DiffFolder,
        }
    }

    fn next(self) -> Self {
        use Field::*;
        match self {
            VoicevoxName => VoicevoxUrl,
            VoicevoxUrl => VoicevoxStyle,
            VoicevoxStyle => VoicevoxQuery,
            VoicevoxQuery => VoicevoxDecode,
            VoicevoxDecode => VoicevoxSampleRate,
            VoicevoxSampleRate => VoicevoxFrameRate,
            VoicevoxFrameRate => VoicevoxEngine,
            VoicevoxEngine => VoicevoxName,
            DiffFolder => DiffPhonemes,
            DiffPhonemes => DiffAcoustic,
            DiffAcoustic => DiffVocoder,
            DiffVocoder => DiffSampleRate,
            DiffSampleRate => DiffHop,
            DiffHop => DiffMelBins,
            DiffMelBins => DiffMelBase,
            DiffMelBase => DiffFolder,
        }
    }
}

struct VoicevoxFields {
    name: TextField,
    url: TextField,
    style: TextField,
    query: TextField,
    decode: TextField,
    sample_rate: TextField,
    frame_rate: TextField,
    engine: TextField,
}

impl Default for VoicevoxFields {
    fn default() -> Self {
        let setup = VoicevoxSetup::default();
        Self {
            name: TextField::new(setup.name),
            url: TextField::new(setup.url),
            style: TextField::new(setup.style_name),
            query: TextField::new(setup.query_style_id.to_string()),
            decode: TextField::new(setup.decode_style_id.to_string()),
            sample_rate: TextField::new(setup.sample_rate.to_string()),
            frame_rate: TextField::new(setup.frame_rate.to_string()),
            engine: TextField::new(String::new()),
        }
    }
}

struct DiffSingerFields {
    folder: TextField,
    phonemes: TextField,
    acoustic: TextField,
    vocoder: TextField,
    sample_rate: TextField,
    hop: TextField,
    mel_bins: TextField,
    mel_base: TextField,
    continuous: bool,
    variable_depth: bool,
    key_shift: bool,
    speed: bool,
}

impl Default for DiffSingerFields {
    fn default() -> Self {
        let setup = DiffSingerSetup::default();
        Self {
            folder: TextField::new(String::new()),
            phonemes: TextField::new(setup.phonemes),
            acoustic: TextField::new(setup.acoustic),
            vocoder: TextField::new(setup.vocoder),
            sample_rate: TextField::new(setup.sample_rate.to_string()),
            hop: TextField::new(setup.hop_size.to_string()),
            mel_bins: TextField::new(setup.num_mel_bins.to_string()),
            mel_base: TextField::new(setup.mel_base),
            continuous: setup.use_continuous_acceleration,
            variable_depth: setup.use_variable_depth,
            key_shift: setup.use_key_shift_embed,
            speed: setup.use_speed_embed,
        }
    }
}

/// A separate window for creating external-backend configuration files.
pub struct VoiceSetupWindow {
    app: WeakEntity<AurisApp>,
    theme: Theme,
    language: Language,
    tab: VoiceSetupTab,
    active: Field,
    voicevox: VoicevoxFields,
    diffsinger: DiffSingerFields,
    engine: Option<Child>,
    status: String,
    focus: FocusHandle,
}

impl Focusable for VoiceSetupWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl VoiceSetupWindow {
    pub(crate) fn new(
        app: WeakEntity<AurisApp>,
        theme: Theme,
        language: Language,
        tab: VoiceSetupTab,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            app,
            theme,
            language,
            tab,
            active: Field::first(tab),
            voicevox: VoicevoxFields::default(),
            diffsinger: DiffSingerFields::default(),
            engine: None,
            status: String::new(),
            focus: cx.focus_handle(),
        }
    }

    fn t(&self, key: Key) -> &'static str {
        key.get(self.language)
    }

    fn field_for(&self, field: Field) -> &TextField {
        use Field::*;
        match field {
            VoicevoxName => &self.voicevox.name,
            VoicevoxUrl => &self.voicevox.url,
            VoicevoxStyle => &self.voicevox.style,
            VoicevoxQuery => &self.voicevox.query,
            VoicevoxDecode => &self.voicevox.decode,
            VoicevoxSampleRate => &self.voicevox.sample_rate,
            VoicevoxFrameRate => &self.voicevox.frame_rate,
            VoicevoxEngine => &self.voicevox.engine,
            DiffFolder => &self.diffsinger.folder,
            DiffPhonemes => &self.diffsinger.phonemes,
            DiffAcoustic => &self.diffsinger.acoustic,
            DiffVocoder => &self.diffsinger.vocoder,
            DiffSampleRate => &self.diffsinger.sample_rate,
            DiffHop => &self.diffsinger.hop,
            DiffMelBins => &self.diffsinger.mel_bins,
            DiffMelBase => &self.diffsinger.mel_base,
        }
    }

    fn field_for_mut(&mut self, field: Field) -> &mut TextField {
        use Field::*;
        match field {
            VoicevoxName => &mut self.voicevox.name,
            VoicevoxUrl => &mut self.voicevox.url,
            VoicevoxStyle => &mut self.voicevox.style,
            VoicevoxQuery => &mut self.voicevox.query,
            VoicevoxDecode => &mut self.voicevox.decode,
            VoicevoxSampleRate => &mut self.voicevox.sample_rate,
            VoicevoxFrameRate => &mut self.voicevox.frame_rate,
            VoicevoxEngine => &mut self.voicevox.engine,
            DiffFolder => &mut self.diffsinger.folder,
            DiffPhonemes => &mut self.diffsinger.phonemes,
            DiffAcoustic => &mut self.diffsinger.acoustic,
            DiffVocoder => &mut self.diffsinger.vocoder,
            DiffSampleRate => &mut self.diffsinger.sample_rate,
            DiffHop => &mut self.diffsinger.hop,
            DiffMelBins => &mut self.diffsinger.mel_bins,
            DiffMelBase => &mut self.diffsinger.mel_base,
        }
    }

    fn render_field(&self, field: Field, label: Key, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let value = self.field_for(field);
        let active = self.active == field;
        let body = match active {
            true => editable_text(
                value.content().to_string().into(),
                value.selection(),
                value.marked(),
                self.focus.clone(),
                cx.entity(),
                theme.clone(),
            )
            .into_any_element(),
            false => field_text(value.content().to_string(), theme.text).into_any_element(),
        };
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .w(px(170.0))
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(label)),
            )
            .child(
                div()
                    .id(("voice-setup-field", field as usize))
                    .debug_selector(move || format!("voice-setup-field-{}", field as usize))
                    .h(px(28.0))
                    .flex_1()
                    .min_w_0()
                    .rounded(Metrics::RADIUS_SM)
                    .bg(theme.surface_sunken)
                    .border_1()
                    .border_color(if active { theme.accent } else { theme.border })
                    .child(body)
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.active = field;
                            this.field_for_mut(field).select_all();
                            window.focus(&this.focus);
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
            )
            .into_any_element()
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        div()
            .flex()
            .gap_1()
            .p_2()
            .bg(theme.surface_raised)
            .border_b_1()
            .border_color(theme.border)
            .child(button(
                "voice-tab-voicevox",
                self.t(Key::VoiceSetupVoicevox),
                ButtonStyle::Normal,
                self.tab == VoiceSetupTab::Voicevox,
                theme.accent,
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.tab = VoiceSetupTab::Voicevox;
                    this.active = Field::first(this.tab);
                    cx.notify();
                }),
            ))
            .child(button(
                "voice-tab-diffsinger",
                self.t(Key::VoiceSetupDiffSinger),
                ButtonStyle::Normal,
                self.tab == VoiceSetupTab::DiffSinger,
                theme.accent,
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.tab = VoiceSetupTab::DiffSinger;
                    this.active = Field::first(this.tab);
                    cx.notify();
                }),
            ))
            .into_any_element()
    }

    fn render_voicevox(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .children([
                self.render_field(Field::VoicevoxName, Key::VoiceSetupName, cx),
                self.render_field(Field::VoicevoxUrl, Key::VoiceSetupUrl, cx),
                self.render_field(Field::VoicevoxStyle, Key::VoiceSetupStyleName, cx),
                self.render_field(Field::VoicevoxQuery, Key::VoiceSetupQueryStyle, cx),
                self.render_field(Field::VoicevoxDecode, Key::VoiceSetupDecodeStyle, cx),
                self.render_field(Field::VoicevoxSampleRate, Key::VoiceSetupSampleRate, cx),
                self.render_field(Field::VoicevoxFrameRate, Key::VoiceSetupFrameRate, cx),
                self.render_field(Field::VoicevoxEngine, Key::VoiceSetupEngine, cx),
            ])
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(button(
                        "voicevox-choose-engine",
                        self.t(Key::VoiceSetupChooseEngine),
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| this.choose_engine(cx)),
                    ))
                    .child(button(
                        "voicevox-start",
                        self.t(Key::VoiceSetupStartEngine),
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| this.start_engine(cx)),
                    ))
                    .child(button(
                        "voicevox-check",
                        self.t(Key::VoiceSetupCheck),
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| this.check_connection(cx)),
                    ))
                    .child(button(
                        "voicevox-save",
                        self.t(Key::VoiceSetupSave),
                        ButtonStyle::Primary,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| this.save_voicevox(cx)),
                    )),
            )
            .into_any_element()
    }

    fn render_diffsinger(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let toggles = [
            (
                Key::VoiceSetupContinuous,
                self.diffsinger.continuous,
                0usize,
            ),
            (
                Key::VoiceSetupVariableDepth,
                self.diffsinger.variable_depth,
                1,
            ),
            (Key::VoiceSetupKeyShift, self.diffsinger.key_shift, 2),
            (Key::VoiceSetupSpeed, self.diffsinger.speed, 3),
        ];
        div()
            .flex()
            .flex_col()
            .gap_2()
            .children([
                self.render_field(Field::DiffFolder, Key::VoiceSetupFolder, cx),
                self.render_field(Field::DiffPhonemes, Key::VoiceSetupPhonemes, cx),
                self.render_field(Field::DiffAcoustic, Key::VoiceSetupAcoustic, cx),
                self.render_field(Field::DiffVocoder, Key::VoiceSetupVocoder, cx),
                self.render_field(Field::DiffSampleRate, Key::VoiceSetupSampleRate, cx),
                self.render_field(Field::DiffHop, Key::VoiceSetupHopSize, cx),
                self.render_field(Field::DiffMelBins, Key::VoiceSetupMelBins, cx),
                self.render_field(Field::DiffMelBase, Key::VoiceSetupMelBase, cx),
            ])
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .children(toggles.into_iter().map(|(label, active, index)| {
                        button(
                            ("diff-option", index),
                            self.t(label),
                            ButtonStyle::Normal,
                            active,
                            theme.accent,
                            &theme,
                            cx.listener(move |this, _, _, cx| {
                                match index {
                                    0 => this.diffsinger.continuous = !this.diffsinger.continuous,
                                    1 => {
                                        this.diffsinger.variable_depth =
                                            !this.diffsinger.variable_depth
                                    }
                                    2 => this.diffsinger.key_shift = !this.diffsinger.key_shift,
                                    _ => this.diffsinger.speed = !this.diffsinger.speed,
                                }
                                cx.notify();
                            }),
                        )
                    })),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(button(
                        "diff-choose-folder",
                        self.t(Key::VoiceSetupChooseFolder),
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| this.choose_voicebank(cx)),
                    ))
                    .child(button(
                        "diff-save",
                        self.t(Key::VoiceSetupWriteConfig),
                        ButtonStyle::Primary,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| this.save_diffsinger(cx)),
                    )),
            )
            .into_any_element()
    }

    fn voicevox_setup(&self) -> Result<VoicevoxSetup, String> {
        Ok(VoicevoxSetup {
            name: self.voicevox.name.content().trim().into(),
            url: self.voicevox.url.content().trim().into(),
            style_name: self.voicevox.style.content().trim().into(),
            query_style_id: parse(
                &self.voicevox.query,
                self.t(Key::VoiceSetupQueryStyle),
                self.language,
            )?,
            decode_style_id: parse(
                &self.voicevox.decode,
                self.t(Key::VoiceSetupDecodeStyle),
                self.language,
            )?,
            sample_rate: parse(
                &self.voicevox.sample_rate,
                self.t(Key::VoiceSetupSampleRate),
                self.language,
            )?,
            frame_rate: parse(
                &self.voicevox.frame_rate,
                self.t(Key::VoiceSetupFrameRate),
                self.language,
            )?,
        })
    }

    fn diffsinger_setup(&self) -> Result<DiffSingerSetup, String> {
        Ok(DiffSingerSetup {
            folder: PathBuf::from(self.diffsinger.folder.content().trim()),
            phonemes: self.diffsinger.phonemes.content().trim().into(),
            acoustic: self.diffsinger.acoustic.content().trim().into(),
            vocoder: self.diffsinger.vocoder.content().trim().into(),
            sample_rate: parse(
                &self.diffsinger.sample_rate,
                self.t(Key::VoiceSetupSampleRate),
                self.language,
            )?,
            hop_size: parse(
                &self.diffsinger.hop,
                self.t(Key::VoiceSetupHopSize),
                self.language,
            )?,
            num_mel_bins: parse(
                &self.diffsinger.mel_bins,
                self.t(Key::VoiceSetupMelBins),
                self.language,
            )?,
            mel_base: self.diffsinger.mel_base.content().trim().into(),
            use_continuous_acceleration: self.diffsinger.continuous,
            use_variable_depth: self.diffsinger.variable_depth,
            use_key_shift_embed: self.diffsinger.key_shift,
            use_speed_embed: self.diffsinger.speed,
        })
    }

    fn choose_engine(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let picked = rfd::AsyncFileDialog::new().pick_file().await;
            if let Some(picked) = picked {
                let path = picked.path().display().to_string();
                let _ = this.update(cx, |this, cx| {
                    this.voicevox.engine = TextField::new(path);
                    this.active = Field::VoicevoxEngine;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn choose_voicebank(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let picked = rfd::AsyncFileDialog::new().pick_folder().await;
            if let Some(picked) = picked {
                let path = picked.path().display().to_string();
                let _ = this.update(cx, |this, cx| {
                    this.diffsinger.folder = TextField::new(path);
                    this.active = Field::DiffFolder;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn start_engine(&mut self, cx: &mut Context<Self>) {
        match start_voicevox_engine(PathBuf::from(self.voicevox.engine.content().trim()).as_path())
        {
            Ok(child) => {
                let pid = child.id();
                self.engine = Some(child);
                self.status = messages::voicevox_engine_started(self.language, pid);
            }
            Err(error) => self.status = error.to_string(),
        }
        cx.notify();
    }

    fn check_connection(&mut self, cx: &mut Context<Self>) {
        let setup = match self.voicevox_setup() {
            Ok(setup) => setup,
            Err(error) => {
                self.status = error;
                cx.notify();
                return;
            }
        };
        self.status = self.t(Key::VoiceSetupChecking).into();
        cx.spawn(async move |this, cx| {
            let result = check_voicevox_connection(&setup);
            let _ = this.update(cx, |this, cx| {
                this.status = match result {
                    Ok(version) => messages::voicevox_engine_connected(this.language, &version),
                    Err(error) => error.to_string(),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn save_voicevox(&mut self, cx: &mut Context<Self>) {
        let result = self
            .voicevox_setup()
            .map_err(|error| error.to_string())
            .and_then(|setup| write_voicevox_connection(&setup).map_err(|error| error.to_string()));
        self.finish_save(result, None, cx);
    }

    fn save_diffsinger(&mut self, cx: &mut Context<Self>) {
        let (result, root) = match self.diffsinger_setup() {
            Ok(setup) => {
                let root = setup.folder.parent().map(PathBuf::from);
                (
                    write_diffsinger_config(&setup).map_err(|error| error.to_string()),
                    root,
                )
            }
            Err(error) => (Err(error), None),
        };
        self.finish_save(result, root, cx);
    }

    fn finish_save(
        &mut self,
        result: Result<PathBuf, String>,
        voice_root: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.status = match result {
            Ok(path) => {
                let _ = self.app.update(cx, |app, cx| {
                    if let Some(root) = voice_root
                        && !app.settings.voice_paths.contains(&root)
                    {
                        app.settings.voice_paths.push(root);
                        if let Err(error) = app.settings.save() {
                            log::warn!("could not save the DiffSinger voice folder: {error}");
                        }
                    }
                    app.voices = None;
                    cx.notify();
                });
                messages::voice_setup_saved(self.language, &path.display().to_string())
            }
            Err(error) => error,
        };
        cx.notify();
    }

    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let key = event.keystroke.key.as_str();
        if key == "tab" && !event.keystroke.modifiers.modified() {
            self.active = self.active.next();
            self.field_for_mut(self.active).select_all();
            cx.notify();
            return true;
        }
        let effect = self.field_for_mut(self.active).apply_key_with_clipboard(
            key,
            event.keystroke.modifiers.shift,
            event.keystroke.modifiers.secondary(),
            false,
            cx,
        );
        if effect != KeyEffect::Ignored {
            cx.notify();
            true
        } else {
            false
        }
    }
}

impl HasTextField for VoiceSetupWindow {
    fn field(&mut self) -> Option<&mut TextField> {
        Some(self.field_for_mut(self.active))
    }

    fn readable_field(&self) -> Option<&TextField> {
        Some(self.field_for(self.active))
    }
}

crate::entity_input_handler!(VoiceSetupWindow);

impl Render for VoiceSetupWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focus.is_focused(window) {
            window.focus(&self.focus);
        }
        let theme = self.theme.clone();
        let body = match self.tab {
            VoiceSetupTab::Voicevox => self.render_voicevox(cx),
            VoiceSetupTab::DiffSinger => self.render_diffsinger(cx),
        };
        div()
            .id("voice-setup-root")
            .key_context("AurisVoiceSetup")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .text_color(theme.text)
            .font(crate::theme::ui_font())
            .text_sm()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if this.on_key(event, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(self.render_tabs(cx))
            .child(div().flex_1().min_h_0().overflow_hidden().p_3().child(body))
            .child(
                div()
                    .h(Metrics::STATUS_HEIGHT)
                    .px_3()
                    .flex()
                    .items_center()
                    .bg(theme.surface_raised)
                    .border_t_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.status.clone()),
            )
    }
}

impl AurisApp {
    /// Opens the external singing-backend setup window on `tab`.
    pub(crate) fn open_voice_setup(&mut self, tab: VoiceSetupTab, cx: &mut Context<Self>) {
        if let Some(handle) = self.voice_setup_window
            && handle
                .update(cx, |view, window, cx| {
                    view.tab = tab;
                    view.active = Field::first(tab);
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
        let bounds = Bounds::centered(None, size(px(680.0), px(650.0)), cx);
        let opened: Result<WindowHandle<VoiceSetupWindow>, _> = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(self.t(Key::VoiceSetupTitle).into()),
                    ..Default::default()
                }),
                focus: true,
                ..Default::default()
            },
            |_, cx| cx.new(|cx| VoiceSetupWindow::new(app, theme, language, tab, cx)),
        );
        match opened {
            Ok(handle) => self.voice_setup_window = Some(handle),
            Err(error) => self.set_status(error.to_string()),
        }
    }
}

fn parse<T: std::str::FromStr>(
    field: &TextField,
    label: &str,
    language: Language,
) -> Result<T, String> {
    field
        .content()
        .trim()
        .parse()
        .map_err(|_| messages::voice_setup_invalid_field(language, label))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn voice_connection_fields_accept_clipboard_shortcuts(cx: &mut TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.open_voice_setup(VoiceSetupTab::Voicevox, cx)
        });
        cx.run_until_parked();
        let handle = app.read_with(cx, |this, _| this.voice_setup_window.unwrap());
        let cx = &mut gpui::VisualTestContext::from_window(handle.into(), cx);
        crate::harness::click("voice-setup-field-1", cx);
        cx.update(|_, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                "http://voice.invalid:50021".into(),
            ));
        });
        cx.simulate_keystrokes("secondary-v");
        handle
            .update(cx, |this, _, _| {
                assert_eq!(this.active, Field::VoicevoxUrl);
                assert_eq!(this.voicevox.url.content(), "http://voice.invalid:50021");
            })
            .unwrap();
        cx.simulate_keystrokes("secondary-a secondary-c secondary-x");
        handle
            .update(cx, |this, _, _| assert_eq!(this.voicevox.url.content(), ""))
            .unwrap();
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard()
                    .and_then(|item| item.text())
                    .as_deref(),
                Some("http://voice.invalid:50021")
            );
        });
        cx.simulate_keystrokes("secondary-v");
        handle
            .update(cx, |this, _, _| {
                assert_eq!(this.voicevox.url.content(), "http://voice.invalid:50021");
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_setup_window_opens_on_the_requested_backend(cx: &mut TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.open_voice_setup(VoiceSetupTab::Voicevox, cx)
        });
        cx.run_until_parked();
        let handle = app.read_with(cx, |this, _| {
            this.voice_setup_window.expect("the setup window opened")
        });
        handle
            .update(cx, |this, _, _| {
                assert_eq!(this.tab, VoiceSetupTab::Voicevox);
                let setup = this.voicevox_setup().expect("the defaults are valid");
                assert_eq!(setup.url, "http://127.0.0.1:50021");
                assert_eq!(setup.query_style_id, 6000);
                assert_eq!(setup.decode_style_id, 3001);
            })
            .unwrap();

        app.update(cx, |this, cx| {
            this.open_voice_setup(VoiceSetupTab::DiffSinger, cx)
        });
        handle
            .update(cx, |this, _, _| {
                assert_eq!(this.tab, VoiceSetupTab::DiffSinger);
                assert_eq!(this.active, Field::DiffFolder);
            })
            .unwrap();
    }
}
