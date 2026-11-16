//! A resonating effect, live, beside its parameters: the strings it hangs beside a track, or
//! the box it puts the track through.
//!
//! The same pictures as a physical instrument's, for the same kind of thing. Sympathetic
//! strings are a [`StringLane`] each, labelled with the note they are tuned to and lit by how
//! strongly they ring, under the chord or scale they are following and over a stem per string of
//! how loud it is. A body is the plate [`super::physical_stage`] draws for an instrument's
//! soundboard, turning under the pointer, over the curve of what it does to a signal.
//!
//! Nothing here can be touched: an effect has no note to play. What it shows is what the track
//! going through it is doing to it. Everything a picture decides is a free function, because the
//! view cannot be asserted on and those can.

use atelier::plot::{LinePlot, Plot3d, Series, Shading};
use atelier::viz::StringLane;
use atelier::{
    Badge, Button, Card, ControlSize, Label, LevelMeter, SectionHeader, StatusDot, TextColor,
    TextSize,
};
use auris_i18n::Key;
use auris_session::prelude::*;
use auris_session::{
    RESONANCE_HIGH_HZ, RESONANCE_LOW_HZ, RESONANCE_MODES, RESONANCE_RESPONSE, ResonanceFrame,
    ResonanceKind, response_hz,
};
use gpui::{AnyElement, Context, div, prelude::*, px};

use super::physical_model::voice_color;
use super::physical_stage::{PLATE, body_surface};
use super::plugin_window::PluginSubject;
use crate::{app::AurisApp, theme::Theme};

/// The quietest a string's level is drawn, in decibels re unity.
const FLOOR_DB: f32 = -72.0;
/// How far above and below the body's average its response curve is drawn, in decibels.
const RESPONSE_RANGE: (f32, f32) = (-30.0, 18.0);

/// What the window is showing of a resonating effect.
#[derive(Debug)]
pub(crate) struct ResonanceView {
    /// The effect slot being drawn, if the open window holds a resonator.
    pub(crate) slot: Option<EffectSlotId>,
    /// Its newest frame.
    pub(crate) frame: Option<ResonanceFrame>,
    /// How much the motion is enlarged to be seen.
    pub(crate) gain: f32,
    /// The angle the body is seen from: azimuth and elevation, in degrees.
    pub(crate) orbit: (f32, f32),
}

impl Default for ResonanceView {
    fn default() -> Self {
        Self {
            slot: None,
            frame: None,
            gain: 24.0,
            orbit: (-32., 34.),
        }
    }
}

/// A value as it is drawn, from -1 to 1, at the display's `gain`.
fn shown(value: f32, gain: f32) -> f32 {
    if value.is_finite() {
        (value * gain).clamp(-1., 1.)
    } else {
        0.
    }
}

/// One string's shape along its length, as a lane draws it.
pub(crate) fn string_points(points: &[f32], gain: f32) -> Vec<f32> {
    points.iter().map(|point| shown(*point, gain)).collect()
}

/// A string's level as a height from zero at [`FLOOR_DB`] to one at unity.
pub(crate) fn level_height(level: f32) -> f32 {
    if level.partial_cmp(&0.) != Some(std::cmp::Ordering::Greater) || !level.is_finite() {
        return 0.;
    }
    (1. - 20. * level.log10() / FLOOR_DB).clamp(0., 1.)
}

/// How much wider than deep the plate a body is drawn as, by its choice on the effect.
pub(crate) fn body_aspect(choice: f32) -> f32 {
    match choice.round() as i32 {
        // A guitar's waist, a violin's and a cello's longer corpus, a grand's harp shape, a
        // dulcimer's trapezoid and a cabinet's square front.
        0 => 1.25,
        1 | 2 => 1.7,
        3 => 1.6,
        4 => 2.2,
        _ => 1.0,
    }
}

/// The response curve on a logarithmic frequency axis, as `(log10 Hz, dB)`.
pub(crate) fn response_points(response: &[f32; RESONANCE_RESPONSE]) -> Vec<(f32, f32)> {
    response
        .iter()
        .enumerate()
        .map(|(index, db)| {
            let db = if db.is_finite() {
                *db
            } else {
                RESPONSE_RANGE.0
            };
            (
                response_hz(index).log10(),
                db.clamp(RESPONSE_RANGE.0, RESPONSE_RANGE.1),
            )
        })
        .collect()
}

/// The pitch classes a bank is tuned to, root first and then upwards: "D · D F# A".
pub(crate) fn tuning_text(root: i32, classes: u16) -> Option<String> {
    if !(0..12).contains(&root) || classes & 0x0FFF == 0 {
        return None;
    }
    let names: Vec<&str> = (0..12)
        .map(|step| (root + step) % 12)
        .filter(|class| classes & (1 << class) != 0)
        .map(|class| PitchClass::new(class).sharp_name())
        .collect();
    Some(format!(
        "{} · {}",
        PitchClass::new(root).sharp_name(),
        names.join(" ")
    ))
}

impl AurisApp {
    /// Follows the open plugin window: watches the resonator it holds, if any, and takes its
    /// newest frame. Asked on every tick, like the physical view.
    pub(crate) fn poll_resonance_view(&mut self) {
        let slot = self.plugin_window.and_then(|window| match window.subject {
            PluginSubject::Insert { slot, .. } if self.session.has_effect_resonance(slot) => {
                Some(slot)
            }
            _ => None,
        });
        if slot != self.resonance_view.slot {
            self.resonance_view.slot = slot;
            self.resonance_view.frame = None;
            self.session.watch_effect_resonance(slot);
        }
        if let Some(frame) = slot.and_then(|slot| self.session.effect_resonance(slot)) {
            self.resonance_view.frame = Some(frame);
        }
    }

    /// The live picture of the resonator `subject` names, or `None` when it is not one.
    pub(crate) fn resonance_display(
        &mut self,
        subject: PluginSubject,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let PluginSubject::Insert { slot, .. } = subject else {
            return None;
        };
        self.poll_resonance_view();
        if self.resonance_view.slot != Some(slot) {
            return None;
        }
        let body = self.resonance_body_choice(subject, slot);
        let frame = self.resonance_view.frame.unwrap_or(ResonanceFrame {
            kind: if body.is_some() {
                ResonanceKind::Body
            } else {
                ResonanceKind::Strings
            },
            ..ResonanceFrame::default()
        });
        let theme = self.theme.clone();
        let card = match frame.kind {
            ResonanceKind::Strings => self.resonance_strings(&frame, &theme, cx),
            ResonanceKind::Body => self.resonance_body(&frame, body.unwrap_or(0.), &theme, cx),
        };
        Some(
            div()
                .id("resonance-view")
                .debug_selector(|| "resonance-view".into())
                .flex()
                .flex_col()
                .w_full()
                .gap(theme.atelier.spacing.lg)
                .child(card)
                .into_any_element(),
        )
    }

    /// The body choice of a body effect, or `None` for an effect without one.
    fn resonance_body_choice(&mut self, subject: PluginSubject, slot: EffectSlotId) -> Option<f32> {
        let descriptors = self.session.effect_descriptors(slot);
        let descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor.key == "body")?;
        Some(
            self.session
                .param_value(subject.param_target(descriptor.id), descriptor),
        )
    }

    fn resonance_badge(&self, frame: &ResonanceFrame) -> Badge {
        Badge::new(self.t(if frame.input > 1.0e-4 || frame.output > 1.0e-4 {
            Key::PhysicalLive
        } else {
            Key::VisualizerWaiting
        }))
    }

    fn resonance_gain_button(&self, cx: &mut Context<Self>) -> Button {
        Button::new(
            "resonance-gain",
            format!(
                "{}: {:.0}×",
                self.t(Key::VisualizerDisplayGain),
                self.resonance_view.gain
            ),
        )
        .ghost()
        .size(ControlSize::Small)
        .on_click(cx.listener(|this, _, _, cx| {
            this.resonance_view.gain = match this.resonance_view.gain as u32 {
                6 => 24.,
                24 => 96.,
                _ => 6.,
            };
            cx.notify();
        }))
    }

    /// The meters for what goes in and what the resonator gives back.
    fn resonance_meters(&self, frame: &ResonanceFrame, theme: &Theme) -> AnyElement {
        let spacing = theme.atelier.spacing;
        let meter = |label: &'static str, level: f32| {
            div()
                .flex()
                .items_center()
                .gap(spacing.sm)
                .flex_1()
                .child(
                    Label::new(label)
                        .size(TextSize::XSmall)
                        .color(TextColor::Muted),
                )
                .child(
                    div()
                        .flex_1()
                        .h(px(6.))
                        .child(LevelMeter::new(level.clamp(0., 1.)).thickness(px(6.))),
                )
        };
        div()
            .flex()
            .gap(spacing.lg)
            .child(meter(self.t(Key::ResonanceInput), frame.input))
            .child(meter(self.t(Key::ResonanceOutput), frame.output))
            .into_any_element()
    }

    fn resonance_strings(
        &self,
        frame: &ResonanceFrame,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tokens = &theme.atelier;
        let spacing = tokens.spacing;
        let gain = self.resonance_view.gain;
        let count = frame.string_count.min(frame.strings.len());
        let loudest = frame.strings[..count]
            .iter()
            .map(|string| string.level)
            .fold(0., f32::max);
        let tuning = tuning_text(frame.root, frame.classes)
            .unwrap_or_else(|| self.t(Key::ResonanceNoTuning).to_owned());
        let mut lanes = div()
            .id("resonance-strings")
            .debug_selector(|| "resonance-strings".into())
            .flex()
            .flex_col()
            .gap(spacing.xxs);
        for string in &frame.strings[..count] {
            let color = voice_color(theme, string.pitch);
            let ringing = string.level > 1.0e-4;
            let mut lane = StringLane::new(string_points(&string.points, gain)).color(color);
            if !ringing {
                lane = lane.inactive();
            }
            lanes = lanes.child(
                div()
                    .flex()
                    .items_center()
                    .gap(spacing.md)
                    .h(gpui::rems(1.75))
                    .child(
                        div()
                            .w(gpui::rems(3.5))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap(spacing.xs)
                            .child(StatusDot::new(if ringing {
                                color
                            } else {
                                tokens.colors.text_faint
                            }))
                            .child(
                                Label::new(midi_name(string.pitch.round().clamp(0., 127.) as i32))
                                    .mono()
                                    .size(TextSize::XSmall),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .rounded(tokens.shape.radius_sm)
                            .bg(tokens.colors.sunken)
                            .child(lane),
                    )
                    .child(
                        div().h_full().py(spacing.xxs).child(
                            LevelMeter::new(if loudest > 0. {
                                (string.level / loudest).clamp(0., 1.)
                            } else {
                                0.
                            })
                            .thickness(px(4.)),
                        ),
                    ),
            );
        }
        let (low, high) = frame.strings[..count]
            .iter()
            .fold((f32::MAX, f32::MIN), |(low, high), string| {
                (low.min(string.pitch), high.max(string.pitch))
            });
        let (low, high) = if count == 0 {
            (36., 84.)
        } else {
            (low - 1., high + 1.)
        };
        let mut levels = LinePlot::new().x_range(low, high).y_range(0., 1.);
        for string in &frame.strings[..count] {
            levels = levels.series(
                Series::stems(vec![(string.pitch, level_height(string.level))])
                    .color(voice_color(theme, string.pitch)),
            );
        }
        Card::new()
            .id("resonance-strings-card")
            .debug_selector(|| "resonance-strings-card".into())
            .w_full()
            .flex_shrink_0()
            .p(spacing.lg)
            .gap(spacing.md)
            .child(
                SectionHeader::new(self.t(Key::ResonanceStrings))
                    .description(self.t(Key::ResonanceStringsHint))
                    .action(self.resonance_badge(frame)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(spacing.sm)
                    .child(self.resonance_gain_button(cx))
                    .child(
                        Label::new(format!("{}: {tuning}", self.t(Key::ResonanceTunedTo)))
                            .size(TextSize::Small)
                            .mono(),
                    ),
            )
            .child(lanes)
            .child(
                Label::new(self.t(Key::ResonanceStringLevels))
                    .size(TextSize::XSmall)
                    .color(TextColor::Muted),
            )
            .child(
                div()
                    .h(gpui::rems(6.))
                    .rounded(tokens.shape.radius_sm)
                    .bg(tokens.colors.sunken)
                    .p(spacing.xs)
                    .child(levels),
            )
            .child(self.resonance_meters(frame, theme))
            .children((frame.input <= 1.0e-4).then(|| {
                Label::new(self.t(Key::ResonanceWaiting))
                    .size(TextSize::XSmall)
                    .color(TextColor::Faint)
            }))
            .into_any_element()
    }

    fn resonance_body(
        &self,
        frame: &ResonanceFrame,
        choice: f32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tokens = &theme.atelier;
        let spacing = tokens.spacing;
        let aspect = body_aspect(choice);
        let (azimuth, elevation) = self.resonance_view.orbit;
        let (columns, rows) = PLATE;
        let modes: [f32; RESONANCE_MODES] = frame.modes;
        let plate = Plot3d::new()
            .id("resonance-body-plot")
            .shading(Shading::Smooth)
            .view(azimuth, elevation)
            .on_orbit({
                let app = cx.entity().downgrade();
                move |angles, _, cx| {
                    if let Some(app) = app.upgrade() {
                        app.update(cx, |this, cx| {
                            this.resonance_view.orbit = *angles;
                            cx.notify();
                        });
                    }
                }
            })
            .surface(
                columns,
                rows,
                body_surface(&modes, aspect, self.resonance_view.gain),
                (0., aspect),
                (0., 1.),
            )
            .z_range(-1., 1.);
        let response = LinePlot::new()
            .x_range(RESONANCE_LOW_HZ.log10(), RESONANCE_HIGH_HZ.log10())
            .y_range(RESPONSE_RANGE.0, RESPONSE_RANGE.1)
            .series(
                Series::line(vec![
                    (RESONANCE_LOW_HZ.log10(), 0.),
                    (RESONANCE_HIGH_HZ.log10(), 0.),
                ])
                .color(tokens.colors.text_faint)
                .faint(),
            )
            .series(Series::line(response_points(&frame.response)).color(tokens.colors.accent));
        let well = |height: f32, plot: AnyElement| {
            div()
                .h(gpui::rems(height))
                .rounded(tokens.shape.radius_sm)
                .bg(tokens.colors.sunken)
                .overflow_hidden()
                .p(spacing.xs)
                .child(plot)
        };
        Card::new()
            .id("resonance-body-card")
            .debug_selector(|| "resonance-body-card".into())
            .w_full()
            .flex_shrink_0()
            .p(spacing.lg)
            .gap(spacing.md)
            .child(
                SectionHeader::new(self.t(Key::ResonanceBody))
                    .description(self.t(Key::PhysicalResonanceHint))
                    .action(self.resonance_badge(frame)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(spacing.sm)
                    .child(self.resonance_gain_button(cx)),
            )
            .child(well(
                15.,
                div()
                    .size_full()
                    .cursor_grab()
                    .child(plate)
                    .into_any_element(),
            ))
            .child(
                Label::new(self.t(Key::ResonanceBodyCaption))
                    .size(TextSize::XSmall)
                    .color(TextColor::Faint),
            )
            .child(
                Label::new(self.t(Key::ResonanceResponse))
                    .size(TextSize::XSmall)
                    .color(TextColor::Muted),
            )
            .child(well(7., response.into_any_element()))
            .child(self.resonance_meters(frame, theme))
            .children((frame.input <= 1.0e-4).then(|| {
                Label::new(self.t(Key::ResonanceWaiting))
                    .size(TextSize::XSmall)
                    .color(TextColor::Faint)
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tuning_is_named_from_its_root_upwards() {
        assert_eq!(
            tuning_text(2, 0b0000_0010_0000_0100 | (1 << 6)).as_deref(),
            Some("D · D F# A")
        );
        assert_eq!(tuning_text(-1, 0b1), None);
        assert_eq!(tuning_text(0, 0), None);
    }

    #[test]
    fn levels_are_drawn_in_decibels_down_to_a_floor() {
        assert_eq!(level_height(1.), 1.);
        assert_eq!(level_height(0.), 0.);
        assert!((level_height(10f32.powf(FLOOR_DB / 40.)) - 0.5).abs() < 1.0e-4);
        assert_eq!(level_height(f32::NAN), 0.);
    }

    #[test]
    fn a_string_and_a_response_are_drawn_inside_their_wells() {
        assert_eq!(
            string_points(&[0.5, -0.01, f32::NAN], 24.),
            vec![1., -0.24, 0.]
        );
        let mut response = [0.; RESONANCE_RESPONSE];
        response[0] = -90.;
        response[1] = f32::INFINITY;
        let points = response_points(&response);
        assert_eq!(points.len(), RESONANCE_RESPONSE);
        assert_eq!(points[0].1, RESPONSE_RANGE.0);
        assert_eq!(points[1].1, RESPONSE_RANGE.0);
        assert!((points[0].0 - RESONANCE_LOW_HZ.log10()).abs() < 1.0e-4);
        assert!(points.windows(2).all(|pair| pair[0].0 < pair[1].0));
    }

    #[gpui::test]
    fn both_resonators_open_beside_their_parameters_and_stop_being_watched_on_close(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::{auxiliary_window::Surface, harness};
        use gpui::{VisualTestContext, size};

        let (app, cx) = harness::open(cx);
        for (effect, card) in [
            ("auris.fx.sympathetic", "resonance-strings-card"),
            ("auris.fx.body", "resonance-body-card"),
        ] {
            let (track, slot) = app.update(cx, |app, cx| {
                app.close_plugin_window();
                let track = app.session.add_default_instrument_track("Lead").unwrap();
                let slot = app.session.add_effect(Some(track), effect).unwrap();
                app.open_plugin_window(PluginSubject::Insert {
                    track: Some(track),
                    slot,
                });
                cx.notify();
                (track, slot)
            });
            harness::paint(&app, cx);
            let handle = app.read_with(cx, |app, _| app.auxiliary_windows[&Surface::Plugin]);
            let mut utility = VisualTestContext::from_window(handle.into(), cx);
            harness::resize(&app, &mut utility, size(px(1080.), px(760.)));
            harness::paint(&app, &mut utility);
            assert!(utility.debug_bounds("resonance-view").is_some(), "{effect}");
            assert!(utility.debug_bounds(card).is_some(), "{effect}");
            assert!(
                utility.debug_bounds("pw-physical-parameters").is_some(),
                "{effect}"
            );
            app.read_with(&utility, |app, _| {
                assert_eq!(app.resonance_view.slot, Some(slot));
                assert!(
                    app.session.has_effect_resonance(slot),
                    "{effect} on {track:?}"
                );
            });
        }
        app.update(cx, |app, cx| {
            app.close_plugin_window();
            app.poll_resonance_view();
            cx.notify();
        });
        app.read_with(cx, |app, _| assert_eq!(app.resonance_view.slot, None));
    }

    #[test]
    fn every_body_has_a_plate_shape() {
        for choice in 0..6 {
            assert!(body_aspect(choice as f32) >= 1.0);
        }
    }
}
