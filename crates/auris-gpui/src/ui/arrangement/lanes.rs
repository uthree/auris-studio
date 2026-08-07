//! The right-hand column — ruler, strips, clip lanes — and the snapshots its paint closures are
//! handed.
//!
//! A gpui paint closure captures `'static` and so cannot borrow the app. Everything a frame
//! draws is therefore copied out first, while `self` is still borrowable, into the plain structs
//! at the foot of this file; `super::lane_paint` is what turns one of them into pixels. The
//! element and the snapshot it takes are together here because they are read together: what the
//! closure can see is exactly what was copied out for it a few lines above.
//!
//! The cursor hitboxes are here too, for the same reason — they are built from the same lane
//! list the painter walks, so an arrow promising a grab the button does not deliver would have
//! to be a mistake made twice.

use auris_session::prelude::*;

use gpui::{
    Bounds, IntoElement, MouseButton, MouseDownEvent, Pixels, canvas, div, point, prelude::*, px,
    size,
};

use crate::app::AurisApp;
use crate::theme::Metrics;
use crate::ui::automation;
use crate::ui::paint;

use super::geometry::{CLIP_INSET, FADE_HANDLE_MIN_WIDTH, edge_zone_rows, resize_grab};
use super::lane_paint::{paint_automation, paint_lane};

impl AurisApp {
    /// The right side: ruler plus the clip lanes, all painted on one canvas.
    pub(super) fn render_timeline(
        &mut self,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let view = self.timeline.clone();
        // The bar lines, as stretches of one meter each. Both painters walk them, and a song that
        // never changes meter is one stretch, which is the arithmetic they always did. Two
        // copies because the ruler and the lanes are two canvases, each capturing `'static`.
        let signatures = self.project().signatures.spans();
        let lane_signatures = signatures.clone();
        let playhead = self.playhead_ticks();
        // The whole map, cheaply: the painter draws nothing for a constant-tempo song. Empty when
        // the marks are turned off, which is the whole of hiding them — a painter given nothing
        // draws nothing, and there is no second code path to keep in step with the first.
        let tempo: Vec<TempoPoint> = match self.panels.lanes.tempo {
            true => self.project().tempo_map.points().to_vec(),
            false => Vec::new(),
        };
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
        let automation_rows = self.automation_paint_data();
        let lanes: Vec<LanePaint> = self.lane_paint_data();
        let edge_zones = self.clip_edge_zones(&lanes);
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
                        AurisApp::opens_menu(cx, |this, at| {
                            let x = at.x - this.timeline_origin().x;
                            let tick = this.snap(this.timeline.x_to_tick(x)).max_zero();
                            this.ruler_menu(at, tick)
                        }),
                    )
                    // The ruler scrolls the same timeline the lanes below it do. Without this the
                    // wheel was dead over the top twenty pixels of the arrangement and worked two
                    // centimetres lower, which is the kind of thing a user finds by accident.
                    .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                        this.scroll_timeline(event, cx);
                    })),
            )
            .when(self.panels.lanes.structure, |this| {
                this.child(self.render_structure_lane(cx))
            })
            .when(self.panels.lanes.harmony, |this| {
                this.child(self.render_harmony_lane(cx))
            })
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
                            move |bounds, window, _| {
                                recorded.set(Some(bounds));
                                // A hitbox each rather than a hovered-thing on the view: a cursor
                                // driven from app state would repaint the whole arrangement on
                                // every pointer move across it, and gpui already knows which
                                // rectangle the pointer is in.
                                edge_zones
                                    .iter()
                                    .map(|zone| {
                                        window.insert_hitbox(
                                            Bounds {
                                                origin: bounds.origin + zone.origin,
                                                size: zone.size,
                                            },
                                            gpui::HitboxBehavior::Normal,
                                        )
                                    })
                                    .collect::<Vec<_>>()
                            },
                            move |bounds, edges: Vec<gpui::Hitbox>, window, cx| {
                                // Nothing on screen said the edges could be taken hold of, so the
                                // only way to find out was to try. The arrow says it now.
                                for edge in &edges {
                                    window
                                        .set_cursor_style(gpui::CursorStyle::ResizeLeftRight, edge);
                                }
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

                                    // Every row is painted and the mask throws away what is off
                                    // the top or the bottom. A project is tens of tracks, not
                                    // thousands, so culling would cost more to read than to run.
                                    //
                                    // Each row is placed by the top it was given rather than by
                                    // adding up what came before it, so the clips and the curves
                                    // cannot walk apart from each other or from the hit tests.
                                    let top_of = |top: Pixels| bounds.origin.y - lane_scroll + top;
                                    for lane in &lanes {
                                        let lane_bounds = Bounds {
                                            origin: point(bounds.origin.x, top_of(lane.top)),
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
                                        paint::hline(
                                            window,
                                            bounds,
                                            lane_bounds.origin.y + lane_bounds.size.height,
                                            theme.border_subtle,
                                        );
                                    }
                                    for row in &automation_rows {
                                        let row_bounds = Bounds {
                                            origin: point(bounds.origin.x, top_of(row.top)),
                                            size: size(
                                                bounds.size.width,
                                                crate::ui::automation::ROW_HEIGHT,
                                            ),
                                        };
                                        paint_automation(
                                            window, cx, row_bounds, row, &view, &theme,
                                        );
                                        paint::hline(
                                            window,
                                            bounds,
                                            row_bounds.origin.y + row_bounds.size.height,
                                            theme.border_subtle,
                                        );
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

    /// Where the pointer would take hold of a clip's edge, relative to the lanes canvas.
    ///
    /// The cursor and the press have to agree about this. An arrow promising a grab the button
    /// does not deliver is worse than no arrow at all, so both are built from the same three
    /// numbers: [`resize_grab`] off the clip's drawn width, the lane it sits in, and the band an
    /// audio clip gives to its fade handles.
    ///
    /// Only the *inner* half of each grab zone is offered. [`Self::begin_lane_drag`] reaches the
    /// resize check only once [`Self::clip_at`] has found a clip under the pointer's tick, so the
    /// half hanging past the edge is a zone no press can ever land in.
    fn clip_edge_zones(&self, lanes: &[LanePaint]) -> Vec<Bounds<Pixels>> {
        let mut zones = Vec::new();
        for lane in lanes {
            let lane_top = lane.top - self.lane_scroll;
            let height = px(lane.height);
            for clip in &lane.clips {
                let left = self.timeline.tick_to_x(clip.start);
                let right = self.timeline.tick_to_x(clip.start + clip.length);
                let grab = px(resize_grab(right - left));
                if grab <= px(0.0) {
                    continue;
                }
                let top = lane_top + CLIP_INSET;
                let bottom = lane_top + height - CLIP_INSET;
                // An audio clip wide enough to draw its fade handles gives them the band under
                // its name bar, and a press there takes a fade rather than the edge.
                let fades = matches!(clip.content, ClipContent::Waveform { .. })
                    && f32::from(right - left) > FADE_HANDLE_MIN_WIDTH;
                for x in [left, right - grab] {
                    zones.extend(edge_zone_rows(x, grab, top, bottom, fades));
                }
            }
        }
        zones
    }

    /// Snapshot of what each automation row draws, taken while `self` is still borrowable.
    ///
    /// `&mut self` because a plugin parameter's descriptor comes out of a cache the session fills
    /// on demand; the mixer's own controls need none, but the row is meant to widen to everything
    /// [`ParamTarget`] can name and taking the borrow now costs nothing.
    fn automation_paint_data(&mut self) -> Vec<AutomationPaint> {
        let rows = self.lane_rows();
        let mut painted = Vec::new();
        for row in rows {
            let Some(target) = row.target() else {
                continue;
            };
            let Some(descriptor) = self.session.descriptor_for(target) else {
                continue;
            };
            let color = self
                .project()
                .track(row.track)
                .map(|track| self.theme.track_color(track.color.0))
                .unwrap_or(self.theme.accent);
            painted.push(AutomationPaint {
                top: row.top,
                name: self.param_label(&descriptor.name),
                color,
                range: (descriptor.min, descriptor.max),
                lane: self.session.automation().lane(target).cloned(),
                resting: self.session.param_value(target, &descriptor),
            });
        }
        painted
    }

    /// Snapshot of what each lane draws, taken while `self` is still borrowable.
    fn lane_paint_data(&self) -> Vec<LanePaint> {
        let tops: Vec<Pixels> = self
            .lane_rows()
            .iter()
            .filter(|row| matches!(row.kind, automation::RowKind::Clips))
            .map(|row| row.top)
            .collect();
        self.project()
            .tracks
            .iter()
            .zip(tops)
            .map(|(track, top)| {
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
                    // A bus is a mixing point rather than a place material sits. Its lane stays
                    // in the arrangement — the track list and the mixer have to agree on what is
                    // where — and stays empty, because there is nothing on it to draw.
                    TrackKind::Bus => Vec::new(),
                };
                LanePaint {
                    top,
                    height: track.height,
                    color,
                    clips,
                    selected: self.selected_clip_ids(),
                }
            })
            .collect()
    }
}

/// What one lane draws.
pub(super) struct LanePaint {
    /// Top of the row in the lane column's own coordinates, from [`AurisApp::lane_rows`].
    ///
    /// Carried rather than accumulated by whoever is walking the list: an automation row between
    /// two tracks makes "add up the heights so far" wrong in a way that is invisible until a
    /// press lands on the clip above the one it looks like.
    top: Pixels,
    height: f32,
    pub(super) color: gpui::Hsla,
    pub(super) clips: Vec<ClipPaint>,
    /// Every selected clip, so a rubber band can light up more than one at a time.
    pub(super) selected: std::collections::BTreeSet<ClipId>,
}

/// What one automation row draws.
pub(super) struct AutomationPaint {
    /// Top of the row in the lane column's own coordinates.
    top: Pixels,
    /// The parameter's name, in the interface language.
    pub(super) name: String,
    /// The track's colour, so a curve is matched to its track at a glance.
    pub(super) color: gpui::Hsla,
    /// Lowest and highest the parameter goes, which is what the row's height spans.
    pub(super) range: (f32, f32),
    /// The curve, or `None` while the parameter has no lane yet.
    pub(super) lane: Option<AutomationLane>,
    /// Where the parameter sits with no lane driving it, so an empty row still shows the value a
    /// first point would be written near rather than an empty box.
    pub(super) resting: f32,
}

/// Waveform peaks keyed by audio source, shared by every lane in a frame.
pub(super) type PeakMap = crate::app::WaveformMap;

pub(super) struct ClipPaint {
    pub(super) id: ClipId,
    pub(super) name: String,
    pub(super) start: Ticks,
    pub(super) length: Ticks,
    pub(super) muted: bool,
    /// Written by the composer rather than played, which the clip says on its own face.
    pub(super) generated: bool,
    pub(super) content: ClipContent,
}

pub(super) enum ClipContent {
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
