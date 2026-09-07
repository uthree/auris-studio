//! CPU tempo, major/minor harmony and isolated monophonic transcription.
//!
//! Input is band-limited PCM at 11025 Hz, prepared by the session's ordinary resampler.
//! FFT frames are pooled across channels after transformation, avoiding stereo cancellation.
//! Pitch uses YIN's cumulative normalized difference, not a trained predictor.

use crate::{
    AnalysisControl, AnalysisError,
    chords::{ChordReading, rank, smooth},
    pitch,
};
use auris_core::AudioBuffer;
use auris_dsp::SpectrumAnalyzer;
use serde::Serialize;

/// Sample rate expected by the analysis worker after resampling.
pub const ANALYSIS_RATE: f64 = 11_025.0;
const HOP: usize = 220;
const FFT: usize = 2048;

/// Independent work the audio worker should perform.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct AudioOptions {
    /// Also extract note events, assuming one pitched voice at a time.
    pub transcribe: bool,
}

/// A plausible constant tempo and its periodicity score.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TempoCandidate {
    /// Quarter-note beats per minute; half/double readings may also be listed.
    pub bpm: f64,
    /// Normalized onset autocorrelation, in 0..=1; not a probability.
    pub score: f32,
}

/// Beat-grid estimate. Empty candidates mean no reliable periodic pulse.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TempoAnalysis {
    /// Up to three tempo readings, strongest first.
    pub candidates: Vec<TempoCandidate>,
    /// Estimated beat timestamps in source seconds, not downbeats or bar lines.
    pub beats: Vec<f64>,
}

/// An audio chord window in source seconds, with an exclusive end.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AudioChordSegment {
    /// Inclusive source time in seconds.
    pub start: f64,
    /// Exclusive source time in seconds.
    pub end: f64,
    /// Major/minor hypotheses for this window.
    pub reading: ChordReading,
}

/// One isolated-voice note hypothesis before musical quantization.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TranscribedNote {
    /// MIDI pitch, 0..=127.
    pub pitch: u8,
    /// Inclusive source time in seconds.
    pub start: f64,
    /// Exclusive source time in seconds.
    pub end: f64,
    /// Relative level mapped into 0..=1; not a recovered MIDI velocity.
    pub strength: f32,
}

/// Audio measurements and an optional editable note draft.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AudioAnalysis {
    /// Algorithm identifier for reproducibility.
    pub algorithm: &'static str,
    /// Analyzed duration in source seconds.
    pub seconds: f64,
    /// Requested analysis modes.
    pub options: AudioOptions,
    /// Constant-tempo alternatives and beat positions.
    pub tempo: TempoAnalysis,
    /// Chord windows at half-second resolution, independent of an uncertain beat grid.
    pub chords: Vec<AudioChordSegment>,
    /// Isolated monophonic note estimates; empty unless requested.
    pub notes: Vec<TranscribedNote>,
}

/// Analyzes prepared audio without I/O, GPU access, models or document edits.
///
/// Bounded to thirty minutes and eight channels. Features are computed from every frame;
/// memory grows with low-rate features rather than the full spectrogram. Transcription
/// supports approximately 65–1000 Hz and assumes an isolated voice, not a polyphonic mix.
pub fn analyze_audio(
    audio: &AudioBuffer,
    options: AudioOptions,
    control: &AnalysisControl,
) -> Result<AudioAnalysis, AnalysisError> {
    control.check(0.0)?;
    if (audio.sample_rate() - ANALYSIS_RATE).abs() > 0.01
        || !audio.sample_rate().is_finite()
        || audio.channel_count() > 8
        || audio.duration_seconds() > 1800.0
    {
        return Err(AnalysisError::Invalid(
            "audio must be at 11025 Hz, at most eight channels and thirty minutes",
        ));
    }
    let count = audio.frame_count().div_ceil(HOP);
    let duration = audio.duration_seconds();
    let chord_count = (duration / 0.5).ceil() as usize;
    let mut chroma = vec![[0.0; 12]; chord_count];
    let mut envelope = Vec::with_capacity(count);
    let mut flux = Vec::with_capacity(count);
    let mut pitches = Vec::with_capacity(count);
    let mut previous_spectrum = vec![0.0f32; FFT / 2 + 1];
    let mut analyzer = SpectrumAnalyzer::new(FFT);
    let mut window = vec![0.0f32; FFT];
    let mut magnitudes = vec![0.0f32; FFT / 2 + 1];
    let mut spectrum = vec![0.0f32; FFT / 2 + 1];
    let mut pitch_window = [0.0f32; pitch::WINDOW];
    let mut difference = [0.0f32; 174];
    for frame in 0..count {
        control.check(frame as f32 / count.max(1) as f32 * 0.8)?;
        let center = frame * HOP;
        spectrum.fill(0.0);
        let mut best_channel = 0;
        let mut rms = 0.0f32;
        for (channel, samples) in audio.iter_channels().enumerate() {
            let end = (center + HOP).min(samples.len());
            let energy = samples[center..end].iter().map(|v| v * v).sum::<f32>()
                / (end - center).max(1) as f32;
            if !energy.is_finite() {
                return Err(AnalysisError::Invalid("audio contains non-finite samples"));
            }
            if energy > rms {
                rms = energy;
                best_channel = channel;
            }
            for (i, sample) in window.iter_mut().enumerate() {
                *sample = samples
                    .get((center as isize + i as isize - FFT as isize / 2) as usize)
                    .copied()
                    .unwrap_or(0.0);
            }
            analyzer.reset();
            analyzer.push(&window);
            analyzer.magnitudes(&mut magnitudes);
            // SpectrumAnalyzer exposes dBFS for its display clients; templates need amplitude.
            for (dst, src) in spectrum.iter_mut().zip(&magnitudes) {
                let amplitude = if *src <= -100.0 {
                    0.0
                } else {
                    10.0f32.powf(*src / 20.0)
                };
                *dst = dst.max(amplitude);
            }
        }
        envelope.push(rms.sqrt());
        let change = spectrum
            .iter()
            .zip(&previous_spectrum)
            .map(|(a, b)| (a - b).max(0.0))
            .sum::<f32>();
        flux.push(change);
        previous_spectrum.copy_from_slice(&spectrum);
        let chord_index = ((center as f64 / ANALYSIS_RATE) / 0.5) as usize;
        // Local spectral peaks prevent one sinusoid's neighbouring bins from inventing notes.
        for bin in 2..spectrum.len() - 1 {
            let peak = spectrum[bin];
            if peak < 1e-5 || peak <= spectrum[bin - 1] || peak <= spectrum[bin + 1] {
                continue;
            }
            let a = spectrum[bin - 1].max(1e-12).ln();
            let b = peak.ln();
            let c = spectrum[bin + 1].max(1e-12).ln();
            let offset = (0.5 * (a - c) / (a - 2.0 * b + c)).clamp(-0.5, 0.5);
            let hz = (bin as f64 + f64::from(offset)) * ANALYSIS_RATE / FFT as f64;
            if !(65.0..=4000.0).contains(&hz) {
                continue;
            }
            let midi = 69.0 + 12.0 * (hz / 440.0).log2();
            if (midi - midi.round()).abs() > 0.4 {
                continue;
            }
            chroma[chord_index][(midi.round() as i32).rem_euclid(12) as usize] += peak;
        }
        if options.transcribe && rms > 1e-7 {
            let samples = audio.channel(best_channel);
            for (i, sample) in pitch_window.iter_mut().enumerate() {
                *sample = samples
                    .get((center as isize + i as isize - pitch::WINDOW as isize / 2) as usize)
                    .copied()
                    .unwrap_or(0.0);
            }
            pitches.push(pitch::candidates(&pitch_window, &mut difference));
        } else {
            pitches.push(Vec::new());
        }
    }
    let mut onset: Vec<f32> = envelope
        .iter()
        .enumerate()
        .map(|(i, x)| (*x - if i == 0 { 0.0 } else { envelope[i - 1] }).max(0.0))
        .collect();
    normalize(&mut onset);
    normalize(&mut flux);
    for (onset, flux) in onset.iter_mut().zip(flux) {
        *onset = 0.8 * *onset + 0.2 * flux;
    }
    let tempo = estimate_tempo(&onset, duration, control)?;
    let raw = chroma.iter().map(|w| rank(w, None, true)).collect();
    let readings = smooth(
        raw,
        control,
        0.9,
        if options.transcribe { 0.94 } else { 1.0 },
    )?;
    let chords = readings
        .into_iter()
        .enumerate()
        .map(|(i, reading)| AudioChordSegment {
            start: i as f64 * 0.5,
            end: ((i + 1) as f64 * 0.5).min(duration),
            reading,
        })
        .collect();
    let notes = if options.transcribe {
        let path = pitch::decode(&pitches, &envelope, control)?;
        note_events(&path, &envelope, duration)
    } else {
        Vec::new()
    };
    control.check(1.0)?;
    Ok(AudioAnalysis {
        algorithm: "cpu-spectral-yin-v2",
        seconds: duration,
        options,
        tempo,
        chords,
        notes,
    })
}

fn normalize(values: &mut [f32]) {
    let max = values.iter().copied().fold(0.0f32, f32::max);
    if max > 1e-8 {
        for v in values {
            *v /= max;
        }
    }
}

fn estimate_tempo(
    onset: &[f32],
    duration: f64,
    control: &AnalysisControl,
) -> Result<TempoAnalysis, AnalysisError> {
    let dt = HOP as f64 / ANALYSIS_RATE;
    let mut result = TempoAnalysis {
        candidates: vec![],
        beats: vec![],
    };
    let pulses = onset
        .iter()
        .enumerate()
        .filter(|(i, v)| {
            **v > 0.15
                && (*i == 0 || **v >= onset[*i - 1])
                && (*i + 1 == onset.len() || **v > onset[*i + 1])
        })
        .count();
    if duration < 2.0 || pulses < 4 {
        return Ok(result);
    }
    let min_lag = (60.0 / 240.0 / dt).ceil() as usize;
    let max_lag = ((60.0 / 40.0 / dt).floor() as usize).min(onset.len() / 3);
    let mut correlations = vec![0.0f32; max_lag + 2];
    for lag in min_lag..=max_lag {
        control.check(0.8 + 0.08 * lag as f32 / (max_lag + 1) as f32)?;
        let (mut dot, mut a, mut b) = (0.0, 0.0, 0.0);
        for i in lag..onset.len() {
            dot += onset[i] * onset[i - lag];
            a += onset[i] * onset[i];
            b += onset[i - lag] * onset[i - lag];
        }
        correlations[lag] = if a * b > 1e-12 {
            dot / (a * b).sqrt()
        } else {
            0.0
        };
    }
    let mut peaks: Vec<_> = (min_lag..=max_lag)
        .filter(|i| {
            correlations[*i] > 0.15
                && correlations[*i] >= correlations[i - 1]
                && correlations[*i] >= correlations[i + 1]
        })
        .collect();
    // Slight preference for the quicker pulse when its every-other-beat match is tied.
    peaks.sort_by(|a, b| {
        (correlations[*b] - 0.0001 * *b as f32).total_cmp(&(correlations[*a] - 0.0001 * *a as f32))
    });
    for &lag in peaks.iter().take(3) {
        let (a, b, c) = (
            correlations[lag - 1],
            correlations[lag],
            correlations[lag + 1],
        );
        let offset = if (a - 2.0 * b + c).abs() > 1e-6 {
            (0.5 * (a - c) / (a - 2.0 * b + c)).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        result.candidates.push(TempoCandidate {
            bpm: 60.0 / ((lag as f64 + f64::from(offset)) * dt),
            score: b,
        });
    }
    let Some(&lag) = peaks.first() else {
        return Ok(result);
    };
    // Dynamic programming follows strong local onsets with a soft inter-beat constraint.
    let mut value = vec![0.0f32; onset.len()];
    let mut previous = vec![None; onset.len()];
    for i in 0..onset.len() {
        if i % 256 == 0 {
            control.check(0.89)?;
        }
        let best = (lag / 2..=lag * 2)
            .filter_map(|gap| {
                i.checked_sub(gap)
                    .map(|j| (j, value[j] - 2.0 * (gap as f32 / lag as f32).ln().powi(2)))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1));
        value[i] = onset[i];
        if let Some((j, v)) = best.filter(|(_, v)| *v > 0.0) {
            value[i] += v;
            previous[i] = Some(j);
        }
    }
    let end = (onset.len().saturating_sub(lag * 2)..onset.len())
        .max_by(|a, b| value[*a].total_cmp(&value[*b]));
    let mut next = end;
    while let Some(i) = next {
        result.beats.push(i as f64 * dt);
        next = previous[i];
    }
    result.beats.reverse();
    Ok(result)
}

fn note_events(pitches: &[Option<u8>], levels: &[f32], duration: f64) -> Vec<TranscribedNote> {
    let dt = HOP as f64 / ANALYSIS_RATE;
    let maximum = levels.iter().copied().fold(0.0f32, f32::max);
    let mut result = Vec::new();
    let mut active: Option<(u8, usize, f32)> = None;
    for i in 0..=pitches.len() {
        let current = pitches
            .get(i)
            .copied()
            .flatten()
            .filter(|_| levels[i] > maximum * 0.03);
        let attack = i > 0 && i < pitches.len() && levels[i] > levels[i - 1] * 2.5;
        if let Some((pitch, from, level)) = active
            && (current != Some(pitch) || (attack && i - from >= 4))
        {
            if i - from >= 3 {
                result.push(TranscribedNote {
                    pitch,
                    start: from as f64 * dt,
                    end: (i as f64 * dt).min(duration),
                    strength: (level / maximum.max(1e-8)).sqrt().clamp(0.1, 1.0),
                });
            }
            active = None;
        }
        if let Some(pitch) = current {
            match &mut active {
                Some((_, _, level)) => *level = level.max(levels[i]),
                None => active = Some((pitch, i, levels[i])),
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn buffer(seconds: f64, f: impl Fn(f64) -> f32) -> AudioBuffer {
        AudioBuffer::from_planar(
            vec![
                (0..(seconds * ANALYSIS_RATE) as usize)
                    .map(|i| f(i as f64 / ANALYSIS_RATE))
                    .collect(),
            ],
            ANALYSIS_RATE,
        )
        .unwrap()
    }
    #[test]
    fn silence_and_short_audio_do_not_invent_music() {
        for seconds in [0.0, 0.001, 3.0] {
            let r = analyze_audio(
                &buffer(seconds, |_| 0.0),
                AudioOptions { transcribe: true },
                &AnalysisControl::default(),
            )
            .unwrap();
            assert!(r.notes.is_empty());
            assert!(r.tempo.candidates.is_empty());
            assert!(
                r.chords
                    .iter()
                    .all(|c| c.reading.state == crate::chords::ChordState::NoChord)
            );
        }
    }
    #[test]
    fn pulse_tempos_and_beats_are_measured_numerically() {
        for bpm in [72.0, 120.0, 180.0] {
            let audio = buffer(10.0, |t| {
                if (t % (60.0 / bpm)) < 0.025 {
                    (t * 2300.0 * std::f64::consts::TAU).sin() as f32 * 0.5
                } else {
                    0.0
                }
            });
            let r = analyze_audio(&audio, AudioOptions::default(), &AnalysisControl::default())
                .unwrap();
            assert!(
                r.tempo
                    .candidates
                    .iter()
                    .any(|c| (c.bpm - bpm).abs() / bpm < 0.04),
                "{bpm}: {:?}",
                r.tempo
            );
            assert!(r.tempo.beats.len() > 5);
            if (r.tempo.candidates[0].bpm - bpm).abs() / bpm < 0.04 {
                assert!(
                    r.tempo
                        .beats
                        .iter()
                        .filter(|t| {
                            let phase = **t % (60.0 / bpm);
                            phase.min(60.0 / bpm - phase) < 0.07
                        })
                        .count() as f32
                        / r.tempo.beats.len() as f32
                        > 0.9
                );
            }
        }
    }
    #[test]
    fn isolated_notes_keep_pitch_rests_and_source_timing() {
        let audio = buffer(2.0, |t| {
            let hz = if (0.2..0.8).contains(&t) {
                440.0
            } else if (1.1..1.7).contains(&t) {
                261.6256
            } else {
                0.0
            };
            (t * hz * std::f64::consts::TAU).sin() as f32 * 0.4
        });
        let r = analyze_audio(
            &audio,
            AudioOptions { transcribe: true },
            &AnalysisControl::default(),
        )
        .unwrap();
        assert_eq!(
            r.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(),
            vec![69, 60],
            "{:?}",
            r.notes
        );
        for (note, from, to) in [(&r.notes[0], 0.2, 0.8), (&r.notes[1], 1.1, 1.7)] {
            assert!((note.start - from).abs() < 0.08, "{note:?}");
            assert!((note.end - to).abs() < 0.08, "{note:?}");
        }
    }
    #[test]
    fn triad_and_opposite_phase_stereo_produce_the_same_chords() {
        let mono = buffer(2.0, |t| {
            [261.6256, 329.6276, 391.9954]
                .iter()
                .map(|hz| (t * hz * std::f64::consts::TAU).sin() as f32 * 0.15)
                .sum()
        });
        let stereo = AudioBuffer::from_planar(
            vec![
                mono.channel(0).to_vec(),
                mono.channel(0).iter().map(|v| -v).collect(),
            ],
            ANALYSIS_RATE,
        )
        .unwrap();
        let a = analyze_audio(&mono, AudioOptions::default(), &AnalysisControl::default()).unwrap();
        let b = analyze_audio(
            &stereo,
            AudioOptions::default(),
            &AnalysisControl::default(),
        )
        .unwrap();
        assert_eq!(a.chords, b.chords);
        assert_eq!(a.chords[1].reading.candidates[0].symbol, "C");
    }
    #[test]
    fn invalid_audio_and_cancellation_are_errors() {
        let control = AnalysisControl::default();
        control.cancel();
        assert!(matches!(
            analyze_audio(&buffer(1.0, |_| 0.0), AudioOptions::default(), &control),
            Err(AnalysisError::Cancelled)
        ));
        assert!(
            analyze_audio(
                &buffer(1.0, |_| f32::NAN),
                AudioOptions::default(),
                &AnalysisControl::default()
            )
            .is_err()
        );
    }
}
