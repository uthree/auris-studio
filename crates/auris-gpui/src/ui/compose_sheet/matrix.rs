//! Participation by part and form occurrence, with fixed instrument labels beside the timeline.

use auris_i18n::Key;
use gpui::{Context, IntoElement, Pixels, Window, div, prelude::*, px};
use gpui_component::scroll::{Scrollbar, ScrollbarShow};

use crate::app::AurisApp;
use crate::theme::{Metrics, Theme};
use crate::ui::icons::{Icon, icon};
use crate::ui::tooltip::keyed_tip;
use crate::ui::widgets::{ButtonStyle, bounded_picker_label, button};

use super::dials::*;

const HEADER_HEIGHT: Pixels = px(52.0);
const ROW_HEIGHT: Pixels = px(56.0);
const MIN_COLUMN_WIDTH: Pixels = px(96.0);
const SCROLLBAR_HEIGHT: Pixels = px(14.0);

impl AurisApp {
    pub(super) fn song_participation_matrix(
        &self,
        dials: &SongDials,
        sheet_width: Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let label_width = if sheet_width < px(760.0) {
            px(160.0)
        } else {
            px(200.0)
        };
        let available = (sheet_width - px(48.0) - label_width).max(MIN_COLUMN_WIDTH);
        let column_width = (available / dials.form.len().max(1) as f32).max(MIN_COLUMN_WIDTH);
        let height = HEADER_HEIGHT + ROW_HEIGHT * dials.parts.len() as f32;
        let horizontal = window
            .use_keyed_state("song-matrix-scroll", cx, |_, _| gpui::ScrollHandle::new())
            .read(cx)
            .clone();
        let mut labels = div()
            .debug_selector(|| "song-matrix-labels".to_string())
            .flex()
            .flex_col()
            .w(label_width)
            .flex_shrink_0()
            .child(
                div()
                    .h(HEADER_HEIGHT)
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .px_2()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::SongPartInstrument)),
            );
        for (index, part) in dials.parts.iter().enumerate() {
            let source = self.song_part_source_label(part);
            let role = self.t(role_key(part.role));
            let name = format!("{} · {role}", part.name);
            labels = labels.child(
                div()
                    .debug_selector(move || format!("song-matrix-row-{index}"))
                    .h(ROW_HEIGHT)
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap_1()
                    .px_2()
                    .border_t_1()
                    .border_color(theme.border_subtle)
                    .child(
                        div()
                            .h(px(16.0))
                            .text_xs()
                            .text_color(theme.text)
                            .child(bounded_picker_label(name.clone()))
                            .id(("song-matrix-part-label", index))
                            .tooltip(keyed_tip(name, "", &theme)),
                    )
                    .child(
                        button(
                            ("song-matrix-instrument", index),
                            "",
                            ButtonStyle::Normal,
                            false,
                            theme.accent,
                            &theme,
                            cx.listener(move |this, _, _, cx| this.open_song_library(index, cx)),
                        )
                        .w_full()
                        .min_w_0()
                        .child(bounded_picker_label(source.clone()))
                        .tooltip(keyed_tip(source, "", &theme)),
                    ),
            );
        }

        let mut columns = div()
            .flex()
            .h(height)
            .w(column_width * dials.form.len() as f32)
            .flex_shrink_0();
        let mut first_bar = 1_usize;
        for place in 0..dials.form.len() {
            let Some(section_index) = section_at(dials, place) else {
                continue;
            };
            let section = &dials.sections[section_index];
            let label = super::lyrics::section_label(self, &section.name);
            let title = format!("{} · {label}", place + 1);
            let last_bar = first_bar.saturating_add(section.bars.saturating_sub(1));
            let bars = format!("{first_bar}–{last_bar} {}", self.t(Key::SongBarsUnit));
            first_bar = first_bar.saturating_add(section.bars);
            let playing = dials
                .parts
                .iter()
                .filter(|part| part_plays_in(section, &part.name))
                .count();
            let mut column = div()
                .debug_selector(move || format!("song-matrix-column-{place}"))
                .w(column_width)
                .flex_shrink_0()
                .flex()
                .flex_col()
                .border_l_1()
                .border_color(theme.border_subtle)
                .child(
                    div()
                        .id(("song-matrix-heading", place))
                        .h(HEADER_HEIGHT)
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .justify_center()
                        .gap_1()
                        .px_2()
                        .text_xs()
                        .text_color(theme.text)
                        .child(div().h(px(18.0)).child(bounded_picker_label(title.clone())))
                        .child(
                            div()
                                .h(px(16.0))
                                .text_color(theme.text_muted)
                                .child(bounded_picker_label(bars.clone())),
                        )
                        .tooltip(keyed_tip(format!("{title} · {bars}"), "", &theme)),
                );
            for (part_index, part) in dials.parts.iter().enumerate() {
                let on = part_plays_in(section, &part.name);
                let locked = on && playing <= 1;
                let part_name = part.name.clone();
                let status = self.t(if on {
                    Key::SongMatrixPlaying
                } else {
                    Key::SongMatrixRest
                });
                let tip = if locked {
                    self.t(Key::SongMatrixLastPart).to_string()
                } else {
                    format!("{} · {label}: {status}", part.name)
                };
                let cell = div()
                    .id(("song-matrix-cell", part_index * dials.form.len() + place))
                    .debug_selector(move || format!("song-matrix-cell-{part_index}-{place}"))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .size_full()
                    .rounded(Metrics::RADIUS_SM)
                    .border_1()
                    .border_color(if on { theme.accent } else { theme.border })
                    .bg(if on {
                        Theme::translucent(theme.accent, 0.2)
                    } else {
                        theme.surface_raised
                    })
                    .text_xs()
                    .text_color(if on { theme.text } else { theme.text_muted })
                    .when(on, |cell| {
                        cell.child(icon(Icon::Check, px(12.0), theme.accent))
                    })
                    .child(status)
                    .when(!locked, |cell| {
                        cell.cursor_pointer()
                            .hover(|cell| cell.bg(theme.surface_hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(dials) = this.song_sheet.as_mut()
                                    && let Some(section) = section_at(dials, place)
                                {
                                    toggle_part_in_section(dials, section, &part_name);
                                }
                                cx.notify();
                            }))
                    })
                    .when(locked, |cell| cell.opacity(0.65))
                    .tooltip(keyed_tip(tip, "", &theme));
                column = column.child(
                    div()
                        .h(ROW_HEIGHT)
                        .flex_shrink_0()
                        .p_1()
                        .border_t_1()
                        .border_color(theme.border_subtle)
                        .child(cell),
                );
            }
            columns = columns.child(column);
        }
        let repeated = dials
            .form
            .iter()
            .enumerate()
            .any(|(place, name)| dials.form[..place].contains(name));
        div()
            .debug_selector(|| "song-participation-matrix".to_string())
            .flex()
            .flex_col()
            .gap_2()
            .min_w_0()
            .child(self.group_heading(Key::SongMatrixHeading))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::SongMatrixHint)),
            )
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .w_full()
                    .rounded(Metrics::RADIUS_SM)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.surface_sunken)
                    .child(labels)
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h(height + SCROLLBAR_HEIGHT)
                            .child(
                                div()
                                    .id("song-matrix-timeline")
                                    .debug_selector(|| "song-matrix-timeline".to_string())
                                    .h(height + SCROLLBAR_HEIGHT)
                                    .w_full()
                                    .min_w_0()
                                    .pb(SCROLLBAR_HEIGHT)
                                    .overflow_x_scroll()
                                    .map(|mut timeline| {
                                        timeline.style().restrict_scroll_to_axis = Some(true);
                                        timeline
                                    })
                                    .track_scroll(&horizontal)
                                    .child(columns),
                            )
                            .child(
                                div().absolute().inset_0().child(
                                    Scrollbar::horizontal(&horizontal)
                                        .scrollbar_show(ScrollbarShow::Always),
                                ),
                            ),
                    ),
            )
            .when(repeated, |view| {
                view.child(
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(self.t(Key::SongMatrixRepeated)),
                )
            })
    }

    /// The sound the part will use, including a GM program rather than only its fallback plugin.
    pub(super) fn song_part_source_label(&self, part: &auris_session::prelude::PartSpec) -> String {
        if let Some(source) = &part.source {
            return self.song_library_source_label(source);
        }
        match part.program {
            Some(program) => program.label(part.role.is_drum()).to_string(),
            None => self
                .registry()
                .instruments()
                .find(|descriptor| descriptor.id == part.instrument)
                .map(|descriptor| {
                    auris_i18n::audio::plugin_name(&descriptor.name, self.language()).to_string()
                })
                .unwrap_or_else(|| part.instrument.clone()),
        }
    }
}

#[cfg(test)]
#[path = "matrix_tests.rs"]
mod tests;
