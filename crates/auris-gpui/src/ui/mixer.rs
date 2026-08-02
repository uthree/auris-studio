//! The mixer: one channel strip per track, plus the master bus.

use auris_session::prelude::*;

use gpui::{AnyElement, Axis, IntoElement, Window, div, prelude::*, px};

use crate::app::{AurisApp, InspectorTab};
use crate::theme::Metrics;
use crate::ui::icons::Icon;
use crate::ui::widgets::{ButtonStyle, button, db_to_meter_position, icon_label, level_meter};

/// Width of one channel strip.
const STRIP_WIDTH: f32 = 128.0;

impl AurisApp {
    /// Renders the mixer panel.
    pub(crate) fn render_mixer(
        &mut self,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let track_ids: Vec<TrackId> = self.project().tracks.iter().map(|track| track.id).collect();

        let mut strips: Vec<AnyElement> = Vec::new();
        for (index, track_id) in track_ids.into_iter().enumerate() {
            strips.push(self.render_strip(index, track_id, cx));
        }
        strips.push(self.render_master_strip(cx));

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(80.0))
            .bg(theme.surface_sunken)
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(crate::theme::Metrics::EDITOR_HEADER_HEIGHT)
                    .px_2()
                    .bg(theme.surface_raised)
                    .border_b_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child("Mixer"),
            )
            .child(
                div()
                    .id("mixer-strips")
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .gap_1()
                    .p_1()
                    .overflow_x_scroll()
                    .children(strips),
            )
    }

    fn render_strip(
        &mut self,
        index: usize,
        track_id: TrackId,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
        let Some(track) = self.project().track(track_id) else {
            return div().into_any_element();
        };
        let name = track.name.clone();
        let color = theme.track_color(track.color.0);
        let gain_db = track.mixer.gain_db;
        let pan = track.mixer.pan;
        let muted = track.mixer.mute;
        let soloed = track.mixer.solo;
        let effects: Vec<(EffectSlotId, String, bool)> = track
            .mixer
            .effects
            .iter()
            .map(|slot| (slot.id, slot.effect_id.clone(), slot.enabled))
            .collect();
        let level_db = gain_to_db(self.track_level(index));
        let selected = self.selected_track == Some(track_id);

        let effect_rows: Vec<AnyElement> = effects
            .into_iter()
            .enumerate()
            .map(|(slot_index, (slot_id, effect_id, enabled))| {
                let label = self
                    .registry()
                    .descriptor(&effect_id)
                    .map(|d| d.name.to_string())
                    .unwrap_or(effect_id);
                button(
                    ("mixer-fx", index * 64 + slot_index),
                    label,
                    ButtonStyle::Ghost,
                    enabled,
                    theme.accent_soft,
                    &theme,
                    cx.listener(move |this, _, _, cx| {
                        this.toggle_effect(Some(track_id), slot_id);
                        cx.notify();
                    }),
                )
                .into_any_element()
            })
            .collect();

        div()
            .id(("strip", index))
            .flex()
            .flex_col()
            .gap_1()
            .w(px(STRIP_WIDTH))
            .flex_shrink_0()
            .p_1p5()
            .rounded(Metrics::RADIUS_MD)
            .bg(theme.surface)
            .border_1()
            .border_color(if selected {
                theme.accent
            } else {
                theme.border_subtle
            })
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.select_track(track_id);
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().w(px(4.0)).h(px(12.0)).bg(color))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(theme.text)
                            .truncate()
                            .child(name),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(div().flex_1().child(button(
                        ("mixer-mute", index),
                        "Mute",
                        ButtonStyle::Normal,
                        muted,
                        theme.mute,
                        &theme,
                        cx.listener(move |this, _, _, cx| {
                            this.toggle_mute(track_id);
                            cx.notify();
                        }),
                    )))
                    .child(div().flex_1().child(button(
                        ("mixer-solo", index),
                        "Solo",
                        ButtonStyle::Normal,
                        soloed,
                        theme.solo,
                        &theme,
                        cx.listener(move |this, _, _, cx| {
                            this.toggle_solo(track_id);
                            cx.notify();
                        }),
                    ))),
            )
            .child(self.fader(
                ("mixer-gain", index),
                "Vol",
                ParamTarget::TrackGain(track_id),
                gain_db,
                cx,
            ))
            .child(self.fader(
                ("mixer-pan", index),
                "Pan",
                ParamTarget::TrackPan(track_id),
                pan,
                cx,
            ))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(48.0))
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .children(effect_rows),
                    )
                    .child(div().w(px(10.0)).h_full().child(level_meter(
                        db_to_meter_position(level_db),
                        db_to_meter_position(level_db),
                        Axis::Vertical,
                        theme.meter_color(level_db),
                        &theme,
                    ))),
            )
            .into_any_element()
    }

    fn render_master_strip(&mut self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let gain_db = self.project().master.gain_db;
        let pan = self.project().master.pan;
        let level_db = gain_to_db(self.master_level());
        let effects: Vec<(EffectSlotId, String, bool)> = self
            .project()
            .master
            .effects
            .iter()
            .map(|slot| (slot.id, slot.effect_id.clone(), slot.enabled))
            .collect();

        let effect_rows: Vec<AnyElement> = effects
            .into_iter()
            .enumerate()
            .map(|(slot_index, (slot_id, effect_id, enabled))| {
                let label = self
                    .registry()
                    .descriptor(&effect_id)
                    .map(|d| d.name.to_string())
                    .unwrap_or(effect_id);
                button(
                    ("master-fx", slot_index),
                    label,
                    ButtonStyle::Ghost,
                    enabled,
                    theme.accent_soft,
                    &theme,
                    cx.listener(move |this, _, _, cx| {
                        this.toggle_effect(None, slot_id);
                        cx.notify();
                    }),
                )
                .into_any_element()
            })
            .collect();

        div()
            .id("strip-master")
            .flex()
            .flex_col()
            .gap_1()
            .w(px(STRIP_WIDTH))
            .flex_shrink_0()
            .p_1p5()
            .rounded(Metrics::RADIUS_MD)
            .bg(theme.surface_raised)
            .border_1()
            .border_color(theme.border)
            .child(div().text_xs().text_color(theme.text).child("Master"))
            .child(self.fader("master-gain", "Vol", ParamTarget::MasterGain, gain_db, cx))
            .child(self.fader("master-pan", "Pan", ParamTarget::MasterPan, pan, cx))
            .child(icon_label(
                "master-add-fx",
                Icon::Plus,
                "Effect",
                &theme,
                cx.listener(|this, _, _, cx| {
                    // The browser adds to the selected track, so clear the selection first to
                    // make the next pick land on the master bus.
                    this.selected_track = None;
                    this.inspector = InspectorTab::Browser;
                    cx.notify();
                }),
            ))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(48.0))
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .children(effect_rows),
                    )
                    .child(div().w(px(10.0)).h_full().child(level_meter(
                        db_to_meter_position(level_db),
                        db_to_meter_position(level_db),
                        Axis::Vertical,
                        theme.meter_color(level_db),
                        &theme,
                    ))),
            )
            .into_any_element()
    }
}
