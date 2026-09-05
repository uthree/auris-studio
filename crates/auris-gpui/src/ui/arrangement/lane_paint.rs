//! Drawing one clip lane, and one automation row.
//!
//! Free functions rather than methods, and a file of their own for what that costs: a paint
//! closure captures `'static`, so nothing here can reach the document. What is drawn arrives as
//! the snapshot `super::lanes` took while `self` was still borrowable, and these two functions
//! paint the per-row clip and automation content. `super::lanes` also paints the canvas
//! background, time grid, loop and punch regions, playhead, and selection band.
//!
//! The clip metrics come out of `super::geometry`, which is also where the hit tests read them,
//! because the edges a pointer is offered have to be the edges that were drawn.

use auris_i18n::Key;
use auris_session::prelude::loop_passes;

use gpui::{Bounds, Corners, Pixels, Window, point, px, size};

use crate::theme::{Metrics, Theme};
use crate::ui::paint;

use super::geometry::{
    BADGE_MIN_WIDTH, CLIP_INSET, FADE_HANDLE_MIN_WIDTH, TITLE_HEIGHT, follow_badge,
};
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

/// The name strip and its ink, including the body underneath a muted strip.
fn clip_title_colors(
    theme: &Theme,
    track: gpui::Hsla,
    selected: bool,
    muted: bool,
) -> (gpui::Hsla, gpui::Hsla) {
    let fill = if selected {
        theme.selection
    } else {
        // Resolve the translucent layers into one opaque colour so the grid and loop tint
        // cannot change the background against which this small label must remain readable.
        theme
            .surface_sunken
            .blend(Theme::translucent(track, if muted { 0.08 } else { 0.30 }))
            .blend(Theme::translucent(track, if muted { 0.28 } else { 0.85 }))
    };
    (fill, theme.text_on(fill))
}

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
        // The block covers the repeats too: a looped clip is one thing on the timeline, with one
        // outline, one name and one selection, however many times it says itself.
        let width = view.duration_to_width(clip.sounding_length());
        if x + width < bounds.origin.x || x > bounds.origin.x + bounds.size.width {
            continue;
        }
        let clip_bounds = Bounds {
            origin: point(x, bounds.origin.y + CLIP_INSET),
            size: size(width.max(px(3.0)), bounds.size.height - CLIP_INSET * 2.0),
        };
        let selected = lane.selected.contains(&clip.id);
        let (title_fill, title_ink) = clip_title_colors(theme, lane.color, selected, clip.muted);
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
            title_fill,
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
                title_ink,
            );
        }

        // One pass at a time, and a clip that does not repeat is one pass — so the ordinary case
        // goes through exactly the code the looped one does. Each pass is drawn at the content's
        // full width inside a mask its own width, which is what cuts the last repeat off wherever
        // the loop ends rather than squeezing it.
        let content_width = view.duration_to_width(clip.length);
        for (index, (offset, span)) in loop_passes(clip.length, clip.loop_end).enumerate() {
            let pass_x = bounds.origin.x + view.tick_to_x(clip.start + offset);
            let visible = Bounds {
                origin: point(pass_x, clip_bounds.origin.y + TITLE_HEIGHT),
                size: size(
                    view.duration_to_width(span),
                    clip_bounds.size.height - TITLE_HEIGHT,
                ),
            };
            let content_bounds = Bounds {
                origin: visible.origin,
                size: size(content_width, visible.size.height),
            };
            // A repeat is drawn a shade back from the phrase it repeats, so which part of the
            // block was played and which is the echo of it reads without clicking anything.
            let ink = if index == 0 {
                theme.text
            } else {
                theme.text_faint
            };
            if index > 0 {
                paint::vline(window, clip_bounds, pass_x, px(1.0), theme.text_faint);
            }
            paint::clipped(window, visible, |window| match &clip.content {
                ClipContent::Notes(notes) => {
                    paint::clip_notes(window, content_bounds, notes, clip.length, ink);
                }
                ClipContent::Waveform {
                    source,
                    offset_frames,
                    length_frames,
                    fade_in_frames,
                    fade_out_frames,
                    fade_in_curve,
                    fade_out_curve,
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
                                ink,
                            );
                        }
                    }
                    // Fades are drawn as a fraction of the clip's frames, exactly as the waveform
                    // above spreads its frames across the width — the ramp ends over the sample it
                    // ends on. Handles only where there is room to grab them; the hit test reads
                    // the same constant. On the first pass and the last, which is where the fades
                    // sound: the renderer runs the joins between repeats flat.
                    if f32::from(content_width) > FADE_HANDLE_MIN_WIDTH && *length_frames > 0 {
                        let width = f32::from(content_bounds.size.width);
                        let fraction =
                            |frames: u64| width * (frames as f64 / *length_frames as f64) as f32;
                        let last =
                            clip.start + offset + span >= clip.start + clip.sounding_length();
                        paint::clip_fades(
                            window,
                            content_bounds,
                            if index == 0 {
                                fraction(*fade_in_frames)
                            } else {
                                0.0
                            },
                            if last {
                                fraction(*fade_out_frames)
                            } else {
                                0.0
                            },
                            (*fade_in_curve, *fade_out_curve),
                            theme,
                        );
                    }
                }
            });
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
                title_ink,
            );
        }

        // A clip that follows the tempo says so on its face, and says how far it is being
        // stretched: this is the only place on screen that admits the audio is going through a
        // stretcher rather than being played as it was recorded. On the name bar because that is
        // where a clip's facts are — beside the name and the gain, not over the waveform — and
        // painted *after* the name, so a name too long for its clip runs under the badge rather
        // than over it.
        if let Some(stretch) = clip.follows
            && f32::from(clip_bounds.size.width) > BADGE_MIN_WIDTH
        {
            let text = follow_badge(stretch);
            let padding = px(3.0);
            let inset = px(3.0);
            let height = TITLE_HEIGHT - inset;
            // Measured before either is drawn, so the pill fits the text rather than a guess at
            // how wide three digits and a per-cent sign are.
            let width = paint::measure_label(window, text.clone(), px(9.0));
            let pill = Bounds {
                origin: point(
                    clip_bounds.origin.x + clip_bounds.size.width - width - padding * 3.0,
                    clip_bounds.origin.y + inset / 2.0,
                ),
                size: size(width + padding * 2.0, height),
            };
            paint::rounded_rect(window, pill, height / 2.0, theme.accent_soft);
            paint::label_right(
                window,
                cx,
                point(
                    pill.origin.x + pill.size.width - padding,
                    clip_bounds.origin.y + px(1.5),
                ),
                text,
                px(9.0),
                theme.text_on(theme.accent_soft),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::clip_title_colors;
    use crate::theme::{SCHEMES, Theme, contrast_ratio};
    use auris_session::prelude::Color;

    #[test]
    fn clip_names_remain_readable_when_selected_or_muted() {
        for scheme in SCHEMES {
            let theme = Theme::from_scheme(scheme);
            for track in Color::PALETTE {
                for (selected, muted) in
                    [(false, false), (false, true), (true, false), (true, true)]
                {
                    let (fill, ink) =
                        clip_title_colors(&theme, theme.track_color(track.0), selected, muted);
                    assert_eq!(fill.a, 1.0, "the name has a stable, opaque backdrop");
                    assert!(
                        contrast_ratio(fill, ink) >= 4.5,
                        "{}: track {track:?}, selected={selected}, muted={muted} is unreadable",
                        scheme.name
                    );
                }
            }
        }
    }

    #[test]
    fn selecting_a_daylight_clip_changes_both_its_title_and_ink() {
        let theme = Theme::named("daylight");
        let track = theme.track_color(0x4f9dde);
        let (plain, plain_ink) = clip_title_colors(&theme, track, false, false);
        let (selected, selected_ink) = clip_title_colors(&theme, track, true, false);
        assert_ne!(plain, selected);
        assert_eq!(selected, theme.selection);
        assert!(plain_ink.l < 0.5);
        assert!(selected_ink.l > 0.5);
    }
}
