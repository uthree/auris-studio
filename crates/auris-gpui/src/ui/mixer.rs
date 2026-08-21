//! The mixer: one channel strip per track, plus the master bus.

use auris_i18n::Key;
use auris_session::prelude::*;

use gpui::{
    AnyElement, Axis, IntoElement, MouseButton, MouseDownEvent, Window, div, prelude::*, px,
};

use crate::app::AurisApp;
use crate::theme::Metrics;
use crate::ui::icons::Icon;
use crate::ui::inspector::insert_element_key;
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
                    .child(self.t(Key::Mixer)),
            )
            .child(
                div()
                    .id("mixer-strips")
                    .flex()
                    .flex_1()
                    .min_h_0()
                    // And the same for the row itself, which is the one that actually scrolls:
                    // `w_full` holds it to the panel and `min_w_0` lets it be held there. Without
                    // the pair `overflow_x_scroll` is a promise about a row that never overflows.
                    .w_full()
                    .min_w_0()
                    .gap_1()
                    .p_1()
                    .overflow_x_scroll()
                    .track_scroll(&self.mixer_scroll)
                    .children(strips),
            )
            .child(self.render_mixer_scrollbar(cx))
            .on_mouse_down(
                gpui::MouseButton::Right,
                // Right-clicking past the last strip is still a request to add a track.
                Self::opens_menu(cx, |this, at| this.arrangement_menu(at)),
            )
    }

    /// The bar under the strips, and the only visible way to reach a track that is off the end.
    ///
    /// The wheel already scrolled the row — gpui turns a vertical wheel sideways for a container
    /// that only scrolls in x — but nothing on the screen said so, and a mixer whose fifteenth
    /// track can only be found by guessing is a mixer with fourteen tracks.
    fn render_mixer_scrollbar(&self, cx: &mut gpui::Context<Self>) -> impl IntoElement + use<> {
        let offset = self.mixer_scroll.offset().x;
        let max_offset = self.mixer_scroll.max_offset().width;
        let viewport = self.mixer_scroll.bounds().size.width;
        let recorded = self.canvas.mixer_scrollbar.clone();
        div()
            .relative()
            .child(
                crate::ui::widgets::horizontal_scrollbar(
                    "mixer-scrollbar",
                    f32::from(offset),
                    f32::from(max_offset),
                    f32::from(viewport),
                    &self.theme,
                    cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                        this.press_mixer_scrollbar(event, cx);
                    }),
                )
                .into_any_element(),
            )
            .child({
                // The press is measured against the bar that was drawn, not against the row it
                // scrolls: the row carries its own padding, and a fraction taken from the wrong
                // rectangle puts the thumb a few pixels from the pointer at one end of the track
                // and nowhere near it at the other.
                gpui::canvas(
                    move |bounds, _, _| recorded.set(Some(bounds)),
                    |_, _, _, _| (),
                )
                .absolute()
                .size_full()
            })
    }

    /// Takes hold of the scrollbar, jumping to the pointer first when it landed off the thumb.
    fn press_mixer_scrollbar(
        &mut self,
        event: &gpui::MouseDownEvent,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(bar) = self.canvas.mixer_scrollbar.get() else {
            return;
        };
        let max_offset = f32::from(self.mixer_scroll.max_offset().width);
        let viewport = f32::from(self.mixer_scroll.bounds().size.width);
        let width = f32::from(bar.size.width).max(1.0);
        let fraction = f32::from(event.position.x - bar.origin.x) / width;
        let offset = crate::ui::widgets::scrollbar_pressed(
            fraction,
            f32::from(self.mixer_scroll.offset().x),
            max_offset,
            viewport,
        );
        self.mixer_scroll
            .set_offset(gpui::point(px(offset), self.mixer_scroll.offset().y));
        self.drag = Some(crate::app::Drag::MixerScroll {
            start_x: event.position.x,
            start_offset: px(offset),
        });
        cx.notify();
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
        let output = track.output;
        let sends: Vec<(SendId, TrackId, f32, bool)> = track
            .sends
            .iter()
            .map(|send| (send.id, send.target, send.level_db, send.pre_fader))
            .collect();
        let destination = match output {
            Output::Master => self.t(Key::Master).to_string(),
            Output::Bus(bus) => self
                .project()
                .track(bus)
                .map(|bus| bus.name.clone())
                .unwrap_or_else(|| self.t(Key::Master).to_string()),
        };

        let effect_rows: Vec<AnyElement> = effects
            .into_iter()
            .map(|(slot_id, effect_id, enabled)| {
                let label = self.effect_label(slot_id, &effect_id);
                self.effect_row(
                    ("mixer-fx", insert_element_key(Some(slot_id))),
                    label,
                    Some(track_id),
                    slot_id,
                    enabled,
                    cx,
                )
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
                            .tooltip(self.tip(Key::MenuRename, ""))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    if event.click_count < 2 {
                                        return;
                                    }
                                    this.prompt_to_rename_track(track_id);
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
            .child(self.output_row(index, track_id, destination, cx))
            .children(sends.into_iter().enumerate().map(
                |(position, (send, bus, level_db, pre_fader))| {
                    self.send_row(
                        index, position, track_id, send, bus, level_db, pre_fader, cx,
                    )
                },
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
            .child(button(
                ("mixer-add-send", index),
                "+",
                ButtonStyle::Ghost,
                false,
                theme.accent_soft,
                &theme,
                Self::opens_menu(cx, move |this, at| this.send_picker_menu(at, track_id)),
            ))
            .into_any_element()
    }

    /// One send: where it goes, and how much of the track goes there.
    ///
    /// The level is a [`ParamTarget`] like a fader, so it drags, takes the wheel, resets on a
    /// double click and can be automated — all of that comes from being a mixer control rather
    /// than a number with a slider drawn next to it.
    #[allow(clippy::too_many_arguments)]
    fn send_row(
        &self,
        index: usize,
        position: usize,
        track_id: TrackId,
        send: SendId,
        bus: TrackId,
        level_db: f32,
        pre_fader: bool,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
        let name = self
            .project()
            .track(bus)
            .map(|bus| bus.name.clone())
            .unwrap_or_default();
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
                    // The tap point, in the one character there is room for. A send taken before
                    // the fader behaves differently enough from one taken after it that a strip
                    // which did not say which is which would be lying by omission.
                    .when(pre_fader, |row| {
                        row.child(
                            div()
                                .text_xs()
                                .text_color(theme.accent)
                                .child(self.t(Key::SendPreFaderMark)),
                        )
                    }),
            )
            .child(self.fader(
                ("mixer-send", index * 64 + position),
                self.t(Key::Sends),
                ParamTarget::Send {
                    track: track_id,
                    send,
                },
                level_db,
                cx,
            ))
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    let menu = this.send_menu(event.position, track_id, send);
                    this.open_menu(menu);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// The button for one effect in a strip: click to bypass, right-click for the full menu.
    ///
    /// Wrapped in a plain `div` because [`button`] only carries a click handler, and a strip
    /// this narrow has no room for reorder arrows next to every effect.
    fn effect_row(
        &self,
        id: impl Into<gpui::ElementId> + Clone,
        label: String,
        track: Option<TrackId>,
        slot: EffectSlotId,
        enabled: bool,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let theme = &self.theme;
        let menu_label = label.clone();
        div()
            .child(button(
                id,
                label,
                ButtonStyle::Ghost,
                enabled,
                theme.accent_soft,
                theme,
                // Opens the plugin's editor, the same as the inspector's own slots. Bypass used
                // to be on this click, which meant the only way to *see* an effect from the
                // mixer was to switch it off on the way.
                cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                    this.open_plugin_window(
                        crate::ui::plugin_window::PluginSubject::Insert { track, slot },
                        event.position(),
                    );
                    cx.notify();
                }),
            ))
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
            .map(|(slot_id, effect_id, enabled)| {
                let label = self.effect_label(slot_id, &effect_id);
                self.effect_row(
                    ("master-fx", insert_element_key(Some(slot_id))),
                    label,
                    None,
                    slot_id,
                    enabled,
                    cx,
                )
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
            .child(icon_label(
                "master-add-fx",
                Icon::Plus,
                self.t(Key::Effect),
                &theme,
                // Aimed at the master bus by name. This used to clear `selected_track` so that
                // whatever was picked next would land there, which silently deselected whatever
                // the user was working on.
                Self::opens_menu(cx, |this, at| this.effect_picker_menu(at, None)),
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
