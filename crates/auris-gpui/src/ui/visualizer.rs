//! Live stereo monitoring in a native utility window.
use super::{
    analyser::{FLOOR_DB, HIGH_HZ, LOW_HZ, x_of},
    paint,
    widgets::{ButtonStyle, button},
};
use crate::{app::AurisApp, theme::Theme};
use auris_i18n::Key;
use auris_session::{VisualizerFrame, prelude::*};
use gpui::{Bounds, Pixels, Window, canvas, div, point, prelude::*, px, size};
use std::{cell::Cell, rc::Rc};

#[derive(Clone, Copy)]
pub(crate) enum VisualizerCommand {
    Toggle,
    Source,
    Freeze,
    Average,
    Peaks,
    Save,
    Reset,
}
#[derive(Default)]
pub(crate) struct VisualizerState {
    pub open: bool,
    selected: bool,
    source: Option<Option<TrackId>>,
    frozen: bool,
    average: bool,
    peaks: bool,
    frame: Option<VisualizerFrame>,
    mean: Vec<f32>,
    peak: Vec<f32>,
    reference: Vec<f32>,
    spectrum_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    stereo_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl VisualizerState {
    fn clear_live(&mut self) {
        self.frame = None;
        self.mean.clear();
        self.peak.clear();
    }

    fn accept(&mut self, frame: VisualizerFrame) {
        if self.mean.len() != frame.spectrum.len() {
            self.mean = frame.spectrum.clone();
            self.peak = frame.spectrum.clone();
        } else {
            for ((mean, peak), &value) in self
                .mean
                .iter_mut()
                .zip(&mut self.peak)
                .zip(&frame.spectrum)
            {
                // Average power, not decibels. The UI samples at its regular timer cadence.
                *mean = (10.0
                    * (0.85 * 10.0f32.powf(*mean / 10.0) + 0.15 * 10.0f32.powf(value / 10.0))
                        .log10())
                .max(Session::spectrum_silence());
                *peak = peak.max(value);
            }
        }
        self.frame = Some(frame);
    }
}

impl AurisApp {
    pub(crate) fn close_visualizer(&mut self) {
        self.visualizer.open = false;
        self.session.stop_visualizer();
    }

    pub(crate) fn visualizer_command(&mut self, command: VisualizerCommand) {
        match command {
            VisualizerCommand::Toggle => {
                self.visualizer.open = !self.visualizer.open;
                if !self.visualizer.open {
                    self.close_visualizer();
                }
            }
            VisualizerCommand::Source => {
                self.visualizer.selected = !self.visualizer.selected;
                self.visualizer.source = None;
                self.visualizer.frozen = false;
                self.visualizer.clear_live();
            }
            VisualizerCommand::Freeze => self.visualizer.frozen = !self.visualizer.frozen,
            VisualizerCommand::Average => self.visualizer.average = !self.visualizer.average,
            VisualizerCommand::Peaks => self.visualizer.peaks = !self.visualizer.peaks,
            VisualizerCommand::Save => {
                if let Some(frame) = &self.visualizer.frame {
                    self.visualizer.reference = if self.visualizer.average {
                        self.visualizer.mean.clone()
                    } else {
                        frame.spectrum.clone()
                    };
                }
            }
            VisualizerCommand::Reset => {
                self.visualizer.reference.clear();
                let spectrum = self
                    .visualizer
                    .frame
                    .as_ref()
                    .map(|frame| frame.spectrum.clone())
                    .unwrap_or_default();
                self.visualizer.peak = spectrum.clone();
                self.visualizer.mean = spectrum;
            }
        }
    }

    pub(crate) fn poll_visualizer(&mut self) {
        if !self.visualizer.open {
            return;
        }
        let source = if self.visualizer.selected {
            self.selected_track
                .filter(|&id| self.session.project().track(id).is_some())
        } else {
            None
        };
        if self.visualizer.selected && source.is_none() {
            self.session.stop_visualizer();
            self.visualizer.clear_live();
            self.visualizer.source = None;
            return;
        }
        if self.visualizer.source != Some(source) {
            self.visualizer.clear_live();
            self.visualizer.source = Some(source);
            self.visualizer.frozen = false;
        }
        self.session.watch_visualizer(source);
        if !self.visualizer.frozen
            && let Some(frame) = self.session.visualizer_frame(LOW_HZ, HIGH_HZ, 96)
        {
            self.visualizer.accept(frame);
        }
    }

    pub(crate) fn render_visualizer(
        &mut self,
        window: &Window,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let mut controls = div().flex().flex_wrap().gap_2();
        for (index, key, active) in [
            (
                VisualizerCommand::Source,
                Key::VisualizerSelected,
                self.visualizer.selected,
            ),
            (
                VisualizerCommand::Freeze,
                Key::VisualizerFreeze,
                self.visualizer.frozen,
            ),
            (
                VisualizerCommand::Average,
                Key::VisualizerAverage,
                self.visualizer.average,
            ),
            (
                VisualizerCommand::Peaks,
                Key::VisualizerPeaks,
                self.visualizer.peaks,
            ),
            (VisualizerCommand::Save, Key::VisualizerSave, false),
            (VisualizerCommand::Reset, Key::VisualizerReset, false),
        ] {
            controls = controls.child(
                button(
                    gpui::SharedString::from(format!("visualizer-{}", index as usize)),
                    self.t(key),
                    ButtonStyle::Normal,
                    active,
                    theme.accent,
                    &theme,
                    cx.listener(move |this, _, _, cx| {
                        this.visualizer_command(index);
                        cx.notify();
                    }),
                )
                .cursor_default(),
            );
        }
        let source = if self.visualizer.selected {
            self.selected_track
                .and_then(|id| self.session.project().track(id))
                .map(|track| track.name.clone())
                .unwrap_or_else(|| self.t(Key::VisualizerNoTrack).to_owned())
        } else {
            self.t(Key::Master).to_owned()
        };
        let correlation = self
            .visualizer
            .frame
            .as_ref()
            .and_then(|frame| frame.correlation);
        let correlation_text =
            correlation.map_or_else(|| "—".to_owned(), |value| format!("{value:+.2}"));
        let frame = self.visualizer.frame.clone();
        let spectrum = if self.visualizer.average {
            self.visualizer.mean.clone()
        } else {
            frame
                .as_ref()
                .map(|f| f.spectrum.clone())
                .unwrap_or_default()
        };
        let peaks = if self.visualizer.peaks {
            self.visualizer.peak.clone()
        } else {
            Vec::new()
        };
        let reference = self.visualizer.reference.clone();
        let spectral_theme = theme.clone();
        let stereo_theme = theme.clone();
        let spectrum_bounds = Rc::clone(&self.visualizer.spectrum_bounds);
        let stereo_bounds = Rc::clone(&self.visualizer.stereo_bounds);
        div()
            .id("visualizer")
            // The utility body is a block with an intrinsic width. Establish a definite
            // width before resolving percentage widths inside its scrolling flex column.
            .w(window.viewport_size().width)
            .h_full()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_3()
            .p_3()
            .bg(theme.surface)
            .text_color(theme.text)
            .text_sm()
            .child(controls)
            .when(self.visualizer.frame.is_none(), |this| {
                this.child(self.t(Key::VisualizerWaiting))
            })
            .child(div().child(source))
            .child(div().child(self.t(Key::VisualizerSpectrum)))
            .child(
                div().w_full().h(px(220.)).flex_shrink_0().child(
                    canvas(
                        move |bounds, _, _| spectrum_bounds.set(Some(bounds)),
                        move |bounds, _, window, cx| {
                            paint_spectrum(
                                window,
                                cx,
                                bounds,
                                &spectrum,
                                &peaks,
                                &reference,
                                &spectral_theme,
                            );
                        },
                    )
                    .size_full(),
                ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::VisualizerLegend)),
            )
            .child(div().child(self.t(Key::VisualizerStereo)))
            .child(
                div().w_full().h(px(220.)).flex_shrink_0().child(
                    canvas(
                        move |bounds, _, _| stereo_bounds.set(Some(bounds)),
                        move |bounds, _, window, cx| {
                            paint_stereo(window, cx, bounds, frame.as_ref(), &stereo_theme);
                        },
                    )
                    .size_full(),
                ),
            )
            .child(div().child(format!(
                "{}: {}",
                self.t(Key::VisualizerCorrelation),
                correlation_text
            )))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::VisualizerHint)),
            )
            .into_any_element()
    }
}

fn paint_spectrum(
    window: &mut Window,
    cx: &mut gpui::App,
    bounds: Bounds<Pixels>,
    spectrum: &[f32],
    peak: &[f32],
    reference: &[f32],
    theme: &Theme,
) {
    let left = bounds.origin.x + px(32.);
    let top = bounds.origin.y;
    let width = (bounds.size.width - px(40.)).max(px(1.));
    let height = (bounds.size.height - px(20.)).max(px(1.));
    let plot = Bounds {
        origin: point(left, top),
        size: size(width, height),
    };
    paint::rect(window, bounds, theme.surface_sunken);
    for db in [0., -18., -36., -54., -72.] {
        let y = top + height * (db / FLOOR_DB);
        paint::hline(window, plot, y, theme.border);
        paint::label(
            window,
            cx,
            point(bounds.origin.x, y),
            format!("{db:.0}"),
            px(10.),
            theme.text_muted,
        );
    }
    for hz in [100., 1000., 10000.] {
        let x = left + width * x_of(hz);
        paint::polyline(
            window,
            &[point(x, top), point(x, top + height)],
            px(1.),
            theme.border,
        );
        paint::label(
            window,
            cx,
            point(x, top + height),
            format!("{hz:.0} Hz"),
            px(10.),
            theme.text_muted,
        );
    }
    for (values, color, stroke) in [
        (reference, theme.text_faint, 2.),
        (peak, theme.text_muted, 1.),
        (spectrum, theme.accent, 1.5),
    ] {
        let points: Vec<_> = values
            .iter()
            .enumerate()
            .map(|(i, db)| {
                point(
                    left + width * ((i as f32 + 0.5) / values.len() as f32),
                    top + height * (db / FLOOR_DB).clamp(0., 1.),
                )
            })
            .collect();
        paint::polyline(window, &points, px(stroke), color);
    }
}

fn paint_stereo(
    window: &mut Window,
    cx: &mut gpui::App,
    bounds: Bounds<Pixels>,
    frame: Option<&VisualizerFrame>,
    theme: &Theme,
) {
    paint::rect(window, bounds, theme.surface_sunken);
    let center = point(
        bounds.origin.x + bounds.size.width / 2.,
        bounds.origin.y + bounds.size.height / 2.,
    );
    let radius = (bounds.size.width.min(bounds.size.height) / 2. - px(30.)).max(px(1.));
    for (a, b) in [
        (
            point(center.x - radius, center.y),
            point(center.x + radius, center.y),
        ),
        (
            point(center.x, center.y - radius),
            point(center.x, center.y + radius),
        ),
    ] {
        paint::polyline(window, &[a, b], px(1.), theme.border);
    }
    paint::label(
        window,
        cx,
        point(center.x + px(4.), center.y - radius),
        "M",
        px(10.),
        theme.text_muted,
    );
    let meter_y = bounds.origin.y + bounds.size.height - px(5.);
    let meter_left = bounds.origin.x + px(4.);
    let meter_width = (bounds.size.width - px(8.)).max(px(1.));
    paint::polyline(
        window,
        &[
            point(meter_left, meter_y),
            point(meter_left + meter_width, meter_y),
        ],
        px(2.),
        theme.border,
    );
    for (unit, label) in [(0., "−1"), (0.5, "0"), (1., "+1")] {
        let x = meter_left + meter_width * unit;
        paint::polyline(
            window,
            &[point(x, meter_y - px(3.)), point(x, meter_y + px(3.))],
            px(1.),
            theme.text_muted,
        );
        paint::label(
            window,
            cx,
            point(x.min(meter_left + meter_width - px(16.)), meter_y - px(16.)),
            label,
            px(10.),
            theme.text_muted,
        );
    }
    paint::label(
        window,
        cx,
        point(center.x + radius, center.y),
        "S",
        px(10.),
        theme.text_muted,
    );
    if let Some(frame) = frame {
        let points: Vec<_> = frame
            .left
            .iter()
            .zip(&frame.right)
            .map(|(&l, &r)| {
                point(
                    center.x + radius * ((l - r) * 0.5).clamp(-1., 1.),
                    center.y - radius * ((l + r) * 0.5).clamp(-1., 1.),
                )
            })
            .collect();
        paint::clipped(window, bounds, |window| {
            paint::polyline(window, &points, px(1.), theme.accent)
        });
        if let Some(value) = frame.correlation {
            let y = meter_y;
            let x = meter_left + meter_width * ((value + 1.) * 0.5);
            paint::rect(
                window,
                Bounds {
                    origin: point(x - px(2.), y - px(3.)),
                    size: size(px(4.), px(6.)),
                },
                theme.accent,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{actions, auxiliary_window::Surface, harness};

    fn frame(db: f32) -> VisualizerFrame {
        VisualizerFrame {
            left: vec![0.5, -0.5],
            right: vec![0.5, -0.5],
            spectrum: vec![db; 96],
            correlation: Some(1.),
        }
    }

    #[test]
    fn history_averages_power_and_retains_peaks() {
        let mut state = VisualizerState::default();
        state.accept(frame(-20.));
        state.accept(frame(0.));
        let expected = 10.0f32 * (0.85f32 * 0.01 + 0.15).log10();
        assert!((state.mean[0] - expected).abs() < 0.001);
        state.accept(frame(-40.));
        assert_eq!(state.peak[0], 0.);
    }

    #[gpui::test]
    fn visualizer_opens_freezes_saves_and_closes_without_editing(cx: &mut gpui::TestAppContext) {
        let (app, cx) = harness::open(cx);
        cx.dispatch_action(actions::ToggleVisualizer);
        harness::paint(&app, cx);
        let handle = app.read_with(cx, |app, _| app.auxiliary_windows[&Surface::Visualizer]);
        app.read_with(cx, |app, _| {
            let spectrum = app
                .visualizer
                .spectrum_bounds
                .get()
                .expect("spectrum painted");
            let stereo = app
                .visualizer
                .stereo_bounds
                .get()
                .expect("stereo scope painted");
            assert!(spectrum.size.width > px(600.), "{spectrum:?}");
            assert_eq!(spectrum.size.width, stereo.size.width);
            assert_eq!(spectrum.origin.x, stereo.origin.x);
            assert_eq!(spectrum.size.height, px(220.));
        });
        app.update(cx, |app, _| app.visualizer.accept(frame(-12.)));
        cx.dispatch_action(actions::VisualizerFreeze);
        cx.dispatch_action(actions::VisualizerSave);
        app.read_with(cx, |app, _| {
            assert!(app.visualizer.frozen);
            assert_eq!(app.visualizer.reference, vec![-12.; 96]);
        });
        cx.dispatch_action(actions::VisualizerSource);
        app.read_with(cx, |app, _| {
            assert!(!app.visualizer.frozen);
            assert!(app.visualizer.frame.is_none());
            assert_eq!(app.visualizer.reference, vec![-12.; 96]);
        });
        cx.dispatch_action(actions::VisualizerReset);
        app.read_with(cx, |app, _| assert!(app.visualizer.reference.is_empty()));
        cx.dispatch_action(actions::ToggleVisualizer);
        harness::paint(&app, cx);
        assert!(handle.read_with(cx, |_, _| ()).is_err());
    }

    #[gpui::test]
    fn opening_another_document_discards_frozen_audio(cx: &mut gpui::TestAppContext) {
        let (app, cx) = harness::open(cx);
        app.update(cx, |app, _| {
            app.visualizer.open = true;
            app.visualizer.frozen = true;
            app.visualizer.accept(frame(-12.));
            app.reset_view();
            assert!(!app.visualizer.open);
            assert!(app.visualizer.frame.is_none());
        });
    }
}
