//! The arrangement: track headers on the left, a clip timeline on the right.

use auris_core::param::gain_to_db;
use auris_core::time::Ticks;
use auris_core::{ClipId, TrackId, TrackKind};
use gpui::{
    Axis, Bounds, IntoElement, MouseButton, MouseDownEvent, Pixels, Window, canvas, div, point,
    prelude::*, px, size,
};

use crate::app::{AurisApp, Drag, ParamTarget};
use crate::theme::{Metrics, Theme};
use crate::ui::paint;
use crate::ui::widgets::{ButtonStyle, button, db_to_meter_position, level_meter};

/// Width of the grab zone on a clip's right edge, in pixels.
const RESIZE_HANDLE: f32 = 7.0;

impl AurisApp {
    /// Renders the arrangement panel.
    pub(crate) fn render_arrangement(
        &mut self,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        div()
            .flex()
            .flex_1()
            .min_h(px(120.0))
            .overflow_hidden()
            .bg(theme.surface)
            .child(self.render_track_headers(cx))
            .child(self.render_timeline(cx))
    }

    /// The left column: one header per track, plus the add-track buttons.
    fn render_track_headers(&mut self, cx: &mut gpui::Context<Self>) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let selected = self.selected_track;
        let has_solo = self.project.has_solo();

        let headers: Vec<gpui::AnyElement> = self
            .project
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
                let kind = track.kind.label();

                div()
                    .id(("track-header", index))
                    .flex()
                    .h(px(track.height))
                    .border_b_1()
                    .border_color(theme.border_subtle)
                    .bg(if selected == Some(id) {
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
                    // A colour stripe is the fastest way to match a header to its clips.
                    .child(div().w(px(4.0)).h_full().bg(color))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .px_2()
                            .py_1()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.text)
                                            .truncate()
                                            .child(name),
                                    )
                                    .child(
                                        div().text_xs().text_color(theme.text_muted).child(kind),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(div().w(px(26.0)).child(button(
                                        ("mute", index),
                                        "M",
                                        ButtonStyle::Normal,
                                        muted,
                                        theme.mute,
                                        &theme,
                                        cx.listener(move |this, _, _, cx| {
                                            this.toggle_mute(id);
                                            cx.notify();
                                        }),
                                    )))
                                    .child(div().w(px(26.0)).child(button(
                                        ("solo", index),
                                        "S",
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
                                    )
                                    .child(div().w(px(8.0)).h(Metrics::CONTROL_HEIGHT).child(
                                        level_meter(
                                            db_to_meter_position(level_db),
                                            db_to_meter_position(level_db),
                                            Axis::Vertical,
                                            theme.meter_color(level_db),
                                            &theme,
                                        ),
                                    )),
                            )
                            .child(self.pan_control(id, pan, cx)),
                    )
                    .into_any_element()
            })
            .collect();

        div()
            .flex()
            .flex_col()
            .w(Metrics::TRACK_HEADER_WIDTH)
            .flex_shrink_0()
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .h(Metrics::RULER_HEIGHT)
                    .px_1()
                    .bg(theme.surface_raised)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().flex_1().child(button(
                        "add-instrument",
                        "+ Inst",
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.add_instrument_track();
                            cx.notify();
                        }),
                    )))
                    .child(div().flex_1().child(button(
                        "add-audio",
                        "+ Audio",
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
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
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_hidden()
                    .children(headers),
            )
    }

    /// The right side: ruler plus the clip lanes, all painted on one canvas.
    fn render_timeline(&mut self, cx: &mut gpui::Context<Self>) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let view = self.timeline.clone();
        let signature = self.project.time_signature;
        let playhead = self.playhead_ticks();
        let loop_region = self
            .project
            .loop_region
            .filter(|_| self.project.loop_enabled);

        // Everything the paint closure needs, copied out so it does not borrow `self`.
        let lanes: Vec<LanePaint> = self.lane_paint_data();
        let peaks = self.waveforms.clone();

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
                                    paint::ruler(window, cx, bounds, &view, signature, &theme);
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
                    ),
            )
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
                                    paint::time_grid(window, bounds, &view, signature, &theme);
                                    if let Some(region) = loop_region {
                                        paint::loop_region(window, bounds, &view, region, &theme);
                                    }

                                    let mut y = bounds.origin.y;
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
                    .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                        this.scroll_timeline(event, cx);
                    })),
            )
    }

    /// Snapshot of what each lane draws, taken while `self` is still borrowable.
    fn lane_paint_data(&self) -> Vec<LanePaint> {
        self.project
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
                                content: ClipContent::Waveform {
                                    source: clip.source,
                                    offset_frames: clip.offset_frames,
                                    length_frames: clip.length_frames,
                                },
                            }
                        })
                        .collect(),
                };
                LanePaint {
                    height: track.height,
                    color,
                    clips,
                    selected_clip: self.selected_clip,
                }
            })
            .collect()
    }

    /// Length of an audio clip on the musical timeline.
    pub(crate) fn audio_clip_length_ticks(&self, clip: &auris_core::AudioClip) -> Ticks {
        let rate = self.project.sample_rate.max(1.0);
        let start_seconds = self.project.tempo_map.ticks_to_seconds(clip.start).0;
        let end_seconds = start_seconds + clip.length_frames as f64 / rate;
        self.project
            .tempo_map
            .seconds_to_ticks(auris_core::time::Seconds(end_seconds))
            - clip.start
    }

    /// Starts a drag on the ruler: plain drag scrubs, alt-drag sets the loop region.
    fn begin_ruler_drag(&mut self, event: &MouseDownEvent, cx: &mut gpui::Context<Self>) {
        let x = event.position.x - self.timeline_origin().x;
        let tick = self.snap(self.timeline.x_to_tick(x));
        if event.modifiers.alt {
            self.edit("Set loop region");
            self.project.loop_region = Some((tick, tick));
            self.project.loop_enabled = true;
            // A degenerate region disables the engine loop, which is what an alt-click that
            // never becomes a drag should do.
            self.push_loop_to_engine();
            self.drag = Some(Drag::LoopRegion { anchor: tick });
        } else {
            self.seek(tick);
            self.begin_drag_without_edit(Drag::Playhead);
        }
        cx.notify();
    }

    /// Starts a drag in the clip lanes.
    fn begin_lane_drag(&mut self, event: &MouseDownEvent, cx: &mut gpui::Context<Self>) {
        let origin = self.lanes_origin();
        let local = point(event.position.x - origin.x, event.position.y - origin.y);
        let tick = self.timeline.x_to_tick(local.x);

        let Some((track_id, _lane_top)) = self.track_at_y(local.y) else {
            self.selected_clip = None;
            cx.notify();
            return;
        };
        self.select_track(track_id);

        match self.clip_at(track_id, tick) {
            Some((clip_id, clip_start, clip_length)) => {
                self.selected_clip = Some(clip_id);
                self.selected_notes.clear();
                let clip_end_x = self.timeline.tick_to_x(clip_start + clip_length);
                if f32::from(clip_end_x - local.x).abs() <= RESIZE_HANDLE {
                    self.begin_drag("Resize clip", Drag::ClipResize { clip: clip_id });
                } else {
                    self.begin_drag(
                        "Move clip",
                        Drag::ClipMove {
                            clip: clip_id,
                            grab_offset: tick - clip_start,
                        },
                    );
                }
            }
            None => {
                // Double-click semantics without a double-click: alt-click on empty space is the
                // "make something here" gesture, matching the piano roll's note drawing.
                if event.modifiers.alt {
                    self.create_clip_at(track_id, self.snap(tick));
                } else {
                    self.selected_clip = None;
                    self.seek(self.snap(tick));
                }
            }
        }
        cx.notify();
    }

    /// Wheel handling: plain scrolls, alt zooms.
    fn scroll_timeline(&mut self, event: &gpui::ScrollWheelEvent, cx: &mut gpui::Context<Self>) {
        let delta = event.delta.pixel_delta(px(24.0));
        if event.modifiers.alt {
            let anchor = event.position.x - self.timeline_origin().x;
            let factor = if delta.y > px(0.0) { 1.12 } else { 1.0 / 1.12 };
            self.timeline.zoom_by(factor, anchor);
        } else {
            self.timeline.scroll_by(-delta.x - delta.y);
        }
        cx.notify();
    }

    /// Track whose lane contains `y`, with the lane's top offset.
    pub(crate) fn track_at_y(&self, y: Pixels) -> Option<(TrackId, Pixels)> {
        let mut top = px(0.0);
        for track in &self.project.tracks {
            let bottom = top + px(track.height);
            if y >= top && y < bottom {
                return Some((track.id, top));
            }
            top = bottom;
        }
        None
    }

    /// Clip on `track` covering `tick`, with its start and length.
    fn clip_at(&self, track: TrackId, tick: Ticks) -> Option<(ClipId, Ticks, Ticks)> {
        let track = self.project.track(track)?;
        match &track.kind {
            TrackKind::Instrument(inner) => inner
                .clips
                .iter()
                .find(|clip| tick >= clip.start && tick < clip.end())
                .map(|clip| (clip.id, clip.start, clip.length)),
            TrackKind::Audio(inner) => inner
                .clips
                .iter()
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
            "Vol",
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
            "Pan",
            ParamTarget::TrackPan(track),
            pan,
            cx,
        )
    }
}

/// What one lane draws.
struct LanePaint {
    height: f32,
    color: gpui::Hsla,
    clips: Vec<ClipPaint>,
    selected_clip: Option<ClipId>,
}

/// Waveform peaks keyed by audio source, shared by every lane in a frame.
type PeakMap =
    std::collections::HashMap<auris_core::SourceId, std::sync::Arc<auris_gpu::WaveformPeaks>>;

struct ClipPaint {
    id: ClipId,
    name: String,
    start: Ticks,
    length: Ticks,
    muted: bool,
    content: ClipContent,
}

enum ClipContent {
    Notes(Vec<auris_core::Note>),
    Waveform {
        source: auris_core::SourceId,
        offset_frames: u64,
        length_frames: u64,
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
        let selected = lane.selected_clip == Some(clip.id);
        let body = if clip.muted {
            Theme::translucent(lane.color, 0.16)
        } else {
            Theme::translucent(lane.color, 0.34)
        };
        paint::rounded_rect(window, clip_bounds, px(3.0), body);
        // A brighter top strip carries the clip name and doubles as the grab area.
        paint::rect(
            window,
            Bounds {
                origin: clip_bounds.origin,
                size: size(clip_bounds.size.width, px(13.0)),
            },
            if selected {
                theme.selection
            } else {
                Theme::translucent(lane.color, 0.8)
            },
        );

        let content_bounds = Bounds {
            origin: point(clip_bounds.origin.x, clip_bounds.origin.y + px(13.0)),
            size: size(clip_bounds.size.width, clip_bounds.size.height - px(13.0)),
        };
        match &clip.content {
            ClipContent::Notes(notes) => {
                paint::clip_notes(window, content_bounds, notes, clip.length, theme.text);
            }
            ClipContent::Waveform {
                source,
                offset_frames,
                length_frames,
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
            }
        }

        if f32::from(clip_bounds.size.width) > 28.0 {
            paint::label(
                window,
                cx,
                point(
                    clip_bounds.origin.x + px(4.0),
                    clip_bounds.origin.y + px(1.0),
                ),
                clip.name.clone(),
                px(9.0),
                theme.text_on_accent,
            );
        }
    }
}
