//! Live mechanical projections beside the physical instrument's parameter editor.

use super::{
    paint,
    plugin_window::PluginSubject,
    widgets::{ButtonState, ButtonStyle, button, button_enabled},
};
use crate::{app::AurisApp, theme::Theme};
use auris_i18n::{Key, messages};
use auris_session::{MotionFrame, MotionGeometry, MotionVoice, prelude::*};
use gpui::{AnyElement, Bounds, Context, Pixels, Window, canvas, div, point, prelude::*, px, size};

#[derive(Debug)]
pub(crate) struct PhysicalView {
    source: Option<(TrackId, String)>,
    pub(crate) frame: Option<MotionFrame>,
    pub(crate) frozen: bool,
    pub(crate) playing: bool,
    pub(crate) pitch: u8,
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
            self.physical_view.playing = true;
        }
    }
    fn transpose_physical_preview(&mut self, delta: i16) {
        let playing = self.physical_view.playing;
        self.release_physical_preview();
        self.physical_view.pitch =
            (i16::from(self.physical_view.pitch) + delta).clamp(24, 96) as u8;
        if playing {
            self.toggle_physical_preview();
        }
    }
    pub(crate) fn physical_model_display(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.poll_physical_view();
        let (track, id) = self.physical_view.source.as_ref()?;
        let bowed = id == "auris.physical.violin";
        let piano = id == "auris.physical.piano";
        let track = *track;
        let frame = self.physical_view.frame.unwrap_or_default();
        let theme = self.theme.clone();
        let frozen = self.physical_view.frozen;
        let playing = self.physical_view.playing;
        let note = midi_name(i32::from(self.physical_view.pitch));
        let mut idle = frame;
        if frame.active == 0 {
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
                            cx.listener(|this, _, _, cx| {
                                this.toggle_physical_freeze();
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
                            self.t(Key::PhysicalLower),
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
                            self.t(Key::PhysicalHigher),
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
                                        (held, released),
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
    labels: (&str, &str),
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
            let label = if voice.level > 0. {
                format!(
                    "{} {:+.0}¢",
                    midi_name(voice.pitch.round().clamp(0., 127.) as i32),
                    (voice.pitch - voice.pitch.round()) * 100.
                )
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
            let points: Vec<_> = body_points(voice, frame.geometry, gain)
                .into_iter()
                .map(|(x, y)| point(left + width * x, center - height * y))
                .collect();
            paint::polyline(window, &points, px(1.5), theme.accent);
            if frame.geometry != MotionGeometry::Shell {
                let x = left + width * voice.contact.clamp(0., 1.);
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
