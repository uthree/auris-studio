//! The panel: the sheet drawn, and the handful of things its buttons hand back.
//!
//! Basic controls and lyrics lead; detailed settings disclose the form and instrument roster.
//! Both layouts reflow into fewer columns in smaller windows. A single scrolling
//! body holds the fields and part cards, while the title and actions stay visible. The pickers
//! the buttons open are in `menus`; the words' own rules and elements are in `lyrics`.

use gpui::{AnyElement, Context, IntoElement, MouseDownEvent, Window, div, prelude::*, px};
use gpui_component::scroll::{Scrollbar, ScrollbarShow};

use auris_i18n::Key;
use auris_session::prelude::*;

use crate::app::{AurisApp, Drag};
use crate::theme::{Metrics, Theme};
use crate::ui::prompt::{Prompt, PromptTarget};
use crate::ui::widgets::{
    ButtonStyle, RowColumn, SliderFill, button, divider, picker_row, value_slider,
};

use super::dials::*;

/// How wide the label at the start of a row is drawn.
const LABEL_WIDTH: gpui::Pixels = px(116.0);

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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        self.reconcile_section_lyrics();
        let dials = self.song_sheet.clone()?;
        let theme = self.theme.clone();
        let viewport = window.viewport_size();
        let width = (viewport.width - px(32.0)).max(px(0.0)).min(px(1120.0));
        let columns = if width >= px(760.0) { 2 } else { 1 };
        // The offset lasts while the sheet is open, and starts at the song fields on reopening.
        let scroll = window
            .use_keyed_state("song-sheet-scroll", cx, |_, _| gpui::ScrollHandle::new())
            .read(cx)
            .clone();
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
                        .debug_selector(|| "song-sheet-panel".to_string())
                        .flex()
                        .flex_col()
                        .gap_3()
                        .w(width)
                        .h(viewport.height * 0.92)
                        .p_4()
                        .rounded(Metrics::RADIUS_LG)
                        .bg(theme.surface_raised)
                        .border_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .flex()
                                .flex_shrink_0()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .text_sm()
                                        .text_color(theme.text)
                                        .child(self.t(Key::SongSheetTitle)),
                                )
                                .child(button(
                                    "song-advanced",
                                    self.t(if self.song_advanced {
                                        Key::SongBasic
                                    } else {
                                        Key::SongAdvanced
                                    }),
                                    ButtonStyle::Normal,
                                    self.song_advanced,
                                    theme.accent,
                                    &theme,
                                    cx.listener(move |this, _, _, cx| {
                                        this.song_advanced = !this.song_advanced;
                                        // The basic fields keep their positions, including the current scroll.
                                        cx.notify();
                                    }),
                                )),
                        )
                        .child(
                            div()
                                .relative()
                                .flex_1()
                                .min_h_0()
                                .child(
                                    div()
                                        .id("song-sheet-body")
                                        .debug_selector(|| "song-sheet-body".to_string())
                                        .size_full()
                                        .pr_3()
                                        .overflow_y_scroll()
                                        .map(|mut body| {
                                            body.style().restrict_scroll_to_axis = Some(true);
                                            body
                                        })
                                        .track_scroll(&scroll)
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap_3()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(theme.text_muted)
                                                        .child(self.t(Key::SongStartHint)),
                                                )
                                                .child(
                                                    div()
                                                        .grid()
                                                        .grid_cols(columns)
                                                        .gap_4()
                                                        .items_start()
                                                        .child(
                                                            div()
                                                                .debug_selector(|| {
                                                                    "song-sheet-song".to_string()
                                                                })
                                                                .flex()
                                                                .flex_col()
                                                                .gap_1()
                                                                .min_w_0()
                                                                .children(
                                                                    self.song_rows(&dials, cx),
                                                                ),
                                                        )
                                                        .child(
                                                            div()
                                                                .debug_selector(|| {
                                                                    "song-sheet-lyrics".to_string()
                                                                })
                                                                .flex()
                                                                .flex_col()
                                                                .gap_1()
                                                                .min_w_0()
                                                                .children(
                                                                    self.song_lyrics_rows(
                                                                        &dials, &scroll, cx,
                                                                    ),
                                                                ),
                                                        ),
                                                )
                                                .child(divider(&theme))
                                                .child(self.song_participation_matrix(&dials, width, window, cx))
                                                .when(self.song_advanced, |this| {
                                                    this.child(divider(&theme))
                                                        .child(
                                                            div().text_xs().text_color(theme.text_muted)
                                                                .child(self.t(Key::SongAdvancedHint)),
                                                        )
                                                        .child(
                                                            div().grid().grid_cols(columns).gap_4().items_start()
                                                                .child(
                                                                    div().debug_selector(|| "song-sheet-harmony".to_string())
                                                                        .flex().flex_col().gap_1().min_w_0()
                                                                        .children(self.song_harmony_rows(&dials, cx)),
                                                                )
                                                                .child(
                                                                    div().debug_selector(|| "song-sheet-performance".to_string())
                                                                        .flex().flex_col().gap_1().min_w_0()
                                                                        .children(self.song_performance_rows(&dials, cx)),
                                                                ),
                                                        )
                                                        .child(divider(&theme))
                                                        .child(
                                                            div().debug_selector(|| "song-sheet-form".to_string())
                                                                .grid().grid_cols(columns).gap_2().min_w_0()
                                                                .children(self.song_form_rows(&dials, cx)),
                                                        )
                                                        .child(divider(&theme))
                                                        .child(
                                                            div()
                                                                .debug_selector(|| {
                                                                    "song-sheet-drums".to_string()
                                                                })
                                                                .flex()
                                                                .flex_col()
                                                                .gap_2()
                                                                .children(
                                                                    self.song_drum_rows(&dials, cx),
                                                                ),
                                                        )
                                                        .child(divider(&theme))
                                                        .child(self.song_parts_header(&dials, cx))
                                                        .child(
                                                            div()
                                                                .debug_selector(|| {
                                                                    "song-sheet-parts".to_string()
                                                                })
                                                                .grid()
                                                                .grid_cols(columns)
                                                                .gap_2()
                                                                .children(
                                                                    self.song_part_rows(&dials, cx),
                                                                ),
                                                        )
                                                }),
                                        ),
                                )
                                .child(
                                    div()
                                        .debug_selector(|| "song-sheet-scrollbar".to_string())
                                        .absolute()
                                        .inset_0()
                                        .child(Scrollbar::vertical(&scroll)
                                            .scrollbar_show(ScrollbarShow::Always)),
                                ),
                        )
                        .child(divider(&theme))
                        .child(
                            div()
                                .flex()
                                .flex_shrink_0()
                                .flex_wrap()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .when(columns > 1, |this| this.flex_1())
                                        .when(columns == 1, |this| this.w_full())
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
                                        // The lyrics box edits the song sheet's sections;
                                        // it cannot outlive them.
                                        this.lyrics_edit = None;
                                        cx.notify();
                                    }),
                                ))
                                .when(self.song_advanced, |this| {
                                    this.child(button(
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
                                })
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
                                        if this.write_song_from_sheet() {
                                            this.song_sheet = None;
                                            this.lyrics_edit = None;
                                        }
                                        cx.notify();
                                    }),
                                )),
                        ),
                ),
        )
    }

    /// Basic song controls, kept in the same order in both detail modes.
    fn song_rows(&mut self, dials: &SongDials, cx: &mut Context<Self>) -> Vec<AnyElement> {
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
                Self::opens_menu(cx, |this, at| this.song_preset_menu(at)),
            )
            .into_any_element(),
        );
        rows.push(
            self.sheet_picker(
                "song-ending",
                Key::SongEnding,
                self.t(match dials.ending {
                    Ending::Held => Key::SongEndingHeld,
                    Ending::Fade => Key::SongEndingFade,
                    Ending::Loop => Key::SongEndingLoop,
                    Ending::None => Key::SongEndingNone,
                })
                .to_string(),
                Self::opens_menu(cx, |this, at| this.song_ending_menu(at)),
            )
            .into_any_element(),
        );
        let singer = dials
            .singer
            .as_ref()
            .map(|path| {
                if let Some((name, _)) = self
                    .voice_list()
                    .into_iter()
                    .find(|(_, installed)| installed == std::path::Path::new(path))
                {
                    return name;
                }
                std::path::Path::new(path)
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_else(|| self.t(Key::SingerNoVoice).to_string());
        rows.push(
            self.sheet_picker(
                "song-singer",
                Key::SingerVoiceLabel,
                singer,
                Self::opens_menu(cx, |this, at| this.song_singer_menu(at)),
            )
            .into_any_element(),
        );
        if dials.singer.is_some() {
            rows.push(
                self.sheet_picker(
                    "song-speaker",
                    Key::SingerSpeakerLabel,
                    dials
                        .singer_speaker
                        .clone()
                        .unwrap_or_else(|| self.t(Key::SongSpeakerDefault).to_string()),
                    cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                        this.open_song_speaker_menu(event.position(), cx);
                    }),
                )
                .into_any_element(),
            );
        }
        if let Some(portrait) = self.song_singer_portrait_row(cx) {
            rows.push(portrait);
        }
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
        rows.push(self.song_pad(dials, false, cx));
        rows.push(self.song_dial_row(dials, SongDial::Tempo, cx));
        rows
    }

    /// Additional choices for the song's harmony, meter and random take.
    fn song_harmony_rows(&mut self, dials: &SongDials, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = vec![
            self.group_heading(Key::SongHarmonyHeading)
                .into_any_element(),
        ];
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
                dials.meter.to_string(),
                Self::opens_menu(cx, |this, at| this.song_meter_menu(at)),
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
                Self::opens_menu(cx, |this, at| this.song_mood_menu(at)),
            )
            .into_any_element(),
        );
        rows.push(
            self.sheet_picker(
                "song-groove",
                Key::PartGroove,
                dials.groove.clone(),
                Self::opens_menu(cx, |this, at| this.song_groove_menu(at)),
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
        rows
    }

    /// Additional controls for the phrasing and performance.
    fn song_performance_rows(
        &mut self,
        dials: &SongDials,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut rows = vec![
            self.group_heading(Key::SongPerformanceHeading)
                .into_any_element(),
            self.song_pad(dials, true, cx),
        ];
        for dial in SONG_DIALS {
            if matches!(
                dial,
                SongDial::Tempo
                    | SongDial::Brightness
                    | SongDial::Energy
                    | SongDial::Tension
                    | SongDial::Syncopation
            ) {
                continue;
            }
            rows.push(self.song_dial_row(dials, *dial, cx));
        }
        rows
    }

    /// A song dial, including the numeric and step controls for tempo.
    fn song_dial_row(
        &self,
        dials: &SongDials,
        dial: SongDial,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
        let target = DialTarget::Song(dial);
        let fraction = dial.fraction(dials);
        let slider = value_slider(
            ("song-dial", dial as usize),
            self.t(dial.label()),
            if dial == SongDial::Tempo {
                String::new()
            } else {
                dial.text(dials)
            },
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
        );
        if dial == SongDial::Tempo {
            let mut tempo_row = div().flex().items_center().gap_1().child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(slider.debug_selector(|| "song-tempo-dial".to_string())),
            );
            for (id, label, step) in [
                ("song-tempo-decrease", "−1", -1.0),
                ("song-tempo-increase", "+1", 1.0),
            ] {
                if step > 0.0 {
                    tempo_row = tempo_row.child(button(
                        "song-tempo-value",
                        format!("{} BPM", dial.text(dials)),
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            let current = this
                                .song_sheet
                                .as_ref()
                                .map_or_else(String::new, |dials| dials.tempo.to_string());
                            this.open_prompt(Prompt::new(
                                format!("{} (20–400 BPM)", this.t(Key::Tempo)),
                                PromptTarget::SongTempo,
                                current,
                            ));
                            cx.notify();
                        }),
                    ));
                }
                tempo_row = tempo_row.child(button(
                    id,
                    label,
                    ButtonStyle::Normal,
                    false,
                    theme.accent,
                    &theme,
                    cx.listener(move |this, _, _, cx| {
                        if let Some(dials) = this.song_sheet.as_mut() {
                            dials.tempo = (dials.tempo + step).clamp(*TEMPO.start(), *TEMPO.end());
                        }
                        cx.notify();
                    }),
                ));
            }
            tempo_row.into_any_element()
        } else {
            slider.into_any_element()
        }
    }

    /// Section structure below the basic controls, one card per playing of a section.
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
        let mut rows: Vec<AnyElement> = vec![
            div()
                .col_span_full()
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
                    Self::opens_menu(cx, |this, at| {
                        let places = this
                            .song_sheet
                            .as_ref()
                            .map_or(0, |dials| dials.form.len().saturating_sub(1));
                        this.song_section_menu(at, places)
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
                .map(|(known, chart)| {
                    // An unwritten chart's own label is just its name, which says nothing about
                    // the one thing the row should say: that the composer is inventing here.
                    if chart.is_unwritten() {
                        self.t(Key::SongChordsOwn).to_string()
                    } else {
                        self.progression_name(&chart_label(known, chart))
                    }
                })
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
            rows.push(
                div()
                    .debug_selector(move || format!("song-form-card-{place}"))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .w_full()
                    .min_w_0()
                    .p_2()
                    .rounded(Metrics::RADIUS_SM)
                    .bg(theme.surface_sunken)
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .gap_1()
                            .child(div().flex_1().min_w_0().child(self.song_card_picker(
                                ("song-form-name", place),
                                Key::SongSectionName,
                                super::lyrics::section_label(self, name),
                                Self::opens_menu(cx, move |this, at| {
                                    this.song_form_name_menu(at, place)
                                }),
                            )))
                            .child(div().flex_1().min_w_0().child(self.song_card_picker(
                                ("song-section-chords", place),
                                Key::SongChords,
                                chart,
                                Self::opens_menu(cx, move |this, at| {
                                    this.song_chords_menu(at, index)
                                }),
                            )))
                            .child(div().w(px(22.0)).flex_shrink_0().child(button(
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
                            .child(div().w(px(22.0)).flex_shrink_0().child(button(
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
                            .child(div().w(px(22.0)).flex_shrink_0().child(button(
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
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(div().flex_1().min_w_0().child(self.song_card_picker(
                                ("song-section-transpose", place),
                                Key::SongTranspose,
                                transpose_label(section.transpose),
                                Self::opens_menu(cx, move |this, at| {
                                    this.song_transpose_menu(at, index)
                                }),
                            )))
                            .child(div().flex_1().min_w_0().child(self.song_card_picker(
                                ("song-section-tempo", place),
                                Key::Tempo,
                                section_tempo_label(section, self.language()),
                                cx.listener(move |this, _, _, cx| {
                                    let current = this
                                        .song_sheet
                                        .as_ref()
                                        .and_then(|d| d.sections.get(index))
                                        .and_then(|s| s.tempo)
                                        .map(|bpm| bpm.to_string())
                                        .unwrap_or_default();
                                    this.open_prompt(Prompt::new(
                                        format!(
                                            "{} · {}",
                                            this.t(Key::SongSectionTempo),
                                            this.t(Key::SongTempoHint)
                                        ),
                                        PromptTarget::SongSectionTempo(index),
                                        current,
                                    ));
                                    cx.notify();
                                }),
                            ))),
                    )
                    .child(
                        self.song_card_picker(
                            if dials.form.iter().position(|known| known == name) == Some(place) {
                                ("song-melody-source", index)
                            } else {
                                ("song-form-melody-source", place)
                            },
                            Key::SongMelodyFrom,
                            section
                                .melody_from
                                .as_deref()
                                .map(|source| super::lyrics::section_label(self, source))
                                .unwrap_or_else(|| self.t(Key::SongChordsOwn).to_string()),
                            Self::opens_menu(cx, move |this, at| this.song_melody_menu(at, index)),
                        ),
                    )
                    .child(dial_row)
                    .into_any_element(),
            );
        }
        rows
    }

    /// The heading over the roster strip, with the button that adds another part.
    fn song_parts_header(&mut self, _dials: &SongDials, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .child(self.group_heading(Key::SongInstrumentsHeading)),
            )
            .child(button(
                "song-add-part",
                self.t(Key::SongAddPart),
                ButtonStyle::Normal,
                false,
                theme.accent,
                &theme,
                Self::opens_menu(cx, |this, at| this.song_add_part_menu(at)),
            ))
            .into_any_element()
    }

    /// The shared kit has its own source and add/remove controls, outside the instrument grid.
    fn song_drum_rows(&mut self, dials: &SongDials, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = self.theme.clone();
        let mut rows = vec![self.group_heading(Key::PresetDrums).into_any_element()];
        if let Some(index) = dials.parts.iter().position(|part| part.role.is_drum()) {
            let part = &dials.parts[index];
            let sound = part
                .program
                .map(|p| p.kit_name().to_string())
                .unwrap_or_else(|| {
                    self.registry()
                        .instruments()
                        .find(|d| d.id == part.instrument)
                        .map(|d| {
                            auris_i18n::audio::plugin_name(&d.name, self.language()).to_string()
                        })
                        .unwrap_or_else(|| part.instrument.clone())
                });
            let mut kit_row =
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(self.sheet_picker(
                        "song-drum-kit",
                        Key::SongPartInstrument,
                        sound,
                        Self::opens_menu(cx, |this, at| this.song_drum_menu(at)),
                    )));
            if dials.parts.iter().any(|part| !part.role.is_drum()) {
                kit_row = kit_row.child(button(
                    "song-remove-drums",
                    self.t(Key::SongRemovePart),
                    ButtonStyle::Normal,
                    false,
                    theme.accent,
                    &theme,
                    cx.listener(|this, _, _, cx| {
                        this.run_menu_command(
                            crate::ui::context_menu::MenuCommand::SongDrums(false),
                            cx,
                        );
                        cx.notify();
                    }),
                ));
            }
            rows.push(kit_row.into_any_element());
        } else {
            rows.push(
                button(
                    "song-add-drums",
                    self.t(Key::SongAddDrums),
                    ButtonStyle::Normal,
                    false,
                    theme.accent,
                    &theme,
                    cx.listener(|this, _, _, cx| {
                        this.run_menu_command(
                            crate::ui::context_menu::MenuCommand::SongDrums(true),
                            cx,
                        );
                        cx.notify();
                    }),
                )
                .into_any_element(),
            );
        }
        rows
    }

    /// The roster: one card per melodic part, sized so two stand side by side.
    fn song_part_rows(&mut self, dials: &SongDials, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = self.theme.clone();
        let removable = dials.parts.len() > 1;
        let mut rows: Vec<AnyElement> = Vec::new();

        for (index, part) in dials.parts.iter().enumerate() {
            if part.role.is_drum() {
                continue;
            }
            let source_owner = part_source_owner(dials, index).unwrap_or(index);
            let shared_source = format!(
                "{} · {}",
                self.t(Key::PresetDrums),
                dials.parts[source_owner].name
            );
            // What the part will be *heard* as: the General MIDI sound where it names one, and
            // otherwise the plugin. Showing the plugin under a part that asked for a violin would
            // name the fallback and never the sound.
            let instrument = self.song_part_source_label(part);

            let mut dial_row = div().flex().gap_2();
            for dial in PART_DIALS {
                if !self.song_advanced || *dial == PartDial::Density {
                    continue;
                }
                let dial = *dial;
                if part.role.is_drum() && matches!(dial, PartDial::Gain | PartDial::Pan) {
                    continue;
                }
                let target = DialTarget::Part(index, dial);
                let fraction = dial.fraction(part, dials.mood);
                dial_row = dial_row.child(div().flex_1().min_w_0().child(value_slider(
                    ("song-part-dial", index * PART_DIALS.len() + dial as usize),
                    self.t(dial.label()),
                    dial.text(part, dials.mood),
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
                    .gap_2()
                    .w_full()
                    .min_w_0()
                    .p_2()
                    .rounded(Metrics::RADIUS_SM)
                    .bg(theme.surface_sunken)
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .gap_2()
                            .child(div().flex_1().min_w_0().child(self.song_card_picker(
                                ("song-part-name", index),
                                Key::SongPartName,
                                part.name.clone(),
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
                            .child(div().flex_1().min_w_0().child(self.song_card_picker(
                                ("song-part-role", index),
                                Key::SongPartRole,
                                self.t(role_key(part.role)).to_string(),
                                Self::opens_menu(cx, move |this, at| {
                                    this.song_role_menu(at, index)
                                }),
                            )))
                            // The last part cannot go: a song with no parts writes no notes, and
                            // the button goes dead rather than Write producing an empty document.
                            .child(div().w(px(64.0)).flex_shrink_0().child(button(
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
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .gap_2()
                            .when(source_owner == index, |row| {
                                row.child(div().flex_1().min_w_0().child(self.song_card_picker(
                                    ("song-part-instrument", index),
                                    Key::SongPartInstrument,
                                    instrument,
                                    Self::opens_menu(cx, move |this, at| {
                                        this.song_instrument_menu(at, index)
                                    }),
                                )))
                            })
                            .when(source_owner != index, |row| {
                                row.child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_xs()
                                        .text_color(theme.text_muted)
                                        .child(shared_source),
                                )
                            })
                            .child(div().w(px(96.0)).flex_shrink_0().child({
                                let drum = part.drum_note();
                                self.song_card_picker(
                                    ("song-part-note", index),
                                    if drum.is_some() {
                                        Key::SongPartNote
                                    } else {
                                        Key::PartOctave
                                    },
                                    drum.map_or_else(
                                        || part.octave.to_string(),
                                        |note| note.to_string(),
                                    ),
                                    Self::opens_menu(cx, move |this, at| match drum.is_some() {
                                        true => this.song_note_menu(at, index),
                                        false => this.song_octave_menu(at, index),
                                    }),
                                )
                            })),
                    )
                    .child(self.song_part_density(dials, index, cx))
                    .child(dial_row)
                    .into_any_element(),
            );
        }
        rows
    }

    /// A caption over a picker keeps short numbers meaningful within compact cards.
    fn song_card_picker<I, F>(&self, id: I, label: Key, value: String, on_click: F) -> gpui::Div
    where
        I: Into<gpui::ElementId>,
        F: Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    {
        let theme = &self.theme;
        div()
            .flex()
            .flex_col()
            .gap_1()
            .min_w_0()
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(label)),
            )
            .child(
                button(
                    id,
                    "",
                    ButtonStyle::Normal,
                    false,
                    theme.accent,
                    theme,
                    on_click,
                )
                .w_full()
                .min_w_0()
                .child(crate::ui::widgets::bounded_picker_label(value.clone()))
                .tooltip(crate::ui::tooltip::keyed_tip(value, "", theme)),
            )
    }

    /// A part's density follows the mood until adjusted, and can be returned to that policy.
    fn song_part_density(
        &self,
        dials: &SongDials,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let part = &dials.parts[index];
        let theme = &self.theme;
        let fraction = PartDial::Density.fraction(part, dials.mood);
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div().flex_1().min_w_0().child(
                    value_slider(
                        ("song-part-density", index),
                        self.t(Key::PartDensity),
                        PartDial::Density.text(part, dials.mood),
                        fraction,
                        theme.accent,
                        SliderFill::FromStart,
                        theme,
                        cx.listener(move |this, event: &MouseDownEvent, _, _| {
                            this.begin_drag(Drag::SongDial {
                                target: DialTarget::Part(index, PartDial::Density),
                                start_fraction: fraction,
                                start_x: event.position.x,
                            });
                        }),
                    )
                    .debug_selector(move || format!("song-part-density-{index}")),
                ),
            )
            .child(button(
                ("song-part-density-auto", index),
                self.t(Key::SongDensityAuto),
                ButtonStyle::Normal,
                part.density.is_none(),
                theme.accent,
                theme,
                cx.listener(move |this, _, _, cx| {
                    if let Some(dials) = this.song_sheet.as_mut()
                        && let Some(part) = dials.parts.get_mut(index)
                    {
                        part.density = match part.density {
                            Some(_) => None,
                            None => Some(dials.mood.density()),
                        };
                    }
                    cx.notify();
                }),
            ))
    }

    /// A row with a label at the start and a button holding the value.
    ///
    /// The same control as the inspector's rows and a plugin's choice parameters — drawn by
    /// [`crate::ui::widgets::picker_row`] — turned the other way round. Its values can be long:
    /// a key, a groove, a whole progression. Pinning the label
    /// instead of the button is what leaves them room.
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
        picker_row(
            id,
            self.t(label),
            value,
            RowColumn::Label(LABEL_WIDTH),
            false,
            &self.theme,
            on_click,
        )
    }

    /// Moves one of the sheet's dials, from a drag.
    ///
    /// The travel every other bar in the application has, because it is the same bar: see
    /// [`crate::ui::widgets::DRAG_RANGE_PIXELS`].
    pub(crate) fn drag_song_dial(&mut self, target: DialTarget, start_fraction: f32, delta: f32) {
        let Some(dials) = self.song_sheet.as_mut() else {
            return;
        };
        target.set(dials, crate::ui::widgets::dragged(start_fraction, delta));
    }

    /// Opens the sheet that takes the song's meter as `11/8`.
    ///
    /// The menu on the row covers what nearly everybody wants; this is the way to the rest,
    /// exactly as the transport's signature field has one.
    pub(crate) fn prompt_for_song_meter(&mut self) {
        let title = self.t(Key::SongMeter);
        let current = self
            .song_sheet
            .as_ref()
            .map_or_else(String::new, |dials| dials.meter.to_string());
        self.open_prompt(Prompt::new(title, PromptTarget::SongMeter, current));
    }

    /// Writes the piece the sheet describes, replacing the document.
    pub(crate) fn write_song_from_sheet(&mut self) -> bool {
        let Some(dials) = self.song_sheet.as_ref() else {
            return false;
        };
        let spec = song_spec(dials);
        if let Err(error) = self.session.validate_song_lyrics(&spec) {
            let message = self.failure(Key::CmdComposeSong, &error);
            self.open_prompt(Prompt::notice(
                self.t(Key::CmdComposeSong),
                [message.into()],
            ));
            return false;
        }
        self.compose_spec(&spec)
    }

    /// Saves the sheet as a specification file.
    pub(crate) fn save_song_specification(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(dials) = self.song_sheet.as_ref() else {
            return;
        };
        let text = song_spec(dials).to_toml();
        let name = format!(
            "{}.{}",
            safe_file_stem(&dials.title),
            auris_session::SPEC_EXTENSION
        );
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

/// A title safe to offer as one file name on every supported desktop platform.
fn safe_file_stem(title: &str) -> String {
    let stem: String = title
        .chars()
        .map(|ch| match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            ch if ch.is_control() => '_',
            ch => ch,
        })
        .collect();
    let stem = stem.trim().trim_end_matches(['.', ' ']);
    if stem.is_empty() {
        "Untitled".to_string()
    } else {
        stem.to_string()
    }
}

#[cfg(test)]
mod window_tests {
    use gpui::{TestAppContext, px, size};

    use auris_session::prelude::*;

    use crate::harness::{choose, click, open, paint, resize};
    use crate::ui::context_menu::MenuCommand;

    #[gpui::test]
    fn basic_ending_picker_creates_and_reopens_a_loop(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            this.open_song_sheet();
            let dials = this.song_sheet.as_mut().unwrap();
            dials.sections.truncate(1);
            dials.sections[0].bars = 2;
            dials.form = vec![dials.sections[0].name.clone()];
        });
        paint(&app, cx);
        click("song-ending", cx);
        paint(&app, cx);
        choose(&app, cx, &MenuCommand::SongEnding(Ending::Loop));
        paint(&app, cx);
        click("song-sheet-write", cx);
        app.update(cx, |this, _| {
            assert!(this.song_sheet.is_none());
            assert!(this.project().loop_enabled);
            this.open_song_sheet();
            let dials = this.song_sheet.as_ref().unwrap();
            assert_eq!(dials.ending, Ending::Loop);
            assert_eq!(dials.form.len(), 1);
        });
    }

    #[gpui::test]
    fn basic_song_controls_reflow_and_disclosure_preserves_the_song_and_lyrics(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            this.open_song_sheet();
            assert!(!this.song_advanced);
        });
        for width in [1500.0, 900.0, 640.0] {
            let viewport = size(px(width), px(600.0));
            resize(&app, cx, viewport);
            for selector in [
                "song-sheet-form",
                "song-sheet-drums",
                "song-sheet-parts",
                "song-rhythm-pad",
                "song-key",
                "song-meter",
                "song-seed",
                "song-sheet-save",
                "song-sheet-take",
            ] {
                assert!(
                    cx.debug_bounds(selector).is_none(),
                    "{selector} is reserved for detailed settings"
                );
            }
            let song = cx.debug_bounds("song-sheet-song").unwrap();
            let lyrics = cx.debug_bounds("song-sheet-lyrics").unwrap();
            if width >= 792.0 {
                assert_eq!(song.top(), lyrics.top());
                assert!(song.right() <= lyrics.left());
            } else {
                assert!(lyrics.top() >= song.bottom());
            }
            for selector in ["song-advanced", "song-sheet-write", "song-sheet-cancel"] {
                let bounds = cx.debug_bounds(selector).unwrap();
                assert!(bounds.left() >= px(0.0) && bounds.right() <= viewport.width);
                assert!(bounds.top() >= px(0.0) && bounds.bottom() <= viewport.height);
            }
        }
        app.update(cx, |this, _| {
            let dials = this.song_sheet.as_mut().unwrap();
            dials.seed = 827;
            dials.motif = vec![0, 2, 4];
            let verse = dials
                .sections
                .iter()
                .position(|s| s.name == "verse")
                .unwrap();
            this.focus_section_lyrics(verse);
        });
        paint(&app, cx);
        cx.simulate_input("さくら");
        let before = app.read_with(cx, |this, _| {
            (this.song_sheet.clone(), this.lyrics_edit.clone())
        });
        for advanced in [true, false] {
            click("song-advanced", cx);
            paint(&app, cx);
            app.read_with(cx, |this, _| {
                assert_eq!(this.song_advanced, advanced);
                assert_eq!((this.song_sheet.clone(), this.lyrics_edit.clone()), before);
            });
        }
        cx.simulate_input("のはな");
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.lyrics_edit.as_ref().unwrap().field.content(),
                "さくらのはな"
            );
            assert!(
                this.song_sheet
                    .as_ref()
                    .unwrap()
                    .sections
                    .iter()
                    .any(|s| s.lyrics == "さくらのはな")
            );
        });
        click("song-sheet-write", cx);
        app.read_with(cx, |this, _| {
            assert!(this.song_sheet.is_none());
            let saved = SongSpec::parse(this.project().song_spec.as_deref().unwrap()).unwrap();
            assert_eq!(saved.seed, 827);
            assert_eq!(saved.motif, vec![0, 2, 4]);
            assert_eq!(saved.sections["verse"].lyrics, "さくらのはな");
        });
    }

    #[gpui::test]
    fn disclosing_details_keeps_basic_controls_in_place(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| this.open_song_sheet());
        for language in [
            auris_i18n::Language::English,
            auris_i18n::Language::Japanese,
        ] {
            app.update(cx, |this, _| this.language = language);
            for width in [1280.0, 900.0, 640.0] {
                resize(&app, cx, size(px(width), px(650.0)));
                let selectors = [
                    "song-title",
                    "song-tempo-value",
                    "song-sheet-lyrics",
                    "song-participation-matrix",
                ];
                let before = selectors.map(|selector| cx.debug_bounds(selector).unwrap());
                click("song-advanced", cx);
                paint(&app, cx);
                for (selector, expected) in selectors.into_iter().zip(before) {
                    assert_eq!(
                        cx.debug_bounds(selector).unwrap(),
                        expected,
                        "{selector} must stay in place after disclosure at width {width}"
                    );
                }
                let lyrics = cx.debug_bounds("song-sheet-lyrics").unwrap();
                let harmony = cx.debug_bounds("song-sheet-harmony").unwrap();
                assert!(harmony.top() >= lyrics.bottom());
                click("song-advanced", cx);
                paint(&app, cx);
            }
        }
    }

    #[gpui::test]
    fn the_song_sheet_reflows_without_hiding_fields_or_actions(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            this.open_song_sheet();
            this.song_advanced = true;
        });
        // gpui's selector lookup requires a static name, as in the harness's menu helper.
        let last_part_selector: &'static str = Box::leak(
            app.read_with(cx, |this, _| {
                format!(
                    "song-part-name-{}",
                    this.song_sheet
                        .as_ref()
                        .unwrap()
                        .parts
                        .iter()
                        .rposition(|p| !p.role.is_drum())
                        .unwrap()
                )
            })
            .into_boxed_str(),
        );
        for language in [
            auris_i18n::Language::English,
            auris_i18n::Language::Japanese,
        ] {
            app.update(cx, |this, _| this.language = language);
            for width in [1500.0, 900.0, 640.0] {
                for height in [600.0, 480.0] {
                    let viewport = size(px(width), px(height));
                    resize(&app, cx, viewport);
                    for selector in [
                        "song-sheet-panel",
                        "song-sheet-cancel",
                        "song-sheet-save",
                        "song-sheet-take",
                        "song-sheet-write",
                    ] {
                        let bounds = cx
                            .debug_bounds(selector)
                            .expect("the sheet control is drawn");
                        assert!(
                            bounds.left() >= px(0.0)
                                && bounds.right() <= viewport.width
                                && bounds.top() >= px(0.0)
                                && bounds.bottom() <= viewport.height,
                            "{selector} must stay in {viewport:?}: {bounds:?}"
                        );
                    }
                    let song = cx.debug_bounds("song-sheet-song").unwrap();
                    let form = cx.debug_bounds("song-sheet-form").unwrap();
                    let lyrics = cx.debug_bounds("song-sheet-lyrics").unwrap();
                    for bounds in [song, form, lyrics] {
                        assert!(
                            bounds.size.width >= px(320.0),
                            "fields retain usable widths: {bounds:?}"
                        );
                        assert!(bounds.left() >= px(0.0) && bounds.right() <= viewport.width);
                    }
                    if width >= 792.0 {
                        assert_eq!(song.top(), lyrics.top());
                        assert!(song.right() <= lyrics.left());
                    } else {
                        assert!(lyrics.top() >= song.bottom());
                    }
                    assert!(form.top() >= song.bottom().max(lyrics.bottom()));
                    let body = cx.debug_bounds("song-sheet-body").unwrap();
                    assert_eq!(cx.debug_bounds("song-sheet-scrollbar").unwrap(), body);
                    assert!(
                        body.size.height >= px(200.0),
                        "the fields must have room to scroll"
                    );
                    cx.simulate_event(gpui::ScrollWheelEvent {
                        position: body.center(),
                        delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(-10000.0))),
                        ..Default::default()
                    });
                    paint(&app, cx);
                    let name = cx.debug_bounds(last_part_selector).unwrap();
                    cx.simulate_event(gpui::ScrollWheelEvent {
                        position: body.center(),
                        delta: gpui::ScrollDelta::Pixels(gpui::point(
                            px(0.0),
                            body.center().y - name.center().y,
                        )),
                        ..Default::default()
                    });
                    paint(&app, cx);
                    let name = cx.debug_bounds(last_part_selector).unwrap();
                    assert!(
                        name.top() >= body.top() && name.bottom() <= body.bottom(),
                        "scrolling reaches the final part's controls: {name:?} inside {body:?}"
                    );
                    cx.simulate_click(name.center(), gpui::Modifiers::none());
                    app.update(cx, |this, _| {
                        assert!(
                            this.prompt.is_some(),
                            "the visible field receives the click"
                        );
                        this.prompt = None;
                    });
                    paint(&app, cx);
                    cx.simulate_event(gpui::ScrollWheelEvent {
                        position: body.center(),
                        delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(10000.0))),
                        ..Default::default()
                    });
                    paint(&app, cx);
                }
            }
        }
        click("song-sheet-write", cx);
        app.read_with(cx, |this, _| {
            assert!(
                this.song_sheet.is_none(),
                "the visible primary action receives the click"
            );
            assert!(
                !this.project().tracks.is_empty(),
                "the click writes the song"
            );
        });
    }

    #[test]
    fn specification_file_names_do_not_treat_titles_as_paths() {
        assert_eq!(super::safe_file_stem("A/B: C?"), "A_B_ C_");
        assert_eq!(super::safe_file_stem(" .. "), "Untitled");
    }

    /// The meter row's menu lists eight; this is the path to the other three hundred and
    /// ninety-two, made as a hand makes it: the field comes up, the meter is typed, Return.
    #[gpui::test]
    fn any_meter_can_be_typed_onto_the_sheet(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            this.open_song_sheet();
            this.prompt_for_song_meter();
        });
        paint(&app, cx);
        app.update(cx, |this, _| {
            // The field opens holding the meter in force; typing over it is what a person
            // does with the selection the click left.
            this.prompt
                .as_mut()
                .and_then(super::Prompt::field_mut)
                .expect("the meter sheet is up")
                .select_all();
        });

        cx.simulate_input("11/8");
        cx.simulate_keystrokes("enter");

        app.read_with(cx, |this, _| {
            assert_eq!(
                this.song_sheet.as_ref().expect("the sheet is open").meter,
                TimeSignature::new(11, 8),
                "the typed meter landed on the dial"
            );
            assert!(this.prompt.is_none(), "and the field closed behind it");
        });
    }

    /// `5/3` is not a meter, and the field says so instead of quietly landing on 4/4 —
    /// `TimeSignature::new` would have changed the subject, which is exactly why the prompt
    /// parses rather than constructs.
    #[gpui::test]
    fn a_meter_that_is_not_one_is_refused_not_rounded(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            this.open_song_sheet();
            if let Some(dials) = this.song_sheet.as_mut() {
                dials.meter = TimeSignature::new(7, 8);
            }
            this.prompt_for_song_meter();
        });
        paint(&app, cx);
        app.update(cx, |this, _| {
            this.prompt
                .as_mut()
                .and_then(super::Prompt::field_mut)
                .expect("the meter sheet is up")
                .select_all();
        });

        cx.simulate_input("5/3");
        cx.simulate_keystrokes("enter");

        app.read_with(cx, |this, _| {
            assert_eq!(
                this.song_sheet.as_ref().expect("the sheet is open").meter,
                TimeSignature::new(7, 8),
                "the dial did not move"
            );
            // The status line also reports the refusal while the field remains editable.
            assert!(this.status_failed, "the refusal reached the status line");
        });
    }
}
