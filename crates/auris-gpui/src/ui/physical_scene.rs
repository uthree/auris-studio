//! Instrument-shaped projections of the captured mechanical state.

use super::{displayed_motion, paint};
use crate::theme::Theme;
use auris_i18n::Language;
use auris_session::{MotionFrame, MotionVoice, prelude::midi_name};
use gpui::{Bounds, Hsla, Pixels, Point, Window, point, px, size};

pub(super) struct MotionDrawing<'a> {
    pub frame: &'a MotionFrame,
    pub history: &'a [MotionFrame],
    pub gain: f32,
    pub theme: &'a Theme,
    pub held: &'a str,
    pub released: &'a str,
    pub language: Language,
    pub drums: bool,
    pub piano: bool,
    pub layout: Option<StringLayout>,
    pub effects: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StringBody {
    Acoustic,
    Electric,
    Bass,
    Violin,
}

#[derive(Clone, Copy)]
pub(super) struct StringLayout {
    tuning: &'static [f32],
    body: StringBody,
}

const GUITAR_TUNING: [f32; 6] = [40., 45., 50., 55., 59., 64.];
const BASS_TUNING: [f32; 4] = [28., 33., 38., 43.];
const VIOLIN_TUNING: [f32; 4] = [55., 62., 69., 76.];

pub(super) fn string_layout(id: &str) -> Option<StringLayout> {
    let (tuning, body): (&'static [f32], _) = match id {
        "auris.physical.guitar" => (&GUITAR_TUNING, StringBody::Acoustic),
        "auris.physical.electric_guitar" => (&GUITAR_TUNING, StringBody::Electric),
        "auris.physical.bass" => (&BASS_TUNING, StringBody::Bass),
        "auris.physical.violin" => (&VIOLIN_TUNING, StringBody::Violin),
        _ => return None,
    };
    Some(StringLayout { tuning, body })
}

/// Pitch and geometry are the available identity hints; never reuse a historical body.
pub(super) fn match_history(current: &MotionFrame, past: &MotionFrame) -> [Option<usize>; 4] {
    let mut used = [false; 4];
    std::array::from_fn(|index| {
        let voice = &current.voices[index];
        if voice.level <= 0. || !voice.pitch.is_finite() {
            return None;
        }
        let found = past
            .voices
            .iter()
            .enumerate()
            .filter(|(candidate, previous)| {
                !used[*candidate]
                    && previous.level > 0.
                    && previous.geometry.unwrap_or(past.geometry)
                        == voice.geometry.unwrap_or(current.geometry)
                    && (previous.pitch - voice.pitch).abs() <= 0.5
            })
            .min_by(|(_, a), (_, b)| {
                (a.pitch - voice.pitch)
                    .abs()
                    .total_cmp(&(b.pitch - voice.pitch).abs())
            })
            .map(|(candidate, _)| candidate);
        if let Some(candidate) = found {
            used[candidate] = true;
        }
        found
    })
}

fn string_assignments(frame: &MotionFrame, layout: StringLayout) -> [Option<usize>; 4] {
    let mut assignments = [None; 4];
    let mut used = [false; 6];
    let mut order = [0, 1, 2, 3];
    // The same chord keeps its placement when excitation recency reorders the telemetry.
    order.sort_by(|a, b| frame.voices[*b].pitch.total_cmp(&frame.voices[*a].pitch));
    for index in order {
        let voice = &frame.voices[index];
        if voice.level <= 0. || !voice.pitch.is_finite() {
            continue;
        }
        let playable = |string: usize| layout.tuning[string] <= voice.pitch + 0.01;
        let string = (0..layout.tuning.len())
            .rev()
            .find(|string| !used[*string] && playable(*string))
            .or_else(|| {
                (0..layout.tuning.len())
                    .rev()
                    .find(|string| playable(*string))
            })
            .unwrap_or(0);
        used[string] = true;
        assignments[index] = Some(string);
    }
    assignments
}

fn fret_position(semitones: f32) -> f32 {
    1.0 - 2_f32.powf(-semitones.clamp(0., 36.) / 12.)
}

/// Responsive drawing bounds reserve the same label lane at every font size.
fn string_stage(bounds: Bounds<Pixels>, rem: Pixels) -> Option<Bounds<Pixels>> {
    let inset = (rem * 0.5).min(bounds.size.width * 0.04);
    let labels = (rem * 7.0).min(bounds.size.width * 0.30);
    let vertical = (rem * 0.75).min(bounds.size.height * 0.08);
    let width = bounds.size.width - labels - inset;
    let height = bounds.size.height - vertical * 2.;
    if width <= px(0.) || height <= px(0.) {
        return None;
    }
    Some(Bounds::new(
        point(bounds.left() + labels, bounds.top() + vertical),
        size(width, height),
    ))
}

fn string_points(voice: &MotionVoice, lane: Bounds<Pixels>, gain: f32) -> [Point<Pixels>; 64] {
    std::array::from_fn(|index| {
        let displacement = if index == 0 || index == 63 {
            0.
        } else {
            displayed_motion(voice.points[index], gain)
        };
        point(
            lane.left() + lane.size.width * (index as f32 / 63.),
            lane.top() - lane.size.height * displacement,
        )
    })
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

fn instrument_body(
    window: &mut Window,
    stage: Bounds<Pixels>,
    layout: StringLayout,
    theme: &Theme,
) {
    let middle = stage.center();
    let outline: Vec<_> = (0..=64)
        .map(|index| {
            let angle = index as f32 / 64. * std::f32::consts::TAU;
            let waist = 0.68 + 0.32 * angle.sin().powi(2);
            point(
                stage.left() + stage.size.width * (0.84 + 0.16 * angle.cos() * waist),
                middle.y + stage.size.height * 0.49 * angle.sin(),
            )
        })
        .collect();
    let mut body = gpui::PathBuilder::fill();
    body.move_to(outline[0]);
    for at in &outline[1..] {
        body.line_to(*at);
    }
    body.close();
    if let Ok(path) = body.build() {
        window.paint_path(path, theme.surface_raised);
    }
    paint::polyline(window, &outline, px(1.), theme.border);
    paint::rounded_rect(
        window,
        Bounds::new(
            point(stage.left(), stage.top() + stage.size.height * 0.15),
            size(stage.size.width * 0.97 * 0.75, stage.size.height * 0.70),
        ),
        px(0.),
        theme.surface,
    );
    if layout.body == StringBody::Acoustic {
        let radius = (stage.size.width * 0.065).min(stage.size.height * 0.20);
        dot(
            window,
            point(stage.left() + stage.size.width * 0.82, middle.y),
            radius * 1.10,
            theme.border,
        );
        dot(
            window,
            point(stage.left() + stage.size.width * 0.82, middle.y),
            radius,
            theme.surface_sunken,
        );
    } else if matches!(layout.body, StringBody::Electric | StringBody::Bass) {
        for at in [0.81, 0.89] {
            paint::rounded_rect(
                window,
                Bounds::new(
                    point(
                        stage.left() + stage.size.width * at,
                        stage.top() + stage.size.height * 0.16,
                    ),
                    size(stage.size.width * 0.025, stage.size.height * 0.68),
                ),
                px(0.),
                theme.border,
            );
        }
    }
    if layout.body != StringBody::Violin {
        let neck = Bounds::new(
            point(stage.left(), stage.top() + stage.size.height * 0.15),
            size(stage.size.width * 0.97 * 0.75, stage.size.height * 0.70),
        );
        for fret in 0..=24 {
            paint::vline(
                window,
                neck,
                stage.left() + stage.size.width * 0.97 * fret_position(fret as f32),
                px(if fret == 0 { 2. } else { 1. }),
                theme.border,
            );
        }
        for fret in [3, 5, 7, 9, 12, 15, 17, 19, 21] {
            let x = stage.left()
                + stage.size.width
                    * 0.97
                    * (fret_position(fret as f32) + fret_position(fret as f32 - 1.))
                    * 0.5;
            let radius = stage.size.height * 0.012;
            if fret == 12 {
                for offset in [-0.11, 0.11] {
                    dot(
                        window,
                        point(x, middle.y + stage.size.height * offset),
                        radius,
                        theme.text_faint,
                    );
                }
            } else {
                dot(window, point(x, middle.y), radius, theme.text_faint);
            }
        }
    }
    let bridge = stage.left() + stage.size.width * 0.97;
    paint::rect(
        window,
        Bounds::new(
            point(bridge, stage.top() + stage.size.height * 0.13),
            size(stage.size.width * 0.015, stage.size.height * 0.74),
        ),
        theme.border,
    );
}

pub(super) fn draw_string_motion(
    bounds: Bounds<Pixels>,
    drawing: &MotionDrawing<'_>,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let Some(layout) = drawing.layout else {
        return;
    };
    let rem = window.rem_size();
    let Some(stage) = string_stage(bounds, rem) else {
        return;
    };
    let theme = drawing.theme;
    let assignments = string_assignments(drawing.frame, layout);
    let spacing = stage.size.height * 0.70 / (layout.tuning.len() - 1) as f32;
    let string_y = |string: usize| {
        stage.top() + stage.size.height * 0.15 + spacing * (layout.tuning.len() - 1 - string) as f32
    };
    let lane_for = |voice: &MotionVoice, string: usize| {
        let stopped = fret_position(voice.pitch - layout.tuning[string]);
        let scale = stage.size.width * 0.97;
        let left = stage.left() + scale * stopped;
        Bounds::new(
            point(left, string_y(string)),
            size(stage.left() + scale - left, spacing * 0.36),
        )
    };
    paint::clipped(window, bounds, |window| {
        instrument_body(window, stage, layout, theme);
        for (string, open) in layout.tuning.iter().enumerate() {
            let y = string_y(string);
            let color = theme.visualizer_color(string);
            paint::polyline(
                window,
                &[point(stage.left(), y), point(stage.right(), y)],
                px(0.7 + (layout.tuning.len() - 1 - string) as f32 * 0.16),
                Theme::translucent(color, 0.48),
            );
            paint::label(
                window,
                cx,
                point(bounds.left() + rem * 0.35, y - rem * 0.3),
                midi_name(*open as i32),
                rem * 0.65,
                theme.text_muted,
            );
        }
        // Paint old shapes from oldest to newest, then the solid current trace.
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
                let matches = match_history(drawing.frame, past);
                for (index, candidate) in matches.iter().enumerate() {
                    let (Some(candidate), Some(string)) = (*candidate, assignments[index]) else {
                        continue;
                    };
                    let previous = &past.voices[candidate];
                    let lane = lane_for(&drawing.frame.voices[index], string);
                    paint::polyline(
                        window,
                        &string_points(previous, lane, drawing.gain),
                        px(1.),
                        Theme::translucent(
                            theme.visualizer_color(string),
                            0.28 * (1. - age as f32 / 6.),
                        ),
                    );
                }
            }
        }
        for (index, assignment) in assignments.iter().enumerate() {
            let Some(string) = *assignment else {
                continue;
            };
            let voice = &drawing.frame.voices[index];
            let color = theme.visualizer_color(string);
            let lane = lane_for(voice, string);
            let points = string_points(voice, lane, drawing.gain);
            if drawing.effects {
                paint::polyline(window, &points, px(5.), Theme::translucent(color, 0.12));
            }
            paint::polyline(
                window,
                &points,
                px(if voice.held { 1.8 } else { 1.2 }),
                color,
            );
            // The stopping point is a schematic fingering; telemetry does not carry string IDs.
            dot(window, lane.origin, (spacing * 0.085).min(rem * 0.2), color);
            let contact = if voice.contact.is_finite() {
                voice.contact.clamp(0., 1.)
            } else {
                0.25
            };
            let sample = (contact * 63.).round() as usize;
            let at = points[sample];
            dot(window, at, (spacing * 0.075).min(rem * 0.18), theme.text);
            let energy = voice
                .points
                .iter()
                .map(|value| displayed_motion(*value, drawing.gain).abs())
                .fold(0., f32::max);
            if drawing.effects && energy > 0.01 {
                dot(
                    window,
                    at,
                    rem * (0.25 + energy * 0.30),
                    Theme::translucent(color, energy * 0.16),
                );
                for mote in 0..5 {
                    let angle =
                        mote as f32 * 2.4 + displayed_motion(voice.points[32], drawing.gain);
                    let radius = rem * energy * (0.4 + mote as f32 * 0.14);
                    dot(
                        window,
                        point(at.x + radius * angle.cos(), at.y + radius * angle.sin()),
                        rem * 0.055,
                        Theme::translucent(color, (0.8 - mote as f32 * 0.12) * energy),
                    );
                }
            }
            if layout.body == StringBody::Violin && voice.excitation > 0. {
                let excursion = displayed_motion(voice.excitation, 30.).abs() * spacing * 0.4;
                paint::polyline(
                    window,
                    &[
                        point(at.x - rem * 0.5, at.y - excursion),
                        point(at.x + rem * 0.5, at.y + excursion),
                    ],
                    px(2.),
                    theme.text_muted,
                );
            }
            paint::label(
                window,
                cx,
                point(bounds.left() + rem * 1.6, lane.top() - rem * 0.4),
                format!(
                    "{} {:+.0}¢",
                    midi_name(voice.pitch.round().clamp(0., 127.) as i32),
                    (voice.pitch - voice.pitch.round()) * 100.
                ),
                rem * 0.65,
                theme.text,
            );
            paint::label(
                window,
                cx,
                point(bounds.left() + rem * 1.6, lane.top() + rem * 0.2),
                if voice.held {
                    drawing.held
                } else {
                    drawing.released
                }
                .to_owned(),
                rem * 0.6,
                theme.text_muted,
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instrument_layouts_use_explicit_ids_and_familiar_tuning() {
        assert_eq!(
            string_layout("auris.physical.guitar").unwrap().tuning,
            &GUITAR_TUNING
        );
        assert_eq!(
            string_layout("auris.physical.electric_guitar")
                .unwrap()
                .tuning,
            &GUITAR_TUNING
        );
        assert_eq!(
            string_layout("auris.physical.bass").unwrap().tuning,
            &BASS_TUNING
        );
        assert_eq!(
            string_layout("auris.physical.violin").unwrap().tuning,
            &VIOLIN_TUNING
        );
        assert!(string_layout("auris.synth.bass").is_none());
    }

    #[test]
    fn fret_spacing_shortens_towards_the_bridge() {
        assert_eq!(fret_position(0.), 0.);
        assert_eq!(fret_position(12.), 0.5);
        assert_eq!(fret_position(24.), 0.75);
        assert!(fret_position(2.) - fret_position(1.) < fret_position(1.));
    }

    #[test]
    fn chord_strings_do_not_depend_on_excitation_order() {
        let layout = string_layout("auris.physical.guitar").unwrap();
        let mut frame = MotionFrame::default();
        for (voice, pitch) in frame.voices.iter_mut().zip([64., 60., 57., 52.]) {
            voice.pitch = pitch;
            voice.level = 1.;
        }
        assert_eq!(
            string_assignments(&frame, layout),
            [Some(5), Some(4), Some(3), Some(2)]
        );
        frame.voices.reverse();
        assert_eq!(
            string_assignments(&frame, layout),
            [Some(2), Some(3), Some(4), Some(5)]
        );
    }

    #[test]
    fn string_projection_preserves_observed_shape_scale_and_fixed_ends() {
        let lane = Bounds::new(point(px(10.), px(80.)), size(px(630.), px(20.)));
        let mut voice = MotionVoice::default();
        voice.points[0] = 9.;
        voice.points[63] = 9.;
        voice.points[7] = 0.3;
        voice.points[32] = -0.2;
        let points = string_points(&voice, lane, 1.);
        assert_eq!(points[0], point(px(10.), px(80.)));
        assert_eq!(points[63], point(px(640.), px(80.)));
        assert!((f32::from(points[7].x) - 80.).abs() < 1e-4);
        assert_eq!(points[7].y, px(74.));
        assert!((f32::from(points[32].x) - 330.).abs() < 1e-4);
        assert_eq!(points[32].y, px(84.));
        voice.points[7] *= 0.1;
        assert!((f32::from(string_points(&voice, lane, 1.)[7].y) - 79.4).abs() < 1e-5);
    }

    #[test]
    fn history_matching_is_distinct_and_does_not_join_unrelated_bodies() {
        let mut frame = MotionFrame::default();
        for voice in &mut frame.voices[..2] {
            voice.pitch = 60.;
            voice.level = 1.;
        }
        let mut past = frame;
        assert_eq!(match_history(&frame, &past), [Some(0), Some(1), None, None]);
        past.voices[0].geometry = Some(auris_session::MotionGeometry::Shell);
        past.voices[1].pitch = 61.;
        assert_eq!(match_history(&frame, &past), [None; 4]);
    }

    #[test]
    fn string_stage_stays_within_small_and_zoomed_canvases() {
        for width in [40., 320., 900.] {
            for rem in [12., 16., 32.] {
                let bounds = Bounds::new(point(px(10.), px(20.)), size(px(width), px(160.)));
                let stage = string_stage(bounds, px(rem)).unwrap();
                assert!(stage.size.width > px(0.) && stage.size.height > px(0.));
                assert!(stage.left() >= bounds.left() && stage.right() <= bounds.right());
                assert!(stage.top() >= bounds.top() && stage.bottom() <= bounds.bottom());
            }
        }
        assert!(
            string_stage(
                Bounds::new(point(px(0.), px(0.)), size(px(0.), px(0.))),
                px(16.)
            )
            .is_none()
        );
    }

    #[gpui::test]
    fn captured_string_models_render_in_a_small_utility_with_effects_toggle(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::{auxiliary_window::Surface, harness, ui::plugin_window::PluginSubject};
        let (app, cx) = harness::open(cx);
        for id in [
            "auris.physical.guitar",
            "auris.physical.electric_guitar",
            "auris.physical.bass",
            "auris.physical.violin",
        ] {
            app.update(cx, |app, cx| {
                let track = app.session.add_instrument_track("Strings", id).unwrap();
                app.open_plugin_window(PluginSubject::Instrument(track));
                app.poll_physical_view();
                app.theme = Theme::named("daylight");
                app.language = Language::Japanese;
                let mut frame = MotionFrame {
                    active: 2,
                    ..Default::default()
                };
                for (voice, pitch) in frame.voices.iter_mut().zip([60., 76.]) {
                    voice.pitch = pitch;
                    voice.level = 0.6;
                    voice.held = true;
                    voice.excitation = 0.1;
                    voice.points[7] = 0.025;
                    voice.points[32] = -0.02;
                }
                app.physical_view.effects_disabled = false;
                app.physical_view.history.clear();
                app.physical_view.frame = None;
                app.physical_view.accept_frame(frame);
                frame.voices[0].points[32] = 0.03;
                app.physical_view.accept_frame(frame);
                app.physical_view.frozen = true;
                cx.notify();
            });
            harness::paint(&app, cx);
            let handle = app.read_with(cx, |app, _| app.auxiliary_windows[&Surface::Plugin]);
            let mut utility = gpui::VisualTestContext::from_window(handle.into(), cx);
            harness::resize(&app, &mut utility, size(px(520.), px(480.)));
            assert!(
                utility
                    .debug_bounds("physical-motion-canvas")
                    .unwrap()
                    .size
                    .width
                    > px(0.)
            );
            harness::click("physical-effects", &mut utility);
            app.read_with(&utility, |app, _| {
                assert!(app.physical_view.effects_disabled);
                assert!(app.physical_view.frozen);
                assert_eq!(app.physical_view.history.len(), 2);
                assert_eq!(app.physical_view.frame.unwrap().voices[0].points[32], 0.03);
            });
        }
    }
}
