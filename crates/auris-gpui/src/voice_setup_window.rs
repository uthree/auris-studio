//! The setup window for external singing backends.
//!
//! It edits small connection/deployment files, never a project. The actual validation, process
//! launch and file writing live in `auris-session`; this window only gathers and presents values.

use std::path::PathBuf;
use std::process::Child;

use auris_i18n::{Key, Language, messages};
use auris_session::{
    DiffSingerSetup, VoicevoxCatalog, VoicevoxSetup, VoicevoxStyle, fetch_voicevox_catalog,
    start_voicevox_engine, write_diffsinger_config, write_voicevox_connection,
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
    catalog: Option<VoicevoxCatalog>,
    styles_selected: bool,
    advanced: bool,
    checking: bool,
    request_generation: u64,
    diffsinger: DiffSingerFields,
    engine: Option<Child>,
    status: String,
    status_failed: bool,
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
            catalog: None,
            styles_selected: false,
            advanced: false,
            checking: false,
            request_generation: 0,
            diffsinger: DiffSingerFields::default(),
            engine: None,
            status: String::new(),
            status_failed: false,
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
                    this.invalidate_request();
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
                    this.invalidate_request();
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
            ])
            .when(self.catalog.is_none(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(self.t(Key::VoiceSetupChooseSingers)),
                )
            })
            .when_some(self.catalog.as_ref(), |this, catalog| {
                this.child(self.render_styles(&catalog.query, true, cx))
                    .child(self.render_styles(&catalog.decode, false, cx))
            })
            .child(button(
                "voicevox-advanced",
                self.t(Key::VoiceSetupAdvanced),
                ButtonStyle::Ghost,
                self.advanced,
                theme.accent,
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.invalidate_request();
                    this.advanced = !this.advanced;
                    if !this.advanced
                        && !matches!(this.active, Field::VoicevoxName | Field::VoicevoxUrl)
                    {
                        this.active = Field::VoicevoxName;
                    }
                    cx.notify();
                }),
            ))
            .when(self.advanced, |this| {
                this.children([
                    self.render_field(Field::VoicevoxStyle, Key::VoiceSetupStyleName, cx),
                    self.render_field(Field::VoicevoxQuery, Key::VoiceSetupQueryStyle, cx),
                    self.render_field(Field::VoicevoxDecode, Key::VoiceSetupDecodeStyle, cx),
                    self.render_field(Field::VoicevoxSampleRate, Key::VoiceSetupSampleRate, cx),
                    self.render_field(Field::VoicevoxFrameRate, Key::VoiceSetupFrameRate, cx),
                    self.render_field(Field::VoicevoxEngine, Key::VoiceSetupEngine, cx),
                ])
            })
            .into_any_element()
    }

    fn render_styles(
        &self,
        styles: &[VoicevoxStyle],
        query: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
        let selected = if query {
            &self.voicevox.query
        } else {
            &self.voicevox.decode
        };
        let selected = selected.content().trim().parse::<u32>().ok();
        let id = if query {
            "voicevox-query-options"
        } else {
            "voicevox-decode-options"
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(if query {
                        Key::VoiceSetupQueryVoice
                    } else {
                        Key::VoiceSetupDecodeVoice
                    })),
            )
            .child(
                div()
                    .id(id)
                    .max_h(px(120.0))
                    .overflow_y_scroll()
                    .children(styles.iter().map(|style| {
                        let chosen = style.clone();
                        let style_id = style.id;
                        button(
                            (id, style.id as usize),
                            style.label(),
                            ButtonStyle::Normal,
                            selected == Some(style.id),
                            theme.accent,
                            &theme,
                            cx.listener(move |this, _, _, cx| {
                                this.select_style(&chosen, query);
                                cx.notify();
                            }),
                        )
                        .debug_selector(move || format!("{id}-{style_id}"))
                        .w_full()
                        .justify_start()
                        .h_auto()
                        .min_h(Metrics::CONTROL_HEIGHT)
                        .py_1()
                    })),
            )
            .into_any_element()
    }

    fn render_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let actions = div()
            .flex()
            .flex_wrap()
            .flex_shrink_0()
            .justify_end()
            .gap_2()
            .p_2()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.surface_raised);
        match self.tab {
            VoiceSetupTab::Voicevox => actions
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
                .child(
                    button(
                        "voicevox-check",
                        self.t(if self.checking {
                            Key::VoiceSetupChecking
                        } else {
                            Key::VoiceSetupLoadSingers
                        }),
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            if !this.checking {
                                this.check_connection(cx);
                            }
                        }),
                    )
                    .when(self.checking, |this| this.opacity(0.6)),
                )
                .child(button(
                    "voicevox-save",
                    self.t(Key::VoiceSetupSave),
                    ButtonStyle::Primary,
                    false,
                    theme.accent,
                    &theme,
                    cx.listener(|this, _, _, cx| this.save_voicevox(cx)),
                ))
                .into_any_element(),
            VoiceSetupTab::DiffSinger => actions
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
                ))
                .into_any_element(),
        }
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
                    this.advanced = true;
                    this.text_changed();
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
        self.invalidate_request();
        match start_voicevox_engine(PathBuf::from(self.voicevox.engine.content().trim()).as_path())
        {
            Ok(child) => {
                let pid = child.id();
                self.engine = Some(child);
                self.status = messages::voicevox_engine_started(self.language, pid);
            }
            Err(error) => {
                self.status = error.to_string();
                self.status_failed = true;
            }
        }
        cx.notify();
    }

    fn check_connection(&mut self, cx: &mut Context<Self>) {
        self.invalidate_request();
        let generation = self.request_generation;
        let url = self.voicevox.url.content().trim().to_string();
        self.checking = true;
        self.status = self.t(Key::VoiceSetupChecking).into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(
                    async move { fetch_voicevox_catalog(&url).map_err(|error| error.to_string()) },
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                this.finish_connection(generation, result);
                cx.notify();
            });
        })
        .detach();
    }

    fn finish_connection(&mut self, generation: u64, result: Result<VoicevoxCatalog, String>) {
        if generation != self.request_generation {
            return;
        }
        self.checking = false;
        match result {
            Ok(catalog) => {
                // A failed refresh or changed Engine must never silently replace a chosen
                // performer. Keep explicit IDs even when they are absent from the new list.
                if !self.styles_selected && !self.advanced {
                    let query = catalog.query.first().cloned();
                    let decode = catalog
                        .decode
                        .iter()
                        .find(|style| {
                            query
                                .as_ref()
                                .is_some_and(|query| query.singer == style.singer)
                        })
                        .or_else(|| catalog.decode.first())
                        .cloned();
                    if let Some(query) = query {
                        self.select_style(&query, true);
                    }
                    if let Some(decode) = decode {
                        self.select_style(&decode, false);
                    }
                }
                self.styles_selected = true;
                let selection = self.voicevox_setup().and_then(|setup| {
                    catalog
                        .validate_styles(&setup)
                        .map_err(|error| error.to_string())
                });
                self.status_failed = selection.is_err();
                self.status = match selection {
                    Ok(()) => messages::voicevox_engine_connected(self.language, &catalog.version),
                    Err(error) => error,
                };
                self.catalog = Some(catalog);
            }
            Err(error) => {
                self.catalog = None;
                self.status = error;
                self.status_failed = true;
            }
        }
    }

    fn select_style(&mut self, style: &VoicevoxStyle, query: bool) {
        self.invalidate_request();
        self.styles_selected = true;
        if query {
            self.voicevox.query = TextField::new(style.id.to_string());
        } else {
            let previous = self.catalog.as_ref().and_then(|catalog| {
                catalog.decode.iter().find(|entry| {
                    Some(entry.id) == self.voicevox.decode.content().trim().parse::<u32>().ok()
                })
            });
            let default_name = VoicevoxSetup::default().name;
            if self.voicevox.name.content() == default_name
                || previous.is_some_and(|entry| entry.singer == self.voicevox.name.content())
            {
                self.voicevox.name = TextField::new(style.singer.clone());
            }
            self.voicevox.decode = TextField::new(style.id.to_string());
            self.voicevox.style = TextField::new(style.label());
        }
    }

    fn invalidate_request(&mut self) {
        self.request_generation = self.request_generation.wrapping_add(1);
        self.checking = false;
        self.status.clear();
        self.status_failed = false;
    }

    fn save_voicevox(&mut self, cx: &mut Context<Self>) {
        self.invalidate_request();
        if self.catalog.is_none() && !self.advanced {
            self.status = self.t(Key::VoiceSetupChooseSingers).into();
            self.status_failed = true;
            cx.notify();
            return;
        }
        let result = self
            .voicevox_setup()
            .map_err(|error| error.to_string())
            .and_then(|setup| {
                if let Some(catalog) = &self.catalog {
                    catalog
                        .validate_styles(&setup)
                        .map_err(|error| error.to_string())?;
                }
                write_voicevox_connection(&setup).map_err(|error| error.to_string())
            });
        self.finish_save(result, None, cx);
    }

    fn save_diffsinger(&mut self, cx: &mut Context<Self>) {
        self.invalidate_request();
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
        self.status_failed = result.is_err();
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
                    app.singer_configuration_changed(&path, cx);
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
            self.active = if self.tab == VoiceSetupTab::Voicevox && !self.advanced {
                match self.active {
                    Field::VoicevoxName => Field::VoicevoxUrl,
                    _ => Field::VoicevoxName,
                }
            } else {
                self.active.next()
            };
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
            if effect == KeyEffect::Changed {
                self.text_changed();
            }
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

    fn text_changed(&mut self) {
        self.invalidate_request();
        if matches!(
            self.active,
            Field::VoicevoxStyle | Field::VoicevoxQuery | Field::VoicevoxDecode
        ) {
            self.styles_selected = true;
        }
        if self.active == Field::VoicevoxUrl {
            self.catalog = None;
        }
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
            .child(
                div()
                    .id("voice-setup-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .child(body),
            )
            .child(self.render_actions(cx))
            .child(
                div()
                    .id("voice-setup-status")
                    .min_h(Metrics::STATUS_HEIGHT)
                    .max_h(px(84.0))
                    .flex_shrink_0()
                    .overflow_y_scroll()
                    .px_3()
                    .flex()
                    .items_center()
                    .bg(theme.surface_raised)
                    .border_t_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(if self.status_failed {
                        theme.danger
                    } else {
                        theme.text_muted
                    })
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
                    view.invalidate_request();
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

    fn catalog() -> VoicevoxCatalog {
        let style = |id, singer: &str, name: &str| VoicevoxStyle {
            id,
            singer: singer.into(),
            name: name.into(),
        };
        VoicevoxCatalog {
            version: "0.25.0".into(),
            query: vec![style(71, "Alpha", "Guide"), style(72, "Teacher", "Guide")],
            decode: vec![
                style(81, "Alpha", "Normal"),
                style(82, "Setup test singer", "Soft"),
            ],
        }
    }

    #[gpui::test]
    fn named_singing_styles_save_the_advertised_ids_without_typing_numbers(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.open_voice_setup(VoiceSetupTab::Voicevox, cx)
        });
        cx.run_until_parked();
        let handle = app.read_with(cx, |this, _| this.voice_setup_window.unwrap());
        handle
            .update(cx, |this, _, cx| {
                this.finish_connection(this.request_generation, Ok(catalog()));
                assert_eq!(
                    this.voicevox_setup().unwrap().decode_style_id,
                    81,
                    "prefer the query singer's matching voice"
                );
                cx.notify();
            })
            .unwrap();
        let cx = &mut gpui::VisualTestContext::from_window(handle.into(), cx);
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("voice-setup-field-3").is_none(),
            "numeric IDs belong to Advanced"
        );
        crate::harness::click("voicevox-query-options-72", cx);
        crate::harness::click("voicevox-decode-options-82", cx);
        handle
            .update(cx, |this, _, _| {
                let setup = this.voicevox_setup().unwrap();
                assert_eq!(setup.query_style_id, 72);
                assert_eq!(setup.decode_style_id, 82);
                assert_eq!(setup.name, "Setup test singer");
                assert_eq!(setup.style_name, "Setup test singer / Soft");
            })
            .unwrap();
        cx.simulate_resize(size(px(480.0), px(360.0)));
        cx.run_until_parked();
        let save = cx
            .debug_bounds("voicevox-save")
            .expect("the save action remains visible");
        assert!(save.left() >= px(0.0) && save.right() <= px(480.0));
        assert!(save.top() >= px(0.0) && save.bottom() <= px(360.0));
        crate::harness::click("voicevox-save", cx);
        handle
            .update(cx, |this, _, _| {
                assert!(!this.status_failed, "{}", this.status)
            })
            .unwrap();
        let path = auris_session::config_dir()
            .join("Voices")
            .join("Setup test singer.voicevox.json");
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["format_version"], 1);
        assert_eq!(saved["styles"][0]["query_style_id"], 72);
        assert_eq!(saved["styles"][0]["decode_style_id"], 82);
        std::fs::remove_file(path).unwrap();
    }

    #[gpui::test]
    fn editing_fields_discards_stale_connection_success_and_failure(cx: &mut TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.open_voice_setup(VoiceSetupTab::Voicevox, cx)
        });
        cx.run_until_parked();
        let handle = app.read_with(cx, |this, _| this.voice_setup_window.unwrap());
        let cx = &mut gpui::VisualTestContext::from_window(handle.into(), cx);
        for (field, text, result) in [
            (
                "voice-setup-field-1",
                "http://new-engine.invalid:50021",
                Ok(catalog()),
            ),
            (
                "voice-setup-field-0",
                "My renamed voice",
                Err("old connection failed".into()),
            ),
        ] {
            let generation = handle
                .update(cx, |this, _, cx| {
                    this.checking = true;
                    this.status = this.t(Key::VoiceSetupChecking).into();
                    cx.notify();
                    this.request_generation
                })
                .unwrap();
            cx.run_until_parked();
            crate::harness::click(field, cx);
            cx.simulate_input(text);
            handle
                .update(cx, |this, _, _| {
                    assert!(
                        !this.checking,
                        "an edit ends the obsolete checking indication"
                    );
                    assert!(this.status.is_empty());
                    this.finish_connection(generation, result);
                    assert!(
                        this.catalog.is_none(),
                        "a reply for the old input must not choose a voice"
                    );
                    assert!(
                        this.status.is_empty(),
                        "a reply for old input must not replace the current status"
                    );
                    assert!(!this.status_failed);
                    assert_eq!(this.field_for(this.active).content(), text);
                })
                .unwrap();
        }
    }

    #[gpui::test]
    fn refreshing_singers_preserves_the_chosen_performer_after_failure(cx: &mut TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.open_voice_setup(VoiceSetupTab::Voicevox, cx)
        });
        cx.run_until_parked();
        let handle = app.read_with(cx, |this, _| this.voice_setup_window.unwrap());
        handle
            .update(cx, |this, _, cx| {
                this.finish_connection(this.request_generation, Ok(catalog()));
                cx.notify();
            })
            .unwrap();
        let cx = &mut gpui::VisualTestContext::from_window(handle.into(), cx);
        cx.run_until_parked();
        crate::harness::click("voicevox-query-options-72", cx);
        crate::harness::click("voicevox-decode-options-82", cx);
        handle
            .update(cx, |this, _, cx| {
                let chosen = this.voicevox_setup().unwrap();
                this.finish_connection(
                    this.request_generation,
                    Err("Engine is temporarily unavailable".into()),
                );
                assert!(this.status_failed);
                assert!(this.catalog.is_none());
                this.finish_connection(this.request_generation, Ok(catalog()));
                assert!(!this.status_failed, "{}", this.status);
                assert_eq!(this.voicevox_setup().unwrap(), chosen);

                let mut changed = catalog();
                changed.decode.retain(|style| style.id != 82);
                this.finish_connection(this.request_generation, Ok(changed));
                assert!(this.status_failed, "a missing performer needs correction");
                assert_eq!(this.voicevox_setup().unwrap(), chosen);
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        crate::harness::click("voicevox-save", cx);
        handle
            .update(cx, |this, _, _| {
                assert!(this.status_failed, "a missing performer cannot be saved");
                assert_eq!(this.voicevox_setup().unwrap().decode_style_id, 82);
            })
            .unwrap();
    }

    #[gpui::test]
    fn fetching_singers_and_saving_do_not_replace_an_unfinished_style_id(cx: &mut TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.open_voice_setup(VoiceSetupTab::Voicevox, cx)
        });
        cx.run_until_parked();
        let handle = app.read_with(cx, |this, _| this.voice_setup_window.unwrap());
        let cx = &mut gpui::VisualTestContext::from_window(handle.into(), cx);
        crate::harness::click("voicevox-advanced", cx);
        crate::harness::click("voice-setup-field-3", cx);
        cx.simulate_input("72x");
        cx.simulate_keystrokes("shift-left");
        let selection = handle
            .update(cx, |this, _, _| this.voicevox.query.selection())
            .unwrap();
        crate::harness::click("voicevox-advanced", cx);
        handle
            .update(cx, |this, _, cx| {
                this.finish_connection(this.request_generation, Ok(catalog()));
                assert!(this.status_failed);
                assert_eq!(this.voicevox.query.content(), "72x");
                assert_eq!(this.voicevox.query.selection(), selection);
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        crate::harness::click("voicevox-save", cx);
        handle
            .update(cx, |this, _, _| {
                assert!(this.status_failed);
                assert_eq!(this.voicevox.query.content(), "72x");
                assert_eq!(this.voicevox.query.selection(), selection);
            })
            .unwrap();
    }

    #[gpui::test]
    fn advanced_fields_scroll_but_actions_stay_reachable_on_both_tabs(cx: &mut TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.open_voice_setup(VoiceSetupTab::Voicevox, cx)
        });
        cx.run_until_parked();
        let handle = app.read_with(cx, |this, _| this.voice_setup_window.unwrap());
        let cx = &mut gpui::VisualTestContext::from_window(handle.into(), cx);
        cx.simulate_resize(size(px(480.0), px(320.0)));
        cx.run_until_parked();
        crate::harness::click("voicevox-advanced", cx);
        cx.run_until_parked();
        for action in ["voicevox-check", "voicevox-save"] {
            let bounds = cx.debug_bounds(action).unwrap();
            assert!(bounds.bottom() <= px(320.0) && bounds.right() <= px(480.0));
        }
        crate::harness::click("voice-tab-diffsinger", cx);
        cx.run_until_parked();
        for action in ["diff-choose-folder", "diff-save"] {
            let bounds = cx.debug_bounds(action).unwrap();
            assert!(bounds.bottom() <= px(320.0) && bounds.right() <= px(480.0));
        }
        crate::harness::click("voice-tab-voicevox", cx);
        cx.run_until_parked();
        crate::harness::click("voicevox-advanced", cx);
        cx.simulate_keystrokes("tab tab");
        handle
            .update(cx, |this, _, _| {
                assert_eq!(
                    this.active,
                    Field::VoicevoxName,
                    "Tab skips the now-hidden numeric fields"
                )
            })
            .unwrap();
    }

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
