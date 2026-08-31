//! The left column: one header per track, and the buttons that add another.
//!
//! Its own file because it is the one surface of the arrangement that is not painted. It is
//! ordinary gpui elements — the same faders, meters and buttons the mixer is built from — and so
//! it shares nothing with the canvases opposite except the row list, which it walks rather than
//! the track list so that the two columns cannot disagree about where a track is.

use auris_i18n::Key;
use auris_session::prelude::*;

use gpui::{Axis, IntoElement, MouseButton, MouseDownEvent, Pixels, div, prelude::*, px};

use crate::app::{AurisApp, Drag};
use crate::i18n::track_kind_key;
use crate::theme::Metrics;
use crate::ui::automation;
use crate::ui::icons::Icon;
use crate::ui::widgets::{
    ButtonStyle, Latch, button, db_to_meter_position, icon_label, level_meter,
};

/// How tall the strip along the bottom of a header that resizes its lane is.
///
/// Narrower than [`Metrics::SPLITTER`], which the dividers between panels use, because a splitter
/// is a gap *between* two panels and this is a strip taken out of one. It is the header's bottom
/// padding — `track_header` sets it from here — so nothing else is ever underneath it, and every
/// pixel spent here is a pixel the pan fader does not get.
const RESIZE_BAND: Pixels = px(4.0);

/// How a track's arm button is latched.
///
/// Three answers rather than two, because the button has two jobs: it says this track was armed
/// by hand, and it says a take would land here anyway. Those parted company when the selection
/// became a target of its own — an audio track that is merely selected has to look like somewhere
/// Record would go, without claiming to have been chosen.
///
/// Several tracks can be `On` at once, and only one can ever be `Ready`: arming is a list and a
/// selection standing in for one is not.
///
/// A free function because it is a rule, and a rule inside a view is a rule with no test.
fn arm_latch(armed: bool, target: bool) -> Latch {
    match (armed, target) {
        (true, _) => Latch::On,
        (false, true) => Latch::Ready,
        (false, false) => Latch::Off,
    }
}

impl AurisApp {
    /// The left column: one header per track, plus the add-track buttons.
    pub(super) fn render_track_headers(
        &mut self,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        let selected = self.selected_track;
        let has_solo = self.project().has_solo();
        let dragging = match self.drag {
            Some(Drag::TrackReorder { track, .. }) => Some(track),
            _ => None,
        };

        // Built from the lane rows rather than from the track list, so the two columns cannot
        // disagree about where a track is. They did: an open automation lane added a row on the
        // canvas side and nothing on this one, and every header below it sat a row too high.
        let rows = self.lane_rows();
        let mut index = 0usize;
        let headers: Vec<gpui::AnyElement> = rows
            .iter()
            .map(|row| match row.kind {
                automation::RowKind::Automation(target) => {
                    self.automation_gutter(row.height, target)
                }
                automation::RowKind::Clips => {
                    let header =
                        self.track_header(index, row.track, dragging, has_solo, selected, cx);
                    index += 1;
                    header
                }
            })
            .collect();

        div()
            .flex()
            .flex_col()
            .w(self.panels.header_width)
            .flex_shrink_0()
            .child(self.track_header_toolbar(cx))
            .child(
                div()
                    .id("track-headers")
                    .relative()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    // The wheel here moves the same column it moves over the clips. A user who
                    // has run out of tracks on screen reaches for the list, not the canvas.
                    .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                        let delta = event.delta.pixel_delta(px(24.0));
                        this.scroll_lanes_by(-delta.y);
                        cx.notify();
                    }))
                    .child(
                        // Pushed up by the shared offset rather than given its own scrollbar, so
                        // a header can never drift out of line with the lane it belongs to.
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(-self.lane_scroll)
                            .flex()
                            .flex_col()
                            .children(headers),
                    ),
            )
    }

    /// The band under a track's header that lines up with its open automation lane.
    ///
    /// It exists to keep the two columns in step, and it carries the parameter's name because an
    /// empty band the height of a lane reads as something that failed to draw.
    fn automation_gutter(&mut self, height: Pixels, target: ParamTarget) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let name = self
            .session
            .descriptor_for(target)
            .map(|descriptor| self.param_label(&descriptor.name))
            .unwrap_or_default();
        div()
            .flex()
            .items_start()
            .h(height)
            .px(px(10.0))
            .pt(px(3.0))
            .border_b_1()
            .border_color(theme.border_subtle)
            .bg(theme.surface_sunken)
            .text_xs()
            .text_color(theme.text_muted)
            .child(name)
            .into_any_element()
    }

    /// One track's header.
    fn track_header(
        &mut self,
        index: usize,
        track_id: TrackId,
        dragging: Option<TrackId>,
        has_solo: bool,
        selected: Option<TrackId>,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        // Asked before the track is borrowed: the answer is cached on `self` against the
        // document revision, and reading the cache needs the borrow the track would hold.
        let take_state = self.singer_take_badge(track_id);
        let Some(track) = self.project().track(track_id) else {
            return div().into_any_element();
        };
        {
            {
                let id = track.id;
                let color = theme.track_color(track.color.0);
                let height = track.height;
                let dimmed = has_solo && !track.mixer.solo;
                let gain_db = track.mixer.gain_db;
                let pan = track.mixer.pan;
                let muted = track.mixer.mute;
                let soloed = track.mixer.solo;
                let name = track.name.clone();
                let kind = self.t(track_kind_key(&track.kind));
                // The voice that sings this track, where one has been chosen — beside the kind
                // label, with the one fact about the take worth a glance: whether it is still
                // what the notes say. Behind is a badge and never a fallback; the take keeps
                // playing, and this is what says to sing it again.
                let voice = track
                    .kind
                    .as_singer()
                    .and_then(|singer| singer.voice.as_ref())
                    .map(|voice| voice.name.clone());
                let behind = take_state == auris_session::SingerTakeState::Behind;
                let behind_tip = self.tip(Key::TakeBehind, "");
                // The voice at work in the background. It outranks `behind` on the badge —
                // a take being re-sung is behind by definition, and "working on it" is the
                // truer word — and it is the on-screen sign the CPU is being spent, which
                // every costly thing switched on without a click owes the screen.
                let singing = self
                    .auto_sing
                    .as_ref()
                    .is_some_and(|auto| auto.track == track_id);
                let singing_tip = self.tip(Key::TakeRendering, "");
                let level_db = gain_to_db(self.track_level(index));
                // Latched by the engine and only put out by asking, so a transient that went
                // over is still saying so long after the meter beside it has fallen back.
                let clipped = self.session.meters().track_clipped(index);
                // Only an audio track gets an arm button, because only an audio track has
                // anywhere for a take to land. An instrument track showing a disabled one would
                // be an invitation to a thing that cannot happen.
                let records = track.kind.as_audio().is_some();
                let armed = arm_latch(
                    self.session.track_arm(id).is_some(),
                    self.session.is_record_target(id, selected),
                );
                // No `Ready` state to match the arm's: monitoring is never inferred from a
                // selection, because it is a thing that costs and those are switched on by hand.
                let monitored = self.session.is_monitored(id);
                // What is arriving on the channels this track is armed to, and only while it is
                // armed and something is listening. The transport bar's meter is the *device* —
                // one number for the whole interface — which cannot tell a room where one
                // microphone is loud and another is silent from one where both are.
                let input = self.input_level_for(id);

                let is_selected = selected == Some(id);
                let is_dragging = dragging == Some(id);

                div()
                    .id(("track-header", index))
                    .flex()
                    // For the resize band at the bottom, which is laid over the header rather
                    // than in it: the row list and the canvas opposite agree on every height to
                    // the pixel, and a strip taking part in the layout would push them apart.
                    .relative()
                    .h(px(height))
                    .pl(px(6.0))
                    .pt(px(3.0))
                    // The resize band's own strip, kept clear of the controls rather than laid
                    // over them. The pan fader runs to the bottom of the header, and a band
                    // merely drawn on top of it is a band fighting a fader for the same four
                    // pixels — which the fader wins, because a press lands on both and its
                    // handler is the one that runs last.
                    .pb(RESIZE_BAND)
                    .pr(px(4.0))
                    .gap(px(6.0))
                    .border_b_1()
                    .border_color(theme.border_subtle)
                    .bg(if is_selected {
                        theme.surface_raised
                    } else {
                        theme.surface
                    })
                    .when(dimmed, |this| this.opacity(0.55))
                    // A header in hand is lifted off the list, so the row that follows the pointer
                    // is the row the drop will land on rather than a guess about it.
                    .when(is_dragging, |this| {
                        this.bg(theme.surface_raised).opacity(0.8)
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.select_track(id);
                            // The header is the *fallback* grab: a press that landed on a fader or
                            // a button inside it has already claimed the gesture, and this runs
                            // afterwards because a parent's handler bubbles last.
                            if this.drag.is_none() {
                                this.begin_drag(Drag::TrackReorder {
                                    track: id,
                                    pressed_at: Some(event.position),
                                });
                            }
                            cx.notify();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        AurisApp::opens_menu(cx, move |this, at| {
                            // Selecting first means the menu and the panels agree about what is
                            // being acted on, the way a right-click does everywhere else.
                            this.select_track(id);
                            this.track_menu(at, id)
                        }),
                    )
                    // A colour stripe is the fastest way to match a header to its clips.
                    .child(
                        div()
                            .w(px(4.0))
                            .h_full()
                            .rounded(Metrics::RADIUS_XS)
                            .bg(color),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    // A track number, as every DAW shows — it is what people
                                    // actually say out loud when pointing at a track.
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .w(px(16.0))
                                            .text_xs()
                                            .text_color(theme.text_faint)
                                            .child(format!("{}", index + 1)),
                                    )
                                    // Double-click to rename, which is what every list of named
                                    // things does. A name is the one thing about a track that is
                                    // written rather than chosen, and reaching a menu for it is a
                                    // step nobody takes twice — which is how a project ends up
                                    // with eight tracks called Audio 1.
                                    //
                                    // Propagation is only stopped on the second click. The first
                                    // still selects the track and arms the reorder drag through
                                    // the header behind this, because that is what one click on
                                    // a name has always meant.
                                    .child(
                                        div()
                                            .id(("track-name", index))
                                            .flex_1()
                                            .min_w_0()
                                            .text_xs()
                                            .text_color(theme.text)
                                            .truncate()
                                            .child(name)
                                            .tooltip(self.tip(Key::MenuRename, ""))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(
                                                    move |this, event: &MouseDownEvent, _, cx| {
                                                        if event.click_count < 2 {
                                                            return;
                                                        }
                                                        this.prompt_to_rename_track(id);
                                                        cx.stop_propagation();
                                                        cx.notify();
                                                    },
                                                ),
                                            ),
                                    )
                                    .child(div().text_xs().text_color(theme.text_faint).child(kind))
                                    .when_some(voice, |this, voice| {
                                        this.child(
                                            div()
                                                .id(("voice-badge", index))
                                                .text_xs()
                                                .max_w(px(120.0))
                                                .truncate()
                                                .text_color(if singing {
                                                    theme.accent
                                                } else if behind {
                                                    theme.record
                                                } else {
                                                    theme.text_faint
                                                })
                                                // The mark goes *before* the name: the label
                                                // truncates from the right, and a warning that
                                                // only long names hide is no warning at all.
                                                .child(match (singing, behind) {
                                                    (true, _) => format!("… ♪ {voice}"),
                                                    (false, true) => format!("! ♪ {voice}"),
                                                    (false, false) => format!("♪ {voice}"),
                                                })
                                                .when(singing, |this| this.tooltip(singing_tip))
                                                .when(behind && !singing, |this| {
                                                    this.tooltip(behind_tip)
                                                }),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    // Each of these says what it is in one letter, which is what
                                    // every console does and is fine for the two everybody knows.
                                    // The other two are not: an unlabelled square that arms a
                                    // microphone and one that opens a monitor are worth naming
                                    // out loud, and once two of the four carry a card all four
                                    // should.
                                    .child(
                                        div().w(px(24.0)).child(
                                            button(
                                                ("mute", index),
                                                self.t(Key::MuteInitial),
                                                ButtonStyle::Normal,
                                                muted,
                                                theme.mute,
                                                &theme,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.toggle_mute(id);
                                                    cx.notify();
                                                }),
                                            )
                                            .tooltip(
                                                self.tip(Key::CmdToggleTrackMute, "track.mute"),
                                            ),
                                        ),
                                    )
                                    .child(
                                        div().w(px(24.0)).child(
                                            button(
                                                ("solo", index),
                                                self.t(Key::SoloInitial),
                                                ButtonStyle::Normal,
                                                soloed,
                                                theme.solo,
                                                &theme,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.toggle_solo(id);
                                                    cx.notify();
                                                }),
                                            )
                                            .tooltip(
                                                self.tip(Key::CmdToggleTrackSolo, "track.solo"),
                                            ),
                                        ),
                                    )
                                    .when(records, |this| {
                                        this.child(
                                            div().w(px(24.0)).child(
                                                button(
                                                    ("arm", index),
                                                    self.t(Key::RecordInitial),
                                                    ButtonStyle::Normal,
                                                    armed,
                                                    theme.record,
                                                    &theme,
                                                    cx.listener(move |this, _, _, cx| {
                                                        this.toggle_arm(id);
                                                        cx.notify();
                                                    }),
                                                )
                                                .tooltip(self.tip(Key::ArmTrack, "")),
                                            ),
                                        )
                                        // Beside the arm because they are the two halves of the
                                        // same device: one keeps what comes in, the other only
                                        // lets it be heard. In the accent rather than in red,
                                        // because listening is not recording and a row of two
                                        // red buttons would say it was.
                                        .child(
                                            div().w(px(24.0)).child(
                                                button(
                                                    ("monitor", index),
                                                    self.t(Key::MonitorInitial),
                                                    ButtonStyle::Normal,
                                                    monitored,
                                                    theme.accent,
                                                    &theme,
                                                    cx.listener(move |this, _, _, cx| {
                                                        this.toggle_monitoring(id);
                                                        cx.notify();
                                                    }),
                                                )
                                                .tooltip(self.tip(
                                                    Key::CmdToggleMonitoring,
                                                    "transport.monitor",
                                                )),
                                            ),
                                        )
                                    })
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .child(self.gain_control(id, gain_db, cx)),
                                    ),
                            )
                            .child(self.pan_control(id, pan, cx)),
                    )
                    // The input, left of what the track puts out, which is the order the signal
                    // travels in — and narrower, so the pair reads as two different things at a
                    // glance rather than as a stereo meter.
                    .children(input.map(|(level, input_clipped)| {
                        let db = gain_to_db(level);
                        div().w(px(4.0)).h_full().py(px(1.0)).child(level_meter(
                            db_to_meter_position(db),
                            db_to_meter_position(db),
                            input_clipped,
                            Axis::Vertical,
                            theme.meter_color(db),
                            &theme,
                        ))
                    }))
                    .child(div().w(px(7.0)).h_full().py(px(1.0)).child(level_meter(
                        db_to_meter_position(level_db),
                        db_to_meter_position(level_db),
                        clipped,
                        Axis::Vertical,
                        theme.meter_color(level_db),
                        &theme,
                    )))
                    // Last, so it paints over the fader it overlaps and so a press on it is the
                    // press that lands. The header's own handler runs afterwards — a parent's
                    // bubbles last — and finds the drag already claimed.
                    .child(self.lane_resize_band(index, id, height, cx))
                    .into_any_element()
            }
        }
    }

    /// The strip along the bottom of a header that drags its lane taller or shorter.
    ///
    /// Invisible until the pointer is on it, which is the same bargain the panel dividers strike:
    /// a line drawn under every header would be a second border under the one already there, and
    /// the cursor changing is what says the edge can be taken hold of.
    ///
    /// `occlude` is what makes it *the* thing a press in that strip lands on. The strip is the
    /// header's own padding, so the header's hitbox covers it too, and without this a press
    /// reached both: the band would begin a resize and the header would begin a reorder over the
    /// top of it. Blocking rather than relying on the header's `drag.is_none()` guard, because
    /// that guard depends on which listener gpui happens to run first, and a gesture that is
    /// correct by accident of dispatch order is one that comes back.
    ///
    /// Nothing selects the track on the way past, and that is deliberate: taking hold of an edge
    /// to resize it is not a request to change what the inspector is showing.
    fn lane_resize_band(
        &self,
        index: usize,
        track: TrackId,
        height: f32,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        let accent = self.theme.accent;
        div()
            .id(("track-height", index))
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .h(RESIZE_BAND)
            .occlude()
            .cursor(gpui::CursorStyle::ResizeUpDown)
            .hover(|this| this.bg(crate::theme::Theme::translucent(accent, 0.35)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.begin_drag(Drag::ResizeTrack {
                        track,
                        start_y: event.position.y,
                        start_height: height,
                    });
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
    }

    /// The row of buttons above the track headers.
    fn track_header_toolbar(&mut self, cx: &mut gpui::Context<Self>) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        div()
            .flex()
            .items_center()
            .gap_1()
            // Matches the ruler and whichever strips are showing opposite. See the method.
            .h(self.panels.lanes.header_height())
            .px_1()
            .bg(theme.surface_raised)
            .border_b_1()
            .border_color(theme.border)
            .child(div().flex_1().child(icon_label(
                "add-instrument",
                Icon::Plus,
                self.t(Key::AddInstrumentShort),
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.add_instrument_track();
                    cx.notify();
                }),
            )))
            .child(div().flex_1().child(icon_label(
                "add-audio",
                Icon::Plus,
                self.t(Key::AddAudioShort),
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.add_audio_track();
                    cx.notify();
                }),
            )))
            .child(div().flex_1().child(icon_label(
                "add-bus",
                Icon::Plus,
                self.t(Key::AddBusShort),
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.add_bus_track();
                    cx.notify();
                }),
            )))
    }

    fn gain_control(
        &self,
        track: TrackId,
        gain_db: f32,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        self.fader(
            ("gain", track.0 as usize),
            self.t(Key::Volume),
            ParamTarget::TrackGain(track),
            gain_db,
            cx,
        )
    }

    fn pan_control(
        &self,
        track: TrackId,
        pan: f32,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        self.fader(
            ("pan", track.0 as usize),
            self.t(Key::Pan),
            ParamTarget::TrackPan(track),
            pan,
            cx,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_session::session::MIN_TRACK_HEIGHT;

    #[test]
    fn the_resize_strip_leaves_a_header_worth_pressing() {
        // The strip is the header's bottom padding, so it comes out of the shortest lane there
        // can be. At a third of that the header would be more grab handle than header, and the
        // mute button under it would be the thing nobody could hit.
        assert!(
            f32::from(RESIZE_BAND) * 3.0 < MIN_TRACK_HEIGHT,
            "a {RESIZE_BAND:?} strip is most of a {MIN_TRACK_HEIGHT} pixel lane"
        );
        // And it is worth pressing itself: a strip thinner than a couple of pixels is a target
        // the pointer has to be aimed at rather than moved towards.
        assert!(RESIZE_BAND >= px(3.0));
    }

    #[test]
    fn the_arm_button_shows_where_a_take_would_land_as_well_as_what_was_armed() {
        // Nothing armed and this track selected: its button is the one that says Record would
        // come here, and it says it without claiming anybody pressed it.
        assert_eq!(arm_latch(false, true), Latch::Ready);
        // Armed by hand: filled, and filled whether or not the eye is on it — which is the case
        // the arm button exists for, and the case a user would otherwise have no way of seeing.
        assert_eq!(arm_latch(true, false), Latch::On);
        assert_eq!(arm_latch(true, true), Latch::On);
        // And nothing at all when there is nowhere for a take to go.
        assert_eq!(arm_latch(false, false), Latch::Off);
    }
}
