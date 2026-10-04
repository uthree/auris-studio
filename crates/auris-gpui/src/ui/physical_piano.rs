//! Keyboard-ordered projections of the piano's captured string motion.

use super::scene::{MotionDrawing, match_history};
use super::{displayed_motion, paint};
use crate::theme::Theme;
use auris_i18n::Key;
use auris_session::{MotionVoice, prelude::midi_name};
use gpui::{Bounds, Hsla, Pixels, Point, Window, point, px, size};

const FIRST_KEY: u8 = 21;
const LAST_KEY: u8 = 108;

#[derive(Clone, Copy)]
struct PianoStage {
    header: Bounds<Pixels>,
    strings: Bounds<Pixels>,
    keyboard: Bounds<Pixels>,
    footer: Bounds<Pixels>,
}

fn piano_stage(bounds: Bounds<Pixels>, rem: Pixels) -> Option<PianoStage> {
    if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
        return None;
    }
    let inset = (rem * 0.75).min(bounds.size.width * 0.06);
    let gap = (rem * 0.35).min(bounds.size.height * 0.025);
    let header_height = (rem * 2.6).min(bounds.size.height * 0.15);
    let keyboard_height = (rem * 3.).min(bounds.size.height * 0.24);
    let footer_height = (rem * 1.3).min(bounds.size.height * 0.09);
    let left = bounds.left() + inset;
    let width = bounds.size.width - inset * 2.;
    let keyboard_top = bounds.bottom() - footer_height - keyboard_height - gap;
    let strings_top = bounds.top() + header_height + gap;
    Some(PianoStage {
        header: Bounds::new(point(left, bounds.top()), size(width, header_height)),
        strings: Bounds::new(
            point(left, strings_top),
            size(width, keyboard_top - strings_top),
        ),
        keyboard: Bounds::new(point(left, keyboard_top), size(width, keyboard_height)),
        footer: Bounds::new(
            point(left, bounds.bottom() - footer_height),
            size(width, footer_height),
        ),
    })
}

fn is_black_key(key: u8) -> bool {
    matches!(key % 12, 1 | 3 | 6 | 8 | 10)
}

fn key_position(key: u8, left: Pixels, white_width: Pixels) -> Pixels {
    let white_before = (FIRST_KEY..key)
        .filter(|candidate| !is_black_key(*candidate))
        .count();
    left + white_width * (white_before as f32 + if is_black_key(key) { 0. } else { 0.5 })
}

fn piano_pitch(pitch: f32) -> f32 {
    if pitch.is_finite() {
        pitch.clamp(f32::from(FIRST_KEY), f32::from(LAST_KEY))
    } else {
        f32::from(FIRST_KEY)
    }
}

fn pitch_position(pitch: f32, left: Pixels, white_width: Pixels) -> Pixels {
    let pitch = piano_pitch(pitch);
    let low = pitch.floor() as u8;
    let high = low.saturating_add(1).min(LAST_KEY);
    let low_x = key_position(low, left, white_width);
    let high_x = key_position(high, left, white_width);
    low_x + (high_x - low_x) * (pitch - f32::from(low))
}

fn pitch_color(theme: &Theme, pitch: f32) -> Hsla {
    theme.visualizer_gradient((piano_pitch(pitch) - f32::from(FIRST_KEY)) / 87.)
}

fn key_bounds(stage: PianoStage, key: u8) -> Bounds<Pixels> {
    let white_width = stage.keyboard.size.width / 52.;
    let black = is_black_key(key);
    let width = white_width * if black { 0.60 } else { 1. };
    Bounds::new(
        point(
            key_position(key, stage.keyboard.left(), white_width) - width / 2.,
            stage.keyboard.top(),
        ),
        size(
            width,
            stage.keyboard.size.height * if black { 0.62 } else { 1. },
        ),
    )
}

#[derive(Clone, Copy)]
struct PianoString {
    x: Pixels,
    top: Pixels,
    bottom: Pixels,
    amplitude: Pixels,
}

fn string_lane(stage: PianoStage, pitch: f32, rem: Pixels) -> PianoString {
    let width = stage.strings.size.width / 52.;
    let fraction = (piano_pitch(pitch) - f32::from(FIRST_KEY)) / 87.;
    PianoString {
        x: pitch_position(pitch, stage.strings.left(), width),
        // Length is schematic; the monitor reports a normalized spatial projection.
        top: stage.strings.top() + stage.strings.size.height * fraction.powf(0.8) * 0.64,
        bottom: stage.strings.bottom(),
        amplitude: (width * 0.95).min(rem * 0.65),
    }
}

fn trace_points(voice: &MotionVoice, lane: PianoString, gain: f32) -> [Point<Pixels>; 64] {
    std::array::from_fn(|index| {
        let displacement = if index == 0 || index == 63 {
            0.
        } else {
            displayed_motion(voice.points[index], gain)
        };
        point(
            lane.x + lane.amplitude * displacement,
            lane.top + (lane.bottom - lane.top) * (index as f32 / 63.),
        )
    })
}

fn active(voice: &MotionVoice) -> bool {
    voice.level.is_finite() && voice.level > 0. && voice.pitch.is_finite()
}

fn dot(window: &mut Window, at: Point<Pixels>, radius: Pixels, color: Hsla) {
    paint::rounded_rect(
        window,
        Bounds::new(
            point(at.x - radius, at.y - radius),
            size(radius * 2., radius * 2.),
        ),
        radius,
        color,
    );
}

fn keyboard(window: &mut Window, stage: PianoStage, drawing: &MotionDrawing<'_>) {
    for black in [false, true] {
        for key in (FIRST_KEY..=LAST_KEY).filter(|key| is_black_key(*key) == black) {
            let bounds = key_bounds(stage, key);
            paint::rect(
                window,
                bounds,
                if black {
                    drawing.theme.key_black
                } else {
                    drawing.theme.key_white
                },
            );
            paint::rounded_outline(window, bounds, px(0.), px(0.6), drawing.theme.border);
            // The nearest key is schematic under pitch bend; the string moves continuously.
            if let Some(voice) = drawing
                .frame
                .voices
                .iter()
                .find(|voice| active(voice) && piano_pitch(voice.pitch).round() as u8 == key)
            {
                let color = pitch_color(drawing.theme, voice.pitch);
                paint::rect(window, bounds, Theme::translucent(color, 0.45));
                paint::rect(
                    window,
                    Bounds::new(
                        point(bounds.left(), bounds.bottom() - bounds.size.height * 0.13),
                        size(bounds.size.width, bounds.size.height * 0.13),
                    ),
                    color,
                );
            }
        }
    }
    paint::rounded_outline(window, stage.keyboard, px(0.), px(1.), drawing.theme.border);
}

fn readout(
    window: &mut Window,
    cx: &mut gpui::App,
    stage: PianoStage,
    drawing: &MotionDrawing<'_>,
) {
    let rem = window.rem_size();
    let font = rem * 0.75;
    let mut voices: Vec<_> = drawing
        .frame
        .voices
        .iter()
        .filter(|voice| active(voice))
        .collect();
    voices.sort_by(|a, b| a.pitch.total_cmp(&b.pitch));
    if stage.header.size.height >= font * paint::LINE_HEIGHT * 2. {
        for (slot, voice) in voices.into_iter().enumerate() {
            let column = slot % 2;
            let row = slot / 2;
            let label = format!(
                "{} {:+.0}¢ · {}",
                midi_name(voice.pitch.round() as i32),
                (voice.pitch - voice.pitch.round()) * 100.,
                if voice.held {
                    drawing.held
                } else {
                    drawing.released
                },
            );
            let origin = point(
                stage.header.left() + stage.header.size.width * (column as f32 / 2.),
                stage.header.top() + rem * 0.15 + font * paint::LINE_HEIGHT * row as f32,
            );
            let column_bounds = Bounds::new(
                origin,
                size(stage.header.size.width / 2., font * paint::LINE_HEIGHT),
            );
            paint::clipped(window, column_bounds, |window| {
                paint::label(window, cx, origin, label, font, drawing.theme.text);
            });
        }
    }
    if stage.footer.size.height >= font * paint::LINE_HEIGHT {
        paint::label_right(
            window,
            cx,
            point(stage.footer.right(), stage.footer.top()),
            format!(
                "{} · {}",
                Key::PhysicalPedal.get(drawing.language),
                if drawing.frame.pedal {
                    Key::ValueOn
                } else {
                    Key::ValueOff
                }
                .get(drawing.language),
            ),
            font,
            if drawing.frame.pedal {
                drawing.theme.playing
            } else {
                drawing.theme.text_muted
            },
        );
    }
}

/// Paints a persistent keyboard and one normalized string projection per key.
pub(super) fn draw_piano_motion(
    bounds: Bounds<Pixels>,
    drawing: &MotionDrawing<'_>,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let rem = window.rem_size();
    let Some(stage) = piano_stage(bounds, rem) else {
        return;
    };
    paint::clipped(window, bounds, |window| {
        paint::rounded_rect(
            window,
            stage.strings,
            rem * 0.3,
            drawing.theme.surface_raised,
        );
        for key in FIRST_KEY..=LAST_KEY {
            let lane = string_lane(stage, f32::from(key), rem);
            let color = pitch_color(drawing.theme, f32::from(key));
            paint::polyline(
                window,
                &[point(lane.x, lane.top), point(lane.x, lane.bottom)],
                px(if is_black_key(key) { 0.7 } else { 1. }),
                Theme::translucent(color, 0.28),
            );
            dot(
                window,
                point(lane.x, lane.top),
                px(1.),
                drawing.theme.border,
            );
        }
        paint::hline(
            window,
            stage.strings,
            stage.strings.bottom() - px(2.),
            drawing.theme.border,
        );
        keyboard(window, stage, drawing);

        if drawing.effects {
            for age in (1..=5).rev() {
                let Some(past) = drawing
                    .history
                    .len()
                    .checked_sub(age + 1)
                    .and_then(|at| drawing.history.get(at))
                else {
                    continue;
                };
                for candidate in match_history(drawing.frame, past).into_iter().flatten() {
                    let voice = &past.voices[candidate];
                    if !active(voice) {
                        continue;
                    }
                    let points =
                        trace_points(voice, string_lane(stage, voice.pitch, rem), drawing.gain);
                    paint::polyline(
                        window,
                        &points,
                        px(1.),
                        Theme::translucent(
                            pitch_color(drawing.theme, voice.pitch),
                            0.16 * (6 - age) as f32 / 6.,
                        ),
                    );
                }
            }
        }
        for voice in drawing.frame.voices.iter().filter(|voice| active(voice)) {
            let lane = string_lane(stage, voice.pitch, rem);
            let color = pitch_color(drawing.theme, voice.pitch);
            let points = trace_points(voice, lane, drawing.gain);
            let energy = (voice
                .points
                .iter()
                .map(|sample| displayed_motion(*sample, drawing.gain).powi(2))
                .sum::<f32>()
                / 64.)
                .sqrt();
            if drawing.effects {
                paint::polyline(
                    window,
                    &points,
                    px(5.),
                    Theme::translucent(color, 0.10 + energy * 0.12),
                );
                for index in [12, 26, 41, 53] {
                    let amount = displayed_motion(voice.points[index], drawing.gain).abs();
                    if amount > 0.025 {
                        dot(
                            window,
                            points[index],
                            rem * (0.05 + amount * 0.08),
                            Theme::translucent(color, 0.40 + amount * 0.4),
                        );
                    }
                }
            }
            paint::polyline(window, &points, px(1.6), color);
            let contact = if voice.contact.is_finite() {
                voice.contact.clamp(0., 1.)
            } else {
                0.5
            };
            let at = point(lane.x, lane.top + (lane.bottom - lane.top) * contact);
            if drawing.effects {
                dot(
                    window,
                    at,
                    rem * (0.25 + energy * 0.45),
                    Theme::translucent(color, 0.14),
                );
            }
            dot(
                window,
                at,
                (rem * 0.12).min(stage.keyboard.size.width / 104.),
                color,
            );
        }
        // Octave labels sit on white key fronts, clear of strings and black keys.
        if stage.keyboard.size.width / 52. >= rem * 0.38 {
            for key in (FIRST_KEY..=LAST_KEY).filter(|key| key % 12 == 0) {
                let key_rect = key_bounds(stage, key);
                let text = midi_name(i32::from(key));
                let font = rem * 0.65;
                let width = paint::measure_label(window, text.clone(), font);
                paint::label(
                    window,
                    cx,
                    point(
                        (key_rect.center().x - width / 2.)
                            .clamp(stage.keyboard.left(), stage.keyboard.right() - width),
                        key_rect.bottom() - font * paint::LINE_HEIGHT,
                    ),
                    text,
                    font,
                    drawing.theme.key_black,
                );
            }
        }
        readout(window, cx, stage, drawing);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_centres_follow_white_keys_and_interleave_black_keys() {
        assert_eq!(
            (FIRST_KEY..=LAST_KEY)
                .filter(|key| !is_black_key(*key))
                .count(),
            52
        );
        for (pitch, x) in [(21, 5.), (22, 10.), (23, 15.), (24, 25.), (108, 515.)] {
            assert_eq!(key_position(pitch, px(0.), px(10.)), px(x));
        }
        assert_eq!(
            pitch_position(60.5, px(0.), px(10.)),
            (key_position(60, px(0.), px(10.)) + key_position(61, px(0.), px(10.))) / 2.
        );
        assert_eq!(pitch_position(f32::NAN, px(0.), px(10.)), px(5.));
        assert_eq!(pitch_position(127., px(0.), px(10.)), px(515.));
    }

    #[test]
    fn stage_reserves_readouts_and_keys_at_small_sizes_and_large_fonts() {
        for width in [40., 320., 900.] {
            for rem in [12., 16., 32.] {
                let bounds = Bounds::new(point(px(10.), px(20.)), size(px(width), px(160.)));
                let stage = piano_stage(bounds, px(rem)).unwrap();
                for region in [stage.header, stage.strings, stage.keyboard, stage.footer] {
                    assert!(region.size.width > px(0.) && region.size.height > px(0.));
                    assert!(region.left() >= bounds.left() && region.right() <= bounds.right());
                    assert!(region.top() >= bounds.top() && region.bottom() <= bounds.bottom());
                }
                assert!(stage.header.bottom() < stage.strings.top());
                assert_eq!(stage.strings.bottom(), stage.keyboard.top());
                assert!(stage.keyboard.bottom() < stage.footer.top());
                assert!(
                    string_lane(stage, 21., px(rem)).top < string_lane(stage, 108., px(rem)).top
                );
                let black = key_bounds(stage, 61);
                assert!(black.size.width < key_bounds(stage, 60).size.width);
                assert!(black.size.height < stage.keyboard.size.height);
            }
        }
        assert!(
            piano_stage(
                Bounds::new(point(px(0.), px(0.)), size(px(0.), px(0.))),
                px(16.)
            )
            .is_none()
        );
    }

    #[test]
    fn captured_points_keep_scale_fixed_ends_and_finite_coordinates() {
        let lane = PianoString {
            x: px(20.),
            top: px(10.),
            bottom: px(110.),
            amplitude: px(5.),
        };
        let mut voice = MotionVoice::default();
        voice.points[0] = 9.;
        voice.points[1] = 0.4;
        voice.points[32] = -0.3;
        voice.points[63] = 9.;
        let points = trace_points(&voice, lane, 2.);
        assert_eq!(points[0], point(px(20.), px(10.)));
        assert_eq!(points[63], point(px(20.), px(110.)));
        assert_eq!(points[1].x, px(24.));
        assert_eq!(points[32].x, px(17.));
        voice.points[1] *= 0.1;
        assert!((f32::from(trace_points(&voice, lane, 2.)[1].x) - 20.4).abs() < 1e-5);
        voice.points[17] = f32::NAN;
        assert_eq!(trace_points(&voice, lane, 2.)[17].x, lane.x);
        voice.level = f32::NAN;
        assert!(!active(&voice));
    }
}
