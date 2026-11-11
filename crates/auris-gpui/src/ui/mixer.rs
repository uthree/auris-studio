//! The mixer: one channel strip per track, plus the master bus.

use auris_i18n::Key;
use auris_session::prelude::*;

use gpui::{
    AnyElement, Axis, IntoElement, MouseButton, MouseDownEvent, Window, div, prelude::*, px,
};

use crate::app::AurisApp;
use crate::theme::Metrics;
use crate::ui::icons::Icon;
use crate::ui::inspector::{Insert, insert_element_key, insert_rows};
use crate::ui::scrollbars::ScrollPanel;
use crate::ui::widgets::{ButtonStyle, button, channel_meter, icon_label};

/// Width of one channel strip.
const STRIP_WIDTH: f32 = 128.0;

impl AurisApp {
    /// Renders the mixer panel.
    pub(crate) fn render_mixer(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        if self.mixer_graph_visible {
            self.render_mixer_graph(window, cx)
        } else {
            self.render_mixer_console(window, cx).into_any_element()
        }
    }

    fn render_mixer_console(
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
            // A flex item's `min-width` is `auto`, which is its *content's* width — so a panel
            // holding fifteen channel strips claimed the width of fifteen channel strips and the
            // dock handed it over. Nothing overflowed, because nothing was too small; the strips
            // simply ran off the side of the window where no scroll could reach them.
            .min_w_0()
            .bg(theme.surface_sunken)
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(crate::theme::Metrics::PANEL_HEADER_HEIGHT)
                    .px_2()
                    .bg(theme.surface_raised)
                    .border_b_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::Mixer))
                    .child(button(
                        "mixer-show-graph",
                        self.t(Key::MixerGraph),
                        ButtonStyle::Ghost,
                        false,
                        theme.accent_soft,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.mixer_graph_visible = true;
                            cx.notify();
                        }),
                    )),
            )
            .child(
                self.scrolling(
                    ScrollPanel::Mixer,
                    div()
                        .id("mixer-strips")
                        .flex()
                        // And the same for the row itself, which is the one that actually scrolls:
                        // `w_full` holds it to the panel and `min_w_0` lets it be held there. Without
                        // the pair `overflow_x_scroll` is a promise about a row that never overflows.
                        .w_full()
                        .gap_1()
                        .p_1()
                        .overflow_x_scroll()
                        .children(strips),
                    cx,
                ),
            )
            .on_mouse_down(
                gpui::MouseButton::Right,
                // Right-clicking past the last strip is still a request to add a track.
                Self::opens_menu(cx, |this, at| this.arrangement_menu(at)),
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
        let instrument_id = track
            .kind
            .as_instrument()
            .map(|instrument| instrument.instrument_id.clone());
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
        let meter = self.track_meter(index);
        let clipped = self.session.meters().track_clipped(index);
        let selected = self.selected_track == Some(track_id);
        let outputs = track.outputs.clone();
        let level_rows = if output_levels_shown(&outputs) {
            outputs.clone()
        } else {
            Vec::new()
        };
        let mut destinations: Vec<String> = Vec::new();
        for output in &outputs {
            let name = self.destination_name(output.to);
            if !destinations.contains(&name) {
                destinations.push(name);
            }
        }
        let destination = if destinations.is_empty() {
            self.t(Key::MixerNoOutput).to_string()
        } else {
            destinations.join(", ")
        };

        let effect_rows: Vec<AnyElement> = insert_rows(&effects)
            .into_iter()
            .map(|row| match row {
                Insert::Filled {
                    slot,
                    effect_id,
                    enabled,
                } => {
                    let label = self.effect_label(slot, &effect_id);
                    self.effect_row(
                        ("mixer-fx", insert_element_key(Some(slot))),
                        label,
                        Some(track_id),
                        slot,
                        enabled,
                        cx,
                    )
                }
                Insert::Empty => self.add_effect_row(("mixer-add-fx", index), Some(track_id), cx),
            })
            .collect();

        let instrument_controls = instrument_id.map(|id| {
            let name = self.instrument_label(track_id, &id);
            self.compact_plugin_controls(
                ("mixer-inst", index),
                crate::ui::plugin_window::PluginSubject::Instrument(track_id),
                name,
                cx,
            )
        });

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
            .on_mouse_down(
                gpui::MouseButton::Right,
                Self::opens_menu(cx, move |this, at| {
                    this.select_track(track_id);
                    this.track_menu(at, track_id)
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().w(px(4.0)).h(px(12.0)).bg(color))
                    // Double-click to rename, as on the header. The strip and the header are the
                    // same track seen from two panels, and a gesture that works in one of them
                    // and not the other is worse than one that works in neither.
                    .child(
                        div()
                            .id(("strip-name", index))
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(theme.text)
                            .truncate()
                            .child(name)
                            .child(
                                self.canvas
                                    .spots
                                    .spot(crate::ui::inline_edit::strip_name_spot(track_id)),
                            )
                            .tooltip(self.tip(Key::MenuRename, ""))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    if event.click_count < 2 {
                                        return;
                                    }
                                    this.prompt_to_rename_track_at(
                                        track_id,
                                        crate::ui::inline_edit::strip_name_spot(track_id),
                                    );
                                    cx.stop_propagation();
                                    cx.notify();
                                }),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    // The initials, as the track header already uses. A strip is sized for
                    // "Mute", and 「ミュート」 does not fit in it: every strip in the mixer read as
                    // broken in the language this is mostly developed in. The word the strip has
                    // no room for is on the tooltip, which is where it can be as long as it needs.
                    .child(
                        div().flex_1().child(
                            button(
                                ("mixer-mute", index),
                                self.t(Key::MuteInitial),
                                ButtonStyle::Normal,
                                muted,
                                theme.mute,
                                &theme,
                                cx.listener(move |this, _, _, cx| {
                                    this.toggle_mute(track_id);
                                    cx.notify();
                                }),
                            )
                            .tooltip(self.tip(Key::CmdToggleTrackMute, "track.mute")),
                        ),
                    )
                    .child(
                        div().flex_1().child(
                            button(
                                ("mixer-solo", index),
                                self.t(Key::SoloInitial),
                                ButtonStyle::Normal,
                                soloed,
                                theme.solo,
                                &theme,
                                cx.listener(move |this, _, _, cx| {
                                    this.toggle_solo(track_id);
                                    cx.notify();
                                }),
                            )
                            .tooltip(self.tip(Key::CmdToggleTrackSolo, "track.solo")),
                        ),
                    ),
            )
            .child(self.fader(
                ("mixer-gain", index),
                self.t(Key::Volume),
                ParamTarget::TrackGain(track_id),
                gain_db,
                cx,
            ))
            .child(self.fader(
                ("mixer-pan", index),
                self.t(Key::Pan),
                ParamTarget::TrackPan(track_id),
                pan,
                cx,
            ))
            .children(instrument_controls)
            .child(self.output_row(index, track_id, destination, cx))
            .children(
                level_rows
                    .into_iter()
                    .enumerate()
                    .map(|(position, output)| {
                        self.output_level_row(index, position, track_id, output, 1.0, cx)
                    }),
            )
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
                    .child(div().w(px(10.0)).h_full().child(channel_meter(
                        meter,
                        clipped,
                        Axis::Vertical,
                        px(10.0),
                    ))),
            )
            .into_any_element()
    }

    /// Where the strip's output goes, and the way to point it somewhere else.
    ///
    /// Shown on every strip rather than only on routed ones. A control that appears once a track
    /// has been routed is a control nobody can find in order to route one — and where a track goes
    /// is worth reading at a glance even when the answer is the obvious one.
    fn output_row(
        &self,
        index: usize,
        track_id: TrackId,
        destination: String,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::OutputShort)),
            )
            .child(div().flex_1().min_w_0().child(button(
                ("mixer-out", index),
                destination,
                ButtonStyle::Ghost,
                false,
                theme.accent_soft,
                &theme,
                Self::opens_menu(cx, move |this, at| this.output_menu(at, track_id)),
            )))
            .into_any_element()
    }

    /// What a destination is called in a strip: the master, or the bus's own name.
    pub(super) fn destination_name(&self, to: Destination) -> String {
        match to {
            Destination::Master => self.t(Key::Master).to_string(),
            Destination::Bus(bus) => self
                .project()
                .track(bus)
                .map(|bus| bus.name.clone())
                .unwrap_or_else(|| self.t(Key::Master).to_string()),
        }
    }

    /// One output: where it goes, and how much of the track goes there.
    ///
    /// The level is a [`ParamTarget`] like a fader, so it drags, takes the wheel, resets on a
    /// double click and can be automated — all of that comes from being a mixer control rather
    /// than a number with a slider drawn next to it.
    pub(super) fn output_level_row(
        &self,
        index: usize,
        position: usize,
        track_id: TrackId,
        output: Output,
        scale: f32,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
        let name = self.destination_name(output.to);
        let Output { to, gain_db, tap } = output;
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .truncate()
                            .child(name),
                    )
                    // The tap point, in the one character there is room for. An output taken
                    // before the fader behaves differently enough from one taken after it that a
                    // strip which did not say which is which would be lying by omission.
                    .when(tap == Tap::PreFader, |row| {
                        row.child(
                            div()
                                .text_xs()
                                .text_color(theme.accent_text)
                                .child(self.t(Key::OutputPreFaderMark)),
                        )
                    }),
            )
            .child(
                self.fader_control(
                    ("mixer-output", index * 64 + position),
                    self.t(Key::OutputLevel),
                    ParamTarget::Output {
                        track: track_id,
                        to,
                        tap,
                    },
                    gain_db,
                    cx,
                )
                .h(Metrics::CONTROL_HEIGHT * scale),
            )
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    let menu = this.output_level_menu(event.position, track_id, to, tap);
                    this.open_menu(menu);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// One effect's editor action, with secondary commands in its visible menu.
    fn effect_row(
        &mut self,
        id: impl Into<gpui::ElementId> + Clone,
        label: String,
        track: Option<TrackId>,
        slot: EffectSlotId,
        enabled: bool,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
        let menu_label = label.clone();
        let subject = crate::ui::plugin_window::PluginSubject::Insert { track, slot };
        div()
            .flex()
            .flex_col()
            .min_w_0()
            .gap_0p5()
            .when(self.dragging_effect(slot), |this| {
                this.bg(theme.surface_raised).opacity(0.8)
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    if this.drag.is_none() {
                        this.begin_drag(crate::app::Drag::EffectReorder { track, slot });
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(cx.listener(move |this, _: &gpui::MouseMoveEvent, _, cx| {
                this.drag_effect_onto(track, Some(slot), cx);
            }))
            .child(self.compact_plugin_controls(id, subject, label, cx))
            .when(!enabled, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(self.t(Key::PluginBypassed)),
                )
            })
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    let menu = this.effect_menu(event.position, track, slot, menu_label.clone());
                    this.open_menu(menu);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// The empty slot at the end of a chain, which is how another effect is added.
    ///
    /// Logic's shape, and the inspector's: the slots that are filled, then one empty one. The
    /// mixer had neither half of that. A track strip offered no way at all to add an effect —
    /// the answer was a right-click on empty space, which is not an answer anybody finds — and
    /// the master strip had a button above its chain instead of a slot at the end of it, so the
    /// two strips beside each other disagreed about where the next effect goes.
    ///
    /// Aimed at its strip by name. It used to clear `selected_track` so that whatever was picked
    /// next would land there, which silently deselected whatever the user was working on.
    fn add_effect_row(
        &self,
        id: impl Into<gpui::ElementId>,
        track: Option<TrackId>,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        div()
            // An effect dragged onto this row means the end of the chain, the same as in the
            // inspector — it is the only way to say "last" with the pointer.
            .on_mouse_move(cx.listener(move |this, _: &gpui::MouseMoveEvent, _, cx| {
                this.drag_effect_onto(track, None, cx);
            }))
            .child(icon_label(
                id,
                Icon::Plus,
                self.t(Key::Effect),
                &self.theme,
                Self::opens_menu(cx, move |this, at| this.effect_picker_menu(at, track)),
            ))
            .into_any_element()
    }

    fn render_master_strip(&mut self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let gain_db = self.project().master.gain_db;
        let pan = self.project().master.pan;
        let meter = self.master_meter();
        let clipped = self.session.meters().master_clipped();
        let effects: Vec<(EffectSlotId, String, bool)> = self
            .project()
            .master
            .effects
            .iter()
            .map(|slot| (slot.id, slot.effect_id.clone(), slot.enabled))
            .collect();

        let effect_rows: Vec<AnyElement> = insert_rows(&effects)
            .into_iter()
            .map(|row| match row {
                Insert::Filled {
                    slot,
                    effect_id,
                    enabled,
                } => {
                    let label = self.effect_label(slot, &effect_id);
                    self.effect_row(
                        ("master-fx", insert_element_key(Some(slot))),
                        label,
                        None,
                        slot,
                        enabled,
                        cx,
                    )
                }
                Insert::Empty => self.add_effect_row("master-add-fx", None, cx),
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
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text)
                    .child(self.t(Key::Master)),
            )
            .child(self.fader(
                "master-gain",
                self.t(Key::Volume),
                ParamTarget::MasterGain,
                gain_db,
                cx,
            ))
            .child(self.fader(
                "master-pan",
                self.t(Key::Pan),
                ParamTarget::MasterPan,
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
                    .child(div().w(px(10.0)).h_full().child(channel_meter(
                        meter,
                        clipped,
                        Axis::Vertical,
                        px(10.0),
                    ))),
            )
            .into_any_element()
    }
}

/// Whether a strip shows a level row for each of its outputs.
///
/// Every track has at least one output, and nearly every track has exactly one: to the master or
/// a bus, at unity after the fader. A level row for that one would repeat the fader beside it on
/// every strip, so it is drawn only once there is something it could say — a second output, a
/// level that is not unity, or a copy taken before the fader.
pub(crate) fn output_levels_shown(outputs: &[Output]) -> bool {
    match outputs {
        [] => false,
        [only] => only.gain_db != 0.0 || only.tap == Tap::PreFader,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_lone_unity_output_gets_no_level_row_and_anything_else_does() {
        use super::output_levels_shown;
        use auris_session::prelude::{Destination, Output, Tap, TrackId};

        assert!(!output_levels_shown(&[]));
        assert!(!output_levels_shown(&[Output::master()]));
        assert!(!output_levels_shown(&[Output::to_bus(TrackId(3))]));
        let quieter = Output {
            gain_db: -6.0,
            ..Output::master()
        };
        assert!(output_levels_shown(&[quieter]));
        let pre = Output {
            to: Destination::Bus(TrackId(3)),
            gain_db: 0.0,
            tap: Tap::PreFader,
        };
        assert!(output_levels_shown(&[pre]));
        assert!(output_levels_shown(&[
            Output::master(),
            Output::to_bus(TrackId(3))
        ]));
    }
}
