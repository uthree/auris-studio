//! The audio sampler's map, drawn in its plugin window.
//!
//! One row per recording: its name, the root it was recorded at with a step either way, the keys
//! it ends up playing, its velocity layer and a way to take it out. The keys are a readout rather
//! than a control because, in the automatic map, they are not a property of the row at all but of
//! its neighbours' roots — moving a root is how a split is moved. Fixed key ranges, tuning and
//! level per recording are the agent's, through `edit_sample_map`, which is where anyone building
//! a drum map by hand would rather say it than drag it.

use auris_i18n::Key;
use auris_session::SampleZoneView;
use auris_session::prelude::*;
use gpui::{AnyElement, Pixels, Point, div, prelude::*, px};

use crate::app::AurisApp;
use crate::theme::Theme;
use crate::ui::context_menu::{ContextMenu, MenuCommand};
use crate::ui::icons::Icon;
use crate::ui::widgets::{ButtonStyle, button, chain_button};

/// Height of one row of the map.
const ROW_HEIGHT: Pixels = px(28.0);

/// The tallest the list grows before it scrolls, so a chromatic multisample does not push the
/// sampler's own controls out of the window.
const LIST_MAX_HEIGHT: Pixels = px(196.0);

/// The velocity layers the window offers, as fractions of the full scale.
///
/// Halves and thirds are what nearly every recorded instrument comes in; anything finer is a map
/// somebody is building with a tool, and the agent's `edit_sample_map` takes any range.
pub(crate) const VELOCITY_LAYERS: [(Key, f32, f32); 6] = [
    (Key::SamplerVelocityAll, 0.0, 1.0),
    (Key::SamplerVelocitySoftHalf, 0.0, 0.5),
    (Key::SamplerVelocityHardHalf, 0.5, 1.0),
    (Key::SamplerVelocitySoftThird, 0.0, 1.0 / 3.0),
    (Key::SamplerVelocityMediumThird, 1.0 / 3.0, 2.0 / 3.0),
    (Key::SamplerVelocityHardThird, 2.0 / 3.0, 1.0),
];

/// A velocity range as the row prints it, in MIDI's 0–127 because that is how a player counts.
pub(crate) fn velocity_label(range: VelocityRange) -> String {
    let midi = |v: f32| (v * 127.0).round() as u8;
    format!("{}–{}", midi(range.low), midi(range.high))
}

/// A root moved by `delta` semitones, kept on the keyboard.
pub(crate) fn nudged_root(root: u8, delta: i32) -> u8 {
    (i32::from(root) + delta).clamp(0, 127) as u8
}

impl AurisApp {
    /// The sampler's map: a header with the way to add files, then a row per recording.
    pub(crate) fn sample_map_section(
        &self,
        track: TrackId,
        theme: &Theme,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let map = self.session.track_sample_map(track);
        let add_label = if map.is_empty() {
            self.t(Key::SamplerNoSample)
        } else {
            self.t(Key::SamplerAddSamples)
        };
        let header = div()
            .h(ROW_HEIGHT)
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .gap_1()
            .px_2()
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::SamplerSamples)),
            )
            .child(button(
                "pw-sample",
                add_label.to_string(),
                ButtonStyle::Ghost,
                true,
                theme.accent_soft,
                theme,
                cx.listener(move |this, _, _, cx| {
                    this.load_sample(track, cx);
                }),
            ));
        let rows: Vec<AnyElement> = map
            .iter()
            .enumerate()
            .map(|(index, view)| self.sample_zone_row(track, index, view, theme, cx))
            .collect();
        div()
            .w_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(theme.border)
            .child(header)
            .when(!rows.is_empty(), |section| {
                section.child(
                    div()
                        .id("pw-sample-map")
                        .debug_selector(|| "pw-sample-map".into())
                        .w_full()
                        .max_h(LIST_MAX_HEIGHT)
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .children(rows),
                )
            })
            .into_any_element()
    }

    /// One recording of the map.
    fn sample_zone_row(
        &self,
        track: TrackId,
        index: usize,
        view: &SampleZoneView,
        theme: &Theme,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let name = if view.loaded {
            view.name.clone()
        } else {
            format!("{} ({})", view.name, self.t(Key::SamplerSampleMissing))
        };
        let plays = view.plays.map_or_else(
            || "—".to_string(),
            |keys| {
                format!(
                    "{}–{}",
                    midi_name(i32::from(keys.low)),
                    midi_name(i32::from(keys.high))
                )
            },
        );
        let zone = view.zone;
        let step = |delta: i32| {
            cx.listener(move |this: &mut Self, _, _, cx| {
                this.nudge_sample_root(track, index, delta);
                cx.notify();
            })
        };
        div()
            .id(("pw-sample-zone", index))
            .debug_selector(move || format!("pw-sample-zone-{index}"))
            .h(ROW_HEIGHT)
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(if view.loaded {
                        theme.text
                    } else {
                        theme.text_muted
                    })
                    .child(name),
            )
            .child(
                chain_button(
                    ("pw-sample-root-down", index),
                    Icon::ChevronDown,
                    theme,
                    step(-1),
                )
                .tooltip(self.tip(Key::SamplerRootDown, "")),
            )
            .child(
                div()
                    .w(px(32.0))
                    .text_xs()
                    .text_center()
                    .text_color(theme.text)
                    .child(midi_name(i32::from(zone.root))),
            )
            .child(
                chain_button(
                    ("pw-sample-root-up", index),
                    Icon::ChevronUp,
                    theme,
                    step(1),
                )
                .tooltip(self.tip(Key::SamplerRootUp, "")),
            )
            .child(
                div()
                    .id(("pw-sample-plays", index))
                    .w(px(64.0))
                    .text_xs()
                    .text_center()
                    .text_color(theme.text_muted)
                    .child(plays)
                    .tooltip(self.tip(Key::SamplerPlays, "")),
            )
            .child(
                button(
                    ("pw-sample-velocity", index),
                    velocity_label(zone.velocity),
                    ButtonStyle::Ghost,
                    !zone.velocity.is_full(),
                    theme.accent_soft,
                    theme,
                    Self::opens_menu(cx, move |this, at| {
                        this.sample_velocity_menu(at, track, index)
                    }),
                )
                .tooltip(self.tip(Key::SamplerVelocity, "")),
            )
            .child(
                chain_button(
                    ("pw-sample-remove", index),
                    Icon::Cross,
                    theme,
                    cx.listener(move |this, _, _, cx| {
                        this.remove_sample_zone(track, index);
                        cx.notify();
                    }),
                )
                .tooltip(self.tip(Key::SamplerRemoveSample, "")),
            )
            .into_any_element()
    }

    /// The velocity layers one recording can answer, with a tick on the one it answers now.
    pub(crate) fn sample_velocity_menu(
        &self,
        anchor: Point<Pixels>,
        track: TrackId,
        zone: usize,
    ) -> ContextMenu {
        let current = self
            .session
            .track_sample_zones(track)
            .get(zone)
            .map(|zone| zone.velocity);
        let mut menu = ContextMenu::new(anchor, self.t(Key::SamplerVelocity));
        for (key, low, high) in VELOCITY_LAYERS {
            let range = VelocityRange::new(low, high);
            menu = menu.toggle(
                format!("{} ({})", self.t(key), velocity_label(range)),
                MenuCommand::SetSampleVelocity {
                    track,
                    zone,
                    low,
                    high,
                },
                current.is_some_and(|current| {
                    (current.low - low).abs() < 1e-4 && (current.high - high).abs() < 1e-4
                }),
            );
        }
        menu
    }

    /// Moves one recording's root by `delta` semitones.
    pub(crate) fn nudge_sample_root(&mut self, track: TrackId, zone: usize, delta: i32) {
        let Some(mut next) = self.session.track_sample_zones(track).get(zone).copied() else {
            return;
        };
        next.root = nudged_root(next.root, delta);
        if let Err(error) = self.session.set_sample_zone(track, zone, next) {
            self.set_failed_status(self.failure(Key::CmdEditSampleMap, &error));
        }
    }

    /// Puts one recording in a velocity layer.
    pub(crate) fn set_sample_velocity(&mut self, track: TrackId, zone: usize, low: f32, high: f32) {
        let Some(mut next) = self.session.track_sample_zones(track).get(zone).copied() else {
            return;
        };
        next.velocity = VelocityRange::new(low, high);
        if let Err(error) = self.session.set_sample_zone(track, zone, next) {
            self.set_failed_status(self.failure(Key::CmdEditSampleMap, &error));
        }
    }

    /// Takes one recording out of the map.
    pub(crate) fn remove_sample_zone(&mut self, track: TrackId, zone: usize) {
        if let Err(error) = self.session.remove_sample_zone(track, zone) {
            self.set_failed_status(self.failure(Key::CmdEditSampleMap, &error));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn velocity_reads_in_midi_steps() {
        assert_eq!(velocity_label(VelocityRange::FULL), "0–127");
        assert_eq!(velocity_label(VelocityRange::new(0.5, 1.0)), "64–127");
    }

    #[test]
    fn a_root_stays_on_the_keyboard() {
        assert_eq!(nudged_root(60, 1), 61);
        assert_eq!(nudged_root(0, -1), 0);
        assert_eq!(nudged_root(127, 1), 127);
    }
}
