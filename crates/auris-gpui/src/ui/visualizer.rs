//! Live stereo monitoring in a native utility window.
use super::{
    analyser::{FLOOR_DB, HIGH_HZ, LOW_HZ, x_of},
    paint,
    widgets::{ButtonState, ButtonStyle, button, button_enabled},
};
use crate::{app::AurisApp, theme::Theme};
use auris_i18n::Key;
use auris_session::{VisualizerFrame, VisualizerSpectrum, prelude::*};
use gpui::{Bounds, Pixels, Window, canvas, div, point, prelude::*, px, size};
use std::{
    cell::Cell,
    collections::VecDeque,
    rc::Rc,
    time::{Duration, Instant},
};

const CORRELATION_HISTORY: Duration = Duration::from_secs(3);
const SPECTRUM_AVERAGE_SECONDS: f32 = 0.2;
const STEREO_TRAIL_FRAMES: usize = 5;

#[derive(Clone, Copy)]
pub(crate) enum VisualizerCommand {
    Toggle,
    Source,
    Freeze,
    Average,
    Peaks,
    Save,
    Reset,
    Timebase,
    ViewNext,
    SpectrumChannel,
    CorrelationReset,
    OscilloscopeGain,
    StereoAutoGain,
    View(VisualizerView),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum VisualizerView {
    Oscilloscope,
    #[default]
    Spectrum,
    Stereo,
    All,
}

impl VisualizerView {
    fn next(self) -> Self {
        match self {
            Self::Oscilloscope => Self::Spectrum,
            Self::Spectrum => Self::Stereo,
            Self::Stereo => Self::All,
            Self::All => Self::Oscilloscope,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum OscilloscopeGain {
    #[default]
    Unity,
    Double,
    Quadruple,
    Auto,
}

impl OscilloscopeGain {
    fn next(self) -> Self {
        match self {
            Self::Unity => Self::Double,
            Self::Double => Self::Quadruple,
            Self::Quadruple => Self::Auto,
            Self::Auto => Self::Unity,
        }
    }

    fn multiplier(self, left: &[f32], right: &[f32]) -> f32 {
        match self {
            Self::Unity => 1.0,
            Self::Double => 2.0,
            Self::Quadruple => 4.0,
            Self::Auto => display_auto_gain(left, right),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct CorrelationPoint {
    at: Instant,
    value: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum OscilloscopeSpan {
    Short,
    Medium,
    #[default]
    Full,
}

impl OscilloscopeSpan {
    fn sample_count(self, available: usize, window_samples: usize) -> usize {
        let full = available.min(window_samples);
        match self {
            Self::Short => full / 4,
            Self::Medium => full / 2,
            Self::Full => full,
        }
        .clamp(2, available)
    }

    fn next(self) -> Self {
        match self {
            Self::Short => Self::Medium,
            Self::Medium => Self::Full,
            Self::Full => Self::Short,
        }
    }
}

#[derive(Default)]
pub(crate) struct VisualizerState {
    pub open: bool,
    selected: bool,
    source: Option<Option<TrackId>>,
    frozen: bool,
    average: bool,
    peaks: bool,
    view: VisualizerView,
    spectrum_mode: VisualizerSpectrum,
    oscilloscope_span: OscilloscopeSpan,
    oscilloscope_gain: OscilloscopeGain,
    stereo_auto_gain: bool,
    frame: Option<VisualizerFrame>,
    mean: Vec<f32>,
    peak: Vec<f32>,
    reference: Vec<f32>,
    last_spectrum_at: Option<Instant>,
    correlation_history: VecDeque<CorrelationPoint>,
    correlation_negative_peak: Option<f32>,
    stereo_trail: VecDeque<VisualizerFrame>,
    spectrum_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    oscilloscope_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    stereo_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    correlation_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl VisualizerState {
    /// Whether there is a captured frame that can actually be held on screen.
    fn can_freeze(&self) -> bool {
        self.frame.is_some()
    }

    /// Whether the spectrum currently shown can become the comparison reference.
    fn can_save(&self) -> bool {
        if self.average {
            !self.mean.is_empty()
        } else {
            self.frame
                .as_ref()
                .is_some_and(|frame| !frame.spectrum.is_empty())
        }
    }

    /// Whether resetting the accumulated display would change anything.
    fn can_reset(&self) -> bool {
        !self.reference.is_empty() || !self.mean.is_empty() || !self.peak.is_empty()
    }

    fn clear_live(&mut self) {
        self.frame = None;
        self.mean.clear();
        self.peak.clear();
        self.last_spectrum_at = None;
        self.correlation_history.clear();
        self.correlation_negative_peak = None;
        self.stereo_trail.clear();
    }

    fn accept(&mut self, frame: VisualizerFrame) {
        self.accept_at(frame, Instant::now());
    }

    fn accept_at(&mut self, frame: VisualizerFrame, now: Instant) {
        if !frame.spectrum.is_empty() && self.mean.len() != frame.spectrum.len() {
            self.mean = frame.spectrum.to_vec();
            self.peak = frame.spectrum.to_vec();
        } else if !frame.spectrum.is_empty() {
            let elapsed = self
                .last_spectrum_at
                .map_or(SPECTRUM_AVERAGE_SECONDS, |last| {
                    now.saturating_duration_since(last).as_secs_f32()
                });
            let alpha = (1.0 - (-elapsed / SPECTRUM_AVERAGE_SECONDS).exp()).clamp(0.0, 1.0);
            for ((mean, peak), &value) in self
                .mean
                .iter_mut()
                .zip(&mut self.peak)
                .zip(frame.spectrum.iter())
            {
                // Average power, not decibels. Elapsed time keeps the response stable after a
                // delayed repaint instead of treating every delivered frame as equally long.
                *mean = (10.0
                    * ((1.0 - alpha) * 10.0f32.powf(*mean / 10.0)
                        + alpha * 10.0f32.powf(value / 10.0))
                    .log10())
                .max(Session::spectrum_silence());
                *peak = peak.max(value);
            }
        }
        if !frame.spectrum.is_empty() {
            self.last_spectrum_at = Some(now);
        }

        while self
            .correlation_history
            .front()
            .is_some_and(|point| now.saturating_duration_since(point.at) > CORRELATION_HISTORY)
        {
            self.correlation_history.pop_front();
        }
        if let Some(value) = frame.correlation {
            self.correlation_history
                .push_back(CorrelationPoint { at: now, value });
            if value < 0.0 {
                self.correlation_negative_peak = Some(
                    self.correlation_negative_peak
                        .map_or(value, |peak| peak.min(value)),
                );
            }
        }
        self.stereo_trail.push_back(frame.clone());
        while self.stereo_trail.len() > STEREO_TRAIL_FRAMES {
            self.stereo_trail.pop_front();
        }
        self.frame = Some(frame);
    }

    fn clear_spectrum_mode(&mut self) {
        self.mean.clear();
        self.peak.clear();
        self.reference.clear();
        self.last_spectrum_at = None;
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
            VisualizerCommand::Freeze if self.visualizer.can_freeze() => {
                self.visualizer.frozen = !self.visualizer.frozen;
            }
            // A hold with no frame used to suppress polling forever while the window kept saying
            // "Waiting". A keyboard shortcut can still arrive when the toolbar button is
            // disabled, so the command itself owns the same availability rule.
            VisualizerCommand::Freeze => self.visualizer.frozen = false,
            VisualizerCommand::Average => self.visualizer.average = !self.visualizer.average,
            VisualizerCommand::Peaks => self.visualizer.peaks = !self.visualizer.peaks,
            VisualizerCommand::Save if self.visualizer.can_save() => {
                let displayed = if self.visualizer.average {
                    (!self.visualizer.mean.is_empty()).then(|| self.visualizer.mean.clone())
                } else {
                    self.visualizer
                        .frame
                        .as_ref()
                        .filter(|frame| !frame.spectrum.is_empty())
                        .map(|frame| frame.spectrum.to_vec())
                };
                if let Some(displayed) = displayed {
                    self.visualizer.reference = displayed;
                }
            }
            VisualizerCommand::Save => {}
            VisualizerCommand::Reset if self.visualizer.can_reset() => {
                self.visualizer.reference.clear();
                let spectrum = self
                    .visualizer
                    .frame
                    .as_ref()
                    .map(|frame| frame.spectrum.to_vec())
                    .unwrap_or_default();
                self.visualizer.peak = spectrum.clone();
                self.visualizer.mean = spectrum;
            }
            VisualizerCommand::Reset => {}
            VisualizerCommand::Timebase => {
                self.visualizer.oscilloscope_span = self.visualizer.oscilloscope_span.next();
            }
            VisualizerCommand::ViewNext => {
                self.visualizer.view = self.visualizer.view.next();
            }
            VisualizerCommand::SpectrumChannel => {
                self.visualizer.spectrum_mode = match self.visualizer.spectrum_mode {
                    VisualizerSpectrum::Average => VisualizerSpectrum::Left,
                    VisualizerSpectrum::Left => VisualizerSpectrum::Right,
                    VisualizerSpectrum::Right => VisualizerSpectrum::Maximum,
                    VisualizerSpectrum::Maximum => VisualizerSpectrum::Mono,
                    VisualizerSpectrum::Mono => VisualizerSpectrum::Average,
                };
                self.visualizer.clear_spectrum_mode();
            }
            VisualizerCommand::CorrelationReset => {
                self.visualizer.correlation_history.clear();
                self.visualizer.correlation_negative_peak = None;
            }
            VisualizerCommand::OscilloscopeGain => {
                self.visualizer.oscilloscope_gain = self.visualizer.oscilloscope_gain.next();
            }
            VisualizerCommand::StereoAutoGain => {
                self.visualizer.stereo_auto_gain = !self.visualizer.stereo_auto_gain;
            }
            VisualizerCommand::View(view) => self.visualizer.view = view,
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
            self.visualizer.frozen = false;
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
        let spectrum_mode = matches!(
            self.visualizer.view,
            VisualizerView::Spectrum | VisualizerView::All
        )
        .then_some(self.visualizer.spectrum_mode);
        if !self.visualizer.frozen
            && let Some(frame) = self
                .session
                .visualizer_frame(LOW_HZ, HIGH_HZ, 96, spectrum_mode)
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
        let can_freeze = self.visualizer.can_freeze();
        let can_save = self.visualizer.can_save();
        let can_reset = self.visualizer.can_reset();
        let mut controls = div().flex().flex_wrap().gap_2();
        for (id, command, key, active) in [
            (
                "visualizer-source",
                VisualizerCommand::Source,
                Key::VisualizerSelected,
                self.visualizer.selected,
            ),
            (
                "visualizer-freeze",
                VisualizerCommand::Freeze,
                Key::VisualizerFreeze,
                self.visualizer.frozen,
            ),
        ] {
            let enabled = !matches!(command, VisualizerCommand::Freeze) || can_freeze;
            controls = controls.child(
                button_enabled(
                    id,
                    self.t(key),
                    ButtonStyle::Normal,
                    ButtonState::available(active, enabled),
                    theme.accent,
                    &theme,
                    cx.listener(move |this, _, _, cx| {
                        this.visualizer_command(command);
                        cx.notify();
                    }),
                )
                .cursor_default(),
            );
        }
        let mut view_controls = div().flex().flex_wrap().gap_1();
        for (id, view, key) in [
            (
                "visualizer-view-oscilloscope",
                VisualizerView::Oscilloscope,
                Key::VisualizerViewOscilloscope,
            ),
            (
                "visualizer-view-spectrum",
                VisualizerView::Spectrum,
                Key::VisualizerViewSpectrum,
            ),
            (
                "visualizer-view-stereo",
                VisualizerView::Stereo,
                Key::VisualizerViewStereo,
            ),
            (
                "visualizer-view-all",
                VisualizerView::All,
                Key::VisualizerViewAll,
            ),
        ] {
            view_controls = view_controls.child(
                button(
                    id,
                    self.t(key),
                    ButtonStyle::Normal,
                    self.visualizer.view == view,
                    theme.accent,
                    &theme,
                    cx.listener(move |this, _, _, cx| {
                        this.visualizer_command(VisualizerCommand::View(view));
                        cx.notify();
                    }),
                )
                .cursor_default(),
            );
        }
        let oscilloscope_span = self.visualizer.oscilloscope_span;
        let oscilloscope_span_label = self
            .visualizer
            .frame
            .as_ref()
            .and_then(|frame| {
                oscilloscope_window(frame, oscilloscope_span).map(|window| (frame, window))
            })
            .map_or_else(
                || self.t(Key::VisualizerFullWindow).to_owned(),
                |(frame, window)| {
                    format!("{:.1} ms", window.len as f64 * 1000.0 / frame.sample_rate)
                },
            );
        let gain_label = match self.visualizer.oscilloscope_gain {
            OscilloscopeGain::Unity => "1×".to_owned(),
            OscilloscopeGain::Double => "2×".to_owned(),
            OscilloscopeGain::Quadruple => "4×".to_owned(),
            OscilloscopeGain::Auto => self.t(Key::VisualizerAuto).to_owned(),
        };
        let oscilloscope_controls = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .child(
                button(
                    "visualizer-timebase",
                    format!(
                        "{}: {}",
                        self.t(Key::VisualizerTimebase),
                        oscilloscope_span_label
                    ),
                    ButtonStyle::Normal,
                    false,
                    theme.accent,
                    &theme,
                    cx.listener(|this, _, _, cx| {
                        this.visualizer_command(VisualizerCommand::Timebase);
                        cx.notify();
                    }),
                )
                .cursor_default(),
            )
            .child(
                button(
                    "visualizer-oscilloscope-gain",
                    format!("{}: {gain_label}", self.t(Key::VisualizerDisplayGain)),
                    ButtonStyle::Normal,
                    self.visualizer.oscilloscope_gain != OscilloscopeGain::Unity,
                    theme.accent,
                    &theme,
                    cx.listener(|this, _, _, cx| {
                        this.visualizer_command(VisualizerCommand::OscilloscopeGain);
                        cx.notify();
                    }),
                )
                .cursor_default(),
            )
            .into_any_element();
        let mut spectrum_controls = div().flex().flex_wrap().gap_2();
        for (id, command, key, active) in [
            (
                "visualizer-average",
                VisualizerCommand::Average,
                Key::VisualizerAverage,
                self.visualizer.average,
            ),
            (
                "visualizer-peaks",
                VisualizerCommand::Peaks,
                Key::VisualizerPeaks,
                self.visualizer.peaks,
            ),
            (
                "visualizer-save",
                VisualizerCommand::Save,
                Key::VisualizerSave,
                false,
            ),
            (
                "visualizer-reset",
                VisualizerCommand::Reset,
                Key::VisualizerReset,
                false,
            ),
        ] {
            let enabled = match command {
                VisualizerCommand::Save => can_save,
                VisualizerCommand::Reset => can_reset,
                _ => true,
            };
            spectrum_controls = spectrum_controls.child(
                button_enabled(
                    id,
                    self.t(key),
                    ButtonStyle::Normal,
                    ButtonState::available(active, enabled),
                    theme.accent,
                    &theme,
                    cx.listener(move |this, _, _, cx| {
                        this.visualizer_command(command);
                        cx.notify();
                    }),
                )
                .cursor_default(),
            );
        }
        let spectrum_mode_label = match self.visualizer.spectrum_mode {
            VisualizerSpectrum::Average => Key::VisualizerSpectrumAverage,
            VisualizerSpectrum::Left => Key::VisualizerSpectrumLeft,
            VisualizerSpectrum::Right => Key::VisualizerSpectrumRight,
            VisualizerSpectrum::Maximum => Key::VisualizerSpectrumMaximum,
            VisualizerSpectrum::Mono => Key::VisualizerSpectrumMono,
        };
        spectrum_controls = spectrum_controls.child(
            button(
                "visualizer-spectrum-channel",
                format!(
                    "{}: {}",
                    self.t(Key::VisualizerSpectrumChannel),
                    self.t(spectrum_mode_label)
                ),
                ButtonStyle::Normal,
                false,
                theme.accent,
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.visualizer_command(VisualizerCommand::SpectrumChannel);
                    cx.notify();
                }),
            )
            .cursor_default(),
        );
        let spectrum_controls = spectrum_controls.into_any_element();
        let stereo_controls = div()
            .flex()
            .child(
                button(
                    "visualizer-stereo-auto-gain",
                    format!(
                        "{}: {}",
                        self.t(Key::VisualizerStereoAutoGain),
                        if self.visualizer.stereo_auto_gain {
                            self.t(Key::VisualizerAuto)
                        } else {
                            "1×"
                        }
                    ),
                    ButtonStyle::Normal,
                    self.visualizer.stereo_auto_gain,
                    theme.accent,
                    &theme,
                    cx.listener(|this, _, _, cx| {
                        this.visualizer_command(VisualizerCommand::StereoAutoGain);
                        cx.notify();
                    }),
                )
                .cursor_default(),
            )
            .into_any_element();
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
        let negative_peak_text = self
            .visualizer
            .correlation_negative_peak
            .map_or_else(|| "—".to_owned(), |value| format!("{value:+.2}"));
        let frame = self.visualizer.frame.clone();
        let spectrum = if self.visualizer.average {
            std::sync::Arc::<[f32]>::from(self.visualizer.mean.clone())
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
        let correlation_values: Vec<_> = self
            .visualizer
            .correlation_history
            .iter()
            .map(|point| point.value)
            .collect();
        let trail: Vec<_> = self.visualizer.stereo_trail.iter().cloned().collect();
        let view = self.visualizer.view;
        let content = match view {
            VisualizerView::Oscilloscope => oscilloscope_section(
                frame.clone(),
                oscilloscope_span,
                self.visualizer.oscilloscope_gain,
                oscilloscope_controls,
                self.t(Key::VisualizerOscilloscope).to_owned(),
                self.t(Key::VisualizerOscilloscopeHint).to_owned(),
                theme.clone(),
                Rc::clone(&self.visualizer.oscilloscope_bounds),
                true,
            ),
            VisualizerView::Spectrum => spectrum_section(
                spectrum,
                peaks,
                reference,
                spectrum_controls,
                self.t(Key::VisualizerSpectrum).to_owned(),
                self.t(Key::VisualizerLegend).to_owned(),
                theme.clone(),
                Rc::clone(&self.visualizer.spectrum_bounds),
                true,
            ),
            VisualizerView::Stereo => stereo_section(
                trail,
                self.visualizer.stereo_auto_gain,
                stereo_controls,
                self.t(Key::VisualizerStereo).to_owned(),
                self.t(Key::VisualizerHint).to_owned(),
                theme.clone(),
                Rc::clone(&self.visualizer.stereo_bounds),
                true,
            ),
            VisualizerView::All => div()
                .id("visualizer-all")
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .gap_3()
                .child(oscilloscope_section(
                    frame.clone(),
                    oscilloscope_span,
                    self.visualizer.oscilloscope_gain,
                    oscilloscope_controls,
                    self.t(Key::VisualizerOscilloscope).to_owned(),
                    self.t(Key::VisualizerOscilloscopeHint).to_owned(),
                    theme.clone(),
                    Rc::clone(&self.visualizer.oscilloscope_bounds),
                    false,
                ))
                .child(spectrum_section(
                    spectrum,
                    peaks,
                    reference,
                    spectrum_controls,
                    self.t(Key::VisualizerSpectrum).to_owned(),
                    self.t(Key::VisualizerLegend).to_owned(),
                    theme.clone(),
                    Rc::clone(&self.visualizer.spectrum_bounds),
                    false,
                ))
                .child(stereo_section(
                    trail,
                    self.visualizer.stereo_auto_gain,
                    stereo_controls,
                    self.t(Key::VisualizerStereo).to_owned(),
                    self.t(Key::VisualizerHint).to_owned(),
                    theme.clone(),
                    Rc::clone(&self.visualizer.stereo_bounds),
                    false,
                ))
                .into_any_element(),
        };
        let correlation_theme = theme.clone();
        let correlation_bounds = Rc::clone(&self.visualizer.correlation_bounds);
        let correlation_negative_peak = self.visualizer.correlation_negative_peak;
        div()
            .id("visualizer")
            .w(window.viewport_size().width)
            .h_full()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_3()
            .p_3()
            .bg(theme.surface)
            .text_color(theme.text)
            .text_sm()
            .child(controls)
            .child(view_controls)
            .when(self.visualizer.frame.is_none(), |this| {
                this.child(self.t(Key::VisualizerWaiting))
            })
            .child(div().child(source))
            .child(content)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(format!(
                                "{}: {} · {}: {}",
                                self.t(Key::VisualizerCorrelation),
                                correlation_text,
                                self.t(Key::VisualizerNegativePeak),
                                negative_peak_text
                            ))
                            .child(
                                button_enabled(
                                    "visualizer-correlation-reset",
                                    self.t(Key::VisualizerCorrelationReset),
                                    ButtonStyle::Normal,
                                    ButtonState::available(
                                        false,
                                        !self.visualizer.correlation_history.is_empty()
                                            || self.visualizer.correlation_negative_peak.is_some(),
                                    ),
                                    theme.accent,
                                    &theme,
                                    cx.listener(|this, _, _, cx| {
                                        this.visualizer_command(
                                            VisualizerCommand::CorrelationReset,
                                        );
                                        cx.notify();
                                    }),
                                )
                                .cursor_default(),
                            ),
                    )
                    .child(
                        div().w_full().h(px(42.)).flex_shrink_0().child(
                            canvas(
                                move |bounds, _, _| correlation_bounds.set(Some(bounds)),
                                move |bounds, _, window, cx| {
                                    paint_correlation(
                                        window,
                                        cx,
                                        bounds,
                                        correlation,
                                        &correlation_values,
                                        correlation_negative_peak,
                                        &correlation_theme,
                                    );
                                },
                            )
                            .size_full(),
                        ),
                    ),
            )
            .into_any_element()
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "a section owns its data, controls, copy, and paint state"
)]
fn oscilloscope_section(
    frame: Option<VisualizerFrame>,
    span: OscilloscopeSpan,
    gain: OscilloscopeGain,
    controls: gpui::AnyElement,
    title: String,
    hint: String,
    theme: Theme,
    bounds_cell: Rc<Cell<Option<Bounds<Pixels>>>>,
    fill: bool,
) -> gpui::AnyElement {
    let paint_theme = theme.clone();
    let graph = div()
        .w_full()
        .when(fill, |this| this.flex_1().min_h(px(160.)))
        .when(!fill, |this| this.h(px(220.)).flex_shrink_0())
        .child(
            canvas(
                move |bounds, _, _| bounds_cell.set(Some(bounds)),
                move |bounds, _, window, cx| {
                    paint_oscilloscope(
                        window,
                        cx,
                        bounds,
                        frame.as_ref(),
                        span,
                        gain,
                        &paint_theme,
                    );
                },
            )
            .size_full(),
        );
    div()
        .flex()
        .flex_col()
        .gap_2()
        .when(fill, |this| this.flex_1().min_h_0())
        .child(div().flex().flex_col().gap_2().child(title).child(controls))
        .child(graph)
        .child(div().text_xs().text_color(theme.text_muted).child(hint))
        .into_any_element()
}

#[expect(
    clippy::too_many_arguments,
    reason = "a section owns its data, controls, copy, and paint state"
)]
fn spectrum_section(
    spectrum: std::sync::Arc<[f32]>,
    peak: Vec<f32>,
    reference: Vec<f32>,
    controls: gpui::AnyElement,
    title: String,
    hint: String,
    theme: Theme,
    bounds_cell: Rc<Cell<Option<Bounds<Pixels>>>>,
    fill: bool,
) -> gpui::AnyElement {
    let paint_theme = theme.clone();
    let graph = div()
        .w_full()
        .when(fill, |this| this.flex_1().min_h(px(160.)))
        .when(!fill, |this| this.h(px(220.)).flex_shrink_0())
        .child(
            canvas(
                move |bounds, _, _| bounds_cell.set(Some(bounds)),
                move |bounds, _, window, cx| {
                    paint_spectrum(
                        window,
                        cx,
                        bounds,
                        &spectrum,
                        &peak,
                        &reference,
                        &paint_theme,
                    );
                },
            )
            .size_full(),
        );
    div()
        .flex()
        .flex_col()
        .gap_2()
        .when(fill, |this| this.flex_1().min_h_0())
        .child(div().flex().flex_col().gap_2().child(title).child(controls))
        .child(graph)
        .child(div().text_xs().text_color(theme.text_muted).child(hint))
        .into_any_element()
}

#[expect(
    clippy::too_many_arguments,
    reason = "a section owns its data, controls, copy, and paint state"
)]
fn stereo_section(
    trail: Vec<VisualizerFrame>,
    auto_gain: bool,
    controls: gpui::AnyElement,
    title: String,
    hint: String,
    theme: Theme,
    bounds_cell: Rc<Cell<Option<Bounds<Pixels>>>>,
    fill: bool,
) -> gpui::AnyElement {
    let paint_theme = theme.clone();
    let graph = div()
        .w_full()
        .when(fill, |this| this.flex_1().min_h(px(160.)))
        .when(!fill, |this| this.h(px(220.)).flex_shrink_0())
        .child(
            canvas(
                move |bounds, _, _| bounds_cell.set(Some(bounds)),
                move |bounds, _, window, cx| {
                    paint_stereo(window, cx, bounds, &trail, auto_gain, &paint_theme);
                },
            )
            .size_full(),
        );
    div()
        .flex()
        .flex_col()
        .gap_2()
        .when(fill, |this| this.flex_1().min_h_0())
        .child(div().flex().flex_col().gap_2().child(title).child(controls))
        .child(graph)
        .child(div().text_xs().text_color(theme.text_muted).child(hint))
        .into_any_element()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OscilloscopeWindow {
    start: usize,
    len: usize,
    trigger_offset: usize,
}

fn oscilloscope_window(
    frame: &VisualizerFrame,
    span: OscilloscopeSpan,
) -> Option<OscilloscopeWindow> {
    let available = frame.left.len().min(frame.right.len());
    if available < 2 || !frame.sample_rate.is_finite() || frame.sample_rate <= 0.0 {
        return None;
    }
    let len = span.sample_count(available, frame.window_samples);
    let trigger_offset = len / 5;
    let maximum_crossing = available - len + trigger_offset;
    let minimum_crossing = trigger_offset.max(1);
    let left_power: f32 = frame.left.iter().map(|sample| sample * sample).sum();
    let right_power: f32 = frame.right.iter().map(|sample| sample * sample).sum();
    let trigger_channel = if right_power > left_power {
        &frame.right
    } else {
        &frame.left
    };
    let has_signal = trigger_channel.iter().any(|sample| sample.abs() >= 1.0e-4);
    let crossing = has_signal.then(|| {
        (minimum_crossing..=maximum_crossing)
            .rev()
            .find(|&index| trigger_channel[index - 1] <= 0.0 && trigger_channel[index] > 0.0)
    });
    let start = crossing.flatten().map_or(available - len, |index| {
        index.saturating_sub(trigger_offset)
    });
    Some(OscilloscopeWindow {
        start,
        len,
        trigger_offset,
    })
}

fn paint_oscilloscope(
    window: &mut Window,
    cx: &mut gpui::App,
    bounds: Bounds<Pixels>,
    frame: Option<&VisualizerFrame>,
    span: OscilloscopeSpan,
    gain: OscilloscopeGain,
    theme: &Theme,
) {
    paint::rect(window, bounds, theme.surface_sunken);
    let left = bounds.origin.x + px(38.);
    let top = bounds.origin.y + px(8.);
    let width = (bounds.size.width - px(46.)).max(px(1.));
    let height = (bounds.size.height - px(28.)).max(px(1.));
    let plot = Bounds {
        origin: point(left, top),
        size: size(width, height),
    };
    let lane_height = height / 2.;
    for unit in [0.0, 0.25, 0.5, 0.75, 1.0] {
        let x = left + width * unit;
        paint::polyline(
            window,
            &[point(x, top), point(x, top + height)],
            px(1.),
            theme.border_subtle,
        );
    }
    for (lane, label) in [(0, "L"), (1, "R")] {
        let lane_top = top + lane_height * lane as f32;
        let center = lane_top + lane_height / 2.;
        for (value, color) in [
            (lane_top, theme.border_subtle),
            (center, theme.border),
            (lane_top + lane_height, theme.border_subtle),
        ] {
            paint::hline(window, plot, value, color);
        }
        paint::label(
            window,
            cx,
            point(bounds.origin.x + px(4.), center - px(6.)),
            label,
            px(11.),
            theme.text,
        );
        for (value, text) in [
            (lane_top, "+1"),
            (center, "0"),
            (lane_top + lane_height, "−1"),
        ] {
            paint::label(
                window,
                cx,
                point(bounds.origin.x + px(18.), value - px(5.)),
                text,
                px(9.),
                theme.text_muted,
            );
        }
    }
    let visible = frame.and_then(|frame| oscilloscope_window(frame, span));
    let trigger_fraction = visible.map_or(0.2, |visible| {
        visible.trigger_offset as f32 / (visible.len - 1) as f32
    });
    let trigger_x = left + width * trigger_fraction;
    paint::polyline(
        window,
        &[point(trigger_x, top), point(trigger_x, top + height)],
        px(1.),
        theme.accent_soft,
    );
    let Some((frame, visible)) = frame.zip(visible) else {
        return;
    };
    let before_ms = visible.trigger_offset as f64 * 1000.0 / frame.sample_rate;
    let after_ms = (visible.len - 1 - visible.trigger_offset) as f64 * 1000.0 / frame.sample_rate;
    for (x, label) in [
        (left, format!("−{before_ms:.1}")),
        (trigger_x, "0".to_owned()),
        (left + width, format!("+{after_ms:.1} ms")),
    ] {
        paint::label(
            window,
            cx,
            point(x.min(left + width - px(38.)), top + height + px(2.)),
            label,
            px(10.),
            theme.text_muted,
        );
    }
    let end = visible.start + visible.len;
    let left_samples = &frame.left[visible.start..end];
    let right_samples = &frame.right[visible.start..end];
    let multiplier = gain.multiplier(left_samples, right_samples);
    for (samples, center, color) in [
        (left_samples, top + lane_height / 2., theme.accent),
        (
            right_samples,
            top + lane_height * 1.5,
            theme.track_palette[1],
        ),
    ] {
        let points: Vec<_> = samples
            .iter()
            .enumerate()
            .map(|(index, &sample)| {
                point(
                    left + width * index as f32 / (visible.len - 1) as f32,
                    center - lane_height * 0.45 * (sample * multiplier).clamp(-1.0, 1.0),
                )
            })
            .collect();
        paint::clipped(window, plot, |window| {
            paint::polyline(window, &points, px(1.), color)
        });
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
    history: &[VisualizerFrame],
    auto_gain: bool,
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
    paint::label(
        window,
        cx,
        point(center.x + radius, center.y),
        "S",
        px(10.),
        theme.text_muted,
    );
    let multiplier = history
        .last()
        .map(|frame| {
            let (left, right) = frame_window(frame);
            if auto_gain {
                display_auto_gain(left, right)
            } else {
                1.0
            }
        })
        .unwrap_or(1.0);
    for (index, frame) in history.iter().enumerate() {
        let (left, right) = frame_window(frame);
        let points: Vec<_> = left
            .iter()
            .zip(right)
            .map(|(&l, &r)| {
                let (l, r) = (l * multiplier, r * multiplier);
                point(
                    center.x + radius * ((l - r) * 0.5).clamp(-1., 1.),
                    center.y - radius * ((l + r) * 0.5).clamp(-1., 1.),
                )
            })
            .collect();
        let alpha = 0.12 + 0.78 * (index + 1) as f32 / history.len() as f32;
        paint::clipped(window, bounds, |window| {
            paint::polyline(
                window,
                &points,
                px(1.),
                Theme::translucent(theme.accent, alpha),
            )
        });
    }
}

fn frame_window(frame: &VisualizerFrame) -> (&[f32], &[f32]) {
    let available = frame.left.len().min(frame.right.len());
    let count = frame.window_samples.min(available);
    let start = available - count;
    (
        &frame.left[start..available],
        &frame.right[start..available],
    )
}

fn display_auto_gain(left: &[f32], right: &[f32]) -> f32 {
    let peak = left
        .iter()
        .chain(right)
        .copied()
        .filter(|sample| sample.is_finite())
        .map(f32::abs)
        .fold(0.0, f32::max);
    if peak <= 1.0e-6 {
        1.0
    } else {
        (0.9 / peak).clamp(1.0, 16.0)
    }
}

fn paint_correlation(
    window: &mut Window,
    cx: &mut gpui::App,
    bounds: Bounds<Pixels>,
    current: Option<f32>,
    history: &[f32],
    negative_peak: Option<f32>,
    theme: &Theme,
) {
    paint::rect(window, bounds, theme.surface_sunken);
    let left = bounds.origin.x + px(24.);
    let right = bounds.origin.x + bounds.size.width - px(24.);
    let width = (right - left).max(px(1.));
    let y = bounds.origin.y + bounds.size.height / 2.;
    paint::polyline(
        window,
        &[point(left, y), point(right, y)],
        px(2.),
        theme.border,
    );
    for (unit, label) in [(0.0, "−1"), (0.5, "0"), (1.0, "+1")] {
        let x = left + width * unit;
        paint::polyline(
            window,
            &[point(x, y - px(5.)), point(x, y + px(5.))],
            px(1.),
            theme.text_muted,
        );
        paint::label(
            window,
            cx,
            point((x - px(8.)).max(bounds.origin.x), bounds.origin.y + px(2.)),
            label,
            px(9.),
            theme.text_muted,
        );
    }
    if let (Some(min), Some(max)) = (
        history.iter().copied().reduce(f32::min),
        history.iter().copied().reduce(f32::max),
    ) {
        let from = left + width * ((min + 1.0) * 0.5);
        let to = left + width * ((max + 1.0) * 0.5);
        paint::rect(
            window,
            Bounds {
                origin: point(from, y - px(3.)),
                size: size((to - from).max(px(2.)), px(6.)),
            },
            theme.accent_soft,
        );
    }
    if let Some(value) = negative_peak {
        let x = left + width * ((value + 1.0) * 0.5);
        paint::polyline(
            window,
            &[point(x, y - px(8.)), point(x, y + px(8.))],
            px(2.),
            theme.danger,
        );
    }
    if let Some(value) = current {
        let x = left + width * ((value + 1.0) * 0.5);
        paint::rect(
            window,
            Bounds {
                origin: point(x - px(2.), y - px(6.)),
                size: size(px(4.), px(12.)),
            },
            theme.accent,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{actions, auxiliary_window::Surface, harness};

    fn frame(db: f32) -> VisualizerFrame {
        let waveform: Vec<_> = (0..1024)
            .map(|index| (std::f32::consts::TAU * index as f32 * 10. / 1024.).sin() * 0.5)
            .collect();
        VisualizerFrame {
            left: waveform.clone().into(),
            right: waveform.into(),
            sample_rate: 48_000.,
            window_samples: 1024,
            spectrum: vec![db; 96].into(),
            correlation: Some(1.),
        }
    }

    #[test]
    fn oscilloscope_span_uses_sample_rate_and_aligns_a_rising_crossing() {
        let mut frame = frame(-12.);
        frame.sample_rate = 1_000.;
        let mut left = vec![-1.; 1_000];
        left[800..].fill(1.);
        frame.left = left.into();
        frame.right = vec![0.; 1_000].into();

        assert_eq!(
            oscilloscope_window(&frame, OscilloscopeSpan::Short),
            Some(OscilloscopeWindow {
                start: 750,
                len: 250,
                trigger_offset: 50,
            })
        );
    }

    #[test]
    fn full_oscilloscope_span_uses_history_to_align_a_rising_crossing() {
        let mut frame = frame(-12.);
        frame.sample_rate = 1_000.;
        let mut left = vec![-1.; 2_048];
        left[1_200..].fill(1.);
        frame.left = left.into();
        frame.right = vec![0.; 2_048].into();

        assert_eq!(
            oscilloscope_window(&frame, OscilloscopeSpan::Full),
            Some(OscilloscopeWindow {
                start: 996,
                len: 1_024,
                trigger_offset: 204,
            })
        );
    }

    #[test]
    fn history_averages_power_and_retains_peaks() {
        let mut state = VisualizerState::default();
        let start = Instant::now();
        state.accept_at(frame(-20.), start);
        state.accept_at(frame(0.), start + Duration::from_millis(200));
        let alpha = 1.0 - (-1.0f32).exp();
        let expected = 10.0f32 * ((1.0 - alpha) * 0.01 + alpha).log10();
        assert!((state.mean[0] - expected).abs() < 0.001);
        state.accept_at(frame(-40.), start + Duration::from_millis(400));
        assert_eq!(state.peak[0], 0.);
    }

    #[test]
    fn correlation_tracks_three_seconds_and_holds_negative_peak_until_reset() {
        let mut state = VisualizerState::default();
        let start = Instant::now();
        let mut first = frame(-20.);
        first.correlation = Some(-0.8);
        state.accept_at(first, start);
        let mut second = frame(-20.);
        second.correlation = Some(0.5);
        state.accept_at(second, start + Duration::from_secs(4));

        assert_eq!(state.correlation_history.len(), 1);
        assert_eq!(state.correlation_negative_peak, Some(-0.8));
        state.correlation_history.clear();
        state.correlation_negative_peak = None;
        assert!(state.correlation_history.is_empty());
        assert_eq!(state.correlation_negative_peak, None);
    }

    #[test]
    fn automatic_display_gain_never_attenuates_or_changes_samples() {
        let left = [0.0, 0.09, -0.03];
        let right = [0.02, -0.01];
        assert!((display_auto_gain(&left, &right) - 10.0).abs() < 0.001);
        assert_eq!(display_auto_gain(&[1.2], &[0.0]), 1.0);
        assert_eq!(left, [0.0, 0.09, -0.03]);
    }

    #[gpui::test]
    fn visualizer_opens_freezes_saves_and_closes_without_editing(cx: &mut gpui::TestAppContext) {
        let (app, cx) = harness::open(cx);
        cx.dispatch_action(actions::ToggleVisualizer);
        app.update(cx, |app, _| app.visualizer.view = VisualizerView::All);
        harness::paint(&app, cx);
        let handle = app.read_with(cx, |app, _| app.auxiliary_windows[&Surface::Visualizer]);
        app.read_with(cx, |app, _| {
            let oscilloscope = app
                .visualizer
                .oscilloscope_bounds
                .get()
                .expect("oscilloscope painted");
            let spectrum = app
                .visualizer
                .spectrum_bounds
                .get()
                .expect("spectrum painted");
            assert!(spectrum.size.width > px(600.), "{spectrum:?}");
            assert_eq!(spectrum.size.width, oscilloscope.size.width);
            assert_eq!(spectrum.origin.x, oscilloscope.origin.x);
            assert_eq!(oscilloscope.size.height, px(220.));
            assert_eq!(spectrum.size.height, px(220.));
        });
        app.update(cx, |app, _| app.visualizer.accept(frame(-12.)));
        harness::paint(&app, cx);
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
        cx.dispatch_action(actions::VisualizerTimebase);
        cx.dispatch_action(actions::VisualizerView);
        cx.dispatch_action(actions::VisualizerSpectrumChannel);
        cx.dispatch_action(actions::VisualizerOscilloscopeGain);
        cx.dispatch_action(actions::VisualizerStereoAutoGain);
        app.read_with(cx, |app, _| {
            assert_eq!(app.visualizer.oscilloscope_span, OscilloscopeSpan::Short);
            assert_eq!(app.visualizer.view, VisualizerView::Oscilloscope);
            assert_eq!(app.visualizer.spectrum_mode, VisualizerSpectrum::Left);
            assert_eq!(app.visualizer.oscilloscope_gain, OscilloscopeGain::Double);
            assert!(app.visualizer.stereo_auto_gain);
        });
        cx.dispatch_action(actions::ToggleVisualizer);
        harness::paint(&app, cx);
        assert!(handle.read_with(cx, |_, _| ()).is_err());
    }

    #[gpui::test]
    fn visualizer_actions_follow_available_data_and_never_freeze_waiting(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = harness::open(cx);
        cx.dispatch_action(actions::ToggleVisualizer);
        harness::paint(&app, cx);
        let handle = app.read_with(cx, |app, _| app.auxiliary_windows[&Surface::Visualizer]);
        let cx = &mut gpui::VisualTestContext::from_window(handle.into(), cx);
        cx.run_until_parked();

        for selector in ["visualizer-freeze", "visualizer-save", "visualizer-reset"] {
            harness::click(selector, cx);
        }
        app.read_with(cx, |app, _| {
            assert!(!app.visualizer.frozen);
            assert!(app.visualizer.reference.is_empty());
            assert!(app.visualizer.mean.is_empty());
            assert!(app.visualizer.peak.is_empty());
        });

        app.update(cx, |app, cx| {
            app.visualizer.accept(frame(-12.));
            cx.notify();
        });
        cx.run_until_parked();
        harness::click("visualizer-freeze", cx);
        app.read_with(cx, |app, _| assert!(app.visualizer.frozen));
        harness::click("visualizer-freeze", cx);
        app.read_with(cx, |app, _| assert!(!app.visualizer.frozen));
        harness::click("visualizer-freeze", cx);
        app.read_with(cx, |app, _| assert!(app.visualizer.frozen));

        harness::click("visualizer-source", cx);
        app.read_with(cx, |app, _| {
            assert!(
                !app.visualizer.frozen,
                "a source change cannot leave Waiting frozen"
            );
            assert!(app.visualizer.frame.is_none());
        });
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
