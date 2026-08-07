//! The panel: the sheet drawn, and the handful of things its buttons hand back.
//!
//! Its own file because none of it can be tested. Every function here builds gpui elements, and
//! every rule they are built from — what a dial means, what a gesture does to the form — is next
//! door in `dials` where a test can reach it. A condition that grows here belongs there.
//!
//! Three columns: the song, the form, the roster. The pickers the buttons open are in `menus`.

use gpui::{
    AnyElement, Context, IntoElement, MouseDownEvent, Window, div, prelude::*, px, relative,
};

use auris_i18n::Key;
use auris_session::prelude::*;

use crate::app::{AurisApp, Drag};
use crate::theme::{Metrics, Theme};
use crate::ui::prompt::{Prompt, PromptTarget};
use crate::ui::widgets::{ButtonStyle, SliderFill, button, divider, value_slider};

use super::dials::*;

/// How wide the label at the start of a row is drawn.
const LABEL_WIDTH: f32 = 116.0;

impl AurisApp {
    /// Opens the song sheet: on the song it was last set to, on the one the document was written
    /// from, or on the default one.
    ///
    /// In that order, and the middle one is the point. A piece composed, saved and reopened used
    /// to come back to a sheet full of defaults — Another Take on it would have written a
    /// different song rather than another take of that one.
    pub(crate) fn open_song_sheet(&mut self) {
        if self.song_sheet.is_some() {
            return;
        }
        // A document written by a build that spelled something differently is not an error worth
        // a dialog: the sheet opens on its defaults, which is where it opened before any of this.
        let remembered = self
            .project()
            .song_spec
            .as_deref()
            .and_then(|text| SongSpec::parse(text).ok());
        let project = self.project();
        self.song_sheet = Some(super::opening_dials(
            remembered.as_ref(),
            project.harmony.keys.initial(),
            project.tempo_map.initial_bpm(),
            project.signatures.initial(),
        ));
    }

    /// The song sheet, or nothing when it is closed.
    ///
    /// A full-screen panel rather than a [`Prompt`]: a prompt asks for one value, and this asks
    /// for a song. It occludes for the same reason the export overlay does — every click behind
    /// a dimmed screen used to land on the arrangement.
    pub(crate) fn render_song_sheet(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let dials = self.song_sheet.clone()?;
        let theme = self.theme.clone();
        let spec = song_spec(&dials);
        let length = format!(
            "{} · {} {}",
            self.t(Key::SongLength),
            spec.total_bars(),
            self.t(Key::SongBarsUnit)
        );

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(Theme::translucent(theme.background, 0.72))
                .occlude()
                // …and occluding is what stopped the dials working. A drag is followed on the
                // root, and the hit test stops dead at the first blocking hitbox — so while this
                // is up the root reads as un-hovered and never sees another pointer move. Every
                // dial took its press and then sat still, however far the pointer travelled. An
                // overlay that occludes carries the drag itself; see `AurisApp::on_mouse_move`.
                .on_mouse_move(cx.listener(AurisApp::on_mouse_move))
                .on_mouse_up(gpui::MouseButton::Left, cx.listener(AurisApp::on_mouse_up))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .w(px(1120.0))
                        .max_h(relative(0.92))
                        .p_4()
                        .rounded(Metrics::RADIUS_LG)
                        .bg(theme.surface_raised)
                        .border_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.text)
                                .child(self.t(Key::SongSheetTitle)),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_4()
                                .flex_1()
                                .min_h_0()
                                .child(
                                    div()
                                        .id("song-sheet-song")
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .w(px(320.0))
                                        .overflow_y_scroll()
                                        .children(self.song_rows(&dials, cx)),
                                )
                                .child(
                                    div()
                                        .id("song-sheet-form")
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .w(px(330.0))
                                        .overflow_y_scroll()
                                        .children(self.song_form_rows(&dials, cx)),
                                )
                                .child(
                                    div()
                                        .id("song-sheet-parts")
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_y_scroll()
                                        .children(self.song_part_rows(&dials, cx)),
                                ),
                        )
                        .child(divider(&theme))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .text_xs()
                                        .text_color(theme.text_muted)
                                        .child(length),
                                )
                                .child(button(
                                    "song-sheet-cancel",
                                    self.t(Key::Cancel),
                                    ButtonStyle::Normal,
                                    false,
                                    theme.accent,
                                    &theme,
                                    cx.listener(|this, _, _, cx| {
                                        this.song_sheet = None;
                                        cx.notify();
                                    }),
                                ))
                                .child(button(
                                    "song-sheet-save",
                                    self.t(Key::SongSaveSpec),
                                    ButtonStyle::Normal,
                                    false,
                                    theme.accent,
                                    &theme,
                                    cx.listener(|this, _, window, cx| {
                                        this.save_song_specification(window, cx);
                                    }),
                                ))
                                .child(button(
                                    "song-sheet-take",
                                    self.t(Key::SongAnotherTake),
                                    ButtonStyle::Normal,
                                    false,
                                    theme.accent,
                                    &theme,
                                    cx.listener(|this, _, _, cx| {
                                        if let Some(dials) = this.song_sheet.as_mut() {
                                            another_take(dials);
                                        }
                                        this.write_song_from_sheet();
                                        cx.notify();
                                    }),
                                ))
                                .child(button(
                                    "song-sheet-write",
                                    self.t(Key::SongWrite),
                                    ButtonStyle::Primary,
                                    false,
                                    theme.accent,
                                    &theme,
                                    // Write closes the sheet and Another Take does not: one is
                                    // "this is the song", the other is "not that one, again".
                                    cx.listener(|this, _, _, cx| {
                                        this.write_song_from_sheet();
                                        this.song_sheet = None;
                                        cx.notify();
                                    }),
                                )),
                        ),
                ),
        )
    }

    /// The left half: everything about the song that is not a part.
    fn song_rows(&mut self, dials: &SongDials, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = self.theme.clone();
        let mut rows: Vec<AnyElement> =
            vec![self.group_heading(Key::SongHeading).into_any_element()];

        // First, because it is the row that sets every other one. Somebody opening this for the
        // first time is looking at thirty dials and no idea which of them matter; a style is the
        // answer to all of them at once, and what they came here to change is what happens next.
        rows.push(
            self.sheet_picker(
                "song-style",
                Key::SongStyle,
                self.t(Key::SongStyleChoose).to_string(),
                cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                    let menu = this.song_preset_menu(event.position());
                    this.open_menu(menu);
                    cx.notify();
                }),
            )
            .into_any_element(),
        );
        rows.push(
            self.sheet_picker(
                "song-title",
                Key::SongTitleField,
                dials.title.clone(),
                cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                    let title = this.t(Key::SongTitleField);
                    let current = this
                        .song_sheet
                        .as_ref()
                        .map_or_else(String::new, |dials| dials.title.clone());
                    this.open_prompt(Prompt::new(title, PromptTarget::SongTitle, current));
                    cx.notify();
                }),
            )
            .into_any_element(),
        );
        rows.push(
            self.sheet_picker(
                "song-key",
                Key::SongKey,
                dials.key.to_text(),
                cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                    let title = this.t(Key::SongKey);
                    let current = this
                        .song_sheet
                        .as_ref()
                        .map_or_else(String::new, |dials| dials.key.to_text());
                    this.open_prompt(Prompt::new(title, PromptTarget::SongKey, current));
                    cx.notify();
                }),
            )
            .into_any_element(),
        );
        rows.push(
            self.sheet_picker(
                "song-meter",
                Key::SongMeter,
                format!("{}/{}", dials.meter.numerator, dials.meter.denominator),
                cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                    let menu = this.song_meter_menu(event.position());
                    this.open_menu(menu);
                    cx.notify();
                }),
            )
            .into_any_element(),
        );
        rows.push(
            self.sheet_picker(
                "song-mood",
                Key::SongMood,
                match mood_word(dials.mood) {
                    Some(name) => this_word(self, name),
                    None => self.t(Key::SongMoodCustom).to_string(),
                },
                cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                    let menu = this.song_mood_menu(event.position());
                    this.open_menu(menu);
                    cx.notify();
                }),
            )
            .into_any_element(),
        );
        rows.push(
            self.sheet_picker(
                "song-groove",
                Key::PartGroove,
                dials.groove.clone(),
                cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                    let menu = this.song_groove_menu(event.position());
                    this.open_menu(menu);
                    cx.notify();
                }),
            )
            .into_any_element(),
        );
        rows.push(
            self.sheet_picker(
                "song-seed",
                Key::PartSeed,
                dials.seed.to_string(),
                cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                    let title = this.t(Key::PartSeed);
                    let current = this
                        .song_sheet
                        .as_ref()
                        .map_or_else(String::new, |dials| dials.seed.to_string());
                    this.open_prompt(Prompt::new(title, PromptTarget::SongSeed, current));
                    cx.notify();
                }),
            )
            .into_any_element(),
        );

        for dial in SONG_DIALS {
            let dial = *dial;
            let target = DialTarget::Song(dial);
            let fraction = dial.fraction(dials);
            rows.push(
                value_slider(
                    ("song-dial", dial as usize),
                    self.t(dial.label()),
                    dial.text(dials),
                    fraction,
                    theme.accent,
                    SliderFill::FromStart,
                    &theme,
                    cx.listener(move |this, event: &MouseDownEvent, _, _| {
                        this.begin_drag(Drag::SongDial {
                            target,
                            start_fraction: fraction,
                            start_x: event.position.x,
                        });
                    }),
                )
                .into_any_element(),
            );
        }
        rows
    }

    /// The middle: the form, one block per playing of a section.
    ///
    /// One row per *place in the order*, not one per section — a chorus played twice is two rows,
    /// and both of them edit the one chorus, because that is what makes it the same chorus.
    fn song_form_rows(
        &mut self,
        dials: &SongDials,
        cx: &mut gpui::Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = self.theme.clone();
        let removable = dials.form.len() > 1;
        let roster = dials.parts.len();
        let mut rows: Vec<AnyElement> = vec![
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .child(self.group_heading(Key::SongFormHeading)),
                )
                .child(button(
                    "song-add-section",
                    self.t(Key::SongAddSection),
                    ButtonStyle::Normal,
                    false,
                    theme.accent,
                    &theme,
                    cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        let places = this
                            .song_sheet
                            .as_ref()
                            .map_or(0, |dials| dials.form.len().saturating_sub(1));
                        let menu = this.song_section_menu(event.position(), places);
                        this.open_menu(menu);
                        cx.notify();
                    }),
                ))
                .into_any_element(),
        ];

        for (place, name) in dials.form.iter().enumerate() {
            let Some(index) = section_at(dials, place) else {
                continue;
            };
            let section = &dials.sections[index];
            let chart = dials
                .charts
                .iter()
                .find(|(known, _)| known == &section.chords)
                .map(|(known, chart)| self.progression_name(&chart_label(known, chart)))
                .unwrap_or_else(|| section.chords.clone());

            let mut dial_row = div().flex().gap_2();
            for dial in SECTION_DIALS {
                let dial = *dial;
                let target = DialTarget::Section(index, dial);
                let fraction = dial.fraction(section);
                dial_row = dial_row.child(div().flex_1().min_w_0().child(value_slider(
                    (
                        "song-section-dial",
                        place * SECTION_DIALS.len() + dial as usize,
                    ),
                    self.t(dial.label()),
                    dial.text(section),
                    fraction,
                    theme.accent,
                    SliderFill::FromStart,
                    &theme,
                    cx.listener(move |this, event: &MouseDownEvent, _, _| {
                        this.begin_drag(Drag::SongDial {
                            target,
                            start_fraction: fraction,
                            start_x: event.position.x,
                        });
                    }),
                )));
            }
            // Beside the dials rather than up in the row of names: the top row is already a name,
            // a progression, a transposition and a roster wide, and how fast a section goes is the
            // same kind of thing as how long it is and how hard it is played.
            dial_row = dial_row.child(div().w(px(52.0)).child(button(
                ("song-section-tempo", place),
                section_tempo_label(section),
                ButtonStyle::Normal,
                false,
                theme.accent,
                &theme,
                cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                    let menu = this.song_section_tempo_menu(event.position(), index);
                    this.open_menu(menu);
                    cx.notify();
                }),
            )));

            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .rounded(Metrics::RADIUS_SM)
                    .bg(theme.surface_sunken)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(div().w(px(84.0)).child(button(
                                ("song-form-name", place),
                                name.clone(),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                                    let menu = this.song_form_name_menu(event.position(), place);
                                    this.open_menu(menu);
                                    cx.notify();
                                }),
                            )))
                            .child(div().flex_1().min_w_0().child(button(
                                ("song-section-chords", place),
                                chart,
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                                    let menu = this.song_chords_menu(event.position(), index);
                                    this.open_menu(menu);
                                    cx.notify();
                                }),
                            )))
                            .child(div().w(px(44.0)).child(button(
                                ("song-section-transpose", place),
                                transpose_label(section.transpose),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                                    let menu = this.song_transpose_menu(event.position(), index);
                                    this.open_menu(menu);
                                    cx.notify();
                                }),
                            )))
                            .child(div().w(px(44.0)).child(button(
                                ("song-section-parts", place),
                                section_parts_label(section, roster),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                                    let menu =
                                        this.song_section_parts_menu(event.position(), index);
                                    this.open_menu(menu);
                                    cx.notify();
                                }),
                            )))
                            .child(div().w(px(22.0)).child(button(
                                ("song-form-up", place),
                                "↑",
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                                    if let Some(dials) = this.song_sheet.as_mut() {
                                        move_in_form(dials, place, false);
                                    }
                                    cx.notify();
                                }),
                            )))
                            .child(div().w(px(22.0)).child(button(
                                ("song-form-down", place),
                                "↓",
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                                    if let Some(dials) = this.song_sheet.as_mut() {
                                        move_in_form(dials, place, true);
                                    }
                                    cx.notify();
                                }),
                            )))
                            // The last playing cannot go: a form of nothing writes nothing, and
                            // the specification refuses one rather than composing silence.
                            .child(div().w(px(22.0)).child(button(
                                ("song-form-remove", place),
                                "✕",
                                ButtonStyle::Normal,
                                false,
                                if removable {
                                    theme.danger
                                } else {
                                    theme.border
                                },
                                &theme,
                                cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                                    if let Some(dials) = this.song_sheet.as_mut() {
                                        remove_from_form(dials, place);
                                    }
                                    cx.notify();
                                }),
                            ))),
                    )
                    .child(dial_row)
                    .into_any_element(),
            );
        }
        rows
    }

    /// The right half: one block per part, with a button that adds another.
    fn song_part_rows(&mut self, dials: &SongDials, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = self.theme.clone();
        let removable = dials.parts.len() > 1;
        let mut rows: Vec<AnyElement> = vec![
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .child(self.group_heading(Key::SongPartsHeading)),
                )
                .child(button(
                    "song-add-part",
                    self.t(Key::SongAddPart),
                    ButtonStyle::Normal,
                    false,
                    theme.accent,
                    &theme,
                    cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                        let menu = this.song_add_part_menu(event.position());
                        this.open_menu(menu);
                        cx.notify();
                    }),
                ))
                .into_any_element(),
        ];

        for (index, part) in dials.parts.iter().enumerate() {
            // What the part will be *heard* as: the General MIDI sound where it names one, and
            // otherwise the plugin. Showing the plugin under a part that asked for a violin would
            // name the fallback and never the sound.
            let instrument = match part.program {
                Some(program) => program.label(part.role.is_drum()).to_string(),
                None => self
                    .registry()
                    .instruments()
                    .find(|descriptor| descriptor.id == part.instrument)
                    .map(|descriptor| {
                        auris_i18n::audio::plugin_name(&descriptor.name, self.language())
                            .to_string()
                    })
                    .unwrap_or_else(|| part.instrument.clone()),
            };

            let mut dial_row = div().flex().gap_2();
            for dial in PART_DIALS {
                let dial = *dial;
                let target = DialTarget::Part(index, dial);
                let fraction = dial.fraction(part);
                dial_row = dial_row.child(div().flex_1().min_w_0().child(value_slider(
                    ("song-part-dial", index * PART_DIALS.len() + dial as usize),
                    self.t(dial.label()),
                    dial.text(part),
                    fraction,
                    theme.accent,
                    match dial.is_centred() {
                        true => SliderFill::FromCentre,
                        false => SliderFill::FromStart,
                    },
                    &theme,
                    cx.listener(move |this, event: &MouseDownEvent, _, _| {
                        this.begin_drag(Drag::SongDial {
                            target,
                            start_fraction: fraction,
                            start_x: event.position.x,
                        });
                    }),
                )));
            }

            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .rounded(Metrics::RADIUS_SM)
                    .bg(theme.surface_sunken)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(div().w(px(96.0)).child(button(
                                ("song-part-name", index),
                                part.name.clone(),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                                    let title = this.t(Key::SongPartNameTitle);
                                    let current = this.song_sheet.as_ref().map_or_else(
                                        String::new,
                                        |dials| {
                                            dials
                                                .parts
                                                .get(index)
                                                .map_or_else(String::new, |part| part.name.clone())
                                        },
                                    );
                                    this.open_prompt(Prompt::new(
                                        title,
                                        PromptTarget::SongPartName(index),
                                        current,
                                    ));
                                    cx.notify();
                                }),
                            )))
                            .child(div().w(px(88.0)).child(button(
                                ("song-part-role", index),
                                self.t(role_key(part.role)),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                                    let menu = this.song_role_menu(event.position(), index);
                                    this.open_menu(menu);
                                    cx.notify();
                                }),
                            )))
                            .child(div().flex_1().min_w_0().child(button(
                                ("song-part-instrument", index),
                                instrument,
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                                    let menu = this.song_instrument_menu(event.position(), index);
                                    this.open_menu(menu);
                                    cx.notify();
                                }),
                            )))
                            // A drum part has no octave — its pitches are drum numbers rather
                            // than notes — and it badly needs the *one* number the octave would
                            // have taken the room for, because General MIDI is the only agreement
                            // there is about which number is a kick and a font need not keep it.
                            .child(div().w(px(52.0)).child({
                                let drum = part.drum_note();
                                button(
                                    ("song-part-note", index),
                                    match drum {
                                        Some(note) => note.to_string(),
                                        None => part.octave.to_string(),
                                    },
                                    ButtonStyle::Normal,
                                    false,
                                    theme.accent,
                                    &theme,
                                    cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                                        let menu = match drum.is_some() {
                                            true => this.song_note_menu(event.position(), index),
                                            false => this.song_octave_menu(event.position(), index),
                                        };
                                        this.open_menu(menu);
                                        cx.notify();
                                    }),
                                )
                            }))
                            // The last part cannot go: a song with no parts writes no notes, and
                            // the button goes dead rather than Write producing an empty document.
                            .child(div().w(px(64.0)).child(button(
                                ("song-part-remove", index),
                                self.t(Key::SongRemovePart),
                                ButtonStyle::Normal,
                                false,
                                if removable {
                                    theme.danger
                                } else {
                                    theme.border
                                },
                                &theme,
                                cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                                    if let Some(dials) = this.song_sheet.as_mut() {
                                        remove_part(dials, index);
                                    }
                                    cx.notify();
                                }),
                            ))),
                    )
                    .child(dial_row)
                    .into_any_element(),
            );
        }
        rows
    }

    /// A row with a label at the start and a button holding the value.
    fn sheet_picker<I, F>(
        &self,
        id: I,
        label: Key,
        value: String,
        on_click: F,
    ) -> impl IntoElement + use<I, F>
    where
        I: Into<gpui::ElementId>,
        F: Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    {
        let theme = self.theme.clone();
        div()
            .flex()
            .items_center()
            .gap_2()
            .h(Metrics::CONTROL_HEIGHT)
            .child(
                div()
                    .w(px(LABEL_WIDTH))
                    .text_xs()
                    .text_color(theme.text_muted)
                    .truncate()
                    .child(self.t(label)),
            )
            .child(div().flex_1().min_w_0().child(button(
                id,
                value,
                ButtonStyle::Normal,
                false,
                theme.accent,
                &theme,
                on_click,
            )))
    }

    /// Moves one of the sheet's dials, from a drag.
    pub(crate) fn drag_song_dial(&mut self, target: DialTarget, start_fraction: f32, delta: f32) {
        let Some(dials) = self.song_sheet.as_mut() else {
            return;
        };
        target.set(dials, dragged(start_fraction, delta));
    }

    /// Writes the piece the sheet describes, replacing the document.
    pub(crate) fn write_song_from_sheet(&mut self) {
        let Some(dials) = self.song_sheet.as_ref() else {
            return;
        };
        let spec = song_spec(dials);
        self.compose_spec(&spec);
    }

    /// Saves the sheet as a specification file.
    pub(crate) fn save_song_specification(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(dials) = self.song_sheet.as_ref() else {
            return;
        };
        let text = song_spec(dials).to_toml();
        let name = format!("{}.{}", dials.title, auris_session::SPEC_EXTENSION);
        let language = self.language();
        cx.spawn(async move |this, cx| {
            let handle = rfd::AsyncFileDialog::new()
                .set_title(Key::SongSaveSpec.get(language))
                .set_file_name(&name)
                .add_filter(
                    Key::FilterSpec.get(language),
                    &[auris_session::SPEC_EXTENSION],
                )
                .save_file()
                .await;
            let Some(handle) = handle else { return };
            let path = handle.path().to_path_buf();
            let written = std::fs::write(&path, text);
            let _ = this.update(cx, |this, cx| {
                match written {
                    Ok(()) => this.set_status(auris_i18n::messages::saved(
                        this.language(),
                        &path.display().to_string(),
                    )),
                    Err(error) => this.set_failed_status(auris_i18n::messages::failed(
                        this.language(),
                        this.t(Key::SongSaveSpec),
                        &error.to_string(),
                    )),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// The interface's word for a mood, as a `String` the picker can hold.
fn this_word(app: &AurisApp, name: &str) -> String {
    app.t(mood_key(name)).to_string()
}
