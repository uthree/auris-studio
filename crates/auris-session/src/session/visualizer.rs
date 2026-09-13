//! Stereo monitoring, independent of the plugin editor's spectrum tap.
use super::Session;
use auris_core::TrackId;
use auris_dsp::{SILENCE_DB, SpectrumAnalyzer, bands_from_bins};

/// One coherent window of audio and its measured stereo spectrum.
#[derive(Clone, Debug)]
pub struct VisualizerFrame {
    /// Left samples, oldest first.
    pub left: Vec<f32>,
    /// Right samples, oldest first.
    pub right: Vec<f32>,
    /// Sample rate of both channels, used to map the oscilloscope's time axis.
    pub sample_rate: f64,
    /// Equal-power average of channel spectra, in dBFS.
    pub spectrum: Vec<f32>,
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
    /// Returns no frame when the copy overlaps a write or no sample rate is available.
    pub fn visualizer_frame(
        &mut self,
        low_hz: f64,
        high_hz: f64,
        bands: usize,
    ) -> Option<VisualizerFrame> {
        let mut left = vec![0.0; auris_engine::SCOPE_WINDOW];
        let mut right = vec![0.0; left.len()];
        if !self.visualizer_scope.read_stereo(&mut left, &mut right) {
            return None;
        }
        let rate = self.visualizer_scope.sample_rate();
        if rate <= 0.0 {
            return None;
        }
        let correlation = correlation(&left, &right);
        let mut spectrum = vec![SILENCE_DB; bands];
        stereo_spectrum(
            &mut self.analyzer,
            &left,
            &right,
            rate,
            low_hz,
            high_hz,
            &mut spectrum,
        );
        Some(VisualizerFrame {
            left,
            right,
            sample_rate: rate,
            spectrum,
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
    low_hz: f64,
    high_hz: f64,
    bands: &mut [f32],
) {
    let mut bins = vec![SILENCE_DB; analyzer.bin_count()];
    let mut other = vec![SILENCE_DB; bins.len()];
    analyzer.reset();
    analyzer.push(left);
    analyzer.magnitudes(&mut bins);
    analyzer.reset();
    analyzer.push(right);
    analyzer.magnitudes(&mut other);
    for (l, r) in bins.iter_mut().zip(other) {
        *l = (10.0 * ((10.0f32.powf(*l / 10.0) + 10.0f32.powf(r / 10.0)) * 0.5).log10())
            .max(SILENCE_DB);
    }
    bands_from_bins(&bins, rate, low_hz, high_hz, bands);
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
        let frame = session.visualizer_frame(30., 18000., 64).unwrap();
        assert_eq!(frame.correlation, Some(-1.));
        assert_eq!(frame.left, vec![1.; 1024]);
        assert_eq!(frame.right, vec![-1.; 1024]);
        assert_eq!(frame.sample_rate, 48000.);
        session.stop_visualizer();
        assert!(session.visualizer_frame(30., 18000., 64).is_none());
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
        stereo_spectrum(&mut analyzer, &tone, &tone, 48000., 30., 18000., &mut mono);
        stereo_spectrum(
            &mut analyzer,
            &tone,
            &inverse,
            48000.,
            30.,
            18000.,
            &mut anti,
        );
        stereo_spectrum(
            &mut analyzer,
            &[0.; 1024],
            &tone,
            48000.,
            30.,
            18000.,
            &mut right,
        );
        assert_eq!(mono, anti);
        let peak = mono.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let right_peak = right.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert!((peak - right_peak - 3.0103).abs() < 0.01);
    }
}
