//! Stereo monitoring, independent of the plugin editor's spectrum tap.
use std::sync::Arc;

use super::Session;
use auris_core::TrackId;
use auris_dsp::{
    LoudnessMeter, SILENCE_DB, SpectrumAnalyzer, bands_from_bins, bands_from_bins_nearest,
};

/// Where on a strip the visualizer listens: before its fader and pan, or after them.
pub use auris_engine::ScopeTap as VisualizerTap;

/// How many frames the visualizer's scope retains: about two thirds of a second at 48 kHz.
///
/// The display needs two windows of it. The rest is slack for the loudness meter, which measures
/// every sample and so loses some whenever the window goes this long without a repaint.
pub(super) const VISUALIZER_HISTORY: usize = 1 << 15;

/// How many samples one visualizer spectrum is taken over: 85 ms at 48 kHz, 12 Hz between bins.
///
/// Four times the plugin editor's window, because the visualizer draws sixth-octave bars down to
/// 20 Hz and a sixth of an octave there is a few hertz wide. It costs time resolution, which is
/// what the spectrogram beside it is for.
pub(super) const VISUALIZER_FFT: usize = 4096;

/// The loudness of what the visualizer follows, to EBU R128.
///
/// Every reading is `None` until enough has been heard to make it, and while what has been heard
/// is silence.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VisualizerLoudness {
    /// The last 400 ms, in LUFS.
    pub momentary: Option<f32>,
    /// The last 3 s, in LUFS.
    pub short_term: Option<f32>,
    /// Everything since the meter was last reset, gated, in LUFS.
    pub integrated: Option<f32>,
    /// The loudness range since the meter was last reset, in LU.
    pub range: Option<f32>,
    /// The highest true peak since the meter was last reset, in dBTP.
    pub true_peak: Option<f32>,
}

/// The visualizer's loudness meter and where it has read up to in the scope.
#[derive(Default)]
pub(super) struct LoudnessTap {
    meter: Option<LoudnessMeter>,
    cursor: u64,
    left: Vec<f32>,
    right: Vec<f32>,
    /// The strip and tap the meter has been hearing, so a change of either starts it again.
    following: Option<(auris_engine::ScopeSource, VisualizerTap)>,
}

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
    /// Whether a band too narrow for a bin of its own repeats the nearest bin rather than
    /// reading silence; see [`auris_dsp::bands_from_bins_nearest`].
    pub(super) fill_narrow_bands: bool,
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
    /// Enables simultaneous post-fader audio and mechanical observation of all tracks.
    /// Independent of the plugin editor and the single-source visualizer; no document edit.
    pub fn watch_track_overview(&mut self, enabled: bool) {
        if self.overview_watched == enabled {
            return;
        }
        self.overview_watched = enabled;
        for (index, track) in self.project.tracks.iter().enumerate() {
            if let Some(scope) = self.overview_scopes.get(&track.id) {
                scope.watch(if enabled {
                    auris_engine::ScopeSource::Track(index)
                } else {
                    auris_engine::ScopeSource::Off
                });
            }
        }
        self.refresh_motion_watches();
    }

    /// Reads one track's own stereo waveform without an FFT or waiting for audio.
    /// Returns no frame until the overview is enabled and audio has been published.
    pub fn track_overview_frame(&self, track: TrackId) -> Option<VisualizerFrame> {
        if !self.overview_watched {
            return None;
        }
        let scope = self.overview_scopes.get(&track)?;
        let sample_rate = scope.sample_rate();
        if sample_rate <= 0.0 {
            return None;
        }
        let mut left = vec![0.; auris_engine::SCOPE_HISTORY];
        let mut right = vec![0.; left.len()];
        if !scope.read_stereo(&mut left, &mut right) {
            return None;
        }
        Some(VisualizerFrame {
            correlation: correlation(
                &left[auris_engine::SCOPE_WINDOW..],
                &right[auris_engine::SCOPE_WINDOW..],
            ),
            window_samples: auris_engine::SCOPE_WINDOW,
            left: left.into(),
            right: right.into(),
            sample_rate,
            spectrum: Arc::default(),
        })
    }

    /// Selects the independent visualizer's signal: `None` means master, and `tap` whether it is
    /// heard before or after the strip's fader. Changing either starts the loudness meter again,
    /// since what it had measured was a different signal.
    pub fn watch_visualizer(&mut self, track: Option<TrackId>, tap: VisualizerTap) {
        let source = match track {
            None => auris_engine::ScopeSource::Master,
            Some(id) => self.project.track_index(id).map_or(
                auris_engine::ScopeSource::Off,
                auris_engine::ScopeSource::Track,
            ),
        };
        self.visualizer_scope.set_tap(tap);
        self.visualizer_scope.watch(source);
        if self.visualizer_loudness.following != Some((source, tap)) {
            self.reset_visualizer_loudness();
            self.visualizer_loudness.following = Some((source, tap));
        }
    }

    /// Stops collecting samples for the visualizer.
    pub fn stop_visualizer(&mut self) {
        self.visualizer_scope.watch(auris_engine::ScopeSource::Off);
        self.visualizer_loudness.following = None;
    }

    /// Forgets the loudness measured so far: the integrated loudness, the range and the true
    /// peak start again from the next sample. No document edit.
    pub fn reset_visualizer_loudness(&mut self) {
        let tap = &mut self.visualizer_loudness;
        if let Some(meter) = &mut tap.meter {
            meter.reset();
        }
        // Whatever is already in the ring was heard before the reset.
        let mut scratch = [0.0; 0];
        let _ = self
            .visualizer_scope
            .read_new(&mut tap.cursor, &mut scratch, &mut []);
    }

    /// Measures everything the visualizer's scope has published since the last call and returns
    /// the readings. Call it on every repaint, frozen or not: the meter has to hear every sample
    /// to be the measure it claims to be, and a display that is held still is not a signal that
    /// has stopped.
    pub fn measure_visualizer_loudness(&mut self) -> VisualizerLoudness {
        let tap = &mut self.visualizer_loudness;
        let history = self.visualizer_scope.history();
        tap.left.resize(history, 0.0);
        tap.right.resize(history, 0.0);
        if let Some(fresh) =
            self.visualizer_scope
                .read_new(&mut tap.cursor, &mut tap.left, &mut tap.right)
            && fresh.frames > 0
        {
            let rate = self.visualizer_scope.sample_rate();
            if rate > 0.0
                && tap
                    .meter
                    .as_ref()
                    .is_none_or(|meter| meter.sample_rate() != rate)
            {
                tap.meter = Some(LoudnessMeter::new(rate));
            }
            if let Some(meter) = &mut tap.meter {
                meter.push(&tap.left[..fresh.frames], &tap.right[..fresh.frames]);
            }
        }
        tap.meter
            .as_ref()
            .map_or_else(VisualizerLoudness::default, |meter| VisualizerLoudness {
                momentary: meter.momentary(),
                short_term: meter.short_term(),
                integrated: meter.integrated(),
                range: meter.range(),
                true_peak: meter.true_peak_db(),
            })
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
        let mut left = vec![0.0; auris_engine::SCOPE_HISTORY.max(VISUALIZER_FFT)];
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
                &mut self.visualizer_analyzer,
                &left,
                &right,
                rate,
                SpectrumRequest {
                    low_hz,
                    high_hz,
                    mode,
                    fill_narrow_bands: true,
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
    if request.fill_narrow_bands {
        bands_from_bins_nearest(&bins, rate, request.low_hz, request.high_hz, bands);
    } else {
        bands_from_bins(&bins, rate, request.low_hz, request.high_hz, bands);
    }
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
    fn overview_waveforms_follow_track_identity_across_rebuilds() {
        let mut session = super::super::fixtures::session();
        let first = session.add_default_instrument_track("First").unwrap();
        let second = session.add_audio_track("Second");
        session.watch_track_overview(true);
        session.overview_scopes[&first].publish_stereo(&[0.25; 2048], &[-0.25; 2048], 48000.);
        session.overview_scopes[&second].publish_stereo(&[0.5; 2048], &[0.5; 2048], 48000.);
        let first_frame = session.track_overview_frame(first).unwrap();
        let second_frame = session.track_overview_frame(second).unwrap();
        assert!(first_frame.left.iter().all(|&value| value == 0.25));
        assert!(first_frame.right.iter().all(|&value| value == -0.25));
        assert!(second_frame.left.iter().all(|&value| value == 0.5));
        let retired = Arc::clone(&session.overview_scopes[&first]);
        session.move_track(first, 1).unwrap();
        assert_eq!(retired.watching(), auris_engine::ScopeSource::Off);
        assert!(session.track_overview_frame(first).is_none());
        assert_eq!(
            session.overview_scopes[&first].watching(),
            auris_engine::ScopeSource::Track(1)
        );
        session.overview_scopes[&first].publish_stereo(&[0.75; 2048], &[0.75; 2048], 48000.);
        assert!(
            session
                .track_overview_frame(first)
                .unwrap()
                .left
                .iter()
                .all(|&value| value == 0.75)
        );
        session.remove_track(second).unwrap();
        assert!(session.track_overview_frame(second).is_none());
        session.watch_track_overview(false);
        assert!(session.track_overview_frame(first).is_none());
    }

    #[test]
    fn overview_and_editor_motion_watches_coexist_and_do_not_edit_the_document() {
        let mut session = super::super::fixtures::session();
        let violin = session
            .add_instrument_track("Violin", "auris.physical.violin")
            .unwrap();
        let guitar = session
            .add_instrument_track("Guitar", "auris.physical.guitar")
            .unwrap();
        let before = session.project().clone();
        let undo = super::super::fixtures::undo_depth(&mut session);
        session.watch_instrument_motion(Some(violin));
        session.watch_track_overview(true);
        session.watch_instrument_motion(None);
        assert!(
            session
                .motion_monitors
                .values()
                .all(|monitor| monitor.is_watched())
        );
        session.watch_instrument_motion(Some(guitar));
        session.watch_track_overview(false);
        assert!(!session.motion_monitors[&violin].is_watched());
        assert!(session.motion_monitors[&guitar].is_watched());
        session.watch_track_overview(true);
        session.rebuild_graph();
        assert!(
            session
                .motion_monitors
                .values()
                .all(|monitor| monitor.is_watched())
        );
        session.watch_track_overview(false);
        assert!(!session.motion_monitors[&violin].is_watched());
        assert!(session.motion_monitors[&guitar].is_watched());
        assert_eq!(session.project(), &before);
        assert_eq!(super::super::fixtures::undo_depth(&mut session), undo);
    }

    #[test]
    fn visualizer_and_plugin_scopes_are_independent() {
        let mut session = super::super::fixtures::session();
        session.watch_strip(None);
        session.watch_visualizer(None, VisualizerTap::PostFader);
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
        let older = frame.left.len() - 1024;
        assert_eq!(&frame.left[..older], &vec![0.; older][..]);
        assert_eq!(&frame.left[older..], &[1.; 1024]);
        assert_eq!(&frame.right[..older], &vec![0.; older][..]);
        assert_eq!(&frame.right[older..], &[-1.; 1024]);
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
                fill_narrow_bands: false,
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
                fill_narrow_bands: false,
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
                fill_narrow_bands: false,
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
                    fill_narrow_bands: false,
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

    #[test]
    fn the_loudness_meter_hears_every_sample_and_starts_again_on_a_new_source() {
        let mut session = super::super::fixtures::session();
        let track = session.add_audio_track("Tone");
        session.watch_visualizer(None, VisualizerTap::PostFader);
        assert_eq!(
            session.measure_visualizer_loudness(),
            VisualizerLoudness::default()
        );
        // Five seconds of the standard's calibration tone, delivered a callback at a time, with
        // the meter read only every eight callbacks the way a repaint would.
        let tone: Vec<f32> = (0..48_000 * 5)
            .map(|n| 0.1 * (std::f32::consts::TAU * 1000. * n as f32 / 48_000.).sin())
            .collect();
        for (index, block) in tone.chunks(512).enumerate() {
            session
                .visualizer_scope
                .publish_stereo(block, block, 48_000.);
            if index % 8 == 7 {
                session.measure_visualizer_loudness();
            }
        }
        let reading = session.measure_visualizer_loudness();
        for value in [reading.momentary, reading.short_term, reading.integrated] {
            let value = value.expect("a tone is loud");
            assert!((value - -20.).abs() < 0.1, "read {value:.2} LUFS");
        }
        assert!((reading.true_peak.unwrap() - -20.).abs() < 0.1);

        // The same strip from before its fader is another signal, and so another measurement.
        session.watch_visualizer(None, VisualizerTap::PreFader);
        assert_eq!(session.measure_visualizer_loudness().integrated, None);
        session.watch_visualizer(Some(track), VisualizerTap::PreFader);
        assert_eq!(session.measure_visualizer_loudness().integrated, None);

        // And a reset forgets what was heard, including what is still in the ring.
        session
            .visualizer_scope
            .publish_stereo(&tone, &tone, 48_000.);
        assert!(session.measure_visualizer_loudness().true_peak.is_some());
        session
            .visualizer_scope
            .publish_stereo(&tone[..4800], &tone[..4800], 48_000.);
        session.reset_visualizer_loudness();
        assert_eq!(session.measure_visualizer_loudness().true_peak, None);
    }
}
