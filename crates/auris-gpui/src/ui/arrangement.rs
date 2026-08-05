//! The arrangement: track headers on the left, a clip timeline on the right.

use auris_i18n::Key;
use auris_session::prelude::*;

use gpui::{
    Axis, Bounds, Corners, IntoElement, MouseButton, MouseDownEvent, Pixels, Window, canvas, div,
    point, prelude::*, px, size,
};

use crate::app::{AurisApp, ClipEdge, Drag, FadeEdge};
use crate::i18n::track_kind_key;
use crate::theme::{Metrics, Theme};
use crate::ui::icons::Icon;
use crate::ui::paint;
use crate::ui::widgets::{
    ButtonStyle, button, db_to_meter_position, icon_label, level_meter, splitter,
};

/// Width of the grab zone on a clip's right edge, in pixels.
const RESIZE_HANDLE: f32 = 7.0;

/// How far a column of `content` can be scrolled inside a `viewport` before it runs out.
///
/// Never negative: content shorter than the viewport does not scroll at all, and a viewport
/// whose size has not been recorded yet reports zero rather than letting the column drift.
pub(crate) fn scroll_limit(content: Pixels, viewport: Pixels) -> Pixels {
    (content - viewport).max(px(0.0))
}

/// The scroll offset that brings a row into view, moving as little as possible.
///
/// A row already on screen returns the offset unchanged. One above the top scrolls up to it, one
/// below the bottom scrolls down until its foot is flush — and a row taller than the viewport
/// puts its *top* on screen, because that is the end with the name on it.
pub(crate) fn reveal_offset(
    scroll: Pixels,
    viewport: Pixels,
    row_top: Pixels,
    row_height: Pixels,
) -> Pixels {
    if row_top < scroll {
        return row_top;
    }
    let below = row_top + row_height - viewport;
    if below > scroll {
        return below.min(row_top);
    }
    scroll
}

/// How wide the grab zone on a clip's right edge is, for a clip drawn `width` across.
///
/// Never more than a third of the clip: zoomed far enough out, a fixed handle either side of the
/// end covers the whole block and the clip can be stretched but never moved.
fn resize_grab(width: Pixels) -> f32 {
    RESIZE_HANDLE.min(f32::from(width) / 3.0)
}

/// Height of a clip's name bar.
const TITLE_HEIGHT: Pixels = px(14.0);

/// Height of the band under the name bar where an audio clip's fade handles live.
///
/// The band splits the clip's edges by height: a press in it takes hold of a fade, one below
/// it takes the resize handle or the clip body, exactly as the painter draws the handles.
const FADE_BAND: Pixels = px(12.0);

/// Half-width of the grab zone around a fade handle, in pixels.
const FADE_GRAB: f32 = 6.0;

/// Clips narrower than this draw no fade handles and offer none to grab.
///
/// On a sliver of a clip the two handles and the resize zone would overlap into a lottery;
/// the fades themselves still play and still draw, only the handles go.
const FADE_HANDLE_MIN_WIDTH: f32 = 40.0;

impl AurisApp {
    /// Renders the arrangement panel.
    pub(crate) fn render_arrangement(
        &mut self,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let headers = self.render_track_headers(cx);
        let timeline = self.render_timeline(cx);
        div()
            .flex()
            .flex_1()
            .min_h(crate::dock::PanelLayout::MIN_ARRANGEMENT)
            .overflow_hidden()
            .bg(theme.surface)
            .child(headers)
            .child(splitter(
                "split-headers",
                Axis::Vertical,
                &theme,
                cx.listener(|this, event: &MouseDownEvent, _, _| {
                    let start_width = this.panels.header_width;
                    this.begin_drag(Drag::ResizeHeaders {
                        start_x: event.position.x,
                        start_width,
                    });
                }),
            ))
            .child(timeline)
    }

    /// The left column: one header per track, plus the add-track buttons.
    fn render_track_headers(&mut self, cx: &mut gpui::Context<Self>) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let selected = self.selected_track;
        let has_solo = self.project().has_solo();

        let headers: Vec<gpui::AnyElement> = self
            .project()
            .tracks
            .iter()
            .enumerate()
            .map(|(index, track)| {
                let id = track.id;
                let color = theme.track_color(track.color.0);
                let level_db = gain_to_db(self.track_level(index));
                let dimmed = has_solo && !track.mixer.solo;
                let gain_db = track.mixer.gain_db;
                let pan = track.mixer.pan;
                let muted = track.mixer.mute;
                let soloed = track.mixer.solo;
                let name = track.name.clone();
                let kind = self.t(track_kind_key(&track.kind));

                let is_selected = selected == Some(id);

                div()
                    .id(("track-header", index))
                    .flex()
                    .h(px(track.height))
                    .pl(px(6.0))
                    .py(px(3.0))
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
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.select_track(id);
                            cx.notify();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            // Selecting first means the menu and the panels agree about what is
                            // being acted on, the way a right-click does everywhere else.
                            this.select_track(id);
                            let menu = this.track_menu(event.position, id);
                            this.open_menu(menu);
                            cx.notify();
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
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_xs()
                                            .text_color(theme.text)
                                            .truncate()
                                            .child(name),
                                    )
                                    .child(
                                        div().text_xs().text_color(theme.text_faint).child(kind),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(div().w(px(24.0)).child(button(
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
                                    )))
                                    .child(div().w(px(24.0)).child(button(
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
                                    )))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .child(self.gain_control(id, gain_db, cx)),
                                    ),
                            )
                            .child(self.pan_control(id, pan, cx)),
                    )
                    .child(div().w(px(7.0)).h_full().py(px(1.0)).child(level_meter(
                        db_to_meter_position(level_db),
                        db_to_meter_position(level_db),
                        Axis::Vertical,
                        theme.meter_color(level_db),
                        &theme,
                    )))
                    .into_any_element()
            })
            .collect();

        div()
            .flex()
            .flex_col()
            .w(self.panels.header_width)
            .flex_shrink_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    // Matches the ruler *and* the harmony lane opposite. See the constant.
                    .h(Metrics::TIMELINE_HEADER_HEIGHT)
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
                    ))),
            )
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

    /// The strip of section names under the ruler: イントロ, Aメロ, サビ.
    ///
    /// Above the harmony because it is the coarser division — the stack reads ruler, structure,
    /// harmony, lanes: the song from its largest units down to its notes.
    fn render_structure_lane(&mut self, cx: &mut gpui::Context<Self>) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let view = self.timeline.clone();

        let width = self
            .canvas
            .structure
            .get()
            .map_or(px(1200.0), |b| b.size.width);
        let (from, to) = view.visible_range(width);
        let sections = &self.project().sections;
        let structure = paint::StructurePaint {
            spans: sections
                .spans_in(from, to)
                .into_iter()
                .map(|span| {
                    // Numbered only when the label repeats: サビ 1 and サビ 2 need telling
                    // apart, a lone イントロ does not.
                    let shown = if sections.repeats(&span.label) > 1 {
                        format!("{} {}", span.label, span.instance)
                    } else {
                        span.label.clone()
                    };
                    (span, shown)
                })
                .collect(),
            held: match self.drag {
                Some(Drag::SectionLabel { at, .. }) => Some(at),
                _ => None,
            },
        };

        div()
            .id("structure-lane")
            .h(Metrics::STRUCTURE_LANE_HEIGHT)
            .w_full()
            .cursor_pointer()
            .child({
                let recorded = self.canvas.structure.clone();
                canvas(
                    move |bounds, _, _| recorded.set(Some(bounds)),
                    move |bounds, _, window, cx| {
                        paint::clipped(window, bounds, |window| {
                            paint::structure_lane(window, cx, bounds, &view, &structure, &theme);
                        });
                    },
                )
                .size_full()
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.press_structure_lane(event);
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    let x = event.position.x - this.timeline_origin().x;
                    let tick = this.timeline.x_to_tick(x).max_zero();
                    let menu = this.structure_menu(event.position, tick);
                    this.open_menu(menu);
                    cx.notify();
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                this.scroll_timeline(event, cx);
            }))
    }

    /// What a left press on the structure lane does: grab a boundary's handle, or — doubled —
    /// open the naming sheet for the section under the pointer.
    fn press_structure_lane(&mut self, event: &MouseDownEvent) {
        let x = event.position.x - self.timeline_origin().x;
        let tick = self.timeline.x_to_tick(x).max_zero();
        if event.click_count >= 2 {
            self.prompt_for_section(tick);
            return;
        }
        if let Some(at) = self.section_handle_at(x) {
            self.begin_drag(Drag::SectionLabel {
                at,
                grab_offset: tick - at,
            });
        }
    }

    /// The section boundary whose handle sits under timeline-relative `x`, if any.
    fn section_handle_at(&self, x: Pixels) -> Option<Ticks> {
        let width = self
            .canvas
            .structure
            .get()
            .map_or(px(1200.0), |bounds| bounds.size.width);
        let (from, to) = self.timeline.visible_range(width);
        self.project()
            .sections
            .points()
            .iter()
            .filter(|point| point.label.is_some() && point.tick >= from && point.tick <= to)
            .map(|point| point.tick)
            .find(|at| {
                let edge = self.timeline.tick_to_x(*at);
                x >= edge && x <= edge + paint::CHORD_HANDLE
            })
    }

    /// The strip of chords under the structure lane.
    ///
    /// It sits above the clip lanes and spans all of them, because that is what harmony is: one
    /// thing the whole arrangement obeys at any one moment, belonging to no track.
    fn render_harmony_lane(&mut self, cx: &mut gpui::Context<Self>) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let view = self.timeline.clone();

        // Only what is on screen is painted, and it is copied out because a paint closure has to
        // capture `'static`.
        let width = self
            .canvas
            .harmony
            .get()
            .map_or(px(1200.0), |b| b.size.width);
        let (from, to) = view.visible_range(width);
        let harmony = paint::HarmonyPaint {
            events: self
                .project()
                .harmony
                .events_in(from, to, &self.project().signatures),
            keys: self.project().harmony.keys.points().to_vec(),
            handles: self.chord_handles(),
            held: match self.drag {
                Some(Drag::HarmonyChord { at, .. }) => Some(at),
                _ => None,
            },
        };

        div()
            .id("harmony-lane")
            .h(Metrics::HARMONY_LANE_HEIGHT)
            .w_full()
            .cursor_pointer()
            .child({
                let recorded = self.canvas.harmony.clone();
                canvas(
                    move |bounds, _, _| recorded.set(Some(bounds)),
                    move |bounds, _, window, cx| {
                        paint::clipped(window, bounds, |window| {
                            paint::harmony_lane(window, cx, bounds, &view, &harmony, &theme);
                        });
                    },
                )
                .size_full()
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.press_harmony_lane(event);
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    let x = event.position.x - this.timeline_origin().x;
                    let tick = this.timeline.x_to_tick(x).max_zero();
                    let menu = this.harmony_menu(event.position, tick);
                    this.open_menu(menu);
                    cx.notify();
                }),
            )
            // The chords sit above the clips they belong to and share their horizontal scale, so
            // the wheel has to move both together or the two rows disagree about where bar 9 is.
            .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                this.scroll_timeline(event, cx);
            }))
    }

    /// What a left press on the harmony lane grabs: a chord's handle, or the sound of it.
    ///
    /// Pressing anywhere but a handle sounds the chord written there, the way you would press a
    /// piano key, and sweeping along the lane plays the progression past. It sounds until the
    /// button comes up, which the window's own mouse-up handler takes care of.
    ///
    /// The tick an audition reads is *not* snapped, unlike everything that writes. A menu acts on
    /// a grid position and so rounds; an audition answers "what is written here", and rounding
    /// forward would sound the chord after the one under the pointer.
    fn press_harmony_lane(&mut self, event: &MouseDownEvent) {
        let x = event.position.x - self.timeline_origin().x;
        let tick = self.timeline.x_to_tick(x).max_zero();

        if self.harmony_row_at(event.position.y) == Some(HarmonyRow::Chords)
            && let Some(at) = chord_handle_at(&self.timeline, &self.chord_handles(), x)
        {
            self.begin_drag(Drag::HarmonyChord {
                at,
                grab_offset: tick - at,
            });
            return;
        }
        self.begin_drag(Drag::AuditionHarmony);
        self.audition_chord(tick);
    }

    /// Which row of the harmony lane a window-space `y` falls in, if either.
    fn harmony_row_at(&self, y: Pixels) -> Option<HarmonyRow> {
        let bounds = self.canvas.harmony.get()?;
        let (keys, chords) = paint::harmony_rows(bounds);
        if (keys.origin.y..keys.origin.y + keys.size.height).contains(&y) {
            Some(HarmonyRow::Keys)
        } else if (chords.origin.y..chords.origin.y + chords.size.height).contains(&y) {
            Some(HarmonyRow::Chords)
        } else {
            None
        }
    }

    /// Where each chord block on screen may be taken hold of.
    ///
    /// A key change splits a chord into two blocks, and only the first of them is where the
    /// numeral is actually written — so only that one gets a handle. Dragging the second would
    /// move a chord that starts somewhere off to the left of it.
    pub(crate) fn chord_handles(&self) -> Vec<Ticks> {
        let width = self
            .canvas
            .harmony
            .get()
            .map_or(px(1200.0), |bounds| bounds.size.width);
        let (from, to) = self.timeline.visible_range(width);
        self.project()
            .harmony
            .chords
            .points()
            .iter()
            .filter(|point| point.chord.is_some() && point.tick >= from && point.tick <= to)
            .map(|point| point.tick)
            .collect()
    }

    /// The right side: ruler plus the clip lanes, all painted on one canvas.
    fn render_timeline(&mut self, cx: &mut gpui::Context<Self>) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let view = self.timeline.clone();
        // The bar lines, as stretches of one meter each. Both painters walk them, and a song that
        // never changes meter is one stretch, which is the arithmetic they always did. Two
        // copies because the ruler and the lanes are two canvases, each capturing `'static`.
        let signatures = self.project().signatures.spans();
        let lane_signatures = signatures.clone();
        let playhead = self.playhead_ticks();
        // The whole map, cheaply: the painter draws nothing for a constant-tempo song.
        let tempo: Vec<TempoPoint> = self.project().tempo_map.points().to_vec();
        let loop_region = self
            .project()
            .loop_region
            .filter(|_| self.project().loop_enabled);

        // A track deleted while the view was scrolled to the bottom leaves the offset past the
        // end, and nothing else would ever pull it back.
        self.lane_scroll = self.lane_scroll.min(self.max_lane_scroll());
        let lane_scroll = self.lane_scroll;
        let language = self.language();

        // Everything the paint closure needs, copied out so it does not borrow `self`.
        let lanes: Vec<LanePaint> = self.lane_paint_data();
        let peaks = self.waveform_map();
        let band = self.rubber_band(crate::app::BandSurface::Lanes);

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .id("ruler")
                    .h(Metrics::RULER_HEIGHT)
                    .w_full()
                    .cursor_pointer()
                    .child({
                        let theme = theme.clone();
                        let view = view.clone();
                        let recorded = self.canvas.ruler.clone();
                        canvas(
                            move |bounds, _, _| recorded.set(Some(bounds)),
                            move |bounds, _, window, cx| {
                                paint::clipped(window, bounds, |window| {
                                    paint::ruler(
                                        window,
                                        cx,
                                        bounds,
                                        &view,
                                        &signatures,
                                        &tempo,
                                        &theme,
                                    );
                                    if let Some(region) = loop_region {
                                        paint::loop_region(window, bounds, &view, region, &theme);
                                    }
                                    paint::playhead(
                                        window,
                                        bounds,
                                        bounds.origin.x + view.tick_to_x(playhead),
                                        &theme,
                                    );
                                });
                            },
                        )
                        .size_full()
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            this.begin_ruler_drag(event, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            let x = event.position.x - this.timeline_origin().x;
                            let tick = this.snap(this.timeline.x_to_tick(x)).max_zero();
                            let menu = this.ruler_menu(event.position, tick);
                            this.open_menu(menu);
                            cx.notify();
                        }),
                    )
                    // The ruler scrolls the same timeline the lanes below it do. Without this the
                    // wheel was dead over the top twenty pixels of the arrangement and worked two
                    // centimetres lower, which is the kind of thing a user finds by accident.
                    .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                        this.scroll_timeline(event, cx);
                    })),
            )
            .child(self.render_structure_lane(cx))
            .child(self.render_harmony_lane(cx))
            .child(
                div()
                    .id("lanes")
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .child({
                        let theme = theme.clone();
                        let view = view.clone();
                        let recorded = self.canvas.lanes.clone();
                        canvas(
                            move |bounds, _, _| recorded.set(Some(bounds)),
                            move |bounds, _, window, cx| {
                                paint::clipped(window, bounds, |window| {
                                    paint::rect(window, bounds, theme.surface_sunken);
                                    paint::time_grid(
                                        window,
                                        bounds,
                                        &view,
                                        &lane_signatures,
                                        &theme,
                                    );
                                    if let Some(region) = loop_region {
                                        paint::loop_region(window, bounds, &view, region, &theme);
                                    }

                                    // Every lane is painted and the mask throws away what is off
                                    // the top or the bottom. A project is tens of tracks, not
                                    // thousands, so culling would cost more to read than to run.
                                    let mut y = bounds.origin.y - lane_scroll;
                                    for lane in &lanes {
                                        let lane_bounds = Bounds {
                                            origin: point(bounds.origin.x, y),
                                            size: size(bounds.size.width, px(lane.height)),
                                        };
                                        paint_lane(
                                            window,
                                            cx,
                                            lane_bounds,
                                            lane,
                                            &peaks,
                                            &view,
                                            &theme,
                                            language,
                                        );
                                        y += px(lane.height);
                                        paint::hline(window, bounds, y, theme.border_subtle);
                                    }

                                    paint::playhead(
                                        window,
                                        bounds,
                                        bounds.origin.x + view.tick_to_x(playhead),
                                        &theme,
                                    );
                                    if let Some(band) = band {
                                        paint::selection_band(window, band, &theme);
                                    }
                                });
                            },
                        )
                        .size_full()
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            this.begin_lane_drag(event, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            this.open_lane_menu(event, cx);
                        }),
                    )
                    .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                        this.scroll_timeline(event, cx);
                    })),
            )
    }

    /// Where every selected clip starts, captured before a move begins.
    fn selected_clip_origins(&self) -> Vec<(ClipId, Ticks)> {
        self.selected_clips
            .iter()
            .filter_map(|id| self.clip_start(*id).map(|start| (*id, start)))
            .collect()
    }

    /// The lane each selected clip currently sits on.
    fn selected_clip_lanes(&self) -> Vec<(ClipId, usize)> {
        self.selected_clips
            .iter()
            .filter_map(|id| {
                let track = self.session.track_of_clip(*id)?;
                Some((*id, self.project().track_index(track)?))
            })
            .collect()
    }

    /// Where a clip of either kind starts.
    fn clip_start(&self, clip: ClipId) -> Option<Ticks> {
        if let Some(midi) = self.session.midi_clip(clip) {
            return Some(midi.start);
        }
        self.project().tracks.iter().find_map(|track| {
            track
                .kind
                .as_audio()?
                .clips
                .iter()
                .find(|candidate| candidate.id == clip)
                .map(|clip| clip.start)
        })
    }

    /// The clip ids selected right now, for the paint closures.
    fn selected_clip_ids(&self) -> std::collections::BTreeSet<ClipId> {
        self.selected_clips.clone()
    }

    /// An audio clip's stored shape: where it sits, how long it is both ways, and its dials.
    ///
    /// `None` for a MIDI clip or an id nothing answers to, which is what lets the fade
    /// gesture and the gain sheet simply not exist for note clips.
    pub(crate) fn audio_clip_shape(
        &self,
        clip: ClipId,
    ) -> Option<(Ticks, Ticks, u64, f32, u64, u64)> {
        self.project().tracks.iter().find_map(|track| {
            let audio = track.kind.as_audio()?;
            let found = audio.clips.iter().find(|c| c.id == clip)?;
            Some((
                found.start,
                self.session.audio_clip_length_ticks(found),
                found.length_frames,
                found.gain_db,
                found.fade_in_frames,
                found.fade_out_frames,
            ))
        })
    }

    /// The audio clip on `track` whose fade handle is under a press, and which handle it is.
    ///
    /// `x` is in timeline pixels and `y_in_lane` measured from the lane's top; the clip is
    /// drawn two pixels inside its lane, which this subtracts before asking the geometry.
    ///
    /// Every audio clip on the track is asked, not only the one under the pointer's tick: a
    /// handle is a grab zone around a point on the clip's edge, so with no fades yet half of
    /// it hangs past the clip — and the fade-out handle sits exactly on the clip's end, the
    /// first tick `clip_at` counts as outside. Searched backwards like `clip_at`, so where
    /// two clips meet the handle drawn on top is the one taken hold of.
    fn fade_grab_at(
        &self,
        track: TrackId,
        x: Pixels,
        y_in_lane: Pixels,
    ) -> Option<(ClipId, FadeEdge)> {
        let track = self.project().track(track)?;
        let audio = track.kind.as_audio()?;
        audio.clips.iter().rev().find_map(|clip| {
            let edge = fade_handle_at(
                &self.timeline,
                clip.start,
                self.audio_clip_length_ticks(clip),
                clip.length_frames,
                clip.fade_in_frames,
                clip.fade_out_frames,
                x,
                y_in_lane - px(2.0),
            )?;
            Some((clip.id, edge))
        })
    }

    /// Shapes a fade towards the pointer, measured as a fraction of the clip.
    ///
    /// The pointer's tick becomes a fraction of the clip's tick length and that fraction a
    /// frame count, which is exactly how the painter spreads the frames across the width —
    /// so the ramp lands where the pointer is, whatever the zoom.
    pub(crate) fn drag_clip_fade(&mut self, clip: ClipId, edge: FadeEdge, at: Ticks) {
        let Some((start, length, frames, _, fade_in, fade_out)) = self.audio_clip_shape(clip)
        else {
            return;
        };
        let fraction = ((at - start).raw() as f64 / length.raw().max(1) as f64).clamp(0.0, 1.0);
        let at_frame = (fraction * frames as f64).round() as u64;
        let (fade_in, fade_out) = match edge {
            FadeEdge::In => (at_frame, fade_out),
            FadeEdge::Out => (fade_in, frames.saturating_sub(at_frame)),
        };
        let _ = self.session.set_clip_fades(clip, fade_in, fade_out);
    }

    /// Snapshot of what each lane draws, taken while `self` is still borrowable.
    fn lane_paint_data(&self) -> Vec<LanePaint> {
        self.project()
            .tracks
            .iter()
            .map(|track| {
                let color = self.theme.track_color(track.color.0);
                let clips = match &track.kind {
                    TrackKind::Instrument(inner) => inner
                        .clips
                        .iter()
                        .map(|clip| ClipPaint {
                            id: clip.id,
                            name: clip.name.clone(),
                            start: clip.start,
                            length: clip.length,
                            muted: clip.muted,
                            generated: clip.is_generated(),
                            content: ClipContent::Notes(clip.notes.clone()),
                        })
                        .collect(),
                    TrackKind::Audio(inner) => inner
                        .clips
                        .iter()
                        .map(|clip| {
                            let length = self.audio_clip_length_ticks(clip);
                            ClipPaint {
                                id: clip.id,
                                name: clip.name.clone(),
                                start: clip.start,
                                length,
                                muted: clip.muted,
                                // Audio is recorded or imported; nothing writes it.
                                generated: false,
                                content: ClipContent::Waveform {
                                    source: clip.source,
                                    offset_frames: clip.offset_frames,
                                    length_frames: clip.length_frames,
                                    gain_db: clip.gain_db,
                                    fade_in_frames: clip.fade_in_frames,
                                    fade_out_frames: clip.fade_out_frames,
                                },
                            }
                        })
                        .collect(),
                };
                LanePaint {
                    height: track.height,
                    color,
                    clips,
                    selected: self.selected_clip_ids(),
                }
            })
            .collect()
    }

    /// Starts a drag on the ruler: plain drag scrubs, alt-drag sets the loop region.
    fn begin_ruler_drag(&mut self, event: &MouseDownEvent, cx: &mut gpui::Context<Self>) {
        let x = event.position.x - self.timeline_origin().x;
        let tick = self.snap(self.timeline.x_to_tick(x));
        if event.modifiers.alt {
            // A degenerate region disables the engine loop, which is what an alt-click that
            // never becomes a drag should do.
            self.begin_drag(Drag::LoopRegion { anchor: tick });
            self.session.set_loop_enabled(true);
            self.session.set_loop_region(tick, tick);
        } else {
            self.seek(tick);
            self.begin_drag(Drag::Playhead);
        }
        cx.notify();
    }

    /// Starts a drag in the clip lanes.
    fn begin_lane_drag(&mut self, event: &MouseDownEvent, cx: &mut gpui::Context<Self>) {
        let origin = self.lanes_origin();
        let local = point(event.position.x - origin.x, self.lane_y(event.position.y));
        let tick = self.timeline.x_to_tick(local.x);

        let Some((track_id, lane_top)) = self.track_at_y(local.y) else {
            // Below the last track there is nothing to grab, so the press can only be the start
            // of a sweep across the lanes above it.
            self.begin_rubber_band(
                crate::app::BandSurface::Lanes,
                event.position,
                event.modifiers.shift,
            );
            cx.notify();
            return;
        };
        let under_pointer = self.clip_at(track_id, tick);
        self.select_track_for_press(track_id, under_pointer.map(|(id, _, _)| id));
        if let Some((clip_id, _, _)) = under_pointer
            && self.pointer.delete.matches(event)
        {
            let _ = self.session.remove_clip(clip_id);
            if self.selected_clip == Some(clip_id) {
                self.select_clip(None);
            }
            self.selected_notes.clear();
            cx.notify();
            return;
        }
        // Reached only when the user has not bound delete here, which the default no longer does.
        if let Some((clip_id, _, _)) = under_pointer
            && event.click_count >= 2
        {
            self.open_clip_in_editor(clip_id);
            cx.notify();
            return;
        }

        // The band under the title belongs to the fades; a press there takes hold of the
        // nearer handle, and the resize edge keeps everything below the band. Asked before
        // the clip under the pointer, not from inside its arm: a corner handle's grab zone
        // hangs past the clip's edge, over ticks where `clip_at` answers nothing — which
        // used to turn a press on the fade-out handle into an invisible rubber band.
        if let Some((clip_id, edge)) = self.fade_grab_at(track_id, local.x, local.y - lane_top) {
            if !self.selected_clips.contains(&clip_id) {
                self.select_clip(Some(clip_id));
            } else {
                self.selected_clip = Some(clip_id);
            }
            self.selected_notes.clear();
            self.begin_drag(Drag::ClipFade {
                clip: clip_id,
                edge,
            });
            cx.notify();
            return;
        }

        match under_pointer {
            Some((clip_id, clip_start, clip_length)) => {
                // Grabbing a clip that is already part of a selection keeps the selection, so a
                // rubber band followed by a drag moves everything it caught.
                if !self.selected_clips.contains(&clip_id) {
                    self.select_clip(Some(clip_id));
                } else {
                    self.selected_clip = Some(clip_id);
                }
                self.selected_notes.clear();
                let clip_start_x = self.timeline.tick_to_x(clip_start);
                let clip_end_x = self.timeline.tick_to_x(clip_start + clip_length);
                let grab = resize_grab(clip_end_x - clip_start_x);
                // The end is asked about first: a clip narrow enough for both zones to reach the
                // middle would otherwise be all front-trim, and the end is the edge people drag.
                if f32::from(clip_end_x - local.x).abs() <= grab {
                    self.begin_drag(Drag::ClipResize {
                        clip: clip_id,
                        edge: ClipEdge::End,
                    });
                } else if f32::from(local.x - clip_start_x).abs() <= grab {
                    self.begin_drag(Drag::ClipResize {
                        clip: clip_id,
                        edge: ClipEdge::Start,
                    });
                } else {
                    let origins = self.selected_clip_origins();
                    let origin_lanes = self.selected_clip_lanes();
                    let grab_lane = self.project().track_index(track_id).unwrap_or(0);
                    self.begin_drag(Drag::ClipMove {
                        clip: clip_id,
                        grab_offset: tick - clip_start,
                        origins,
                        origin_lanes,
                        grab_lane,
                        pressed_at: Some(event.position),
                    });
                }
            }
            None => {
                if self.pointer.create.matches(event) {
                    self.create_clip_at(track_id, self.snap(tick));
                } else {
                    self.begin_rubber_band(
                        crate::app::BandSurface::Lanes,
                        event.position,
                        event.modifiers.shift,
                    );
                }
            }
        }
        cx.notify();
    }

    /// Opens the menu for whatever is under the pointer in the clip lanes.
    fn open_lane_menu(&mut self, event: &MouseDownEvent, cx: &mut gpui::Context<Self>) {
        let origin = self.lanes_origin();
        let local = point(event.position.x - origin.x, self.lane_y(event.position.y));
        let tick = self.timeline.x_to_tick(local.x);

        let menu = match self.track_at_y(local.y) {
            Some((track_id, _)) => {
                let under_pointer = self.clip_at(track_id, tick);
                self.select_track_for_press(track_id, under_pointer.map(|(id, _, _)| id));
                match under_pointer {
                    Some((clip_id, _, _)) => {
                        // A right-click on a clip selects it, so Split at Playhead and the
                        // inspector are talking about the clip the menu is titled after — but a
                        // right-click *inside* a selection leaves that selection alone.
                        if !self.selected_clips.contains(&clip_id) {
                            self.select_clip(Some(clip_id));
                        } else {
                            self.selected_clip = Some(clip_id);
                        }
                        self.selected_notes.clear();
                        self.clip_menu(event.position, clip_id)
                    }
                    None => self.lane_menu(event.position, track_id, self.snap(tick).max_zero()),
                }
            }
            None => self.arrangement_menu(event.position),
        };
        self.open_menu(menu);
        cx.notify();
    }

    /// Wheel handling: plain moves down the tracks, Shift moves along the song, Alt zooms.
    fn scroll_timeline(&mut self, event: &gpui::ScrollWheelEvent, cx: &mut gpui::Context<Self>) {
        let delta = event.delta.pixel_delta(px(24.0));
        if event.modifiers.alt {
            let anchor = event.position.x - self.timeline_origin().x;
            let factor = if delta.y > px(0.0) { 1.12 } else { 1.0 / 1.12 };
            self.timeline.zoom_by(factor, anchor);
        } else if event.modifiers.shift {
            self.timeline.scroll_by(-delta.y - delta.x);
        } else {
            // Plain wheel moves down the track list, the way it does in every other panel and
            // every other DAW; Shift is what turns it sideways. It used to scroll the timeline
            // horizontally, which was the only thing it could do while the lanes had no scroll
            // of their own — and left every track past the sixth unreachable.
            self.scroll_lanes_by(-delta.y);
            self.timeline.scroll_by(-delta.x);
        }
        cx.notify();
    }

    /// Moves the lane column, keeping it inside what there is to see.
    pub(crate) fn scroll_lanes_by(&mut self, delta: Pixels) {
        self.lane_scroll = (self.lane_scroll + delta).clamp(px(0.0), self.max_lane_scroll());
    }

    /// Combined height of every track's lane.
    pub(crate) fn lanes_height(&self) -> Pixels {
        px(self.project().tracks.iter().map(|track| track.height).sum())
    }

    /// How far the lanes may be scrolled before the last one is flush with the bottom.
    pub(crate) fn max_lane_scroll(&self) -> Pixels {
        let viewport = self
            .canvas
            .lanes
            .get()
            .map_or(px(0.0), |bounds| bounds.size.height);
        scroll_limit(self.lanes_height(), viewport)
    }

    /// Scrolls the lanes just far enough to bring `track` into view.
    ///
    /// Just far enough, so selecting a track that is already on screen does not move the view out
    /// from under the pointer — which is exactly the frame a click is landing in.
    pub(crate) fn reveal_track(&mut self, track: TrackId) {
        let Some((top, height)) = self.lane_span(track) else {
            return;
        };
        let viewport = self
            .canvas
            .lanes
            .get()
            .map_or(px(0.0), |bounds| bounds.size.height);
        self.lane_scroll = reveal_offset(self.lane_scroll, viewport, top, height)
            .min(self.max_lane_scroll())
            .max(px(0.0));
    }

    /// Where a track's lane starts in the column, and how tall it is.
    fn lane_span(&self, track: TrackId) -> Option<(Pixels, Pixels)> {
        let mut top = px(0.0);
        for candidate in &self.project().tracks {
            let height = px(candidate.height);
            if candidate.id == track {
                return Some((top, height));
            }
            top += height;
        }
        None
    }

    /// Where a window position falls in the lane column's own coordinates.
    ///
    /// Not merely the offset from the canvas: the column scrolls, so what is under the pointer is
    /// how far into the canvas it is *plus* how far the column has been pushed up.
    pub(crate) fn lane_y(&self, window_y: Pixels) -> Pixels {
        window_y - self.lanes_origin().y + self.lane_scroll
    }

    /// Track whose lane contains `y`, with the lane's top offset.
    pub(crate) fn track_at_y(&self, y: Pixels) -> Option<(TrackId, Pixels)> {
        let mut top = px(0.0);
        for track in &self.project().tracks {
            let bottom = top + px(track.height);
            if y >= top && y < bottom {
                return Some((track.id, top));
            }
            top = bottom;
        }
        None
    }

    /// Tracks whose lanes intersect the vertical span, in lane-local pixels.
    pub(crate) fn tracks_in_rows(&self, top: Pixels, bottom: Pixels) -> Vec<TrackId> {
        let (top, bottom) = (top.min(bottom), top.max(bottom));
        let mut found = Vec::new();
        let mut lane_top = px(0.0);
        for track in &self.project().tracks {
            let lane_bottom = lane_top + px(track.height);
            if lane_bottom > top && lane_top < bottom {
                found.push(track.id);
            }
            lane_top = lane_bottom;
        }
        found
    }

    /// Clip on `track` covering `tick`, with its start and length.
    ///
    /// Searched backwards, because [`paint_lane`] draws the list forwards: where two clips
    /// overlap the later one is on top, and a hit test that walked the same way as the painter
    /// answered with the clip *underneath* — so the click selected, moved and deleted something
    /// the user could not see.
    fn clip_at(&self, track: TrackId, tick: Ticks) -> Option<(ClipId, Ticks, Ticks)> {
        let track = self.project().track(track)?;
        match &track.kind {
            TrackKind::Instrument(inner) => inner
                .clips
                .iter()
                .rev()
                .find(|clip| tick >= clip.start && tick < clip.end())
                .map(|clip| (clip.id, clip.start, clip.length)),
            TrackKind::Audio(inner) => inner
                .clips
                .iter()
                .rev()
                .map(|clip| (clip.id, clip.start, self.audio_clip_length_ticks(clip)))
                .find(|(_, start, length)| tick >= *start && tick < *start + *length),
        }
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

/// One of the harmony lane's two rows.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum HarmonyRow {
    /// The thin strip of key changes along the top.
    Keys,
    /// The chord blocks below it.
    Chords,
}

/// The chord a press at `x` takes hold of, if it landed on that chord's handle.
///
/// Free rather than a method so the rule can be tested without a window: a handle is six pixels
/// wide, and which side of them a press lands on is the difference between moving a chord and
/// playing it.
fn chord_handle_at(
    view: &crate::ui::timeline::TimelineView,
    handles: &[Ticks],
    x: Pixels,
) -> Option<Ticks> {
    handles.iter().copied().find(|start| {
        // The same inset the painter uses, or the zone would sit a pixel to the left of the bar
        // it belongs to and miss its rightmost column entirely.
        let left = view.tick_to_x(*start) + paint::CHORD_BLOCK_INSET;
        x >= left && x < left + paint::CHORD_HANDLE
    })
}

/// The fade handle a press at `x`, `y_in_clip` takes hold of, on an audio clip drawn from
/// `start` for `length` whose fades cover `fade_in` and `fade_out` of its `frames` frames.
///
/// Free rather than a method for the same reason [`chord_handle_at`] is. `y_in_clip` is
/// measured from the clip's own top edge: the handles live in the band just under the title
/// strip, which is what leaves the rest of the right edge to the resize grab. When both
/// handles are within reach — a clip with no fades yet is exactly its two corners — the
/// nearer one wins, and a tie goes to the fade-in.
#[allow(clippy::too_many_arguments)]
fn fade_handle_at(
    view: &crate::ui::timeline::TimelineView,
    start: Ticks,
    length: Ticks,
    frames: u64,
    fade_in: u64,
    fade_out: u64,
    x: Pixels,
    y_in_clip: Pixels,
) -> Option<FadeEdge> {
    if frames == 0 {
        return None;
    }
    let width = f32::from(view.duration_to_width(length));
    if width <= FADE_HANDLE_MIN_WIDTH {
        return None;
    }
    if y_in_clip < TITLE_HEIGHT || y_in_clip > TITLE_HEIGHT + FADE_BAND {
        return None;
    }
    let left = f32::from(view.tick_to_x(start));
    let in_x = left + width * (fade_in as f64 / frames as f64) as f32;
    let out_x = left + width * (1.0 - (fade_out as f64 / frames as f64) as f32);
    let x = f32::from(x);
    let to_in = (x - in_x).abs();
    let to_out = (x - out_x).abs();
    if to_in > FADE_GRAB && to_out > FADE_GRAB {
        return None;
    }
    Some(if to_in <= to_out {
        FadeEdge::In
    } else {
        FadeEdge::Out
    })
}

/// What one lane draws.
struct LanePaint {
    height: f32,
    color: gpui::Hsla,
    clips: Vec<ClipPaint>,
    /// Every selected clip, so a rubber band can light up more than one at a time.
    selected: std::collections::BTreeSet<ClipId>,
}

/// Waveform peaks keyed by audio source, shared by every lane in a frame.
type PeakMap = crate::app::WaveformMap;

struct ClipPaint {
    id: ClipId,
    name: String,
    start: Ticks,
    length: Ticks,
    muted: bool,
    /// Written by the composer rather than played, which the clip says on its own face.
    generated: bool,
    content: ClipContent,
}

enum ClipContent {
    Notes(Vec<Note>),
    Waveform {
        source: SourceId,
        offset_frames: u64,
        length_frames: u64,
        gain_db: f32,
        fade_in_frames: u64,
        fade_out_frames: u64,
    },
}

#[allow(clippy::too_many_arguments)]
fn paint_lane(
    window: &mut Window,
    cx: &mut gpui::App,
    bounds: Bounds<Pixels>,
    lane: &LanePaint,
    peaks: &PeakMap,
    view: &crate::ui::timeline::TimelineView,
    theme: &Theme,
    language: auris_i18n::Language,
) {
    for clip in &lane.clips {
        let x = bounds.origin.x + view.tick_to_x(clip.start);
        let width = view.duration_to_width(clip.length);
        if x + width < bounds.origin.x || x > bounds.origin.x + bounds.size.width {
            continue;
        }
        let clip_bounds = Bounds {
            origin: point(x, bounds.origin.y + px(2.0)),
            size: size(width.max(px(3.0)), bounds.size.height - px(4.0)),
        };
        let selected = lane.selected.contains(&clip.id);
        let body = if clip.muted {
            Theme::translucent(lane.color, 0.08)
        } else {
            Theme::translucent(lane.color, 0.30)
        };
        let radius = Metrics::RADIUS_MD;
        paint::rounded_rect(window, clip_bounds, radius, body);
        // A brighter top strip carries the clip name and doubles as the grab area. Only its
        // top corners are rounded, so it sits inside the clip's outline instead of cutting a
        // square notch out of it.
        paint::rect_with_corners(
            window,
            Bounds {
                origin: clip_bounds.origin,
                size: size(clip_bounds.size.width, TITLE_HEIGHT),
            },
            Corners {
                top_left: radius,
                top_right: radius,
                bottom_right: px(0.0),
                bottom_left: px(0.0),
            },
            if selected {
                theme.selection
            } else if clip.muted {
                // Mute used to be a body fill two per cent more transparent than an unmuted
                // clip's — about 1.1:1 apart, which is to say invisible. It is the title strip
                // that has to say it, because that is the part with the colour in it.
                Theme::translucent(lane.color, 0.28)
            } else {
                Theme::translucent(lane.color, 0.85)
            },
        );
        // A selected clip gets an outline as well as a lit title bar, so the selection reads
        // at a glance on a lane packed with clips.
        if selected {
            paint::rounded_outline(window, clip_bounds, radius, px(1.5), theme.selection);
        }

        // A clip the composer wrote says so on its own face, because what can be done to it
        // differs: it can be written again, and it will be if somebody asks.
        if clip.generated {
            let dot = px(5.0);
            paint::rounded_rect(
                window,
                Bounds {
                    origin: point(
                        clip_bounds.origin.x + clip_bounds.size.width - dot - px(4.0),
                        clip_bounds.origin.y + (TITLE_HEIGHT - dot) / 2.0,
                    ),
                    size: size(dot, dot),
                },
                dot / 2.0,
                theme.text_on(lane.color),
            );
        }

        let content_bounds = Bounds {
            origin: point(clip_bounds.origin.x, clip_bounds.origin.y + TITLE_HEIGHT),
            size: size(
                clip_bounds.size.width,
                clip_bounds.size.height - TITLE_HEIGHT,
            ),
        };
        match &clip.content {
            ClipContent::Notes(notes) => {
                paint::clip_notes(window, content_bounds, notes, clip.length, theme.text);
            }
            ClipContent::Waveform {
                source,
                offset_frames,
                length_frames,
                fade_in_frames,
                fade_out_frames,
                ..
            } => {
                if let Some(peaks) = peaks.get(source) {
                    // Peaks are stored channel-major, so take the first channel's run only —
                    // passing the whole vector would draw the right channel's data as if it
                    // were the tail of the left.
                    let per_channel = peaks.bucket_count();
                    if per_channel > 0 {
                        let per_bucket = peaks.samples_per_bucket.max(1) as f64;
                        let first = (*offset_frames as f64 / per_bucket) as usize;
                        let buckets = (*length_frames as f64 / per_bucket).max(1.0);
                        let columns = f32::from(content_bounds.size.width).max(1.0);
                        paint::waveform(
                            window,
                            content_bounds,
                            &peaks.min[..per_channel],
                            &peaks.max[..per_channel],
                            first,
                            (buckets / columns as f64) as f32,
                            theme.text,
                        );
                    }
                }
                // Fades are drawn as a fraction of the clip's frames, exactly as the waveform
                // above spreads its frames across the width — the ramp ends over the sample it
                // ends on. Handles only where there is room to grab them; the hit test reads
                // the same constant.
                if f32::from(clip_bounds.size.width) > FADE_HANDLE_MIN_WIDTH && *length_frames > 0 {
                    let width = f32::from(content_bounds.size.width);
                    let fade_in = width * (*fade_in_frames as f64 / *length_frames as f64) as f32;
                    let fade_out = width * (*fade_out_frames as f64 / *length_frames as f64) as f32;
                    paint::clip_fades(window, content_bounds, fade_in, fade_out, theme);
                }
            }
        }

        if f32::from(clip_bounds.size.width) > 28.0 {
            // The name says it too, so mute is not carried by a shade of a colour the user
            // chose — which is unreadable for anyone who cannot separate those two shades, and
            // was barely readable for anyone who can.
            let mut name = if clip.muted {
                format!("{} · {}", Key::MuteInitial.get(language), clip.name)
            } else {
                clip.name.clone()
            };
            // An audio clip carrying its own gain says the number on its face — the mix
            // reads differently from how the faders alone say it should, and this is why.
            if let ClipContent::Waveform { gain_db, .. } = &clip.content
                && *gain_db != 0.0
            {
                name = format!("{name} · {gain_db:+.1} dB");
            }
            paint::label(
                window,
                cx,
                point(
                    clip_bounds.origin.x + px(5.0),
                    clip_bounds.origin.y + px(1.5),
                ),
                name,
                px(9.0),
                // Read against the track's own colour rather than against the accent: the user
                // chooses one and the scheme the other, and only one of them is behind this text.
                theme.text_on(lane.color),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::timeline::TimelineView;

    fn view() -> TimelineView {
        TimelineView {
            pixels_per_beat: 48.0,
            scroll_ticks: Ticks::ZERO,
        }
    }

    #[test]
    fn a_chord_is_taken_hold_of_by_its_leading_edge_and_played_anywhere_else() {
        // The whole gesture rests on this: a press inside the handle moves the chord, and a press
        // one pixel past it sounds the chord instead and sweeps the progression. Both are the
        // same button on the same block.
        let view = view();
        let bar = Ticks(3840);
        let handles = [bar, bar * 4];

        // Where the *bar* is drawn, which is the block's inset edge and not the chord's tick.
        let edge = view.tick_to_x(bar) + paint::CHORD_BLOCK_INSET;
        assert_eq!(chord_handle_at(&view, &handles, edge), Some(bar));
        assert_eq!(
            chord_handle_at(&view, &handles, edge + paint::CHORD_HANDLE - px(1.0)),
            Some(bar),
            "the last column the handle is painted on is still the handle"
        );
        assert_eq!(
            chord_handle_at(&view, &handles, edge + paint::CHORD_HANDLE),
            None,
            "past the handle is the body of the chord"
        );
        assert_eq!(
            chord_handle_at(&view, &handles, edge - px(1.0)),
            None,
            "and before it is the gap between the blocks"
        );
        assert_eq!(
            chord_handle_at(
                &view,
                &handles,
                view.tick_to_x(bar * 4) + paint::CHORD_BLOCK_INSET
            ),
            Some(bar * 4)
        );
        assert_eq!(chord_handle_at(&view, &[], edge), None);
    }

    #[test]
    fn the_grab_zone_covers_the_bar_that_is_drawn_and_nothing_beside_it() {
        // The painter insets the block; the hit test did not, so the whole zone sat one pixel to
        // the left of the bar and its rightmost column could not be pressed at all.
        let view = view();
        let bar = Ticks(3840);
        let drawn = view.tick_to_x(bar) + paint::CHORD_BLOCK_INSET;
        for offset in 0..(f32::from(paint::CHORD_HANDLE) as i32) {
            assert_eq!(
                chord_handle_at(&view, &[bar], drawn + px(offset as f32)),
                Some(bar),
                "every column the bar is painted on takes hold of it",
            );
        }
    }

    #[test]
    fn a_fade_is_taken_hold_of_in_the_band_under_the_title() {
        // Four beats at 48 px/beat: a 192-pixel clip, 48 000 frames behind it.
        let view = view();
        let length = Ticks::from_beats(4.0);
        let frames = 48_000;
        let y = TITLE_HEIGHT + px(4.0);

        // With no fades yet, the handles are the clip's top corners.
        let grab = |fade_in, fade_out, x: f32, y| {
            fade_handle_at(
                &view,
                Ticks::ZERO,
                length,
                frames,
                fade_in,
                fade_out,
                px(x),
                y,
            )
        };
        assert_eq!(grab(0, 0, 0.0, y), Some(FadeEdge::In));
        assert_eq!(grab(0, 0, 192.0, y), Some(FadeEdge::Out));
        assert_eq!(grab(0, 0, 96.0, y), None, "between the handles is the body");
        assert_eq!(
            grab(0, 0, 192.0, TITLE_HEIGHT + FADE_BAND + px(1.0)),
            None,
            "below the band the same pixel is the resize edge"
        );
        assert_eq!(
            grab(0, 0, 96.0, TITLE_HEIGHT - px(2.0)),
            None,
            "the title strip stays the move grab"
        );

        // A fade already drawn moves its handle to the top of the ramp.
        let quarter = frames / 4;
        assert_eq!(grab(quarter, 0, 48.0, y), Some(FadeEdge::In));
        assert_eq!(
            grab(quarter, 0, 0.0, y),
            None,
            "the corner stopped being the handle once the fade moved it"
        );
        assert_eq!(grab(0, quarter, 144.0, y), Some(FadeEdge::Out));
    }

    #[test]
    fn a_corner_handle_answers_on_both_of_its_halves() {
        // With no fades yet the handles sit on the clip's corners, so half of each grab zone
        // hangs past the clip — ticks `clip_at` counts as outside, which is why the lane
        // press asks for fades before it asks what clip is under the pointer. Gated behind
        // the clip under the tick, the fade-out handle could only be taken by the sliver of
        // its zone that was still inside the clip.
        let view = view();
        let y = TITLE_HEIGHT + px(4.0);
        let grab = |x: f32| {
            fade_handle_at(
                &view,
                Ticks::ZERO,
                Ticks::from_beats(4.0),
                48_000,
                0,
                0,
                px(x),
                y,
            )
        };
        assert_eq!(grab(-4.0), Some(FadeEdge::In));
        assert_eq!(grab(196.0), Some(FadeEdge::Out));
    }

    #[test]
    fn a_sliver_of_a_clip_offers_no_fade_handles() {
        // Half a beat is 24 pixels — under the minimum, where the two handles and the resize
        // grab would overlap into a lottery.
        let view = view();
        assert_eq!(
            fade_handle_at(
                &view,
                Ticks::ZERO,
                Ticks::from_beats(0.5),
                6_000,
                0,
                0,
                px(0.0),
                TITLE_HEIGHT + px(4.0),
            ),
            None
        );
    }

    #[test]
    fn a_column_shorter_than_its_panel_does_not_scroll() {
        assert_eq!(scroll_limit(px(300.0), px(600.0)), px(0.0));
        assert_eq!(scroll_limit(px(600.0), px(600.0)), px(0.0));
        assert_eq!(scroll_limit(px(900.0), px(600.0)), px(300.0));
        // Before the first paint the panel has no recorded height, and the column must not drift.
        assert_eq!(scroll_limit(px(900.0), px(0.0)), px(900.0));
    }

    #[test]
    fn revealing_a_row_moves_the_view_as_little_as_it_can() {
        let viewport = px(500.0);
        let row = px(100.0);

        // Already on screen: nothing moves, so a click does not shift the row out from under the
        // pointer that is still on it.
        assert_eq!(reveal_offset(px(0.0), viewport, px(200.0), row), px(0.0));
        // Below the bottom edge: down until its foot is flush, and no further.
        assert_eq!(reveal_offset(px(0.0), viewport, px(800.0), row), px(400.0));
        // Above the top edge: up to it exactly.
        assert_eq!(
            reveal_offset(px(400.0), viewport, px(200.0), row),
            px(200.0)
        );
    }

    #[test]
    fn a_row_taller_than_the_panel_is_shown_from_the_top() {
        // Scrolling until its foot is flush would put the name off the top of the screen.
        assert_eq!(
            reveal_offset(px(0.0), px(200.0), px(300.0), px(600.0)),
            px(300.0)
        );
    }

    #[test]
    fn a_resize_handle_never_swallows_the_thing_it_belongs_to() {
        // Zoomed far out a clip is a few pixels wide, and a fixed handle either side of its end
        // covered the whole block: the clip could be stretched and never moved.
        assert_eq!(resize_grab(px(300.0)), RESIZE_HANDLE);
        assert!(resize_grab(px(9.0)) < RESIZE_HANDLE);
        assert!(
            resize_grab(px(9.0)) * 2.0 < 9.0,
            "a grab zone on both sides of the end still leaves a middle to drag by",
        );
        assert_eq!(resize_grab(px(0.0)), 0.0);
    }

    #[test]
    fn a_chord_scrolled_off_the_left_keeps_its_handle_where_it_belongs() {
        // Handles are hit-tested in the same space the lane is painted in, so a scrolled view
        // must not leave them at the position they had before it scrolled.
        let view = TimelineView {
            pixels_per_beat: 48.0,
            scroll_ticks: Ticks(3840),
        };
        let bar = Ticks(3840);
        assert_eq!(view.tick_to_x(bar), px(0.0));
        assert_eq!(
            chord_handle_at(&view, &[bar], paint::CHORD_BLOCK_INSET),
            Some(bar)
        );
        // A chord off the left edge is hit at a negative x, which no press can produce.
        assert_eq!(chord_handle_at(&view, &[Ticks::ZERO], px(0.0)), None);
    }
}
