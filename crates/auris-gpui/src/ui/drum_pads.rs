//! The drum sampler's pads, drawn in its plugin window.
//!
//! One row per drum lane: its name, the file it plays, and the settings that are a pad's rather
//! than the instrument's — level, tuning, whether it plays to its end, and which choke group it
//! is in. A lane with no file shows only the way to give it one. The rows follow the lanes rather
//! than the files, because the lanes are what the drum editor and generation already call the
//! kit; a pad on a key no lane names is listed after them so nothing that sounds is hidden.

use auris_i18n::Key;
use auris_session::prelude::*;
use auris_session::{DrumPad, DrumPadEdit, PAD_MAX_GAIN_DB, PAD_MIN_GAIN_DB, PAD_TUNE_RANGE};
use gpui::{AnyElement, Pixels, Point, div, prelude::*, px};

use crate::app::AurisApp;
use crate::theme::Theme;
use crate::ui::context_menu::{ContextMenu, MenuCommand};
use crate::ui::icons::Icon;
use crate::ui::widgets::{ButtonStyle, button, chain_button};

/// Height of one pad row.
const ROW_HEIGHT: Pixels = px(28.0);

/// The tallest the list grows before it scrolls.
const LIST_MAX_HEIGHT: Pixels = px(280.0);

/// The choke groups the window offers; the agent reaches all sixteen.
const OFFERED_CHOKE_GROUPS: u8 = 8;

/// The keys the window lists, in order: every lane's, then any pad's that no lane names.
fn pad_keys(lanes: &[u8], pads: &[DrumPad]) -> Vec<u8> {
    let mut keys: Vec<u8> = lanes.to_vec();
    for pad in pads {
        if !keys.contains(&pad.note) {
            keys.push(pad.note);
        }
    }
    keys
}

/// A pad's level moved by `delta` decibels, kept inside the range a pad takes.
fn nudged_gain(gain: f32, delta: f32) -> f32 {
    (gain + delta).clamp(PAD_MIN_GAIN_DB, PAD_MAX_GAIN_DB)
}

/// A pad's tuning moved by `delta` semitones, kept inside the range a pad takes.
fn nudged_tune(tune: f32, delta: f32) -> f32 {
    (tune + delta).clamp(-PAD_TUNE_RANGE, PAD_TUNE_RANGE)
}

/// A level as the row prints it.
fn gain_label(gain: f32) -> String {
    match gain.abs() < 0.05 {
        true => "0 dB".into(),
        false => format!("{gain:+.0} dB"),
    }
}

/// A tuning as the row prints it: semitones, and cents when there are any.
fn tune_label(tune: f32) -> String {
    let cents = (tune * 100.0).round() as i32;
    match (cents % 100, cents / 100) {
        (0, 0) => "±0".into(),
        (0, semitones) => format!("{semitones:+}"),
        _ => format!("{:+.2}", tune),
    }
}

/// What the files on a pad are called, as one line.
fn names_label(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [only] => only.clone(),
        [first, rest @ ..] => format!("{first} +{}", rest.len()),
    }
}

impl AurisApp {
    /// The drum sampler's pads: a row per lane, each with its file and its settings.
    pub(crate) fn drum_pad_section(
        &self,
        track: TrackId,
        theme: &Theme,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let lanes: Vec<u8> = self
            .session
            .drum_lanes(track)
            .unwrap_or_default()
            .iter()
            .map(|lane| lane.note)
            .collect();
        let pads = self.session.drum_pads(track);
        let keys = pad_keys(&lanes, &pads);
        let header = div()
            .h(ROW_HEIGHT)
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .px_2()
            .text_xs()
            .text_color(theme.text_muted)
            .child(self.t(Key::DrumSamplerPads));
        let body = match keys.is_empty() {
            true => div()
                .px_2()
                .pb_2()
                .text_xs()
                .text_color(theme.text_muted)
                .child(self.t(Key::DrumSamplerNoLanes))
                .into_any_element(),
            false => div()
                .id("pw-drum-pads")
                .debug_selector(|| "pw-drum-pads".into())
                .w_full()
                .max_h(LIST_MAX_HEIGHT)
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .children(keys.into_iter().map(|note| {
                    let pad = pads.iter().find(|pad| pad.note == note);
                    self.drum_pad_row(track, note, pad, theme, cx)
                }))
                .into_any_element(),
        };
        div()
            .w_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(theme.border)
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// One lane's pad.
    fn drum_pad_row(
        &self,
        track: TrackId,
        note: u8,
        pad: Option<&DrumPad>,
        theme: &Theme,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let index = usize::from(note);
        let title = div()
            .w(px(120.0))
            .flex_shrink_0()
            .truncate()
            .text_xs()
            .text_color(theme.text)
            .child(self.drum_lane_title(track, note));
        let file = match pad {
            Some(pad) => {
                let name = match pad.loaded {
                    true => names_label(&pad.names),
                    false => format!(
                        "{} ({})",
                        names_label(&pad.names),
                        self.t(Key::SamplerSampleMissing)
                    ),
                };
                div()
                    .text_color(match pad.loaded {
                        true => theme.text,
                        false => theme.danger,
                    })
                    .child(name)
            }
            None => div()
                .text_color(theme.text_muted)
                .child(self.t(Key::DrumPadEmpty)),
        };
        let mut row = div()
            .id(("pw-drum-pad", index))
            .debug_selector(move || format!("pw-drum-pad-{note}"))
            .h(ROW_HEIGHT)
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .child(title)
            .child(div().flex_1().min_w_0().truncate().text_xs().child(file));
        if let Some(pad) = pad.cloned() {
            let edit = |edit: DrumPadEdit| {
                cx.listener(move |this: &mut Self, _, _, cx| {
                    this.edit_drum_pad(track, note, edit);
                    cx.notify();
                })
            };
            row = row
                .child(
                    chain_button(
                        ("pw-pad-gain-down", index),
                        Icon::ChevronDown,
                        theme,
                        edit(DrumPadEdit {
                            gain_db: Some(nudged_gain(pad.gain_db, -1.0)),
                            ..Default::default()
                        }),
                    )
                    .tooltip(self.t(Key::DrumPadLevel)),
                )
                .child(
                    div()
                        .w(px(48.0))
                        .text_xs()
                        .text_center()
                        .child(gain_label(pad.gain_db)),
                )
                .child(
                    chain_button(
                        ("pw-pad-gain-up", index),
                        Icon::ChevronUp,
                        theme,
                        edit(DrumPadEdit {
                            gain_db: Some(nudged_gain(pad.gain_db, 1.0)),
                            ..Default::default()
                        }),
                    )
                    .tooltip(self.t(Key::DrumPadLevel)),
                )
                .child(
                    chain_button(
                        ("pw-pad-tune-down", index),
                        Icon::ChevronDown,
                        theme,
                        edit(DrumPadEdit {
                            tune: Some(nudged_tune(pad.tune, -1.0)),
                            ..Default::default()
                        }),
                    )
                    .tooltip(self.t(Key::DrumPadTune)),
                )
                .child(
                    div()
                        .w(px(40.0))
                        .text_xs()
                        .text_center()
                        .child(tune_label(pad.tune)),
                )
                .child(
                    chain_button(
                        ("pw-pad-tune-up", index),
                        Icon::ChevronUp,
                        theme,
                        edit(DrumPadEdit {
                            tune: Some(nudged_tune(pad.tune, 1.0)),
                            ..Default::default()
                        }),
                    )
                    .tooltip(self.t(Key::DrumPadTune)),
                )
                .child(
                    button(
                        ("pw-pad-one-shot", index),
                        self.t(Key::DrumPadOneShot).to_string(),
                        ButtonStyle::Ghost,
                        pad.one_shot,
                        theme.accent_soft,
                        theme,
                        edit(DrumPadEdit {
                            one_shot: Some(!pad.one_shot),
                            ..Default::default()
                        }),
                    )
                    .tooltip(self.tip(Key::DrumPadOneShotTip, "")),
                )
                .child(
                    button(
                        ("pw-pad-choke", index),
                        match pad.choke {
                            Some(group) => format!("{} {group}", self.t(Key::DrumPadChoke)),
                            None => format!("{} —", self.t(Key::DrumPadChoke)),
                        },
                        ButtonStyle::Ghost,
                        pad.choke.is_some(),
                        theme.accent_soft,
                        theme,
                        Self::opens_menu(cx, move |this, at| {
                            this.drum_pad_choke_menu(at, track, note)
                        }),
                    )
                    .tooltip(self.tip(Key::DrumPadChokeTip, "")),
                );
        }
        row = row.child(
            chain_button(
                ("pw-pad-load", index),
                Icon::Plus,
                theme,
                cx.listener(move |this, _, _, cx| {
                    this.load_drum_sample(track, note, false, cx);
                }),
            )
            .tooltip(self.t(Key::DrumLaneLoadSample)),
        );
        if pad.is_some() {
            row = row.child(
                chain_button(
                    ("pw-pad-clear", index),
                    Icon::Cross,
                    theme,
                    cx.listener(move |this, _, _, cx| {
                        this.clear_drum_sample(track, note);
                        cx.notify();
                    }),
                )
                .tooltip(self.t(Key::DrumLaneClearSample)),
            );
        }
        row.into_any_element()
    }

    /// The choke groups one pad can join, with a tick on the one it is in.
    pub(crate) fn drum_pad_choke_menu(
        &self,
        anchor: Point<Pixels>,
        track: TrackId,
        note: u8,
    ) -> ContextMenu {
        let current = self
            .session
            .drum_pads(track)
            .into_iter()
            .find(|pad| pad.note == note)
            .and_then(|pad| pad.choke);
        let mut menu = ContextMenu::new(anchor, self.t(Key::DrumPadChoke)).toggle(
            self.t(Key::DrumPadChokeNone),
            MenuCommand::SetDrumPadChoke {
                track,
                note,
                choke: None,
            },
            current.is_none(),
        );
        for group in 1..=OFFERED_CHOKE_GROUPS {
            menu = menu.toggle(
                format!("{} {group}", self.t(Key::DrumPadChoke)),
                MenuCommand::SetDrumPadChoke {
                    track,
                    note,
                    choke: Some(group),
                },
                current == Some(group),
            );
        }
        menu
    }

    /// Changes one pad, and says why when it cannot.
    pub(crate) fn edit_drum_pad(&mut self, track: TrackId, note: u8, edit: DrumPadEdit) {
        if let Err(error) = self.session.set_drum_pad(track, note, edit) {
            self.set_failed_status(self.failure(Key::CmdEditSampleMap, &error));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad(note: u8) -> DrumPad {
        DrumPad {
            note,
            names: vec!["Hit".into()],
            loaded: true,
            gain_db: 0.0,
            tune: 0.0,
            one_shot: true,
            choke: None,
        }
    }

    #[test]
    fn the_rows_follow_the_lanes_and_keep_a_pad_no_lane_names() {
        assert_eq!(pad_keys(&[38, 36], &[pad(36), pad(50)]), vec![38, 36, 50]);
        assert_eq!(pad_keys(&[], &[]), Vec::<u8>::new());
    }

    #[test]
    fn a_step_stops_at_the_range_the_session_accepts() {
        assert_eq!(nudged_gain(PAD_MAX_GAIN_DB, 1.0), PAD_MAX_GAIN_DB);
        assert_eq!(nudged_gain(PAD_MIN_GAIN_DB, -1.0), PAD_MIN_GAIN_DB);
        assert_eq!(nudged_gain(-3.0, 1.0), -2.0);
        assert_eq!(nudged_tune(PAD_TUNE_RANGE, 1.0), PAD_TUNE_RANGE);
        assert_eq!(nudged_tune(0.0, -1.0), -1.0);
    }

    #[test]
    fn levels_and_tunings_print_the_way_a_player_reads_them() {
        assert_eq!(gain_label(0.0), "0 dB");
        assert_eq!(gain_label(-6.0), "-6 dB");
        assert_eq!(gain_label(3.0), "+3 dB");
        assert_eq!(tune_label(0.0), "±0");
        assert_eq!(tune_label(-2.0), "-2");
        assert_eq!(tune_label(0.25), "+0.25");
        assert_eq!(names_label(&["A".into(), "B".into()]), "A +1");
    }
}
