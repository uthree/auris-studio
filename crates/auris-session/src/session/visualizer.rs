//! Stereo monitoring, independent of the plugin editor's spectrum tap.
use std::sync::Arc;

use super::Session;
use auris_core::TrackId;
use auris_dsp::{SILENCE_DB, SpectrumAnalyzer, bands_from_bins};

/// Which stereo interpretation the live spectrum displays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VisualizerSpectrum {
    /// Equal-power average of the left and right spectra.
    #[default]
    Average,
    /// Left channel only.
    Left,
    /// Right channel only.
    Right,
    /// Louder of the two channels in every frequency bin.
    Maximum,
    /// Spectrum of the mono sum.
    Mono,
}

#[derive(Clone, Copy)]
/// One spectrum transform's frequency range and channel interpretation.
pub(super) struct SpectrumRequest {
    /// Lowest displayed frequency.
    pub(super) low_hz: f64,
    /// Highest displayed frequency.
    pub(super) high_hz: f64,
    /// Stereo interpretation to transform.
    pub(super) mode: VisualizerSpectrum,
}

/// One coherent window of audio and its measured stereo spectrum.
#[derive(Clone, Debug)]
pub struct VisualizerFrame {
    /// Left samples, oldest first.
    pub left: Arc<[f32]>,
    /// Right samples, oldest first.
    pub right: Arc<[f32]>,
    /// Sample rate of both channels, used to map the oscilloscope's time axis.
    pub sample_rate: f64,
    /// Number of newest samples in the retained history that form one display window.
    pub window_samples: usize,
    /// Spectrum in the requested channel mode, in dBFS; empty when analysis was skipped.
    pub spectrum: Arc<[f32]>,
    /// Normalized channel correlation; absent when either channel is silent.
    pub correlation: Option<f32>,
}

impl Session {
    /// Selects the independent visualizer's post-fader signal. `None` means master.
    pub fn watch_visualizer(&self, track: Option<TrackId>) {
        let source = match track {
            None => auris_engine::ScopeSource::Master,
            Some(id) => self.project.track_index(id).map_or(
                auris_engine::ScopeSource::Off,
                auris_engine::ScopeSource::Track,
            ),
        };
        self.visualizer_scope.watch(source);
    }

    /// Stops collecting samples for the visualizer.
    pub fn stop_visualizer(&self) {
        self.visualizer_scope.watch(auris_engine::ScopeSource::Off);
    }

    /// Reads a stereo window without waiting for the audio thread.
    ///
    /// `spectrum_mode` selects the channel interpretation, or skips the FFT when the current
    /// display does not need a spectrum.
    /// Returns no frame when the copy overlaps a write or no sample rate is available.
    pub fn visualizer_frame(
        &mut self,
        low_hz: f64,
        high_hz: f64,
        bands: usize,
        spectrum_mode: Option<VisualizerSpectrum>,
    ) -> Option<VisualizerFrame> {
        let mut left = vec![0.0; auris_engine::SCOPE_HISTORY];
        let mut right = vec![0.0; left.len()];
        if !self.visualizer_scope.read_stereo(&mut left, &mut right) {
            return None;
        }
        let rate = self.visualizer_scope.sample_rate();
        if rate <= 0.0 {
            return None;
        }
        let window_samples = auris_engine::SCOPE_WINDOW.min(left.len()).min(right.len());
        let start = left.len().min(right.len()) - window_samples;
        let correlation = correlation(&left[start..], &right[start..]);
        let mut spectrum = spectrum_mode
            .map(|_| vec![SILENCE_DB; bands])
            .unwrap_or_default();
        if let Some(mode) = spectrum_mode {
            stereo_spectrum(
                &mut self.analyzer,
                &left,
                &right,
                rate,
                SpectrumRequest {
                    low_hz,
                    high_hz,
                    mode,
                },
                &mut spectrum,
            );
        }
        Some(VisualizerFrame {
            left: left.into(),
            right: right.into(),
            sample_rate: rate,
            window_samples,
            spectrum: spectrum.into(),
            correlation,
        })
    }
}

fn correlation(left: &[f32], right: &[f32]) -> Option<f32> {
    let (mut ll, mut rr, mut lr) = (0.0f64, 0.0f64, 0.0f64);
    for (&l, &r) in left.iter().zip(right) {
        let (l, r) = (f64::from(l), f64::from(r));
        ll += l * l;
        rr += r * r;
        lr += l * r;
    }
    if ll <= 1e-12 || rr <= 1e-12 {
        None
    } else {
        Some((lr / (ll * rr).sqrt()).clamp(-1.0, 1.0) as f32)
    }
}

pub(super) fn stereo_spectrum(
    analyzer: &mut SpectrumAnalyzer,
    left: &[f32],
    right: &[f32],
    rate: f64,
    request: SpectrumRequest,
    bands: &mut [f32],
) {
    let mut bins = vec![SILENCE_DB; analyzer.bin_count()];
    match request.mode {
        VisualizerSpectrum::Left => analyze_channel(analyzer, left, &mut bins),
        VisualizerSpectrum::Right => analyze_channel(analyzer, right, &mut bins),
        VisualizerSpectrum::Mono => {
            let mono: Vec<_> = left
                .iter()
                .zip(right)
                .map(|(&left, &right)| (left + right) * 0.5)
                .collect();
            analyze_channel(analyzer, &mono, &mut bins);
        }
        VisualizerSpectrum::Average | VisualizerSpectrum::Maximum => {
            let mut other = vec![SILENCE_DB; bins.len()];
            analyze_channel(analyzer, left, &mut bins);
            analyze_channel(analyzer, right, &mut other);
            for (left, right) in bins.iter_mut().zip(other) {
                *left = match request.mode {
                    VisualizerSpectrum::Average => (10.0
                        * ((10.0f32.powf(*left / 10.0) + 10.0f32.powf(right / 10.0)) * 0.5)
                            .log10())
                    .max(SILENCE_DB),
                    VisualizerSpectrum::Maximum => (*left).max(right),
                    _ => unreachable!(),
                };
            }
        }
    }
    bands_from_bins(&bins, rate, request.low_hz, request.high_hz, bands);
}

fn analyze_channel(analyzer: &mut SpectrumAnalyzer, samples: &[f32], bins: &mut [f32]) {
    analyzer.reset();
    analyzer.push(samples);
    analyzer.magnitudes(bins);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visualizer_and_plugin_scopes_are_independent() {
        let mut session = super::super::fixtures::session();
        session.watch_strip(None);
        session.watch_visualizer(None);
        session
            .scope
            .publish_stereo(&[1.; 1024], &[1.; 1024], 48000.);
        session
            .visualizer_scope
            .publish_stereo(&[1.; 1024], &[-1.; 1024], 48000.);
        session.stop_watching();
        let frame = session
            .visualizer_frame(30., 18000., 64, Some(VisualizerSpectrum::Average))
            .unwrap();
        assert_eq!(frame.correlation, Some(-1.));
        assert_eq!(&frame.left[..1024], &[0.; 1024]);
        assert_eq!(&frame.left[1024..], &[1.; 1024]);
        assert_eq!(&frame.right[..1024], &[0.; 1024]);
        assert_eq!(&frame.right[1024..], &[-1.; 1024]);
        assert_eq!(frame.sample_rate, 48000.);
        assert_eq!(frame.window_samples, 1024);
        session.stop_visualizer();
        assert!(
            session
                .visualizer_frame(30., 18000., 64, Some(VisualizerSpectrum::Average))
                .is_none()
        );
    }
    #[test]
    fn correlation_distinguishes_mono_antiphase_and_silence() {
        assert_eq!(correlation(&[1., -1.], &[1., -1.]), Some(1.));
        assert_eq!(correlation(&[1., -1.], &[-1., 1.]), Some(-1.));
        assert_eq!(correlation(&[1., -1.], &[1., 1.]), Some(0.));
        assert_eq!(correlation(&[0., 0.], &[1., 1.]), None);
    }
    #[test]
    fn spectrum_preserves_antiphase_and_right_only_audio() {
        let mut analyzer = SpectrumAnalyzer::new(1024);
        let tone: Vec<_> = (0..1024)
            .map(|n| (std::f32::consts::TAU * n as f32 * 32. / 1024.).sin())
            .collect();
        let inverse: Vec<_> = tone.iter().map(|v| -v).collect();
        let mut mono = [0.; 80];
        let mut anti = mono;
        let mut right = mono;
        stereo_spectrum(
            &mut analyzer,
            &tone,
            &tone,
            48000.,
            SpectrumRequest {
                low_hz: 30.,
                high_hz: 18000.,
                mode: VisualizerSpectrum::Average,
            },
            &mut mono,
        );
        stereo_spectrum(
            &mut analyzer,
            &tone,
            &inverse,
            48000.,
            SpectrumRequest {
                low_hz: 30.,
                high_hz: 18000.,
                mode: VisualizerSpectrum::Average,
            },
            &mut anti,
        );
        stereo_spectrum(
            &mut analyzer,
            &[0.; 1024],
            &tone,
            48000.,
            SpectrumRequest {
                low_hz: 30.,
                high_hz: 18000.,
                mode: VisualizerSpectrum::Average,
            },
            &mut right,
        );
        assert_eq!(mono, anti);
        let peak = mono.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let right_peak = right.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert!((peak - right_peak - 3.0103).abs() < 0.01);
    }

    #[test]
    fn spectrum_channel_modes_keep_their_distinct_meanings() {
        let mut analyzer = SpectrumAnalyzer::new(1024);
        let tone: Vec<_> = (0..1024)
            .map(|n| (std::f32::consts::TAU * n as f32 * 32. / 1024.).sin())
            .collect();
        let silence = [0.; 1024];
        let peak_for = |mode, analyzer: &mut SpectrumAnalyzer| {
            let mut bands = [0.; 80];
            stereo_spectrum(
                analyzer,
                &tone,
                &silence,
                48000.,
                SpectrumRequest {
                    low_hz: 30.,
                    high_hz: 18000.,
                    mode,
                },
                &mut bands,
            );
            bands.into_iter().fold(f32::NEG_INFINITY, f32::max)
        };
        let left = peak_for(VisualizerSpectrum::Left, &mut analyzer);
        let right = peak_for(VisualizerSpectrum::Right, &mut analyzer);
        let maximum = peak_for(VisualizerSpectrum::Maximum, &mut analyzer);
        let average = peak_for(VisualizerSpectrum::Average, &mut analyzer);
        let mono = peak_for(VisualizerSpectrum::Mono, &mut analyzer);

        assert!(right <= SILENCE_DB + f32::EPSILON);
        assert!((maximum - left).abs() < 0.001);
        assert!((left - average - 3.0103).abs() < 0.01);
        assert!((left - mono - 6.0206).abs() < 0.01);
    }
}
