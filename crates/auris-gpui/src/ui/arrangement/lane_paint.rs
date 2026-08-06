//! Drawing one clip lane, and one automation row.
//!
//! Free functions rather than methods, and a file of their own for what that costs: a paint
//! closure captures `'static`, so nothing here can reach the document. What is drawn arrives as
//! the snapshot `super::lanes` took while `self` was still borrowable, and these two functions
//! are the whole of what the arrangement puts on a canvas below the ruler.
//!
//! The clip metrics come out of `super::geometry`, which is also where the hit tests read them,
//! because the edges a pointer is offered have to be the edges that were drawn.

use auris_i18n::Key;

use gpui::{Bounds, Corners, Pixels, Window, point, px, size};

use crate::theme::{Metrics, Theme};
use crate::ui::paint;

use super::geometry::{CLIP_INSET, FADE_HANDLE_MIN_WIDTH, TITLE_HEIGHT};
use super::lanes::{AutomationPaint, ClipContent, LanePaint, PeakMap};

/// Draws one automation row: the curve, its points, and the parameter it drives.
///
/// A row with no lane yet still draws a line — flat, at the value the parameter is resting on —
/// because an empty box says nothing about where a first point would land, and the answer is
/// "near that line".
pub(super) fn paint_automation(
    window: &mut Window,
    cx: &mut gpui::App,
    bounds: Bounds<Pixels>,
    row: &AutomationPaint,
    view: &crate::ui::timeline::TimelineView,
    theme: &Theme,
) {
    use crate::ui::automation::{curve_polyline, point_positions, value_to_y};

    paint::rect(window, bounds, theme.surface_sunken);
    // Where the parameter rests with nothing written, so the eye has a datum to read the curve
    // against — a fader's 0 dB, a pan's centre.
    let datum = value_to_y(
        row.resting,
        row.range.0,
        row.range.1,
        bounds.origin.y,
        bounds.size.height,
    );
    paint::hline(window, bounds, datum, theme.border_subtle);

    match &row.lane {
        None => paint::hline(window, bounds, datum, theme.text_faint),
        Some(lane) => {
            paint::polyline(
                window,
                &curve_polyline(lane, row.range, view, bounds),
                px(1.5),
                row.color,
            );
            for at in point_positions(lane, row.range, view, bounds) {
                paint::rect(
                    window,
                    Bounds {
                        origin: point(at.x - POINT_RADIUS, at.y - POINT_RADIUS),
                        size: size(POINT_RADIUS * 2.0, POINT_RADIUS * 2.0),
                    },
                    row.color,
                );
            }
        }
    }

    paint::label(
        window,
        cx,
        point(bounds.origin.x + px(4.0), bounds.origin.y + px(2.0)),
        row.name.clone(),
        px(9.0),
        theme.text_faint,
    );
}

/// Half the side of the square drawn at each automation point.
///
/// Smaller than the grab: a handle that filled its own tolerance would leave no lane between two
/// points to add a third one in.
const POINT_RADIUS: Pixels = px(3.0);

#[allow(clippy::too_many_arguments)]
pub(super) fn paint_lane(
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
            origin: point(x, bounds.origin.y + CLIP_INSET),
            size: size(width.max(px(3.0)), bounds.size.height - CLIP_INSET * 2.0),
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
