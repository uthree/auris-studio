//! The analyser: what a strip sounds like, on five plots at once.
//!
//! Laid out as the Atelier analyser is — a spectrum beside a loudness meter, a spectrogram beside
//! a goniometer, an oscilloscope under them all, each a card with its plot on a sunken well — and
//! drawn with Atelier's own plots, so a bar, a heat ramp or a trace means the same thing here as
//! in every other window that uses them.
//!
//! Everything a view decides is a free function with tests underneath it; the view only lays
//! out what they return.
use super::widgets::{ButtonStyle, button};
use super::{analyser::FLOOR_DB, paint};
use crate::{app::AurisApp, theme::Theme};
use atelier::viz::{Heatmap, Spectrum, Vectorscope, Waveform};
use atelier::{
    Badge, Button, Card, ControlSize, IconName, Label, LevelMeter, METER_FLOOR_DB, SectionHeader,
    Segment, SegmentedControl, Stat, TextColor, TextSize, Tone,
};
use auris_i18n::Key;
use auris_session::{
    VisualizerFrame, VisualizerLoudness, VisualizerSpectrum, VisualizerTap, prelude::*,
};
use gpui::{
    AnyElement, Bounds, Hsla, Pixels, Window, canvas, div, point, prelude::*, px, relative, size,
};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// Lowest frequency the spectrum shows: the bottom of hearing.
const SPECTRUM_LOW_HZ: f64 = 20.0;

/// Highest frequency the spectrum shows: exactly ten octaves above the lowest, so each of
/// [`Spectrum`]'s ten vertical divisions is one octave.
const SPECTRUM_HIGH_HZ: f64 = SPECTRUM_LOW_HZ * 1024.0;

/// Octaves across the spectrum, one grid division each.
const SPECTRUM_OCTAVES: usize = 10;

/// Bands across the spectrum: six to the octave.
const SPECTRUM_BANDS: usize = SPECTRUM_OCTAVES * 6;

/// Decibels per horizontal division of the spectrum. [`Spectrum`] draws six, so the floor is
/// [`FLOOR_DB`].
const SPECTRUM_DB_STEP: f32 = 12.0;

/// How long the spectrogram remembers.
const SPECTROGRAM_SPAN: Duration = Duration::from_secs(3);

/// Columns across the spectrogram, one per 25 ms.
const SPECTROGRAM_COLUMNS: usize = 120;

const CORRELATION_HISTORY: Duration = Duration::from_secs(3);
const SPECTRUM_AVERAGE_SECONDS: f32 = 0.2;

/// How long a channel meter's peak mark stays put before it lets a lower one replace it.
const METER_HOLD: Duration = Duration::from_secs(2);

/// The integrated loudness the common streaming services normalise to, and how far either side
/// of it counts as on target.
const STREAMING_TARGET_LUFS: f32 = -14.0;
const STREAMING_TOLERANCE_LU: f32 = 1.0;

/// The true peak the same services ask a master to stay under.
const STREAMING_CEILING_DBTP: f32 = -1.0;

/// The width at which the cards stop pairing up and stack in one column.
const TWO_COLUMNS_FROM: Pixels = px(900.);

/// The width of the narrow card in each pair.
const SIDE_CARD_WIDTH: Pixels = px(300.);

/// A compact stereo scope using the same trigger and amplitude scale as the utility window.
pub(crate) fn waveform_thumbnail(frame: Option<VisualizerFrame>, theme: Theme) -> AnyElement {
    let (left, right) = frame
        .as_ref()
        .and_then(|frame| {
            oscilloscope_window(frame, OscilloscopeSpan::Full).map(|window| {
                let end = window.start + window.len;
                (
                    frame.left[window.start..end].to_vec(),
                    frame.right[window.start..end].to_vec(),
                )
            })
        })
        .unwrap_or_default();
    let series = theme.atelier.colors.series;
    div()
        .relative()
        .size_full()
        .bg(theme.atelier.colors.sunken)
        .child(
            div()
                .absolute()
                .inset_0()
                .child(Waveform::new(left).no_grid()),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .child(Waveform::new(right).no_grid().color(series[1])),
        )
        .into_any_element()
}

#[derive(Clone, Copy)]
pub(crate) enum VisualizerCommand {
    Toggle,
    Source,
    SetSource(bool),
    Tap,
    SetTap(VisualizerTap),
    Freeze,
    Average,
    Peaks,
    Save,
    Reset,
    Timebase,
    SetTimebase(OscilloscopeSpan),
    SpectrumChannel,
    SetSpectrumChannel(VisualizerSpectrum),
    CorrelationReset,
    OscilloscopeGain,
    SetOscilloscopeGain(OscilloscopeGain),
    StereoAutoGain,
    LoudnessReset,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum OscilloscopeGain {
    #[default]
    Unity,
    Double,
    Quadruple,
    Auto,
}

impl OscilloscopeGain {
    const ALL: [Self; 4] = [Self::Unity, Self::Double, Self::Quadruple, Self::Auto];

    fn next(self) -> Self {
        let index = Self::ALL.iter().position(|&gain| gain == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
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
pub(crate) enum OscilloscopeSpan {
    Short,
    Medium,
    #[default]
    Full,
}

impl OscilloscopeSpan {
    const ALL: [Self; 3] = [Self::Short, Self::Medium, Self::Full];

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
        let index = Self::ALL.iter().position(|&span| span == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }
}

/// The spectrum's channel readings, in the order the control offers them.
const SPECTRUM_CHANNELS: [VisualizerSpectrum; 5] = [
    VisualizerSpectrum::Average,
    VisualizerSpectrum::Left,
    VisualizerSpectrum::Right,
    VisualizerSpectrum::Maximum,
    VisualizerSpectrum::Mono,
];

pub(crate) struct VisualizerState {
    pub open: bool,
    selected: bool,
    source: Option<Option<TrackId>>,
    tap: VisualizerTap,
    frozen: bool,
    average: bool,
    peaks: bool,
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
    /// Past spectra, oldest first, one per [`SPECTROGRAM_SPAN`] / [`SPECTROGRAM_COLUMNS`].
    spectrogram: VecDeque<Vec<f32>>,
    last_column_at: Option<Instant>,
    /// The loudness as last shown; held while the display is frozen.
    loudness: VisualizerLoudness,
    /// Each channel's held sample peak in dBFS and when it was set.
    meter_hold: [Option<(f32, Instant)>; 2],
}

impl Default for VisualizerState {
    fn default() -> Self {
        Self {
            open: false,
            selected: false,
            source: None,
            tap: VisualizerTap::PostFader,
            frozen: false,
            average: false,
            // The Atelier analyser holds its peaks, and a spectrum without them forgets a
            // transient before anybody has seen it.
            peaks: true,
            spectrum_mode: VisualizerSpectrum::default(),
            oscilloscope_span: OscilloscopeSpan::default(),
            oscilloscope_gain: OscilloscopeGain::default(),
            stereo_auto_gain: false,
            frame: None,
            mean: Vec::new(),
            peak: Vec::new(),
            reference: Vec::new(),
            last_spectrum_at: None,
            correlation_history: VecDeque::new(),
            correlation_negative_peak: None,
            spectrogram: VecDeque::new(),
            last_column_at: None,
            loudness: VisualizerLoudness::default(),
            meter_hold: [None; 2],
        }
    }
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

    /// The spectrum as drawn: the running average or the newest frame, in dBFS.
    fn displayed_spectrum(&self) -> Vec<f32> {
        if self.average && !self.mean.is_empty() {
            self.mean.clone()
        } else {
            self.frame
                .as_ref()
                .map(|frame| frame.spectrum.to_vec())
                .unwrap_or_default()
        }
    }

    pub(crate) fn clear_live(&mut self) {
        self.frame = None;
        self.mean.clear();
        self.peak.clear();
        self.last_spectrum_at = None;
        self.correlation_history.clear();
        self.correlation_negative_peak = None;
        self.spectrogram.clear();
        self.last_column_at = None;
        self.loudness = VisualizerLoudness::default();
        self.meter_hold = [None; 2];
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
            let column_every = SPECTROGRAM_SPAN / SPECTROGRAM_COLUMNS as u32;
            if self
                .last_column_at
                .is_none_or(|last| now.saturating_duration_since(last) >= column_every)
            {
                self.spectrogram.push_back(frame.spectrum.to_vec());
                while self.spectrogram.len() > SPECTROGRAM_COLUMNS {
                    self.spectrogram.pop_front();
                }
                self.last_column_at = Some(now);
            }
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
        for (hold, peak) in self.meter_hold.iter_mut().zip(channel_peaks_db(&frame)) {
            if hold.is_none_or(|(held, at)| {
                peak >= held || now.saturating_duration_since(at) > METER_HOLD
            }) {
                *hold = Some((peak, now));
            }
        }
        self.frame = Some(frame);
    }

    fn clear_spectrum_mode(&mut self) {
        self.mean.clear();
        self.peak.clear();
        self.reference.clear();
        self.last_spectrum_at = None;
        self.spectrogram.clear();
        self.last_column_at = None;
    }

    /// The spectrogram as [`Heatmap`] wants it: row-major, highest band on top, newest column on
    /// the right, and blank columns on the left until three seconds have been heard.
    fn spectrogram_cells(&self) -> (usize, Vec<f32>) {
        let rows = self.spectrogram.back().map_or(SPECTRUM_BANDS, Vec::len);
        let mut cells = vec![0.0; rows * SPECTROGRAM_COLUMNS];
        let offset = SPECTROGRAM_COLUMNS - self.spectrogram.len().min(SPECTROGRAM_COLUMNS);
        for (index, column) in self.spectrogram.iter().enumerate() {
            for (band, &db) in column.iter().enumerate().take(rows) {
                cells[(rows - 1 - band) * SPECTROGRAM_COLUMNS + offset + index] =
                    spectrum_level(db);
            }
        }
        (rows, cells)
    }
}

impl AurisApp {
    pub(crate) fn close_visualizer(&mut self) {
        self.visualizer.open = false;
        if !self
            .settings
            .toolbar
            .contains(auris_session::ToolbarItem::Visualizer)
        {
            self.session.stop_visualizer();
        }
    }

    pub(crate) fn visualizer_command(&mut self, command: VisualizerCommand) {
        let state = &mut self.visualizer;
        match command {
            VisualizerCommand::Toggle => {
                state.open = !state.open;
                if !state.open {
                    self.close_visualizer();
                }
            }
            VisualizerCommand::Source => {
                state.selected = !state.selected;
                state.source = None;
                state.frozen = false;
                state.clear_live();
            }
            VisualizerCommand::SetSource(selected) => {
                if state.selected != selected {
                    self.visualizer_command(VisualizerCommand::Source);
                }
            }
            VisualizerCommand::Tap => {
                let tap = match state.tap {
                    VisualizerTap::PreFader => VisualizerTap::PostFader,
                    VisualizerTap::PostFader => VisualizerTap::PreFader,
                };
                self.visualizer_command(VisualizerCommand::SetTap(tap));
            }
            VisualizerCommand::SetTap(tap) => {
                if state.tap != tap {
                    // What was drawn was the other signal; nothing of it carries over.
                    state.tap = tap;
                    state.frozen = false;
                    state.clear_live();
                }
            }
            VisualizerCommand::Freeze if state.can_freeze() => state.frozen = !state.frozen,
            // A hold with no frame used to suppress polling forever while the window kept saying
            // "Waiting". A keyboard shortcut can still arrive when the toolbar button is
            // disabled, so the command itself owns the same availability rule.
            VisualizerCommand::Freeze => state.frozen = false,
            VisualizerCommand::Average => state.average = !state.average,
            VisualizerCommand::Peaks => state.peaks = !state.peaks,
            VisualizerCommand::Save => {
                if state.can_save() {
                    state.reference = state.displayed_spectrum();
                }
            }
            VisualizerCommand::Reset => {
                if state.can_reset() {
                    state.reference.clear();
                    let spectrum = state
                        .frame
                        .as_ref()
                        .map(|frame| frame.spectrum.to_vec())
                        .unwrap_or_default();
                    state.peak = spectrum.clone();
                    state.mean = spectrum;
                }
            }
            VisualizerCommand::Timebase => {
                state.oscilloscope_span = state.oscilloscope_span.next();
            }
            VisualizerCommand::SetTimebase(span) => state.oscilloscope_span = span,
            VisualizerCommand::SpectrumChannel => {
                let index = SPECTRUM_CHANNELS
                    .iter()
                    .position(|&mode| mode == state.spectrum_mode)
                    .unwrap_or(0);
                let next = SPECTRUM_CHANNELS[(index + 1) % SPECTRUM_CHANNELS.len()];
                self.visualizer_command(VisualizerCommand::SetSpectrumChannel(next));
            }
            VisualizerCommand::SetSpectrumChannel(mode) => {
                if state.spectrum_mode != mode {
                    state.spectrum_mode = mode;
                    state.clear_spectrum_mode();
                }
            }
            VisualizerCommand::CorrelationReset => {
                state.correlation_history.clear();
                state.correlation_negative_peak = None;
            }
            VisualizerCommand::OscilloscopeGain => {
                state.oscilloscope_gain = state.oscilloscope_gain.next();
            }
            VisualizerCommand::SetOscilloscopeGain(gain) => state.oscilloscope_gain = gain,
            VisualizerCommand::StereoAutoGain => state.stereo_auto_gain = !state.stereo_auto_gain,
            VisualizerCommand::LoudnessReset => {
                state.loudness = VisualizerLoudness::default();
                self.session.reset_visualizer_loudness();
            }
        }
    }

    pub(crate) fn poll_visualizer(&mut self) {
        let in_toolbar = self
            .settings
            .toolbar
            .contains(auris_session::ToolbarItem::Visualizer);
        if !self.visualizer.open && !in_toolbar {
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
        self.session.watch_visualizer(source, self.visualizer.tap);
        // Measured on every repaint, frozen or not: integrated loudness is only that if it has
        // heard every sample, and holding the display still does not stop the music.
        let loudness = self.session.measure_visualizer_loudness();
        if self.visualizer.frozen {
            return;
        }
        self.visualizer.loudness = loudness;
        if let Some(frame) = self.session.visualizer_frame(
            SPECTRUM_LOW_HZ,
            SPECTRUM_HIGH_HZ,
            SPECTRUM_BANDS,
            Some(self.visualizer.spectrum_mode),
        ) {
            self.visualizer.accept(frame);
        }
    }

    /// Compact master/selected-track spectrum, sharing the utility window's live tap, `height`
    /// tall: the transport row's, or the title bar's when that carries the transport.
    pub(crate) fn render_toolbar_visualizer(
        &self,
        height: gpui::Pixels,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let spectrum = self.visualizer.displayed_spectrum();
        button(
            "toolbar-visualizer",
            self.t(Key::ToolbarSpectrum),
            ButtonStyle::Ghost,
            self.visualizer.open,
            theme.accent,
            &theme,
            cx.listener(|this, _, _, cx| {
                this.visualizer_command(VisualizerCommand::Toggle);
                cx.notify();
            }),
        )
        .tooltip(self.tip(Key::VisualizerTitle, "view.visualizer"))
        .flex_col()
        .gap_1()
        .w(px(124.))
        .h(height)
        .flex_shrink_0()
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    paint::rect(window, bounds, theme.surface_sunken);
                    if spectrum.is_empty() {
                        return;
                    }
                    let width = bounds.size.width / spectrum.len() as f32;
                    for (index, db) in spectrum.iter().enumerate() {
                        let height = bounds.size.height * spectrum_level(*db);
                        paint::rect(
                            window,
                            Bounds {
                                origin: point(
                                    bounds.left() + width * index as f32,
                                    bounds.bottom() - height,
                                ),
                                size: size(width, height),
                            },
                            theme.accent,
                        );
                    }
                },
            )
            .w_full()
            .h(px(24.)),
        )
    }

    pub(crate) fn render_visualizer(
        &mut self,
        window: &Window,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let spacing = self.theme.atelier.spacing;
        let background = self.theme.atelier.colors.background;
        let toolbar = self.visualizer_toolbar(cx);
        let spectrum = self.spectrum_card(cx);
        let loudness = self.loudness_card(cx);
        let spectrogram = self.spectrogram_card();
        let stereo = self.stereo_card(cx);
        let oscilloscope = self.oscilloscope_card(cx);
        let two_columns = window.viewport_size().width >= TWO_COLUMNS_FROM;
        let pair = |main: AnyElement, side: AnyElement| {
            if two_columns {
                div()
                    .flex()
                    .gap(spacing.lg)
                    .child(div().flex().flex_1().min_w_0().child(main))
                    .child(div().flex().flex_none().w(SIDE_CARD_WIDTH).child(side))
            } else {
                div()
                    .flex()
                    .flex_col()
                    .gap(spacing.lg)
                    .child(main)
                    .child(side)
            }
        };
        div()
            .id("visualizer")
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(background)
            .child(toolbar)
            .child(
                div()
                    .id("visualizer-cards")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap(spacing.lg)
                    .p(spacing.lg)
                    .child(pair(spectrum, loudness))
                    .child(pair(spectrogram, stereo))
                    .child(oscilloscope),
            )
            .into_any_element()
    }

    /// What is being listened to and whether the display is live: master or the selected track,
    /// before or after its fader, and Freeze.
    fn visualizer_toolbar(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let tokens = &self.theme.atelier;
        let state = &self.visualizer;
        let source_name = if state.selected {
            self.selected_track
                .and_then(|id| self.session.project().track(id))
                .map(|track| track.name.clone())
                .unwrap_or_else(|| self.t(Key::VisualizerNoTrack).to_owned())
        } else {
            self.t(Key::Master).to_owned()
        };
        let (status, tone) = if state.frozen {
            (Key::PhysicalFrozen, Tone::Warning)
        } else if state.frame.is_some() {
            (Key::PhysicalLive, Tone::Success)
        } else {
            (Key::VisualizerWaiting, Tone::Neutral)
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(tokens.spacing.sm)
            .px(tokens.spacing.lg)
            .py(tokens.spacing.sm)
            .border_b_1()
            .border_color(tokens.colors.border)
            .child(
                SegmentedControl::new(
                    "visualizer-source",
                    [
                        Segment::label(self.t(Key::Master)),
                        Segment::label(self.t(Key::VisualizerSelectedTrack)),
                    ],
                    usize::from(state.selected),
                )
                .size(ControlSize::Small)
                .on_select(cx.listener(|this, index: &usize, _, cx| {
                    this.visualizer_command(VisualizerCommand::SetSource(*index == 1));
                    cx.notify();
                })),
            )
            .child(
                SegmentedControl::new(
                    "visualizer-tap",
                    [
                        Segment::label(self.t(Key::VisualizerPreFader)),
                        Segment::label(self.t(Key::VisualizerPostFader)),
                    ],
                    usize::from(state.tap == VisualizerTap::PostFader),
                )
                .size(ControlSize::Small)
                .on_select(cx.listener(|this, index: &usize, _, cx| {
                    let tap = if *index == 0 {
                        VisualizerTap::PreFader
                    } else {
                        VisualizerTap::PostFader
                    };
                    this.visualizer_command(VisualizerCommand::SetTap(tap));
                    cx.notify();
                })),
            )
            .child(
                Button::new(
                    "visualizer-freeze",
                    self.t(if state.frozen {
                        Key::PhysicalResume
                    } else {
                        Key::VisualizerFreeze
                    }),
                )
                .icon(if state.frozen {
                    IconName::Play
                } else {
                    IconName::Pause
                })
                .size(ControlSize::Small)
                .selected(state.frozen)
                .disabled(!state.can_freeze())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.visualizer_command(VisualizerCommand::Freeze);
                    cx.notify();
                })),
            )
            .child(div().flex_1())
            .child(
                Label::new(source_name)
                    .size(TextSize::Small)
                    .color(TextColor::Muted),
            )
            .child(Badge::new(self.t(status)).tone(tone).dot())
            .into_any_element()
    }

    fn spectrum_card(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let tokens = &self.theme.atelier;
        let spacing = tokens.spacing;
        let state = &self.visualizer;
        let levels: Vec<f32> = state
            .displayed_spectrum()
            .into_iter()
            .map(spectrum_level)
            .collect();
        let peaks: Vec<f32> = state.peak.iter().copied().map(spectrum_level).collect();
        let reference: Vec<f32> = state
            .reference
            .iter()
            .copied()
            .map(spectrum_level)
            .collect();
        let mut plot = Spectrum::new(levels).octaves(SPECTRUM_OCTAVES);
        if state.peaks && !peaks.is_empty() {
            plot = plot.peaks(peaks);
        }
        let channel = SPECTRUM_CHANNELS
            .iter()
            .position(|&mode| mode == state.spectrum_mode)
            .unwrap_or(0);
        let controls = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(spacing.sm)
            .child(
                SegmentedControl::new(
                    "visualizer-spectrum-channel",
                    SPECTRUM_CHANNELS
                        .map(|mode| Segment::label(self.t(spectrum_channel_key(mode)))),
                    channel,
                )
                .size(ControlSize::Small)
                .on_select(cx.listener(|this, index: &usize, _, cx| {
                    if let Some(&mode) = SPECTRUM_CHANNELS.get(*index) {
                        this.visualizer_command(VisualizerCommand::SetSpectrumChannel(mode));
                    }
                    cx.notify();
                })),
            )
            .child(self.visualizer_toggle(
                "visualizer-average",
                Key::VisualizerAverage,
                state.average,
                true,
                VisualizerCommand::Average,
                cx,
            ))
            .child(self.visualizer_toggle(
                "visualizer-peaks",
                Key::VisualizerPeaks,
                state.peaks,
                true,
                VisualizerCommand::Peaks,
                cx,
            ))
            .child(self.visualizer_toggle(
                "visualizer-save",
                Key::VisualizerSave,
                false,
                state.can_save(),
                VisualizerCommand::Save,
                cx,
            ))
            .child(self.visualizer_toggle(
                "visualizer-reset",
                Key::VisualizerReset,
                false,
                state.can_reset(),
                VisualizerCommand::Reset,
                cx,
            ));
        let scale_width = px(28.);
        let reference_color = tokens.colors.series[2];
        let well = plot_well(&self.theme)
            .relative()
            .flex_1()
            .h_full()
            .child(div().absolute().inset_0().p(spacing.xs).child(plot))
            .when(!reference.is_empty(), |well| {
                well.child(
                    div()
                        .absolute()
                        .inset_0()
                        .p(spacing.xs)
                        .child(reference_curve(reference, reference_color)),
                )
            });
        let scale = div()
            .relative()
            .flex_none()
            .w(scale_width)
            .h_full()
            .children((0..6).map(|row| {
                let db = -(row as f32 * SPECTRUM_DB_STEP);
                let text = if row == 0 {
                    "0".to_owned()
                } else {
                    minus(format!("{db:.0}"))
                };
                axis_mark(
                    Label::new(text)
                        .size(TextSize::XSmall)
                        .mono()
                        .color(TextColor::Faint),
                    1.0 - spectrum_level(db),
                    true,
                    spacing.xs,
                )
            }));
        let axis = div()
            .flex()
            .gap(spacing.sm)
            .child(div().flex_none().w(scale_width))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .h(px(14.))
                    .mx(spacing.xs)
                    .children(
                        [20., 50., 100., 200., 500., 1e3, 2e3, 5e3, 1e4, 2e4]
                            .into_iter()
                            .map(|hz| {
                                axis_mark(
                                    Label::new(frequency_label(hz))
                                        .size(TextSize::XSmall)
                                        .mono()
                                        .color(TextColor::Faint),
                                    spectrum_x(hz),
                                    false,
                                    px(0.),
                                )
                            }),
                    ),
            );
        visualizer_card(
            "visualizer-spectrum",
            self.t(Key::VisualizerSpectrum),
            self.t(Key::VisualizerSpectrumDescription),
        )
        .flex_1()
        .child(controls)
        .child(
            div()
                .flex()
                .flex_1()
                .gap(spacing.sm)
                .h(px(240.))
                .child(scale)
                .child(well),
        )
        .child(axis)
        .child(
            Label::new(self.t(Key::VisualizerLegend))
                .size(TextSize::XSmall)
                .color(TextColor::Faint),
        )
        .into_any_element()
    }

    fn loudness_card(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let tokens = &self.theme.atelier;
        let spacing = tokens.spacing;
        let loudness = self.visualizer.loudness;
        let lufs = "LUFS";
        let readings = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(spacing.md)
            .child(figure(
                self.t(Key::VisualizerMomentary),
                loudness.momentary,
                lufs,
                TextSize::Display,
            ))
            .child(figure(
                self.t(Key::VisualizerShortTerm),
                loudness.short_term,
                lufs,
                TextSize::XLarge,
            ))
            .child(figure(
                self.t(Key::VisualizerIntegrated),
                loudness.integrated,
                lufs,
                TextSize::XLarge,
            ))
            .child(
                div()
                    .flex()
                    .gap(spacing.lg)
                    .child(figure(
                        self.t(Key::VisualizerTruePeak),
                        loudness.true_peak,
                        "dBTP",
                        TextSize::XLarge,
                    ))
                    .child(figure(
                        self.t(Key::VisualizerRange),
                        loudness.range,
                        "LU",
                        TextSize::XLarge,
                    )),
            )
            .children(
                streaming_verdict(&loudness)
                    .map(|(key, tone)| Badge::new(self.t(key)).tone(tone).dot()),
            );
        let now = Instant::now();
        let peaks = self
            .visualizer
            .frame
            .as_ref()
            .map_or([METER_FLOOR_DB; 2], channel_peaks_db);
        // Atelier's meters fill the height the row stretches this column to.
        let meters = div()
            .id("visualizer-meters")
            .debug_selector(|| "visualizer-meters".into())
            .flex()
            .flex_none()
            .gap(spacing.xs)
            .min_h(px(230.))
            .children(
                peaks
                    .into_iter()
                    .zip(self.visualizer.meter_hold)
                    .map(|(peak, hold)| {
                        let meter = LevelMeter::db(peak).thickness(px(8.));
                        match hold {
                            Some((held, at)) if now.saturating_duration_since(at) <= METER_HOLD => {
                                meter.peak_db(held)
                            }
                            _ => meter,
                        }
                    }),
            );
        visualizer_card_with(
            "visualizer-loudness",
            self.t(Key::VisualizerLoudness),
            "EBU R128",
            Some(
                Button::icon_only("visualizer-loudness-reset", IconName::Refresh)
                    .ghost()
                    .size(ControlSize::Small)
                    .tooltip(self.t(Key::VisualizerLoudnessReset))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.visualizer_command(VisualizerCommand::LoudnessReset);
                        cx.notify();
                    }))
                    .into_any_element(),
            ),
        )
        .w_full()
        .child(
            div()
                .flex()
                .flex_1()
                .gap(spacing.lg)
                .child(readings)
                .child(meters),
        )
        .into_any_element()
    }

    fn spectrogram_card(&self) -> AnyElement {
        let tokens = &self.theme.atelier;
        let spacing = tokens.spacing;
        let (rows, cells) = self.visualizer.spectrogram_cells();
        visualizer_card(
            "visualizer-spectrogram",
            self.t(Key::VisualizerSpectrogram),
            self.t(Key::VisualizerSpectrogramDescription),
        )
        .flex_1()
        .child(
            plot_well(&self.theme)
                .id("visualizer-spectrogram-plot")
                .flex_1()
                .min_h(px(180.))
                .child(Heatmap::new(SPECTROGRAM_COLUMNS, rows, cells)),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(spacing.sm)
                .child(
                    Label::new(self.t(Key::VisualizerQuiet))
                        .size(TextSize::XSmall)
                        .color(TextColor::Faint),
                )
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .h(px(6.))
                        .rounded(px(3.))
                        .overflow_hidden()
                        .children(
                            tokens
                                .colors
                                .heat
                                .iter()
                                .map(|color| div().flex_1().h_full().bg(*color)),
                        ),
                )
                .child(
                    Label::new(self.t(Key::VisualizerLoud))
                        .size(TextSize::XSmall)
                        .color(TextColor::Faint),
                ),
        )
        .into_any_element()
    }

    fn stereo_card(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let tokens = &self.theme.atelier;
        let spacing = tokens.spacing;
        let state = &self.visualizer;
        let points: Vec<(f32, f32)> = state.frame.as_ref().map_or_else(Vec::new, |frame| {
            let (left, right) = frame_window(frame);
            let gain = if state.stereo_auto_gain {
                display_auto_gain(left, right)
            } else {
                1.0
            };
            left.iter()
                .zip(right)
                .map(|(&left, &right)| (left * gain, right * gain))
                .collect()
        });
        let correlation = state.frame.as_ref().and_then(|frame| frame.correlation);
        let bar_color = if correlation.is_some_and(|value| value < 0.0) {
            tokens.colors.danger
        } else {
            tokens.colors.success
        };
        let value = correlation.unwrap_or(0.0).clamp(-1.0, 1.0);
        // Where the correlation has been in the last three seconds, behind where it is now.
        let seen = state
            .correlation_history
            .iter()
            .map(|point| point.value)
            .fold(None, |range: Option<(f32, f32)>, value| {
                Some(range.map_or((value, value), |(low, high)| {
                    (low.min(value), high.max(value))
                }))
            });
        let bar = div()
            .relative()
            .flex_1()
            .h(px(6.))
            .rounded(px(3.))
            .bg(tokens.colors.sunken)
            .child(
                div()
                    .absolute()
                    .left(relative(0.5))
                    .top_0()
                    .bottom_0()
                    .w(px(1.))
                    .bg(tokens.colors.grid),
            )
            .when_some(seen, |bar, (low, high)| {
                bar.child(
                    div()
                        .absolute()
                        .left(relative((low.clamp(-1.0, 1.0) + 1.0) / 2.0))
                        .w(relative((high - low).clamp(0.0, 2.0) / 2.0))
                        .top_0()
                        .bottom_0()
                        .rounded(px(3.))
                        .bg(bar_color.opacity(0.25)),
                )
            })
            .when(correlation.is_some(), |bar| {
                bar.child(
                    div()
                        .absolute()
                        .left(relative(0.5 + value.min(0.0) / 2.0))
                        .w(relative(value.abs() / 2.0))
                        .top_0()
                        .bottom_0()
                        .rounded(px(3.))
                        .bg(bar_color),
                )
            });
        let faint = |text: &str| {
            Label::new(text.to_owned())
                .size(TextSize::XSmall)
                .mono()
                .color(TextColor::Faint)
        };
        visualizer_card_with(
            "visualizer-stereo",
            self.t(Key::VisualizerStereo),
            self.t(Key::VisualizerStereoDescription),
            Some(
                self.visualizer_toggle(
                    "visualizer-stereo-auto-gain",
                    Key::VisualizerStereoAutoGain,
                    state.stereo_auto_gain,
                    true,
                    VisualizerCommand::StereoAutoGain,
                    cx,
                )
                .into_any_element(),
            ),
        )
        .w_full()
        .child(
            plot_well(&self.theme)
                .id("visualizer-stereo-plot")
                .flex_1()
                .min_h(px(180.))
                .child(Vectorscope::new(points)),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(spacing.sm)
                .child(faint("−1"))
                .child(bar)
                .child(faint("+1"))
                .child(
                    Label::new(
                        correlation.map_or_else(|| "—".to_owned(), |value| signed(value, 2)),
                    )
                    .size(TextSize::Small)
                    .mono(),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(spacing.sm)
                .child(
                    Label::new(format!(
                        "{} {}",
                        self.t(Key::VisualizerNegativePeak),
                        state
                            .correlation_negative_peak
                            .map_or_else(|| "—".to_owned(), |value| signed(value, 2)),
                    ))
                    .size(TextSize::XSmall)
                    .color(TextColor::Muted),
                )
                .child(
                    Button::icon_only("visualizer-correlation-reset", IconName::Refresh)
                        .ghost()
                        .size(ControlSize::Small)
                        .tooltip(self.t(Key::VisualizerCorrelationReset))
                        .disabled(
                            state.correlation_history.is_empty()
                                && state.correlation_negative_peak.is_none(),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.visualizer_command(VisualizerCommand::CorrelationReset);
                            cx.notify();
                        })),
                ),
        )
        .into_any_element()
    }

    fn oscilloscope_card(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let tokens = &self.theme.atelier;
        let spacing = tokens.spacing;
        let state = &self.visualizer;
        let series = tokens.colors.series;
        let shown = state.frame.as_ref().and_then(|frame| {
            oscilloscope_window(frame, state.oscilloscope_span).map(|window| (frame, window))
        });
        let (left, right) = shown.map_or_else(Default::default, |(frame, window)| {
            let end = window.start + window.len;
            let (left, right) = (
                &frame.left[window.start..end],
                &frame.right[window.start..end],
            );
            let gain = state.oscilloscope_gain.multiplier(left, right);
            (
                left.iter().map(|sample| sample * gain).collect::<Vec<_>>(),
                right.iter().map(|sample| sample * gain).collect::<Vec<_>>(),
            )
        });
        // The spans as the times they show, at the rate of what is being shown.
        let (rate, window_samples, available) = state.frame.as_ref().map_or(
            // Before the first frame: what the engine delivers, at the commonest rate.
            (48_000.0, 1024, 1024),
            |frame| {
                (
                    frame.sample_rate,
                    frame.window_samples,
                    frame.left.len().min(frame.right.len()),
                )
            },
        );
        let span_labels = OscilloscopeSpan::ALL.map(|span| {
            let samples = span.sample_count(available.max(2), window_samples);
            Segment::label(format!("{:.1} ms", samples as f64 * 1000.0 / rate))
        });
        let span_index = OscilloscopeSpan::ALL
            .iter()
            .position(|&span| span == state.oscilloscope_span)
            .unwrap_or(0);
        let gain_index = OscilloscopeGain::ALL
            .iter()
            .position(|&gain| gain == state.oscilloscope_gain)
            .unwrap_or(0);
        let legend = |label: &'static str, color: Hsla| {
            div()
                .flex()
                .items_center()
                .gap(spacing.xs)
                .child(div().w(px(12.)).h(px(2.)).bg(color))
                .child(
                    Label::new(label)
                        .size(TextSize::XSmall)
                        .color(TextColor::Muted),
                )
        };
        visualizer_card(
            "visualizer-oscilloscope",
            self.t(Key::VisualizerOscilloscope),
            self.t(Key::VisualizerOscilloscopeHint),
        )
        .w_full()
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(spacing.sm)
                .child(
                    Label::new(self.t(Key::VisualizerTimebase))
                        .size(TextSize::Small)
                        .color(TextColor::Muted),
                )
                .child(
                    SegmentedControl::new("visualizer-timebase", span_labels, span_index)
                        .size(ControlSize::Small)
                        .on_select(cx.listener(|this, index: &usize, _, cx| {
                            if let Some(&span) = OscilloscopeSpan::ALL.get(*index) {
                                this.visualizer_command(VisualizerCommand::SetTimebase(span));
                            }
                            cx.notify();
                        })),
                )
                .child(
                    Label::new(self.t(Key::VisualizerDisplayGain))
                        .size(TextSize::Small)
                        .color(TextColor::Muted),
                )
                .child(
                    SegmentedControl::new(
                        "visualizer-oscilloscope-gain",
                        [
                            Segment::label("1×"),
                            Segment::label("2×"),
                            Segment::label("4×"),
                            Segment::label(self.t(Key::VisualizerAuto)),
                        ],
                        gain_index,
                    )
                    .size(ControlSize::Small)
                    .on_select(cx.listener(|this, index: &usize, _, cx| {
                        if let Some(&gain) = OscilloscopeGain::ALL.get(*index) {
                            this.visualizer_command(VisualizerCommand::SetOscilloscopeGain(gain));
                        }
                        cx.notify();
                    })),
                ),
        )
        .child(
            plot_well(&self.theme)
                .id("visualizer-oscilloscope-plot")
                .relative()
                .h(px(160.))
                .child(div().absolute().inset_0().child(Waveform::new(left)))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .child(Waveform::new(right).no_grid().color(series[1])),
                ),
        )
        .child(
            div()
                .flex()
                .gap(spacing.lg)
                .child(legend(self.t(Key::VisualizerLeftChannel), series[0]))
                .child(legend(self.t(Key::VisualizerRightChannel), series[1])),
        )
        .into_any_element()
    }

    /// A small Atelier button that runs `command`, lit while `active`.
    fn visualizer_toggle(
        &self,
        id: &'static str,
        key: Key,
        active: bool,
        enabled: bool,
        command: VisualizerCommand,
        cx: &mut gpui::Context<Self>,
    ) -> Button {
        Button::new(id, self.t(key))
            .size(ControlSize::Small)
            .selected(active)
            .disabled(!enabled)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.visualizer_command(command);
                cx.notify();
            }))
    }
}

/// One analyser card: a title, what it shows, and room for its controls and plot.
fn visualizer_card(id: &'static str, title: &str, description: &str) -> Card {
    visualizer_card_with(id, title, description, None)
}

/// [`visualizer_card`] with an action at the trailing end of its header.
fn visualizer_card_with(
    id: &'static str,
    title: &str,
    description: &str,
    action: Option<AnyElement>,
) -> Card {
    let mut header = SectionHeader::new(title.to_owned()).description(description.to_owned());
    if let Some(action) = action {
        header = header.action(action);
    }
    Card::new()
        .id(id)
        .flex()
        .flex_col()
        .gap(px(12.))
        .p(px(20.))
        .child(header)
}

/// The sunken well a plot sits in: content, never chrome.
fn plot_well(theme: &Theme) -> gpui::Div {
    div()
        .w_full()
        .rounded(theme.atelier.shape.radius_sm)
        .bg(theme.atelier.colors.sunken)
        .overflow_hidden()
}

/// `label` pinned at `at` (0 to 1) along a box, centred on that point: down the box when
/// `vertical`, across it otherwise. `inset` is the plot's own padding inside its well.
fn axis_mark(label: Label, at: f32, vertical: bool, inset: Pixels) -> gpui::Div {
    let anchor = div().absolute();
    let anchor = if vertical {
        anchor
            .left_0()
            .right_0()
            .top(relative(at))
            .mt(inset * (1.0 - 2.0 * at))
    } else {
        anchor.top_0().left(relative(at))
    };
    let mark = div().absolute().flex().whitespace_nowrap().child(label);
    anchor.child(if vertical {
        mark.top(px(-7.)).right_0()
    } else {
        mark.left(px(-14.)).top_0()
    })
}

/// A line through the saved comparison, drawn over the bars it is compared with.
fn reference_curve(levels: Vec<f32>, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds: Bounds<Pixels>, _, window, _| {
            if levels.is_empty() {
                return;
            }
            let band = bounds.size.width / levels.len() as f32;
            let points: Vec<_> = levels
                .iter()
                .enumerate()
                .map(|(index, level)| {
                    point(
                        bounds.left() + band * (index as f32 + 0.5),
                        bounds.bottom() - bounds.size.height * level.clamp(0.0, 1.0),
                    )
                })
                .collect();
            paint::polyline(window, &points, px(1.5), color);
        },
    )
    .size_full()
}

/// One loudness reading: Atelier's [`Stat`] with its caption, the figure to one decimal with a
/// true minus, and the unit; a dash before anything is measured.
fn figure(caption: &str, value: Option<f32>, unit: &str, size: TextSize) -> Stat {
    Stat::maybe(
        caption.to_owned(),
        value.map(|value| signed_plain(value, 1)),
    )
    .unit(unit.to_owned())
    .size(size)
}

fn spectrum_channel_key(mode: VisualizerSpectrum) -> Key {
    match mode {
        VisualizerSpectrum::Average => Key::VisualizerSpectrumAverage,
        VisualizerSpectrum::Left => Key::VisualizerSpectrumLeft,
        VisualizerSpectrum::Right => Key::VisualizerSpectrumRight,
        VisualizerSpectrum::Maximum => Key::VisualizerSpectrumMaximum,
        VisualizerSpectrum::Mono => Key::VisualizerSpectrumMono,
    }
}

/// Where a level in dBFS sits up the spectrum, from 0 at [`FLOOR_DB`] to 1 at full scale.
fn spectrum_level(db: f32) -> f32 {
    (1.0 - db / FLOOR_DB).clamp(0.0, 1.0)
}

/// Where a frequency sits across the spectrum, from 0 at the left edge to 1 at the right.
fn spectrum_x(hz: f64) -> f32 {
    ((hz / SPECTRUM_LOW_HZ).log2() / SPECTRUM_OCTAVES as f64).clamp(0.0, 1.0) as f32
}

/// A frequency as the axis writes it: 500, 1k, 20k.
fn frequency_label(hz: f64) -> String {
    if hz >= 1000.0 {
        format!("{}k", hz / 1000.0)
    } else {
        format!("{hz}")
    }
}

/// The sample peak of each channel's newest window, in dBFS, no lower than the meter's floor.
fn channel_peaks_db(frame: &VisualizerFrame) -> [f32; 2] {
    let (left, right) = frame_window(frame);
    [left, right].map(|samples| {
        let peak = samples
            .iter()
            .copied()
            .filter(|sample| sample.is_finite())
            .map(f32::abs)
            .fold(0.0, f32::max);
        if peak > 0.0 {
            (20.0 * peak.log10()).max(METER_FLOOR_DB)
        } else {
            METER_FLOOR_DB
        }
    })
}

/// What the loudness says about a master meant for the streaming services, once there is an
/// integrated loudness to say it about. The true peak is checked first: a master that is the
/// right loudness but clips a converter is not on target.
fn streaming_verdict(loudness: &VisualizerLoudness) -> Option<(Key, Tone)> {
    let integrated = loudness.integrated?;
    if loudness
        .true_peak
        .is_some_and(|peak| peak > STREAMING_CEILING_DBTP)
    {
        Some((Key::VisualizerPeakOver, Tone::Warning))
    } else if integrated > STREAMING_TARGET_LUFS + STREAMING_TOLERANCE_LU {
        Some((Key::VisualizerTooLoud, Tone::Warning))
    } else if integrated < STREAMING_TARGET_LUFS - STREAMING_TOLERANCE_LU {
        Some((Key::VisualizerTooQuiet, Tone::Neutral))
    } else {
        Some((Key::VisualizerOnTarget, Tone::Success))
    }
}

/// `text` with its hyphen-minus written as a minus sign, as a number is printed.
fn minus(text: String) -> String {
    text.replace('-', "\u{2212}")
}

/// `value` to `decimals` places with a minus sign when negative and nothing when not.
fn signed_plain(value: f32, decimals: usize) -> String {
    minus(format!("{value:.decimals$}"))
}

/// `value` to `decimals` places with its sign always shown: `+0.72`, `−0.31`.
fn signed(value: f32, decimals: usize) -> String {
    minus(format!("{value:+.decimals$}"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{actions, auxiliary_window::Surface, harness};

    #[gpui::test]
    fn toolbar_visualizer_stays_visible_when_utility_window_closes(cx: &mut gpui::TestAppContext) {
        let (app, cx) = harness::open(cx);
        app.update(cx, |app, cx| {
            app.settings
                .toolbar
                .entries
                .iter_mut()
                .find(|entry| entry.item == auris_session::ToolbarItem::Visualizer)
                .unwrap()
                .visible = true;
            app.visualizer.accept(frame(-12.));
            cx.notify();
        });
        harness::resize(&app, cx, size(px(1360.), px(700.)));
        let button = cx.debug_bounds("toolbar-visualizer").unwrap();
        assert!(button.size.height < crate::theme::Metrics::TRANSPORT_HEIGHT);
        harness::click("toolbar-visualizer", cx);
        app.read_with(cx, |app, _| assert!(app.visualizer.open));
        cx.dispatch_action(actions::ToggleVisualizer);
        harness::paint(&app, cx);
        app.read_with(cx, |app, _| {
            assert!(!app.visualizer.open);
            assert!(
                app.settings
                    .toolbar
                    .contains(auris_session::ToolbarItem::Visualizer)
            );
        });
        assert!(cx.debug_bounds("toolbar-visualizer").is_some());
    }

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
        let before = app.read_with(cx, |app, _| app.session.project().clone());
        cx.dispatch_action(actions::ToggleVisualizer);
        harness::paint(&app, cx);
        let handle = app.read_with(cx, |app, _| app.auxiliary_windows[&Surface::Visualizer]);
        let mut utility = gpui::VisualTestContext::from_window(handle.into(), cx);
        utility.simulate_resize(size(px(1200.), px(900.)));
        app.update(&mut utility, |app, cx| {
            app.visualizer.accept(frame(-12.));
            cx.notify();
        });
        utility.run_until_parked();
        // The five cards, paired the way the Atelier analyser pairs them when there is room.
        let mut card = |name| {
            utility
                .debug_bounds(name)
                .unwrap_or_else(|| panic!("no {name} card was drawn"))
        };
        let spectrum = card("visualizer-spectrum");
        let loudness = card("visualizer-loudness");
        let spectrogram = card("visualizer-spectrogram");
        let stereo = card("visualizer-stereo");
        let oscilloscope = card("visualizer-oscilloscope");
        assert!(
            loudness.left() > spectrum.right(),
            "{spectrum:?} {loudness:?}"
        );
        assert_eq!(loudness.size.width, SIDE_CARD_WIDTH);
        assert_eq!(stereo.size.width, SIDE_CARD_WIDTH);
        assert!(spectrogram.top() > spectrum.bottom());
        assert!(oscilloscope.top() > spectrogram.bottom());
        assert_eq!(oscilloscope.left(), spectrum.left());
        assert_eq!(oscilloscope.right(), loudness.right());

        harness::click("visualizer-freeze", &mut utility);
        harness::click("visualizer-save", &mut utility);
        app.read_with(&utility, |app, _| {
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
        cx.dispatch_action(actions::VisualizerSpectrumChannel);
        cx.dispatch_action(actions::VisualizerOscilloscopeGain);
        cx.dispatch_action(actions::VisualizerStereoAutoGain);
        cx.dispatch_action(actions::VisualizerTap);
        cx.dispatch_action(actions::VisualizerLoudnessReset);
        app.read_with(cx, |app, _| {
            assert_eq!(app.visualizer.oscilloscope_span, OscilloscopeSpan::Short);
            assert_eq!(app.visualizer.spectrum_mode, VisualizerSpectrum::Left);
            assert_eq!(app.visualizer.oscilloscope_gain, OscilloscopeGain::Double);
            assert!(app.visualizer.stereo_auto_gain);
            assert_eq!(app.visualizer.tap, VisualizerTap::PreFader);
            assert_eq!(app.session.project(), &before, "watching is not editing");
        });
        cx.dispatch_action(actions::ToggleVisualizer);
        harness::paint(&app, cx);
        assert!(handle.read_with(cx, |_, _| ()).is_err());
    }

    #[gpui::test]
    fn a_narrow_window_stacks_the_cards_in_one_column(cx: &mut gpui::TestAppContext) {
        let (app, cx) = harness::open(cx);
        cx.dispatch_action(actions::ToggleVisualizer);
        harness::paint(&app, cx);
        let handle = app.read_with(cx, |app, _| app.auxiliary_windows[&Surface::Visualizer]);
        let mut utility = gpui::VisualTestContext::from_window(handle.into(), cx);
        utility.simulate_resize(size(px(640.), px(900.)));
        utility.run_until_parked();
        let spectrum = utility.debug_bounds("visualizer-spectrum").unwrap();
        let loudness = utility.debug_bounds("visualizer-loudness").unwrap();
        assert!(loudness.top() > spectrum.bottom());
        assert_eq!(loudness.left(), spectrum.left());
        assert_eq!(loudness.size.width, spectrum.size.width);
    }

    #[gpui::test]
    fn the_segmented_controls_choose_source_tap_channel_span_and_gain(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = harness::open(cx);
        cx.dispatch_action(actions::ToggleVisualizer);
        harness::paint(&app, cx);
        let handle = app.read_with(cx, |app, _| app.auxiliary_windows[&Surface::Visualizer]);
        let mut utility = gpui::VisualTestContext::from_window(handle.into(), cx);
        utility.simulate_resize(size(px(1200.), px(1400.)));
        utility.run_until_parked();
        for selector in [
            "visualizer-source-1",
            "visualizer-tap-0",
            "visualizer-spectrum-channel-4",
            "visualizer-timebase-0",
            "visualizer-oscilloscope-gain-3",
        ] {
            harness::click(selector, &mut utility);
        }
        app.read_with(&utility, |app, _| {
            assert!(app.visualizer.selected);
            assert_eq!(app.visualizer.tap, VisualizerTap::PreFader);
            assert_eq!(app.visualizer.spectrum_mode, VisualizerSpectrum::Mono);
            assert_eq!(app.visualizer.oscilloscope_span, OscilloscopeSpan::Short);
            assert_eq!(app.visualizer.oscilloscope_gain, OscilloscopeGain::Auto);
        });
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

        harness::click("visualizer-source-1", cx);
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

    #[test]
    fn the_streaming_verdict_checks_the_peak_before_the_loudness() {
        let reading = |integrated, true_peak| VisualizerLoudness {
            integrated,
            true_peak,
            ..VisualizerLoudness::default()
        };
        assert_eq!(streaming_verdict(&reading(None, Some(-3.))), None);
        assert_eq!(
            streaming_verdict(&reading(Some(-14.3), Some(-1.5))),
            Some((Key::VisualizerOnTarget, Tone::Success))
        );
        assert_eq!(
            streaming_verdict(&reading(Some(-14.3), Some(-0.2))),
            Some((Key::VisualizerPeakOver, Tone::Warning))
        );
        assert_eq!(
            streaming_verdict(&reading(Some(-9.), Some(-1.2))),
            Some((Key::VisualizerTooLoud, Tone::Warning))
        );
        assert_eq!(
            streaming_verdict(&reading(Some(-23.), Some(-6.))),
            Some((Key::VisualizerTooQuiet, Tone::Neutral))
        );
    }

    #[test]
    fn the_spectrogram_puts_high_bands_on_top_and_the_newest_column_on_the_right() {
        let mut state = VisualizerState::default();
        let start = Instant::now();
        let mut rising = frame(0.);
        rising.spectrum = (0..4).map(|band| FLOOR_DB + band as f32 * 24.).collect();
        state.accept_at(rising, start);
        // A repaint 10 ms later is inside the same 25 ms column and adds nothing.
        state.accept_at(frame(0.), start + Duration::from_millis(10));
        assert_eq!(state.spectrogram.len(), 1);
        let (rows, cells) = state.spectrogram_cells();
        assert_eq!(rows, 4);
        assert_eq!(cells.len(), 4 * SPECTROGRAM_COLUMNS);
        let last = SPECTROGRAM_COLUMNS - 1;
        let column: Vec<f32> = (0..rows)
            .map(|row| cells[row * SPECTROGRAM_COLUMNS + last])
            .collect();
        for (cell, wanted) in column.iter().zip([1., 2. / 3., 1. / 3., 0.]) {
            assert!((cell - wanted).abs() < 1e-6, "{column:?}");
        }
        assert!(
            cells[..last].iter().all(|&cell| cell == 0.),
            "unheard time is blank"
        );
        for step in 1..=SPECTROGRAM_COLUMNS * 2 {
            state.accept_at(frame(-6.), start + Duration::from_millis(25 * step as u64));
        }
        assert_eq!(state.spectrogram.len(), SPECTROGRAM_COLUMNS);
    }

    #[test]
    fn a_meter_peak_holds_for_two_seconds_then_follows_the_signal_down() {
        let mut state = VisualizerState::default();
        let start = Instant::now();
        let loud = frame(0.);
        let mut quiet = frame(0.);
        quiet.left = quiet.left.iter().map(|sample| sample * 0.1).collect();
        quiet.right = quiet.left.clone();
        state.accept_at(loud, start);
        let held = state.meter_hold[0].unwrap().0;
        assert!((held - -6.02).abs() < 0.1, "{held}");
        state.accept_at(quiet.clone(), start + Duration::from_secs(1));
        assert_eq!(state.meter_hold[0].unwrap().0, held);
        state.accept_at(quiet, start + Duration::from_secs(3));
        assert!((state.meter_hold[0].unwrap().0 - -26.02).abs() < 0.1);
    }

    #[test]
    fn the_axes_use_the_numbers_people_read() {
        assert_eq!(spectrum_x(SPECTRUM_LOW_HZ), 0.);
        assert_eq!(spectrum_x(SPECTRUM_HIGH_HZ), 1.);
        assert!(
            (spectrum_x(640.) - 0.5).abs() < 1e-6,
            "five octaves up is halfway"
        );
        assert_eq!(frequency_label(500.), "500");
        assert_eq!(frequency_label(2_000.), "2k");
        assert_eq!(spectrum_level(0.), 1.);
        assert_eq!(spectrum_level(FLOOR_DB), 0.);
        assert!((spectrum_level(-12.) - 5. / 6.).abs() < 1e-6);
        assert_eq!(signed(0.724, 2), "+0.72");
        assert_eq!(signed(-0.31, 2), "\u{2212}0.31");
        assert_eq!(signed_plain(-15.44, 1), "\u{2212}15.4");
    }

    #[gpui::test]
    fn switching_tap_starts_the_display_again(cx: &mut gpui::TestAppContext) {
        let (app, cx) = harness::open(cx);
        app.update(cx, |app, _| {
            app.visualizer.open = true;
            app.visualizer.accept(frame(-12.));
            app.visualizer.frozen = true;
            app.visualizer_command(VisualizerCommand::SetTap(VisualizerTap::PostFader));
            assert!(
                app.visualizer.frozen,
                "choosing the tap already chosen changes nothing"
            );
            app.visualizer_command(VisualizerCommand::Tap);
            assert_eq!(app.visualizer.tap, VisualizerTap::PreFader);
            assert!(!app.visualizer.frozen);
            assert!(app.visualizer.frame.is_none());
            assert!(app.visualizer.spectrogram.is_empty());
        });
    }
}
