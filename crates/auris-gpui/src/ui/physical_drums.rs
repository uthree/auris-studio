//! Stable drum-kit projection for physical-model monitoring.

use super::{scene::MotionDrawing, surface_lines};
use crate::{theme::Theme, ui::paint};
use auris_i18n::{Key, Language};
use auris_session::{MotionGeometry, MotionVoice};
use gpui::{Bounds, Hsla, Pixels, Point, Window, point, px, size};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DrumPad {
    Kick,
    Snare,
    HiHat,
    HighTom,
    MidTom,
    FloorTom,
    Crash,
    Ride,
}

impl DrumPad {
    const ALL: [Self; 8] = [
        Self::Crash,
        Self::Ride,
        Self::HiHat,
        Self::HighTom,
        Self::MidTom,
        Self::FloorTom,
        Self::Snare,
        Self::Kick,
    ];
    fn from_pitch(pitch: f32) -> Self {
        match pitch.round().clamp(0., 127.) as u8 {
            35 | 36 => Self::Kick,
            37..=40 => Self::Snare,
            42 | 44 | 46 => Self::HiHat,
            49 | 52 | 55 | 57 => Self::Crash,
            51 | 53 | 59 => Self::Ride,
            41 | 43 => Self::FloorTom,
            45 | 47 => Self::MidTom,
            48 | 50 => Self::HighTom,
            _ => Self::MidTom,
        }
    }
    fn position(self) -> (f32, f32) {
        match self {
            Self::Crash => (0.16, 0.18),
            Self::Ride => (0.84, 0.18),
            Self::HiHat => (0.15, 0.47),
            Self::HighTom => (0.43, 0.32),
            Self::MidTom => (0.62, 0.32),
            Self::FloorTom => (0.82, 0.58),
            Self::Snare => (0.31, 0.67),
            Self::Kick => (0.54, 0.73),
        }
    }
    fn radius(self, stage: Bounds<Pixels>) -> Pixels {
        let base = stage.size.width.min(stage.size.height);
        match self {
            Self::Kick => base * 0.15,
            Self::Crash | Self::Ride => base * 0.11,
            Self::HiHat => base * 0.075,
            _ => base * 0.105,
        }
    }
    fn palette_slot(self) -> usize {
        match self {
            Self::Kick => 0,
            Self::Snare => 1,
            Self::HiHat => 2,
            Self::HighTom => 3,
            Self::MidTom => 4,
            Self::FloorTom => 5,
            Self::Crash => 6,
            Self::Ride => 7,
        }
    }
    fn geometry(self) -> MotionGeometry {
        match self {
            Self::HiHat | Self::Crash | Self::Ride => MotionGeometry::Plate,
            _ => MotionGeometry::Membrane,
        }
    }
    fn label(self, language: Language) -> &'static str {
        match self {
            Self::Kick => Key::RoleKick.get(language),
            Self::Snare => Key::RoleSnare.get(language),
            Self::HiHat => Key::RoleHat.get(language),
            Self::Crash => Key::RoleCrash.get(language),
            Self::Ride => Key::DrumRide.get(language),
            Self::HighTom => Key::DrumHighTom.get(language),
            Self::MidTom => Key::DrumMidTom.get(language),
            Self::FloorTom => Key::DrumFloorTom.get(language),
        }
    }
}

fn stage(bounds: Bounds<Pixels>, rem: Pixels) -> Option<Bounds<Pixels>> {
    let inset = (rem * 1.25).min(bounds.size.width * 0.08);
    let top = (rem * 0.8).min(bounds.size.height * 0.08);
    let size = size(
        bounds.size.width - inset * 2.,
        bounds.size.height - top * 2.,
    );
    (size.width > px(1.) && size.height > px(1.))
        .then(|| Bounds::new(point(bounds.left() + inset, bounds.top() + top), size))
}
fn center(stage: Bounds<Pixels>, pad: DrumPad) -> Point<Pixels> {
    let (x, y) = pad.position();
    point(
        stage.left() + stage.size.width * x,
        stage.top() + stage.size.height * y,
    )
}
fn ring(window: &mut Window, at: Point<Pixels>, radius: Pixels, color: Hsla, width: Pixels) {
    let points: Vec<_> = (0..=32)
        .map(|i| {
            let a = i as f32 / 32. * std::f32::consts::TAU;
            point(at.x + radius * a.cos(), at.y + radius * a.sin())
        })
        .collect();
    paint::polyline(window, &points, width, color);
}
fn resting_pad(window: &mut Window, stage: Bounds<Pixels>, pad: DrumPad, color: Hsla) {
    let at = center(stage, pad);
    let radius = pad.radius(stage);
    paint::rounded_rect(
        window,
        Bounds::new(
            point(at.x - radius, at.y - radius),
            size(radius * 2., radius * 2.),
        ),
        radius,
        Theme::translucent(color, 0.12),
    );
    ring(window, at, radius, Theme::translucent(color, 0.82), px(1.));
    ring(
        window,
        at,
        radius * 0.82,
        Theme::translucent(color, 0.28),
        px(1.),
    );
    if matches!(pad, DrumPad::Crash | DrumPad::Ride | DrumPad::HiHat) {
        for divisor in [3., 4., 5.] {
            ring(
                window,
                at,
                radius * divisor / 6.,
                Theme::translucent(color, 0.25),
                px(0.7),
            );
        }
        paint::polyline(
            window,
            &[
                point(at.x, at.y + radius),
                point(at.x, at.y + radius * 1.55),
            ],
            px(1.),
            color,
        );
    } else {
        for angle in [0., 0.5, 1.0, 1.5] {
            let a = angle * std::f32::consts::PI;
            paint::polyline(
                window,
                &[
                    point(at.x - radius * a.cos(), at.y - radius * a.sin()),
                    point(at.x + radius * a.cos(), at.y + radius * a.sin()),
                ],
                px(0.6),
                Theme::translucent(color, 0.26),
            );
        }
    }
}
fn voice_for_pad(frame: &auris_session::MotionFrame, pad: DrumPad) -> Option<&MotionVoice> {
    frame
        .voices
        .iter()
        .filter(|v| v.level.is_finite() && v.level > 0. && v.pitch.is_finite())
        .find(|v| {
            DrumPad::from_pitch(v.pitch) == pad
                && v.geometry.unwrap_or(frame.geometry) == pad.geometry()
        })
}
fn mesh_lines(
    pad: DrumPad,
    stage: Bounds<Pixels>,
    voice: &MotionVoice,
    gain: f32,
) -> Vec<Vec<Point<Pixels>>> {
    let at = center(stage, pad);
    let radius = pad.radius(stage);
    let sx = radius * 2.;
    let sy = radius * 2.;
    surface_lines(voice, gain)
        .into_iter()
        .map(|line| {
            line.into_iter()
                .map(|(x, y)| point(at.x + (x - 0.5) * sx, at.y + y * sy))
                .collect()
        })
        .collect()
}
fn draw_surface(
    window: &mut Window,
    pad: DrumPad,
    stage: Bounds<Pixels>,
    voice: &MotionVoice,
    gain: f32,
    color: Hsla,
) {
    for points in mesh_lines(pad, stage, voice, gain) {
        paint::polyline(window, &points, px(0.9), color);
    }
}
fn draw_history(
    window: &mut Window,
    pad: DrumPad,
    stage: Bounds<Pixels>,
    current: &MotionVoice,
    drawing: &MotionDrawing<'_>,
    color: Hsla,
) {
    for (age, past) in drawing.history.iter().rev().skip(1).take(5).enumerate() {
        let Some(previous) = past.voices.iter().find(|voice| {
            voice.level.is_finite()
                && voice.level > 0.
                && voice.pitch.is_finite()
                && DrumPad::from_pitch(voice.pitch) == pad
                && (voice.pitch - current.pitch).abs() <= 0.5
                && voice.geometry.unwrap_or(past.geometry)
                    == current.geometry.unwrap_or(drawing.frame.geometry)
        }) else {
            continue;
        };
        let alpha = 0.18 * (5 - age) as f32 / 5.;
        for points in mesh_lines(pad, stage, previous, drawing.gain) {
            paint::polyline(window, &points, px(0.7), Theme::translucent(color, alpha));
        }
    }
}

/// Draw the complete fixed kit and current-frame measured membrane/plate motion.
pub(super) fn draw_drum_motion(
    bounds: Bounds<Pixels>,
    drawing: &MotionDrawing<'_>,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let Some(stage) = stage(bounds, window.rem_size()) else {
        return;
    };
    paint::rect(window, bounds, drawing.theme.surface_sunken);
    paint::clipped(window, bounds, |window| {
        for pad in DrumPad::ALL {
            let color = drawing.theme.visualizer_color(pad.palette_slot());
            let at = center(stage, pad);
            let radius = pad.radius(stage);
            resting_pad(window, stage, pad, color);
            let label_size = window.rem_size() * 0.7;
            let label = pad.label(drawing.language);
            let label_width = paint::measure_label(window, label, label_size);
            paint::label(
                window,
                cx,
                point(at.x - label_width / 2., at.y + radius + px(3.)),
                label,
                label_size,
                drawing.theme.text_muted,
            );
            let idle = MotionVoice::default();
            draw_surface(
                window,
                pad,
                stage,
                &idle,
                drawing.gain,
                Theme::translucent(color, 0.32),
            );
            if let Some(voice) = voice_for_pad(drawing.frame, pad) {
                if drawing.effects {
                    draw_history(window, pad, stage, voice, drawing, color);
                }
                draw_surface(window, pad, stage, voice, drawing.gain, color);
                let contact = if voice.contact.is_finite() {
                    voice.contact.clamp(0., 1.)
                } else {
                    0.5
                };
                let strike = point(at.x + radius * (contact - 0.5), at.y - radius * 0.75);
                paint::rounded_rect(
                    window,
                    Bounds::new(
                        point(strike.x - px(2.), strike.y - px(2.)),
                        size(px(4.), px(4.)),
                    ),
                    px(2.),
                    drawing.theme.text,
                );
                if drawing.effects {
                    let energy = (voice
                        .points
                        .iter()
                        .map(|sample| super::displayed_motion(*sample, drawing.gain).powi(2))
                        .sum::<f32>()
                        / 64.)
                        .sqrt();
                    ring(
                        window,
                        at,
                        radius * (1. + energy * 0.18),
                        Theme::translucent(color, 0.28),
                        px(1.5),
                    );
                    if energy > 0.025 {
                        for (index, line) in mesh_lines(pad, stage, voice, drawing.gain)
                            .iter()
                            .enumerate()
                            .step_by(3)
                        {
                            let Some(at) = line.get(line.len() / 2) else {
                                continue;
                            };
                            let sparkle = window.rem_size() * (0.04 + energy * 0.06);
                            paint::rounded_rect(
                                window,
                                Bounds::new(
                                    point(at.x - sparkle, at.y - sparkle),
                                    size(sparkle * 2., sparkle * 2.),
                                ),
                                sparkle,
                                Theme::translucent(
                                    drawing.theme.visualizer_color(pad.palette_slot() + index),
                                    0.65,
                                ),
                            );
                        }
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::super::{displayed_motion, drum_geometry};
    use super::*;
    #[test]
    fn gm_mapping_covers_every_preview_key() {
        for pitch in [36, 38, 42, 46, 49, 51, 41, 43, 45, 47, 48, 50] {
            assert!(DrumPad::ALL.contains(&DrumPad::from_pitch(pitch as f32)));
        }
        assert_eq!(DrumPad::from_pitch(37.), DrumPad::Snare);
        assert_eq!(DrumPad::from_pitch(52.), DrumPad::Crash);
        for pitch in [
            35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 55, 57, 59,
        ] {
            assert_eq!(
                DrumPad::from_pitch(f32::from(pitch)).geometry(),
                drum_geometry(pitch)
            );
        }
    }
    #[test]
    fn captured_mesh_keeps_absolute_amplitude_and_ignores_invalid_voices() {
        let stage = Bounds::new(point(px(0.), px(0.)), size(px(600.), px(300.)));
        let mut voice = MotionVoice::default();
        let idle = mesh_lines(DrumPad::Snare, stage, &voice, 6.);
        voice.points.fill(0.01);
        let small = mesh_lines(DrumPad::Snare, stage, &voice, 6.);
        let large = mesh_lines(DrumPad::Snare, stage, &voice, 24.);
        assert!(
            (f32::from(large[0][0].y - idle[0][0].y)
                - 4. * f32::from(small[0][0].y - idle[0][0].y))
            .abs()
                < 1e-4
        );
        let mut frame = auris_session::MotionFrame::default();
        frame.voices[0] = MotionVoice {
            pitch: 38.,
            level: 0.5,
            geometry: Some(MotionGeometry::Membrane),
            ..Default::default()
        };
        assert!(voice_for_pad(&frame, DrumPad::Snare).is_some());
        frame.voices.reverse();
        assert_eq!(voice_for_pad(&frame, DrumPad::Snare).unwrap().pitch, 38.);
        frame.voices[3].level = f32::NAN;
        assert!(voice_for_pad(&frame, DrumPad::Snare).is_none());
    }
    #[test]
    fn surface_mesh_is_real_motion_and_clamped() {
        let mut voice = MotionVoice::default();
        let idle = surface_lines(&voice, 6.);
        voice.points.fill(100.);
        let loud = surface_lines(&voice, 6.);
        assert_ne!(idle, loud);
        assert!(
            loud.iter()
                .flatten()
                .all(|(_, y)| y.is_finite() && *y <= 0.9)
        );
        assert_eq!(displayed_motion(f32::NAN, 6.), 0.);
    }
    #[test]
    fn kit_positions_and_radii_are_inside_stage() {
        let stage = Bounds::new(point(px(10.), px(20.)), size(px(600.), px(300.)));
        let centers: Vec<_> = DrumPad::ALL.map(|pad| center(stage, pad)).to_vec();
        assert!(centers.iter().all(|at| at.x >= stage.left()
            && at.x <= stage.right()
            && at.y >= stage.top()
            && at.y <= stage.bottom()));
        assert!(DrumPad::ALL.iter().all(|pad| pad.radius(stage) > px(0.)));
    }

    #[test]
    fn stage_is_bounded_at_supported_sizes() {
        for width in [80., 320., 900.] {
            for height in [80., 180., 480.] {
                for rem in [12., 16., 32.] {
                    let bounds = Bounds::new(point(px(0.), px(0.)), size(px(width), px(height)));
                    let Some(stage) = stage(bounds, px(rem)) else {
                        continue;
                    };
                    assert!(stage.left() >= bounds.left() && stage.right() <= bounds.right());
                    assert!(stage.top() >= bounds.top() && stage.bottom() <= bounds.bottom());
                }
            }
        }
    }
}
