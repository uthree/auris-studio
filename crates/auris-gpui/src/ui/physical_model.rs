//! Live mechanical projections beside the physical instrument's parameter editor.

use super::{
    paint,
    plugin_window::PluginSubject,
    widgets::{ButtonState, ButtonStyle, button, button_enabled},
};
use crate::{app::AurisApp, theme::Theme};
use auris_i18n::{Key, Language, messages};
use auris_session::{MotionFrame, MotionGeometry, MotionVoice, prelude::*};
use gpui::{AnyElement, Bounds, Context, Pixels, Window, canvas, div, point, prelude::*, px, size};

#[derive(Debug)]
pub(crate) struct PhysicalView {
    source: Option<(TrackId, String)>,
    pub(crate) frame: Option<MotionFrame>,
    pub(crate) frozen: bool,
    pub(crate) playing: bool,
    pub(crate) pitch: u8,
    drum_preview: bool,
    gain: f32,
}
impl Default for PhysicalView {
    fn default() -> Self {
        Self {
            source: None,
            frame: None,
            frozen: false,
            playing: false,
            pitch: 60,
            drum_preview: false,
            gain: 24.0,
        }
    }
}

impl AurisApp {
    pub(crate) fn close_physical_view(&mut self) {
        self.release_physical_preview();
        self.session.watch_instrument_motion(None);
        self.physical_view = PhysicalView {
            pitch: self.physical_view.pitch,
            drum_preview: self.physical_view.drum_preview,
            gain: self.physical_view.gain,
            ..Default::default()
        };
    }
    fn release_physical_preview(&mut self) {
        if self.physical_view.playing {
            if let Some((track, _)) = self.physical_view.source.as_ref() {
                self.session.note_off(*track, self.physical_view.pitch);
            }
            self.physical_view.playing = false;
        }
    }
    pub(crate) fn poll_physical_view(&mut self) {
        let source = self.plugin_window.and_then(|editor| {
            let PluginSubject::Instrument(track) = editor.subject else {
                return None;
            };
            let (id, _) = self.resolve_plugin(editor.subject)?;
            self.session
                .has_instrument_motion(track)
                .then_some((track, id))
        });
        if source != self.physical_view.source {
            self.close_physical_view();
            if let Some((_, id)) = &source {
                let drums = is_drum_model(id);
                if drums != self.physical_view.drum_preview {
                    self.physical_view.pitch = if drums { 36 } else { 60 };
                }
                self.physical_view.drum_preview = drums;
            }
            self.physical_view.source = source;
        }
        let track = self.physical_view.source.as_ref().map(|(track, _)| *track);
        self.session
            .watch_instrument_motion(track.filter(|_| !self.physical_view.frozen));
        if !self.physical_view.frozen
            && let Some(frame) = track.and_then(|track| self.session.instrument_motion(track))
        {
            self.physical_view.frame = Some(frame);
        }
    }
    pub(crate) fn toggle_physical_freeze(&mut self) {
        if self
            .physical_view
            .frame
            .is_some_and(|frame| frame.active > 0)
        {
            self.physical_view.frozen = !self.physical_view.frozen;
            self.poll_physical_view();
        }
    }
    fn toggle_physical_preview(&mut self) {
        if self.physical_view.playing {
            self.release_physical_preview();
        } else if let Some((track, _)) = &self.physical_view.source {
            self.session.note_on(*track, self.physical_view.pitch, 0.75);
            if self.physical_view.drum_preview {
                // A drum preview is a strike, so the same button can immediately strike again.
                self.session.note_off(*track, self.physical_view.pitch);
            } else {
                self.physical_view.playing = true;
            }
        }
    }
    fn transpose_physical_preview(&mut self, delta: i16) {
        let playing = self.physical_view.playing;
        self.release_physical_preview();
        self.physical_view.pitch = if self.physical_view.drum_preview {
            next_drum_preview(self.physical_view.pitch, delta > 0)
        } else {
            (i16::from(self.physical_view.pitch) + delta).clamp(24, 96) as u8
        };
        if playing {
            self.toggle_physical_preview();
        }
    }
    pub(crate) fn physical_model_display(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.poll_physical_view();
        let (track, id) = self.physical_view.source.as_ref()?;
        let bowed = id == "auris.physical.violin";
        let piano = id == "auris.physical.piano";
        let drums = is_drum_model(id);
        let track = *track;
        let frame = self.physical_view.frame.unwrap_or_default();
        let theme = self.theme.clone();
        let frozen = self.physical_view.frozen;
        let playing = self.physical_view.playing;
        let language = self.language;
        let note = if drums {
            drum_name(self.physical_view.pitch, language).to_owned()
        } else {
            midi_name(i32::from(self.physical_view.pitch))
        };
        let mut idle = frame;
        if frame.active == 0 {
            if drums {
                idle.geometry = drum_geometry(self.physical_view.pitch);
            }
            for descriptor in self.session.instrument_descriptors(track).iter() {
                if descriptor.key == "position" {
                    idle.voices[0].contact = self.session.param_value(
                        ParamTarget::Instrument {
                            track,
                            param: descriptor.id,
                        },
                        descriptor,
                    );
                }
            }
        }
        let drawing_theme = theme.clone();
        let gain = self.physical_view.gain;
        let held = self.t(Key::PhysicalHeld);
        let released = self.t(Key::PhysicalReleased);
        let caption = if frame.active == 0 {
            self.t(Key::PhysicalWaiting).to_owned()
        } else {
            messages::physical_activity(
                self.language,
                frame.active,
                frame.expression,
                bowed.then_some(frame.pressure),
                piano.then_some(frame.pedal),
            )
        };
        Some(
            div()
                .id("physical-model")
                .debug_selector(|| "physical-model".into())
                .w_full()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap_2()
                .pb_2()
                .border_b_1()
                .border_color(theme.border)
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .text_color(theme.text)
                                .child(self.t(Key::PhysicalMotion)),
                        )
                        .child(button(
                            "physical-gain",
                            format!("{}: {gain:.0}×", self.t(Key::VisualizerDisplayGain)),
                            ButtonStyle::Ghost,
                            false,
                            theme.accent,
                            &theme,
                            cx.listener(|this, _, _, cx| {
                                this.physical_view.gain = match this.physical_view.gain as u32 {
                                    6 => 24.,
                                    24 => 96.,
                                    _ => 6.,
                                };
                                cx.notify();
                            }),
                        ))
                        .child(button_enabled(
                            "physical-freeze",
                            self.t(if frozen {
                                Key::PhysicalResume
                            } else {
                                Key::VisualizerFreeze
                            }),
                            ButtonStyle::Normal,
                            ButtonState::available(frozen, frame.active > 0),
                            theme.accent,
                            &theme,
                            cx.listener(|this, _, window, cx| {
                                this.toggle_physical_freeze();
                                // Resuming an ended hit disables this button. Keep keyboard
                                // input on the window instead of a removed focus target.
                                if this
                                    .physical_view
                                    .frame
                                    .is_none_or(|frame| frame.active == 0)
                                {
                                    window.focus(&this.focus);
                                }
                                cx.notify();
                            }),
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(button(
                            "physical-octave-down",
                            self.t(if drums {
                                Key::PhysicalPreviousPad
                            } else {
                                Key::PhysicalLower
                            }),
                            ButtonStyle::Ghost,
                            false,
                            theme.accent,
                            &theme,
                            cx.listener(|this, _, _, cx| {
                                this.transpose_physical_preview(-12);
                                cx.notify();
                            }),
                        ))
                        .child(button(
                            "physical-play",
                            messages::physical_preview(self.language, &note, playing),
                            ButtonStyle::Normal,
                            playing,
                            theme.accent,
                            &theme,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_physical_preview();
                                cx.notify();
                            }),
                        ))
                        .child(button(
                            "physical-octave-up",
                            self.t(if drums {
                                Key::PhysicalNextPad
                            } else {
                                Key::PhysicalHigher
                            }),
                            ButtonStyle::Ghost,
                            false,
                            theme.accent,
                            &theme,
                            cx.listener(|this, _, _, cx| {
                                this.transpose_physical_preview(12);
                                cx.notify();
                            }),
                        )),
                )
                .child(
                    div()
                        .id("physical-motion-canvas")
                        .debug_selector(|| "physical-motion-canvas".into())
                        .h_48()
                        .flex_shrink_0()
                        .w_full()
                        .bg(theme.surface_sunken)
                        .child(
                            canvas(
                                move |bounds, _, _| bounds,
                                move |bounds, _, window, cx| {
                                    draw_motion(
                                        bounds,
                                        &idle,
                                        gain,
                                        &drawing_theme,
                                        (held, released, language, drums),
                                        window,
                                        cx,
                                    )
                                },
                            )
                            .size_full(),
                        ),
                )
                .child(div().text_xs().text_color(theme.text_muted).child(caption))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.text_faint)
                        .child(self.t(Key::PhysicalProjection)),
                )
                .into_any_element(),
        )
    }
}

fn is_drum_model(id: &str) -> bool {
    id == "auris.synth.drumkit"
}

fn next_drum_preview(pitch: u8, forward: bool) -> u8 {
    const KEYS: [u8; 12] = [36, 38, 42, 46, 49, 51, 41, 43, 45, 47, 48, 50];
    let current = KEYS.iter().position(|key| *key == pitch).unwrap_or(0);
    KEYS[(current + if forward { 1 } else { KEYS.len() - 1 }) % KEYS.len()]
}

fn drum_geometry(pitch: u8) -> MotionGeometry {
    if matches!(pitch, 42 | 44 | 46 | 49 | 51 | 52 | 53 | 55 | 57 | 59) {
        MotionGeometry::Plate
    } else {
        MotionGeometry::Membrane
    }
}

fn drum_name(pitch: u8, language: Language) -> &'static str {
    let key = match pitch {
        35 | 36 => Key::RoleKick,
        37..=40 => Key::RoleSnare,
        42 | 44 => Key::DrumClosedHat,
        46 => Key::DrumOpenHat,
        49 | 52 | 55 | 57 => Key::RoleCrash,
        51 | 53 | 59 => Key::DrumRide,
        _ => Key::DrumTom,
    };
    key.get(language)
}

fn surface_lines(voice: &MotionVoice, gain: f32) -> Vec<Vec<(f32, f32)>> {
    let mut lines = Vec::new();
    for transpose in [false, true] {
        for row in 0..8 {
            let mut line = Vec::new();
            for column in 0..8 {
                let (x, y) = if transpose {
                    (row, column)
                } else {
                    (column, row)
                };
                let horizontal = x as f32 / 3.5 - 1.0;
                let vertical = y as f32 / 3.5 - 1.0;
                if horizontal.hypot(vertical) < 1.0 {
                    line.push((
                        0.5 + horizontal * 0.42,
                        vertical * 0.5 + displayed_motion(voice.points[y * 8 + x], gain) * 0.4,
                    ));
                }
            }
            if line.len() > 1 {
                lines.push(line);
            }
        }
    }
    lines
}

fn displayed_motion(value: f32, gain: f32) -> f32 {
    if value.is_finite() {
        (value * gain).clamp(-1., 1.)
    } else {
        0.0
    }
}
fn body_points(voice: &MotionVoice, geometry: MotionGeometry, gain: f32) -> Vec<(f32, f32)> {
    voice
        .points
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let x = index as f32 / (voice.points.len() - 1) as f32;
            if geometry == MotionGeometry::Shell {
                let angle = x * std::f32::consts::TAU;
                let radius = 0.65 + displayed_motion(*value, gain) * 0.2;
                (0.5 + angle.cos() * radius * 0.22, angle.sin() * radius)
            } else {
                (x, displayed_motion(*value, gain))
            }
        })
        .collect()
}
fn draw_motion(
    bounds: Bounds<Pixels>,
    frame: &MotionFrame,
    gain: f32,
    theme: &Theme,
    labels: (&str, &str, Language, bool),
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let rows = frame
        .voices
        .iter()
        .filter(|voice| voice.level > 0.)
        .count()
        .max(1);
    let rem = window.rem_size();
    let inset = rem * 0.75;
    let lane = bounds.size.height / rows as f32;
    paint::clipped(window, bounds, |window| {
        for (index, voice) in frame.voices.iter().take(rows).enumerate() {
            let center = bounds.top() + lane * (index as f32 + 0.5);
            let left = bounds.left() + bounds.size.width * 0.22;
            let width = bounds.size.width * 0.60;
            let height = (lane * 0.35).min(rem * 2.0);
            let geometry = voice.geometry.unwrap_or(frame.geometry);
            let surface = matches!(geometry, MotionGeometry::Membrane | MotionGeometry::Plate);
            let label = if voice.level > 0. {
                if labels.3 {
                    format!(
                        "{} {}",
                        drum_name(voice.pitch.round() as u8, labels.2),
                        voice.pitch.round() as u8
                    )
                } else {
                    format!(
                        "{} {:+.0}¢",
                        midi_name(voice.pitch.round().clamp(0., 127.) as i32),
                        (voice.pitch - voice.pitch.round()) * 100.
                    )
                }
            } else {
                String::new()
            };
            paint::label(
                window,
                cx,
                point(bounds.left() + inset, center - rem),
                label,
                rem * 0.75,
                theme.text,
            );
            if voice.level > 0. {
                paint::label(
                    window,
                    cx,
                    point(bounds.left() + inset, center),
                    if voice.held { labels.0 } else { labels.1 }.to_owned(),
                    rem * 0.7,
                    theme.text_muted,
                );
            }
            paint::polyline(
                window,
                &[point(left, center), point(left + width, center)],
                px(1.),
                theme.border,
            );
            let points: Vec<_> = if surface {
                (0..=64)
                    .map(|index| {
                        let angle = index as f32 / 64.0 * std::f32::consts::TAU;
                        (0.5 + angle.cos() * 0.42, angle.sin() * 0.5)
                    })
                    .collect()
            } else {
                body_points(voice, geometry, gain)
            }
            .into_iter()
            .map(|(x, y)| point(left + width * x, center - height * y))
            .collect();
            paint::polyline(window, &points, px(1.5), theme.accent);
            if surface {
                // A tilted wire mesh exposes nodal lines without inventing motion between frames.
                for line in surface_lines(voice, gain) {
                    let points: Vec<_> = line
                        .into_iter()
                        .map(|(x, y)| point(left + width * x, center - height * y))
                        .collect();
                    paint::polyline(window, &points, px(1.), theme.accent);
                }
            }
            if geometry != MotionGeometry::Shell {
                let contact = voice.contact.clamp(0., 1.);
                let x = left
                    + width
                        * if surface {
                            0.5 + contact * 0.42
                        } else {
                            contact
                        };
                paint::rect(
                    window,
                    Bounds::new(
                        point(x - rem * 0.125, center - height - rem * 0.5),
                        size(rem * 0.25, rem * 0.5),
                    ),
                    theme.text_muted,
                );
                if voice.excitation > 0. {
                    let length = (voice.excitation * 30.).clamp(0., 1.) * height;
                    paint::polyline(
                        window,
                        &[
                            point(x - rem * 0.5, center - length),
                            point(x + rem * 0.5, center + length),
                        ],
                        px(2.),
                        theme.text,
                    );
                }
            }
            for (mode, value) in voice.modes.iter().enumerate() {
                let h = displayed_motion(*value, gain).abs() * height * 2.;
                let spacing = bounds.size.width * 0.10 / 8.0;
                let x = bounds.left() + bounds.size.width * 0.87 + spacing * mode as f32;
                paint::rect(
                    window,
                    Bounds::new(point(x, center + height - h), size(spacing * 0.65, h)),
                    theme.text_muted,
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drum_preview_covers_the_kit_and_surface_displacement_retains_its_scale() {
        let mut pitch = 36;
        let mut pitches = Vec::new();
        for _ in 0..12 {
            pitches.push(pitch);
            pitch = next_drum_preview(pitch, true);
        }
        assert_eq!(pitch, 36);
        assert_eq!(next_drum_preview(36, false), 50);
        assert!(pitches.contains(&51) && pitches.contains(&46));
        assert_eq!(drum_name(38, Language::Japanese), "スネア");
        let mut voice = MotionVoice::default();
        voice.points.fill(0.01);
        let small = surface_lines(&voice, 6.0);
        let large = surface_lines(&voice, 24.0);
        let idle = surface_lines(&MotionVoice::default(), 6.0);
        assert!((large[0][0].1 - idle[0][0].1 - 4.0 * (small[0][0].1 - idle[0][0].1)).abs() < 1e-6);
        assert!(
            small
                .iter()
                .flatten()
                .all(|(x, y)| x.is_finite() && y.is_finite())
        );
    }

    #[gpui::test]
    fn drum_editor_exposes_repeatable_strikes_pad_selection_freeze_and_escape(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::{auxiliary_window::Surface, harness};
        use gpui::VisualTestContext;
        let (app, cx) = harness::open(cx);
        let track = app.update(cx, |app, cx| {
            let track = app.session.add_default_drum_track("Kit").unwrap();
            app.open_plugin_window(PluginSubject::Instrument(track));
            cx.notify();
            track
        });
        harness::paint(&app, cx);
        let handle = app.read_with(cx, |app, _| app.auxiliary_windows[&Surface::Plugin]);
        let mut utility = VisualTestContext::from_window(handle.into(), cx);
        utility.update(|window, cx| window.focus(&app.read(cx).focus));
        assert!(utility.debug_bounds("physical-motion-canvas").is_some());
        app.read_with(&utility, |app, _| assert_eq!(app.physical_view.pitch, 36));
        harness::click("physical-play", &mut utility);
        harness::click("physical-play", &mut utility);
        app.read_with(&utility, |app, _| assert!(!app.physical_view.playing));
        harness::click("physical-octave-up", &mut utility);
        app.read_with(&utility, |app, _| assert_eq!(app.physical_view.pitch, 38));
        harness::click("physical-octave-down", &mut utility);
        app.read_with(&utility, |app, _| assert_eq!(app.physical_view.pitch, 36));
        // Supply an observed state: the window harness deliberately has no audio callback.
        app.update(&mut utility, |app, cx| {
            let mut frame = MotionFrame {
                geometry: MotionGeometry::Membrane,
                active: 2,
                ..Default::default()
            };
            frame.voices[0].geometry = Some(MotionGeometry::Membrane);
            frame.voices[0].pitch = 36.0;
            frame.voices[0].level = 0.5;
            frame.voices[1].geometry = Some(MotionGeometry::Plate);
            frame.voices[1].pitch = 49.0;
            frame.voices[1].level = 0.3;
            app.physical_view.frame = Some(frame);
            app.toggle_physical_freeze();
            cx.notify();
        });
        harness::paint(&app, &mut utility);
        harness::click("physical-freeze", &mut utility);
        app.read_with(&utility, |app, _| assert!(!app.physical_view.frozen));
        utility.simulate_keystrokes("escape");
        harness::paint(&app, cx);
        app.read_with(cx, |app, _| {
            assert!(app.plugin_window.is_none());
            assert!(app.physical_view.source.is_none());
            assert!(app.session.has_instrument_motion(track));
        });
    }

    #[test]
    fn projection_retains_fixed_ends_and_decay_without_frame_normalization() {
        let mut voice = MotionVoice::default();
        for (index, value) in voice.points.iter_mut().enumerate() {
            *value = (std::f32::consts::PI * index as f32 / 63.).sin() * 0.1;
        }
        voice.points[63] = 0.;
        let loud = body_points(&voice, MotionGeometry::String, 6.0);
        voice.points.iter_mut().for_each(|value| *value *= 0.1);
        let quiet = body_points(&voice, MotionGeometry::String, 6.0);
        assert_eq!(loud[0], (0., 0.));
        assert_eq!(loud[63], (1., 0.));
        assert!((quiet[32].1 / loud[32].1 - 0.1).abs() < 1e-5);
        assert_eq!(displayed_motion(f32::NAN, 6.0), 0.);
    }

    #[gpui::test]
    fn physical_editor_supports_preview_freeze_parameter_undo_and_escape(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::{auxiliary_window::Surface, harness};
        use gpui::VisualTestContext;
        let (app, cx) = harness::open(cx);
        let (track, target, descriptor, before) = app.update(cx, |app, cx| {
            let track = app
                .session
                .add_instrument_track("Violin", "auris.physical.violin")
                .unwrap();
            let descriptor = app
                .session
                .instrument_descriptors(track)
                .iter()
                .find(|d| d.key == "bow_response")
                .unwrap()
                .clone();
            let target = ParamTarget::Instrument {
                track,
                param: descriptor.id,
            };
            let before = app.session.param_value(target, &descriptor);
            app.open_plugin_window(PluginSubject::Instrument(track));
            cx.notify();
            (track, target, descriptor, before)
        });
        harness::paint(&app, cx);
        let handle = app.read_with(cx, |app, _| app.auxiliary_windows[&Surface::Plugin]);
        let mut utility = VisualTestContext::from_window(handle.into(), cx);
        assert!(utility.debug_bounds("physical-motion-canvas").is_some());
        harness::click("physical-gain", &mut utility);
        app.read_with(&utility, |app, _| assert_eq!(app.physical_view.gain, 96.));
        // Mouse activation need not leave keyboard focus on the button. Walk the real
        // tab order from the window root: Close, then display gain.
        utility.update(|window, cx| window.focus(&app.read(cx).focus));
        utility.simulate_keystrokes("tab");
        utility.simulate_keystrokes("tab");
        utility.simulate_keystrokes("enter");
        utility.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("enter").unwrap(),
        });
        app.read_with(&utility, |app, _| assert_eq!(app.physical_view.gain, 6.));
        harness::resize(&app, &mut utility, size(px(520.), px(480.)));
        harness::click("physical-play", &mut utility);
        app.read_with(&utility, |app, _| assert!(app.physical_view.playing));
        harness::click("physical-octave-up", &mut utility);
        app.read_with(&utility, |app, _| assert_eq!(app.physical_view.pitch, 72));
        // No audio device exists in this harness: supply a captured-frame fixture for the
        // presentation-only resume gesture. DSP-to-monitor integration has numerical tests.
        app.update(&mut utility, |app, cx| {
            app.physical_view.frame = Some(MotionFrame {
                active: 1,
                ..Default::default()
            });
            app.toggle_physical_freeze();
            cx.notify();
        });
        app.read_with(&utility, |app, _| assert!(app.physical_view.frozen));
        harness::click("physical-freeze", &mut utility);
        app.read_with(&utility, |app, _| assert!(!app.physical_view.frozen));
        let body = utility.debug_bounds("pw-body").unwrap();
        utility.simulate_event(gpui::ScrollWheelEvent {
            position: body.center(),
            delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-10000.))),
            ..Default::default()
        });
        harness::paint(&app, &mut utility);
        let selector = Box::leak(format!("pw-inst-param-{}", descriptor.id.0).into_boxed_str());
        let bounds = utility.debug_bounds(selector).unwrap();
        harness::drag(
            &mut utility,
            bounds.center(),
            bounds.center() + point(px(60.), px(0.)),
        );
        app.read_with(&utility, |app, _| {
            assert_ne!(app.session.param_value(target, &descriptor), before)
        });
        utility.dispatch_action(crate::actions::Undo);
        app.read_with(&utility, |app, _| {
            assert_eq!(app.session.param_value(target, &descriptor), before)
        });
        utility.simulate_keystrokes("escape");
        harness::paint(&app, cx);
        app.read_with(cx, |app, _| {
            assert!(!app.physical_view.playing);
            assert!(app.plugin_window.is_none());
            assert!(app.session.has_instrument_motion(track));
        });
    }

    #[gpui::test]
    fn nonphysical_instrument_replaces_the_motion_surface_and_releases_preview(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::harness;
        let (app, cx) = harness::open(cx);
        app.update(cx, |app, cx| {
            let physical = app
                .session
                .add_instrument_track("Guitar", "auris.physical.guitar")
                .unwrap();
            app.open_plugin_window(PluginSubject::Instrument(physical));
            app.poll_physical_view();
            app.toggle_physical_preview();
            assert!(app.physical_view.playing);
            let plain = app
                .session
                .add_instrument_track("Chip", "auris.synth.chiptune")
                .unwrap();
            app.open_plugin_window(PluginSubject::Instrument(plain));
            app.poll_physical_view();
            assert!(!app.physical_view.playing);
            assert!(app.physical_view.source.is_none());
            cx.notify();
        });
        harness::paint(&app, cx);
    }
}
