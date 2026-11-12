//! Appearance controls and a draft editor whose changes apply only when saved.

use super::*;
use crate::appearance::{Appearance, CustomScheme};
use crate::theme::GradientStop;
use gpui::SharedString;

struct GradientFields {
    fields: [TextField; 2],
    focus: [Option<FocusHandle>; 3],
}

impl GradientFields {
    fn new(stop: &GradientStop) -> Self {
        Self {
            fields: [
                TextField::new((stop.position * 100.0).to_string()),
                palette_field(Some(stop.color)),
            ],
            focus: [None, None, None],
        }
    }
}

/// The editable part of a theme, kept separate from the applied preferences.
pub(super) struct AppearanceEditor {
    editing_id: Option<String>,
    base: String,
    name: TextField,
    accent: TextField,
    palette: [TextField; 8],
    signals: [TextField; 4],
    chords: [TextField; 7],
    velocity: [TextField; 2],
    stops: Vec<GradientFields>,
    active: ThemeField,
}

/// The id a field is drawn and found under, for every field but a gradient stop's.
fn theme_field_id(field: ThemeField) -> Option<&'static str> {
    match field {
        ThemeField::Name => Some("theme-name"),
        ThemeField::Accent => Some("theme-accent"),
        ThemeField::Palette(index) => Some(PALETTE_FIELDS[index]),
        ThemeField::Signal(index) => Some(SIGNAL_FIELDS[index]),
        ThemeField::Chord(index) => Some(CHORD_FIELDS[index]),
        ThemeField::Velocity(index) => Some(VELOCITY_FIELDS[index]),
        ThemeField::Stop(..) => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum ThemeField {
    Name,
    Accent,
    Palette(usize),
    Signal(usize),
    Chord(usize),
    Velocity(usize),
    Stop(usize, usize),
}

const PALETTE_FIELDS: [&str; 8] = [
    "theme-color-1",
    "theme-color-2",
    "theme-color-3",
    "theme-color-4",
    "theme-color-5",
    "theme-color-6",
    "theme-color-7",
    "theme-color-8",
];

const VELOCITY_FIELDS: [&str; 2] = ["theme-velocity-soft", "theme-velocity-loud"];
const CHORD_FIELDS: [&str; 7] = [
    "theme-chord-1",
    "theme-chord-2",
    "theme-chord-3",
    "theme-chord-4",
    "theme-chord-5",
    "theme-chord-6",
    "theme-chord-7",
];
const CHORD_LABELS: [&str; 7] = ["I", "II", "III", "IV", "V", "VI", "VII"];
const SIGNAL_FIELDS: [&str; 4] = [
    "theme-signal-active",
    "theme-signal-warning",
    "theme-signal-danger",
    "theme-signal-mute",
];
const SIGNAL_LABELS: [Key; 4] = [
    Key::ThemeSignalActive,
    Key::ThemeSignalWarning,
    Key::ThemeSignalDanger,
    Key::ThemeSignalMute,
];

impl AppearanceEditor {
    /// Replaces inherited colours when the base changes, keeping explicit draft edits.
    fn set_base(&mut self, id: String, appearance: &Appearance) {
        let (Some(previous), Some(next)) = (
            appearance.scheme_definition(&self.base),
            appearance.scheme_definition(&id),
        ) else {
            return;
        };
        let previous = CustomScheme::from_scheme(String::new(), String::new(), &previous);
        let next = CustomScheme::from_scheme(String::new(), String::new(), &next);
        if parse_colour(self.accent.content()) == Some(previous.accent) {
            self.accent = palette_field(Some(next.accent));
        }
        for (index, field) in self.palette.iter_mut().enumerate() {
            let inherited = match previous.track_palette[index] {
                Some(color) => parse_colour(field.content()) == Some(color),
                None => field.content().trim().is_empty(),
            };
            if inherited {
                *field = palette_field(next.track_palette[index]);
            }
        }
        for (index, field) in self.velocity.iter_mut().enumerate() {
            let inherited = match previous.velocity_palette[index] {
                Some(color) => parse_colour(field.content()) == Some(color),
                None => field.content().trim().is_empty(),
            };
            if inherited {
                *field = palette_field(next.velocity_palette[index]);
            }
        }
        for (index, field) in self.chords.iter_mut().enumerate() {
            let inherited = match previous.chord_palette[index] {
                Some(color) => parse_colour(field.content()) == Some(color),
                None => field.content().trim().is_empty(),
            };
            if inherited {
                *field = palette_field(next.chord_palette[index]);
            }
        }
        for (index, field) in self.signals.iter_mut().enumerate() {
            let inherited = match previous.signal_palette[index] {
                Some(color) => parse_colour(field.content()) == Some(color),
                None => field.content().trim().is_empty(),
            };
            if inherited {
                *field = palette_field(next.signal_palette[index]);
            }
        }
        if self
            .parsed_stops()
            .is_ok_and(|stops| stops == previous.velocity_stops)
        {
            self.stops = next
                .velocity_stops
                .iter()
                .map(GradientFields::new)
                .collect();
            if matches!(self.active, ThemeField::Stop(..)) {
                self.active = ThemeField::Name;
            }
        }
        self.base = id;
    }

    pub(super) fn field(&mut self) -> &mut TextField {
        self.field_of(self.active)
    }

    fn field_of(&mut self, field: ThemeField) -> &mut TextField {
        match field {
            ThemeField::Name => &mut self.name,
            ThemeField::Accent => &mut self.accent,
            ThemeField::Palette(index) => &mut self.palette[index],
            ThemeField::Signal(index) => &mut self.signals[index],
            ThemeField::Chord(index) => &mut self.chords[index],
            ThemeField::Velocity(index) => &mut self.velocity[index],
            ThemeField::Stop(index, component) => &mut self.stops[index].fields[component],
        }
    }

    pub(super) fn readable_field(&self) -> &TextField {
        match self.active {
            ThemeField::Name => &self.name,
            ThemeField::Accent => &self.accent,
            ThemeField::Palette(index) => &self.palette[index],
            ThemeField::Signal(index) => &self.signals[index],
            ThemeField::Chord(index) => &self.chords[index],
            ThemeField::Velocity(index) => &self.velocity[index],
            ThemeField::Stop(index, component) => &self.stops[index].fields[component],
        }
    }

    fn parsed_stops(&self) -> Result<Vec<GradientStop>, Key> {
        let mut stops = self
            .stops
            .iter()
            .map(|row| {
                let position = row.fields[0]
                    .content()
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| Key::ThemeVelocityInvalid)?
                    / 100.0;
                if !position.is_finite() || position <= 0.0 || position >= 1.0 {
                    return Err(Key::ThemeVelocityInvalid);
                }
                Ok(GradientStop {
                    position,
                    color: parse_colour(row.fields[1].content())
                        .ok_or(Key::ThemeVelocityInvalid)?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        stops.sort_by(|left, right| left.position.total_cmp(&right.position));
        if stops
            .windows(2)
            .any(|pair| pair[0].position == pair[1].position)
        {
            return Err(Key::ThemeVelocityInvalid);
        }
        Ok(stops)
    }

    fn draft(&self, appearance: &Appearance) -> Result<CustomScheme, Key> {
        let name = self.name.content().trim();
        if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
            return Err(Key::ThemeNameRequired);
        }
        let duplicate = SCHEMES
            .iter()
            .any(|s| s.name.to_lowercase() == name.to_lowercase())
            || appearance.custom_schemes.iter().any(|s| {
                Some(&s.id) != self.editing_id.as_ref()
                    && s.name.to_lowercase() == name.to_lowercase()
            });
        if duplicate {
            return Err(Key::ThemeNameExists);
        }
        let base = appearance
            .scheme_definition(&self.base)
            .ok_or(Key::ThemeInvalid)?;
        let id = self
            .editing_id
            .clone()
            .unwrap_or_else(|| next_theme_id(appearance));
        let mut draft = CustomScheme::from_scheme(id, name.to_owned(), &base);
        self.apply_colors(&mut draft)?;
        draft.validate().map_err(|_| Key::ThemeInvalid)?;
        Ok(draft)
    }

    fn preview(&self, appearance: &Appearance) -> Result<Theme, Key> {
        let base = appearance
            .scheme_definition(&self.base)
            .ok_or(Key::ThemeInvalid)?;
        let mut draft = CustomScheme::from_scheme(String::new(), String::new(), &base);
        self.apply_colors(&mut draft)?;
        Ok(Theme::from_scheme(&draft.definition()))
    }

    fn apply_colors(&self, draft: &mut CustomScheme) -> Result<(), Key> {
        draft.accent = parse_colour(self.accent.content()).ok_or(Key::ThemeAccentInvalid)?;
        for (index, field) in self.palette.iter().enumerate() {
            draft.track_palette[index] = if field.content().trim().is_empty() {
                None
            } else {
                Some(parse_colour(field.content()).ok_or(Key::ThemeAccentInvalid)?)
            };
        }
        for (index, field) in self.velocity.iter().enumerate() {
            draft.velocity_palette[index] = if field.content().trim().is_empty() {
                None
            } else {
                Some(parse_colour(field.content()).ok_or(Key::ThemeAccentInvalid)?)
            };
        }
        draft.velocity_stops = self.parsed_stops()?;
        for (index, field) in self.chords.iter().enumerate() {
            draft.chord_palette[index] = if field.content().trim().is_empty() {
                None
            } else {
                Some(parse_colour(field.content()).ok_or(Key::ThemeAccentInvalid)?)
            };
        }
        for (index, field) in self.signals.iter().enumerate() {
            draft.signal_palette[index] = if field.content().trim().is_empty() {
                None
            } else {
                Some(parse_colour(field.content()).ok_or(Key::ThemeAccentInvalid)?)
            };
        }
        Ok(())
    }
}

fn parse_colour(text: &str) -> Option<u32> {
    let text = text.trim().strip_prefix('#').unwrap_or(text.trim());
    (text.len() == 6 && text.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(text, 16).ok())
        .flatten()
}

fn palette_field(color: Option<u32>) -> TextField {
    TextField::new(
        color
            .map(|color| format!("#{color:06X}"))
            .unwrap_or_default(),
    )
}

fn next_theme_id(appearance: &Appearance) -> String {
    (1..)
        .map(|number| format!("custom-{number}"))
        .find(|id| appearance.scheme_definition(id).is_none())
        .expect("a finite collection always has a free identifier")
}

impl SettingsWindow {
    fn scheme_choices(&self) -> Vec<(String, String, String)> {
        SCHEMES
            .iter()
            .map(|s| (s.id.to_owned(), s.name.to_owned(), String::new()))
            .chain(
                self.appearance
                    .custom_schemes
                    .iter()
                    .map(|s| (s.id.clone(), s.name.clone(), String::new())),
            )
            .collect()
    }

    pub(super) fn render_appearance(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let schemes = self.scheme_choices();
        let selected = theme.scheme.clone();
        let font = self.appearance.font_family.clone();
        let mut fonts = vec![(None, self.t(Key::UiFontDefault).to_owned(), String::new())];
        fonts.extend(
            self.font_families
                .iter()
                .cloned()
                .map(|name| (Some(name.clone()), name, String::new())),
        );
        if let Some(name) = &font
            && !self.font_families.contains(name)
        {
            fonts.push((
                Some(name.clone()),
                name.clone(),
                self.t(Key::SettingUnavailable).to_owned(),
            ));
        }
        let custom = self
            .appearance
            .custom_schemes
            .iter()
            .any(|s| s.id == selected);
        let picker = self.dropdown(
            "scheme",
            schemes,
            &selected,
            |this, id, cx| {
                let mut appearance = this.appearance.clone();
                appearance.scheme = id;
                this.apply_appearance_preferences(appearance, cx);
            },
            cx,
        );
        let font_picker = self.dropdown(
            "ui-font",
            fonts,
            &font,
            |this, font, cx| {
                let mut appearance = this.appearance.clone();
                appearance.font_family = font;
                this.apply_appearance_preferences(appearance, cx);
            },
            cx,
        );
        let mut content = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(section_title(self.t(Key::AppearanceHeading), &theme))
            .child(picker)
            .when(self.appearance_editor.is_none(), |content| {
                content.child(
                    FormField::new(self.t(Key::GlassEffect))
                        .description(self.t(Key::GlassEffectNote))
                        .beside()
                        .child(
                            Switch::new("glass-effect", self.appearance.glass_effect).on_change(
                                cx.listener(|this, enabled: &bool, _, cx| {
                                    let mut appearance = this.appearance.clone();
                                    appearance.glass_effect = *enabled;
                                    this.apply_appearance_preferences(appearance, cx);
                                }),
                            ),
                        ),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(self.appearance_button(
                        "create-theme",
                        Key::CreateTheme,
                        |this, window, cx| this.open_theme_editor(false, window, cx),
                        cx,
                    ))
                    .child(
                        Button::new("import-theme", self.t(Key::ImportTheme))
                            .disabled(self.appearance_editor.is_some())
                            .on_click(cx.listener(|this, _, _, cx| this.import_theme(cx))),
                    )
                    .when(custom, |row| {
                        row.child(self.appearance_button(
                            "edit-theme",
                            Key::EditTheme,
                            |this, window, cx| this.open_theme_editor(true, window, cx),
                            cx,
                        ))
                    }),
            );
        if self.appearance_editor.is_none() {
            content = content.child(note(self.t(Key::ImportThemeNote), &theme));
        }
        if self.appearance_editor.is_some() {
            content = content.child(self.render_theme_editor(cx));
        }
        content
            .child(section_title(self.t(Key::UiFont), &theme))
            .child(font_picker)
            .child(note(self.t(Key::UiFontNote), &theme))
            .into_any_element()
    }

    fn open_theme_editor(&mut self, editing: bool, window: &mut Window, cx: &mut Context<Self>) {
        let base = self.appearance.selected_scheme();
        let copy = CustomScheme::from_scheme(String::new(), String::new(), &base);
        self.appearance_editor = Some(AppearanceEditor {
            editing_id: editing.then(|| self.appearance.scheme.clone()),
            base: self.appearance.scheme.clone(),
            name: TextField::new(if editing {
                base.name.to_owned()
            } else {
                String::new()
            }),
            accent: TextField::new(format!("#{:06X}", copy.accent)),
            palette: copy.track_palette.map(palette_field),
            signals: copy.signal_palette.map(palette_field),
            chords: copy.chord_palette.map(palette_field),
            velocity: copy.velocity_palette.map(palette_field),
            stops: copy
                .velocity_stops
                .iter()
                .map(GradientFields::new)
                .collect(),
            active: ThemeField::Name,
        });
        let focus = self.theme_input_focus(ThemeField::Name, cx);
        window.focus(&focus);
        cx.notify();
    }

    fn import_theme(&mut self, cx: &mut Context<Self>) {
        let title = self.t(Key::ImportTheme).to_owned();
        cx.spawn(async move |this, cx| {
            let Some(file) = rfd::AsyncFileDialog::new()
                .set_title(title)
                .add_filter("VS Code colour theme", &["json", "jsonc"])
                .pick_file()
                .await
            else {
                return;
            };
            let path = file.path().to_owned();
            let result = cx
                .background_executor()
                .spawn(async move { crate::theme_import::import_vscode(&path) })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(theme) => this.apply_imported_theme(theme, cx),
                    Err(error) => {
                        this.status = format!("{}: {error}", this.t(Key::ImportThemeFailed))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn apply_imported_theme(&mut self, mut theme: CustomScheme, cx: &mut Context<Self>) {
        let name = theme.name.clone();
        // Preserve existing presets and imports even when several files use the same name.
        for number in 1.. {
            if !self
                .scheme_choices()
                .iter()
                .any(|(_, existing, _)| existing.to_lowercase() == theme.name.to_lowercase())
            {
                break;
            }
            theme.name = format!("{} ({number})", name.chars().take(65).collect::<String>());
        }
        theme.id = next_theme_id(&self.appearance);
        let mut appearance = self.appearance.clone();
        appearance.scheme = theme.id.clone();
        appearance.custom_schemes.push(theme);
        self.apply_appearance_preferences(appearance, cx);
    }

    pub(super) fn apply_appearance_preferences(
        &mut self,
        appearance: Appearance,
        cx: &mut Context<Self>,
    ) {
        let appearance = appearance.normalised();
        let outcome = self.app.update(cx, |app, cx| {
            let result = app.apply_appearance(appearance.clone(), cx);
            cx.notify();
            result
        });
        match outcome {
            Ok(result) => {
                self.theme = appearance.theme();
                self.appearance = appearance;
                self.status = match result {
                    Ok(()) => self.t(Key::AppearanceSaved).to_owned(),
                    Err(error) => format!("{}: {error}", self.t(Key::AppearanceSaveFailed)),
                };
            }
            Err(error) => self.status = error.to_string(),
        }
        cx.notify();
    }

    fn save_theme_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &self.appearance_editor else {
            return;
        };
        let draft = match editor.draft(&self.appearance) {
            Ok(draft) => draft,
            Err(key) => {
                self.status = self.t(key).to_owned();
                cx.notify();
                return;
            }
        };
        let mut appearance = self.appearance.clone();
        appearance.scheme = draft.id.clone();
        if let Some(existing) = appearance
            .custom_schemes
            .iter_mut()
            .find(|s| s.id == draft.id)
        {
            *existing = draft;
        } else {
            appearance.custom_schemes.push(draft);
        }
        self.appearance_editor = None;
        self.apply_appearance_preferences(appearance, cx);
    }

    fn render_theme_editor(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let editor = self.appearance_editor.as_ref().expect("the editor is open");
        let base = editor.base.clone();
        let draft = editor.draft(&self.appearance);
        let preview = editor.preview(&self.appearance).unwrap_or(theme.clone());
        let stop_count = editor.stops.len();
        let base_picker = self.dropdown(
            "theme-base",
            self.scheme_choices(),
            &base,
            |this, id, cx| {
                if let Some(editor) = &mut this.appearance_editor {
                    editor.set_base(id, &this.appearance);
                }
                cx.notify();
            },
            cx,
        );
        div()
            .id("theme-editor")
            .debug_selector(|| "theme-editor".into())
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .border_1()
            .border_color(theme.border)
            .rounded_md()
            .child(section_title(self.t(Key::ThemeName), &theme))
            .child(self.theme_text_field(ThemeField::Name, cx))
            .child(section_title(self.t(Key::ThemeBase), &theme))
            .child(base_picker)
            .child(section_title(self.t(Key::ThemeAccent), &theme))
            .child(self.theme_text_field(ThemeField::Accent, cx))
            .child(section_title(self.t(Key::ThemeTrackPalette), &theme))
            .child(note(self.t(Key::ThemeTrackPaletteNote), &theme))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children((0..8).map(|index| {
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .w(px(250.0))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .w(px(14.0))
                                    .h(px(14.0))
                                    .rounded_sm()
                                    .bg(preview.track_palette[index]),
                            )
                            .child(
                                div()
                                    .w(px(90.0))
                                    .flex_shrink_0()
                                    .text_xs()
                                    .child(crate::theme::palette_label(self.language, index)),
                            )
                            .child(self.theme_text_field(ThemeField::Palette(index), cx))
                    })),
            )
            .child(section_title(self.t(Key::ThemeChordPalette), &theme))
            .child(note(self.t(Key::ThemeChordNote), &theme))
            .child(div().flex().flex_wrap().gap_2().children(
                CHORD_LABELS.into_iter().enumerate().map(|(index, label)| {
                    let fill = preview.chord_fill(index as u8 + 1, false);
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .w(px(250.0))
                        .child(
                            div()
                                .w(px(42.0))
                                .flex_shrink_0()
                                .text_xs()
                                .rounded_sm()
                                .p_1()
                                .bg(fill)
                                .text_color(preview.text_on(fill))
                                .child(label),
                        )
                        .child(self.theme_text_field(ThemeField::Chord(index), cx))
                }),
            ))
            .child(section_title(self.t(Key::ThemeSignalPalette), &theme))
            .child(note(self.t(Key::ThemeSignalNote), &theme))
            .child(div().flex().flex_wrap().gap_2().children(
                SIGNAL_LABELS.into_iter().enumerate().map(|(index, label)| {
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .w(px(250.0))
                        .child(div().text_xs().child(self.t(label)))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .w(px(14.0))
                                        .h(px(14.0))
                                        .flex_shrink_0()
                                        .rounded_sm()
                                        .bg(preview.signal_palette[index]),
                                )
                                .child(self.theme_text_field(ThemeField::Signal(index), cx)),
                        )
                }),
            ))
            .child(section_title(self.t(Key::ThemeVelocityPalette), &theme))
            .child(note(self.t(Key::ThemeVelocityPaletteNote), &theme))
            .child(
                div().flex().flex_wrap().gap_2().children(
                    [Key::ThemeVelocitySoft, Key::ThemeVelocityLoud]
                        .into_iter()
                        .enumerate()
                        .map(|(index, label)| {
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .w(px(250.0))
                                .child(div().text_xs().child(self.t(label)))
                                .child(self.theme_text_field(ThemeField::Velocity(index), cx))
                        }),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children((0..stop_count).map(|index| self.gradient_stop_row(index, cx))),
            )
            .child(self.appearance_button(
                "add-velocity-stop",
                Key::ThemeVelocityAdd,
                |this, window, cx| this.add_gradient_stop(window, cx),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .w_full()
                    .h(px(20.0))
                    .children((0..32).map(|step| {
                        div()
                            .flex_1()
                            .h_full()
                            .bg(preview.velocity_color(step as f32 / 31.0))
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .rounded_md()
                    .bg(preview.background)
                    .text_color(preview.text)
                    .child(div().text_xs().child(self.t(Key::ThemePreview)))
                    .child(
                        div()
                            .p_2()
                            .bg(preview.surface)
                            .child(self.t(Key::ThemePreviewText)),
                    )
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .rounded_sm()
                            .bg(preview.accent)
                            .text_color(preview.text_on_accent)
                            .child(self.t(Key::ThemeSave)),
                    ),
            )
            .when_some(draft.err(), |el, key| {
                el.child(div().text_xs().text_color(theme.danger).child(self.t(key)))
            })
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(self.appearance_button(
                        "save-theme",
                        Key::ThemeSave,
                        |this, _, cx| this.save_theme_editor(cx),
                        cx,
                    ))
                    .child(self.appearance_button(
                        "cancel-theme",
                        Key::Cancel,
                        |this, _, cx| {
                            this.appearance_editor = None;
                            cx.notify();
                        },
                        cx,
                    )),
            )
            .into_any_element()
    }

    fn appearance_button(
        &mut self,
        id: &'static str,
        label: Key,
        action: fn(&mut Self, &mut Window, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focus = self
            .dropdown_focus
            .entry(id)
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        button(
            id,
            self.t(label),
            ButtonStyle::Normal,
            false,
            self.theme.accent,
            &self.theme,
            cx.listener(move |this, _, window, cx| action(this, window, cx)),
        )
        .track_focus(&focus)
        .tab_index(0)
        .focus(|el| el.border_color(self.theme.accent))
        .into_any_element()
    }

    fn add_gradient_stop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.appearance_editor else {
            return;
        };
        let stops = match editor.parsed_stops() {
            Ok(stops) => stops,
            Err(key) => {
                self.status = self.t(key).to_owned();
                cx.notify();
                return;
            }
        };
        let positions: Vec<_> = std::iter::once(0.0)
            .chain(stops.iter().map(|stop| stop.position))
            .chain(std::iter::once(1.0))
            .collect();
        let gap = positions
            .windows(2)
            .max_by(|a, b| (a[1] - a[0]).total_cmp(&(b[1] - b[0])))
            .expect("the endpoints always enclose a gap");
        let position = (gap[0] + gap[1]) * 0.5;
        let preview = editor
            .preview(&self.appearance)
            .unwrap_or_else(|_| self.theme.clone());
        let color: gpui::Rgba = preview.velocity_color(position).into();
        let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
        let row = GradientFields::new(&GradientStop {
            position,
            color: (channel(color.r) << 16) | (channel(color.g) << 8) | channel(color.b),
        });
        let field = ThemeField::Stop(editor.stops.len(), 0);
        editor.active = field;
        editor.stops.push(row);
        let focus = self.theme_input_focus(field, cx);
        window.focus(&focus);
        cx.notify();
    }

    fn gradient_stop_row(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let editor = self.appearance_editor.as_mut().expect("the editor is open");
        let focus = editor.stops[index].focus[2]
            .get_or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        div()
            .flex()
            .items_end()
            .gap_2()
            .child(
                div()
                    .w(px(110.0))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_xs().child(self.t(Key::ThemeVelocityPosition)))
                    .child(self.theme_text_field(ThemeField::Stop(index, 0), cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_xs().child(self.t(Key::ThemeVelocityColor)))
                    .child(self.theme_text_field(ThemeField::Stop(index, 1), cx)),
            )
            .child(
                button(
                    SharedString::from(format!("remove-velocity-stop-{index}")),
                    self.t(Key::ThemeVelocityRemove),
                    ButtonStyle::Normal,
                    false,
                    self.theme.accent,
                    &self.theme,
                    cx.listener(move |this, _, window, cx| {
                        if let Some(editor) = &mut this.appearance_editor {
                            editor.stops.remove(index);
                            editor.active = ThemeField::Name;
                        }
                        let focus = this.theme_input_focus(ThemeField::Name, cx);
                        window.focus(&focus);
                        cx.notify();
                    }),
                )
                .track_focus(&focus)
                .tab_index(0)
                .focus(|el| el.border_color(self.theme.accent)),
            )
            .into_any_element()
    }

    /// The focus handle of the input `field` is typed into, made the first time it is asked for.
    fn theme_input_focus(&mut self, field: ThemeField, cx: &mut Context<Self>) -> FocusHandle {
        self.theme_inputs.entry(field).or_insert_with(|| {
            crate::ui::text_input::MirroredInput::new(
                cx,
                move |this: &mut SettingsWindow, event, text, cx| {
                    match event {
                        atelier::TextInputEvent::Changed => {
                            if let Some(editor) = &mut this.appearance_editor {
                                let edited = editor.field_of(field);
                                let all = 0..edited.content().len();
                                edited.replace(all, &text);
                                this.text_changed();
                            }
                        }
                        atelier::TextInputEvent::Submit => this.save_theme_editor(cx),
                        atelier::TextInputEvent::Cancel => this.appearance_editor = None,
                    }
                    cx.notify();
                },
            )
        });
        let focus = self.theme_inputs[&field].focus_handle().clone();
        // Where the editor looks for which field holds the keyboard.
        match theme_field_id(field) {
            Some(id) => {
                self.dropdown_focus.insert(id, focus.clone());
            }
            None => {
                if let (ThemeField::Stop(index, component), Some(editor)) =
                    (field, self.appearance_editor.as_mut())
                    && let Some(row) = editor.stops.get_mut(index)
                {
                    row.focus[component] = Some(focus.clone());
                }
            }
        }
        focus
    }

    fn theme_text_field(&mut self, field: ThemeField, cx: &mut Context<Self>) -> AnyElement {
        self.theme_input_focus(field, cx);
        let id: SharedString = match (theme_field_id(field), field) {
            (Some(id), _) => id.into(),
            (None, ThemeField::Stop(index, component)) => {
                format!("theme-velocity-stop-{index}-{component}").into()
            }
            (None, _) => unreachable!(),
        };
        let editor = self.appearance_editor.as_ref().expect("the editor is open");
        let text = match field {
            ThemeField::Name => &editor.name,
            ThemeField::Accent => &editor.accent,
            ThemeField::Palette(index) => &editor.palette[index],
            ThemeField::Signal(index) => &editor.signals[index],
            ThemeField::Chord(index) => &editor.chords[index],
            ThemeField::Velocity(index) => &editor.velocity[index],
            ThemeField::Stop(index, component) => &editor.stops[index].fields[component],
        };
        let input = self.theme_inputs[&field].sync(text.content(), cx);
        div()
            .w_full()
            .min_w_0()
            // On the way down, before the input under the pointer takes the press.
            .capture_any_mouse_down(cx.listener(move |this, _, _, cx| {
                if let Some(editor) = &mut this.appearance_editor {
                    editor.active = field;
                }
                cx.notify();
            }))
            .child(
                atelier::TextField::new()
                    .id(id.clone())
                    .debug_selector(move || id.to_string())
                    .track_focus(&self.theme_inputs[&field].tab_stop())
                    .tab_index(0)
                    .child(input),
            )
            .into_any_element()
    }

    pub(super) fn sync_editor_focus(&mut self, window: &Window) -> bool {
        if self.tab != SettingsTab::General && !self.searching() {
            return false;
        }
        let Some(editor) = &mut self.appearance_editor else {
            return false;
        };
        for (index, row) in editor.stops.iter().enumerate() {
            for (component, focus) in row.focus.iter().enumerate().take(2) {
                if focus
                    .as_ref()
                    .is_some_and(|handle| handle.is_focused(window))
                {
                    editor.active = ThemeField::Stop(index, component);
                    return true;
                }
            }
        }
        for (id, field) in [
            ("theme-name", ThemeField::Name),
            ("theme-accent", ThemeField::Accent),
        ]
        .into_iter()
        .chain(
            PALETTE_FIELDS
                .into_iter()
                .enumerate()
                .map(|(index, id)| (id, ThemeField::Palette(index))),
        )
        .chain(
            CHORD_FIELDS
                .into_iter()
                .enumerate()
                .map(|(index, id)| (id, ThemeField::Chord(index))),
        )
        .chain(
            SIGNAL_FIELDS
                .into_iter()
                .enumerate()
                .map(|(index, id)| (id, ThemeField::Signal(index))),
        )
        .chain(
            VELOCITY_FIELDS
                .into_iter()
                .enumerate()
                .map(|(index, id)| (id, ThemeField::Velocity(index))),
        ) {
            if self
                .dropdown_focus
                .get(id)
                .is_some_and(|focus| focus.is_focused(window))
            {
                editor.active = field;
                return true;
            }
        }
        false
    }

    /// The theme editor's fields are Atelier's inputs, which take their own keys: Return
    /// saves the draft and Escape closes it.
    pub(super) fn appearance_editor_key(
        &mut self,
        _event: &KeyDownEvent,
        _cx: &mut Context<Self>,
    ) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_input_accepts_hex_and_rejects_partial_or_non_ascii_values() {
        assert_eq!(parse_colour(" #60a5FA "), Some(0x60a5fa));
        assert_eq!(parse_colour("123456"), Some(0x123456));
        for invalid in ["#123", "#1234567", "GG0000", "１２３４５６", ""] {
            assert_eq!(parse_colour(invalid), None);
        }
    }

    #[test]
    fn invalid_or_duplicate_theme_names_cannot_replace_the_applied_theme() {
        let appearance = Appearance::default();
        let mut editor = AppearanceEditor {
            editing_id: None,
            base: appearance.scheme.clone(),
            name: TextField::new(""),
            accent: TextField::new("#60A5FA"),
            palette: std::array::from_fn(|_| TextField::new("")),
            signals: std::array::from_fn(|_| TextField::new("")),
            chords: std::array::from_fn(|_| TextField::new("")),
            velocity: std::array::from_fn(|_| TextField::new("")),
            stops: Vec::new(),
            active: ThemeField::Name,
        };
        assert_eq!(
            editor.draft(&appearance).unwrap_err(),
            Key::ThemeNameRequired
        );
        editor.name = TextField::new("Midnight");
        assert_eq!(editor.draft(&appearance).unwrap_err(), Key::ThemeNameExists);
        editor.name = TextField::new("私のテーマ");
        let draft = editor.draft(&appearance).unwrap();
        assert_eq!(draft.name, "私のテーマ");
        assert_eq!(draft.accent, 0x60a5fa);
        assert_eq!(appearance, Appearance::default());
    }
}
