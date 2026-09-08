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
    velocity: [TextField; 2],
    stops: Vec<GradientFields>,
    active: ThemeField,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ThemeField {
    Name,
    Accent,
    Palette(usize),
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
        match self.active {
            ThemeField::Name => &mut self.name,
            ThemeField::Accent => &mut self.accent,
            ThemeField::Palette(index) => &mut self.palette[index],
            ThemeField::Velocity(index) => &mut self.velocity[index],
            ThemeField::Stop(index, component) => &mut self.stops[index].fields[component],
        }
    }

    pub(super) fn readable_field(&self) -> &TextField {
        match self.active {
            ThemeField::Name => &self.name,
            ThemeField::Accent => &self.accent,
            ThemeField::Palette(index) => &self.palette[index],
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
                    .when(custom, |row| {
                        row.child(self.appearance_button(
                            "edit-theme",
                            Key::EditTheme,
                            |this, window, cx| this.open_theme_editor(true, window, cx),
                            cx,
                        ))
                    }),
            );
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
            velocity: copy.velocity_palette.map(palette_field),
            stops: copy
                .velocity_stops
                .iter()
                .map(GradientFields::new)
                .collect(),
            active: ThemeField::Name,
        });
        let focus = self
            .dropdown_focus
            .entry("theme-name")
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        window.focus(&focus);
        cx.notify();
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
        let mut row = GradientFields::new(&GradientStop {
            position,
            color: (channel(color.r) << 16) | (channel(color.g) << 8) | channel(color.b),
        });
        let focus = cx.focus_handle().tab_stop(true);
        row.focus[0] = Some(focus.clone());
        editor.active = ThemeField::Stop(editor.stops.len(), 0);
        editor.stops.push(row);
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
                        if let Some(focus) = this.dropdown_focus.get("theme-name") {
                            window.focus(focus);
                        }
                        cx.notify();
                    }),
                )
                .track_focus(&focus)
                .tab_index(0)
                .focus(|el| el.border_color(self.theme.accent)),
            )
            .into_any_element()
    }

    fn theme_text_field(&mut self, field: ThemeField, cx: &mut Context<Self>) -> AnyElement {
        let static_id = match field {
            ThemeField::Name => Some("theme-name"),
            ThemeField::Accent => Some("theme-accent"),
            ThemeField::Palette(index) => Some(PALETTE_FIELDS[index]),
            ThemeField::Velocity(index) => Some(VELOCITY_FIELDS[index]),
            ThemeField::Stop(..) => None,
        };
        let (id, focus): (SharedString, FocusHandle) = if let Some(id) = static_id {
            (
                id.into(),
                self.dropdown_focus
                    .entry(id)
                    .or_insert_with(|| cx.focus_handle().tab_stop(true))
                    .clone(),
            )
        } else if let ThemeField::Stop(index, component) = field {
            let editor = self.appearance_editor.as_mut().expect("the editor is open");
            (
                format!("theme-velocity-stop-{index}-{component}").into(),
                editor.stops[index].focus[component]
                    .get_or_insert_with(|| cx.focus_handle().tab_stop(true))
                    .clone(),
            )
        } else {
            unreachable!()
        };
        let editor = self.appearance_editor.as_ref().expect("the editor is open");
        let text = match field {
            ThemeField::Name => &editor.name,
            ThemeField::Accent => &editor.accent,
            ThemeField::Palette(index) => &editor.palette[index],
            ThemeField::Velocity(index) => &editor.velocity[index],
            ThemeField::Stop(index, component) => &editor.stops[index].fields[component],
        };
        let active = editor.active == field;
        let contents = if active {
            crate::ui::prompt::editable_text(
                text.content().to_owned().into(),
                text.selection(),
                text.marked(),
                focus.clone(),
                cx.entity(),
                self.theme.clone(),
            )
            .into_any_element()
        } else {
            crate::ui::prompt::field_text(text.content().to_owned(), self.theme.text)
                .into_any_element()
        };
        div()
            .id(id.clone())
            .debug_selector(move || id.to_string())
            .track_focus(&focus)
            .tab_index(0)
            .h(px(30.0))
            .w_full()
            .min_w_0()
            .bg(self.theme.surface_sunken)
            .border_1()
            .rounded_sm()
            .border_color(if active {
                self.theme.accent
            } else {
                self.theme.border
            })
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    if let Some(editor) = &mut this.appearance_editor {
                        editor.active = field;
                    }
                    window.focus(&focus);
                    cx.notify();
                }),
            )
            .child(contents)
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
            for component in 0..2 {
                if row.focus[component]
                    .as_ref()
                    .is_some_and(|focus| focus.is_focused(window))
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

    pub(super) fn appearance_editor_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(editor) = &mut self.appearance_editor else {
            return false;
        };
        match event.keystroke.key.as_str() {
            "escape" => self.appearance_editor = None,
            "enter" if editor.readable_field().marked().is_none() => self.save_theme_editor(cx),
            "enter" => return true,
            "tab" => return false,
            key => {
                let modifiers = event.keystroke.modifiers;
                if editor.field().apply_key_with_clipboard(
                    key,
                    modifiers.shift,
                    modifiers.secondary(),
                    false,
                    cx,
                ) == KeyEffect::Ignored
                {
                    return false;
                }
            }
        }
        self.text_changed();
        cx.notify();
        true
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
