//! Read-only library audition, acoustic indexing and explicit adoption of a discovered sound.

use super::Session;
use crate::{Edit, SessionError};
use auris_core::plugin::{NoteEvent, PluginState, PrepareContext, ProcessContext};
use auris_core::project::PresetRef;
use auris_core::{AudioBuffer, ParamTarget, PluginRegistry, TrackId};
use auris_dsp::timbre::{TimbreProjection, project_timbres, timbre_features};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

const BLOCK: usize = 512;
const HOLD: f64 = 0.6;
const SECONDS: f64 = 1.0;

/// Stable library identity; names are presentation only and never enter feature extraction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimbreSound {
    /// Display name, including the library name for SoundFont presets.
    pub name: String,
    /// Built-in registry identifier.
    pub instrument_id: String,
    /// Exact library preset where this is a SoundFont sound.
    pub preset: Option<PresetRef>,
}

/// Shareable cancellation and completed-source count for a library scan.
#[derive(Clone, Default)]
pub struct TimbreMapControl {
    cancelled: Arc<AtomicBool>,
    completed: Arc<AtomicUsize>,
}
impl TimbreMapControl {
    /// Stops work at the next audio block or source boundary.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
    /// Number of source sounds whose measurement finished, including silent sources.
    pub fn completed(&self) -> usize {
        self.completed.load(Ordering::Relaxed)
    }
    fn check(&self) -> Result<(), SessionError> {
        if self.cancelled.load(Ordering::Relaxed) {
            Err(failure("cancelled"))
        } else {
            Ok(())
        }
    }
}

/// Immutable library snapshot. Move to an ordinary background thread before calling `run`.
pub struct TimbreMapJob {
    registry: Arc<PluginRegistry>,
    sounds: Vec<TimbreSound>,
    sample_rate: f64,
}

/// A reusable acoustic index and audition buffers. Nothing here is a project edit.
pub struct TimbreMap {
    /// Audible sources in the same order as coordinates and group labels.
    pub sounds: Vec<TimbreSound>,
    /// Sources omitted because at least one reference trigger was silent.
    pub skipped: Vec<String>,
    projection: TimbreProjection,
    previews: Vec<Arc<AudioBuffer>>,
    sample_rate: f64,
}

impl Session {
    /// Snapshots built-in melodic instruments and loaded melodic SoundFont presets, up to 512.
    ///
    /// Fonts are shared immutably in a new bank so a later project load cannot change this job.
    /// Each source is measured at MIDI 48, 60 and 72, at velocities 0.45 and 0.85, with a 600 ms
    /// hold and 400 ms release. Neither live tracks nor their mixer state enter the analysis.
    pub fn timbre_map_job(&self) -> Result<TimbreMapJob, SessionError> {
        let fonts = auris_sampler::SoundFontBank::shared();
        let mut sounds: Vec<_> = self
            .registry
            .instruments()
            .filter(|d| {
                d.id.as_ref() != auris_sampler::SAMPLER_ID
                    && d.id.as_ref() != "auris.synth.noisedrum"
            })
            .map(|d| TimbreSound {
                name: d.name.to_string(),
                instrument_id: d.id.to_string(),
                preset: None,
            })
            .collect();
        for reference in self.soundfonts() {
            if let Some(font) = self.fonts.get(reference.id) {
                fonts.insert(reference.id, font);
                for preset in self
                    .soundfont_presets(reference.id)
                    .into_iter()
                    .filter(|p| p.bank != 128)
                {
                    sounds.push(TimbreSound {
                        name: format!(
                            "{} / {} [{}:{}]",
                            reference.name, preset.name, preset.bank, preset.patch
                        ),
                        instrument_id: auris_sampler::SAMPLER_ID.into(),
                        preset: Some(PresetRef {
                            font: reference.id,
                            bank: preset.bank,
                            patch: preset.patch,
                        }),
                    });
                }
            }
        }
        if sounds.len() > 512 {
            return Err(failure(
                "the timbre map accepts at most 512 sounds; use a smaller library",
            ));
        }
        Ok(TimbreMapJob {
            registry: crate::default_registry(fonts),
            sounds,
            sample_rate: self.sample_rate(),
        })
    }

    /// Adopts a mapped sound on a track as one undoable instrument or preset edit.
    /// A built-in sound restores its measured default patch and clears instrument automation,
    /// even when the track already uses that instrument. Mixer settings remain in place.
    pub fn use_timbre_sound(
        &mut self,
        track: TrackId,
        sound: &TimbreSound,
    ) -> Result<(), SessionError> {
        match sound.preset {
            Some(preset) => self.set_track_preset(track, preset),
            None => {
                self.set_track_instrument(track, &sound.instrument_id)?;
                // The ordinary library picker keeps a current instrument's edited patch.
                // A map result instead names the default patch used for its reference recording.
                let needs_reset = self
                    .project
                    .track(track)
                    .and_then(|track| track.kind.as_instrument())
                    .is_some_and(|inner| !inner.instrument_state.is_empty() || inner.file.is_some())
                    || self.project.automation.lanes().iter().any(|lane| {
                        matches!(lane.target, ParamTarget::Instrument { track: id, .. } if id == track)
                    });
                if needs_reset {
                    self.record(Edit::ChangeInstrument);
                    let inner = self
                        .project
                        .track_mut(track)
                        .and_then(|track| track.kind.as_instrument_mut())
                        .expect("set_track_instrument validated the target");
                    inner.instrument_state = PluginState::empty();
                    inner.file = None;
                    self.project.remove_instrument_automation(track);
                    self.invalidate_graph();
                }
                Ok(())
            }
        }
    }

    /// Plays a cached, level-matched reference through the chosen track without changing it.
    ///
    /// The track's mute, inserts, fader and routing apply to audition, but never to analysis.
    pub fn preview_timbre(
        &mut self,
        map: &TimbreMap,
        index: usize,
        track: TrackId,
    ) -> Result<(), SessionError> {
        self.require_track(track)?;
        if (map.sample_rate - self.sample_rate()).abs() > 0.1 {
            return Err(failure("the audio rate changed; rescan the library"));
        }
        let buffer = map
            .previews
            .get(index)
            .ok_or_else(|| failure("unknown mapped sound"))?;
        self.play_singer_preview(track, buffer);
        Ok(())
    }
}

impl TimbreMapJob {
    /// Number of library sources to measure.
    pub fn sound_count(&self) -> usize {
        self.sounds.len()
    }

    /// Measures fresh instances and builds a deterministic PCA map and feature-space clusters.
    pub fn run(self, control: &TimbreMapControl) -> Result<TimbreMap, SessionError> {
        let mut sounds = Vec::new();
        let mut rows = Vec::new();
        let mut previews = Vec::new();
        let mut skipped = Vec::new();
        for (index, sound) in self.sounds.into_iter().enumerate() {
            control.check()?;
            let mut features = Vec::new();
            let mut preview = None;
            let mut silent = false;
            for pitch in [48, 60, 72] {
                for velocity in [0.45, 0.85] {
                    let mut instrument = self.registry.create_instrument(&sound.instrument_id)?;
                    let mut state = PluginState::default();
                    if let Some(preset) = sound.preset {
                        auris_sampler::store_preset(&mut state, preset);
                    }
                    instrument.load_state(&state);
                    instrument.prepare(&PrepareContext::new(self.sample_rate, BLOCK, 2));
                    let frames = (self.sample_rate * SECONDS).round() as usize;
                    let off = (self.sample_rate * HOLD).round() as usize;
                    let mut audio = AudioBuffer::stereo(frames, self.sample_rate);
                    let mut block = AudioBuffer::stereo(BLOCK, self.sample_rate);
                    for start in (0..frames).step_by(BLOCK) {
                        control.check()?;
                        let count = (frames - start).min(BLOCK);
                        block.set_frame_count(count);
                        block.clear();
                        let mut events = Vec::with_capacity(2);
                        if start == 0 {
                            events.push(NoteEvent::NoteOn {
                                frame: 0,
                                pitch,
                                velocity,
                            });
                        }
                        if (start..start + count).contains(&off) {
                            events.push(NoteEvent::NoteOff {
                                frame: (off - start) as u32,
                                pitch,
                            });
                        }
                        instrument.process(
                            &events,
                            &mut block,
                            &ProcessContext {
                                sample_rate: self.sample_rate,
                                block_frames: count,
                                playhead_samples: start as u64,
                                bpm: 120.0,
                                is_playing: true,
                                is_offline: true,
                            },
                        );
                        for channel in 0..2 {
                            audio.channel_mut(channel)[start..start + count]
                                .copy_from_slice(block.channel(channel));
                        }
                    }
                    match timbre_features(&audio, HOLD).map_err(failure)? {
                        Some(values) => features.extend(values),
                        None => silent = true,
                    }
                    if pitch == 60 && velocity == 0.85 {
                        level_match(&mut audio);
                        preview = Some(Arc::new(audio));
                    }
                }
            }
            if silent {
                skipped.push(sound.name);
            } else {
                sounds.push(sound);
                rows.push(features);
                previews.push(preview.expect("the reference conditions include middle C"));
            }
            control.completed.store(index + 1, Ordering::Relaxed);
        }
        control.check()?;
        let groups = (sounds.len() as f64).sqrt().round() as usize;
        let projection = project_timbres(&rows, groups.clamp(1, 12)).map_err(failure)?;
        Ok(TimbreMap {
            sounds,
            skipped,
            projection,
            previews,
            sample_rate: self.sample_rate,
        })
    }
}

impl TimbreMap {
    /// Raw two-component PCA coordinates in source order.
    pub fn positions(&self) -> &[[f64; 2]] {
        &self.projection.positions
    }
    /// Zero-based group labels computed in standardized feature space.
    pub fn clusters(&self) -> &[usize] {
        &self.projection.clusters
    }
    /// Fraction of feature variance retained by the two displayed components.
    pub fn explained_variance(&self) -> f64 {
        self.projection.explained_variance
    }
    /// Up to `limit` other sources ordered by full-feature Euclidean distance.
    pub fn nearest(&self, index: usize, limit: usize) -> Vec<(usize, f64)> {
        let Some(query) = self.projection.standardized.get(index) else {
            return Vec::new();
        };
        let mut neighbors: Vec<_> = self
            .projection
            .standardized
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(i, r)| {
                (
                    i,
                    r.iter()
                        .zip(query)
                        .map(|(a, b)| (a - b).powi(2))
                        .sum::<f64>()
                        .sqrt(),
                )
            })
            .collect();
        neighbors.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        neighbors.truncate(limit);
        neighbors
    }
}

fn failure(message: impl Into<String>) -> SessionError {
    SessionError::MusicAnalysis(message.into())
}
fn level_match(audio: &mut AudioBuffer) {
    let energy = audio
        .channels()
        .iter()
        .flatten()
        .map(|v| f64::from(*v).powi(2))
        .sum::<f64>();
    let peak = audio
        .channels()
        .iter()
        .flatten()
        .map(|v| v.abs())
        .fold(0.0f32, f32::max);
    let rms = (energy / (audio.frame_count() * audio.channel_count()).max(1) as f64).sqrt();
    let gain = (0.1 / rms.max(1e-12)).min(0.8 / f64::from(peak).max(1e-12)) as f32;
    for channel in 0..audio.channel_count() {
        let frames = audio.frame_count();
        let fade = (audio.sample_rate() * 0.01) as usize;
        for (i, sample) in audio.channel_mut(channel).iter_mut().enumerate() {
            *sample *= gain * ((frames - 1 - i) as f32 / fade.max(1) as f32).min(1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionOptions;
    use auris_core::Ticks;

    #[test]
    fn adopting_the_current_builtin_restores_its_measured_sound_in_one_undo_step() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let descriptors = session.param_descriptors(crate::DEFAULT_INSTRUMENT);
        let waveform = descriptors.iter().find(|d| d.key == "waveform").unwrap();
        let target = ParamTarget::Instrument {
            track,
            param: waveform.id,
        };
        session.set_param(target, 2.0);
        assert!(session.set_automation_point(target, Ticks::ZERO, 2.0));
        let gain = ParamTarget::TrackGain(track);
        assert!(session.set_automation_point(gain, Ticks::ZERO, -6.0));
        let before = session.project().clone();
        session.forget_history();
        let sound = TimbreSound {
            name: "Reference".into(),
            instrument_id: crate::DEFAULT_INSTRUMENT.into(),
            preset: None,
        };

        session.use_timbre_sound(track, &sound).unwrap();
        assert_eq!(session.param_value(target, waveform), waveform.default);
        assert!(session.automation().lane(target).is_none());
        assert_eq!(
            session.automation().lane(gain),
            before.automation.lane(gain)
        );
        assert_eq!(session.undo(), Some(Edit::ChangeInstrument));
        assert_eq!(session.project(), &before);
        assert!(!session.can_undo(), "adoption is one edit");
    }

    #[test]
    fn scanning_is_read_only_and_returns_playable_finite_neighbors() {
        let session = Session::new(SessionOptions::headless()).unwrap();
        let before = session.project().clone();
        let control = TimbreMapControl::default();
        let job = session.timbre_map_job().unwrap();
        let count = job.sound_count();
        let map = job.run(&control).unwrap();
        assert_eq!(&before, session.project());
        assert_eq!(control.completed(), count);
        assert!(map.sounds.len() >= 2);
        assert_eq!(map.positions().len(), map.sounds.len());
        assert_eq!(map.nearest(0, 3).len(), 3.min(map.sounds.len() - 1));
        assert!(
            map.nearest(0, 10)
                .iter()
                .all(|(i, d)| *i != 0 && d.is_finite())
        );
        assert!(map.previews.iter().all(|p| {
            p.channels()
                .iter()
                .flatten()
                .all(|v| v.is_finite() && v.abs() <= 0.801)
        }));
        let cancelled = TimbreMapControl::default();
        cancelled.cancel();
        assert!(session.timbre_map_job().unwrap().run(&cancelled).is_err());
    }

    #[test]
    fn soundfont_jobs_keep_their_samples_after_the_live_bank_changes() {
        let scratch = super::super::fixtures::Scratch::new("timbre-font");
        let path = scratch.soundfont("Tone.sf2");
        let mut bytes = std::fs::read(&path).unwrap();
        let start = bytes.windows(4).position(|w| w == b"smpl").unwrap() + 8;
        // The generic fixture has only 64 samples: higher notes end during SoundFont's
        // default envelope delay. Give this playback fixture a full second plus guard samples.
        let mut pcm = vec![0u8; (48_000 + 46) * 2];
        for i in 0..48_000 {
            let value =
                ((std::f64::consts::TAU * 440.0 * i as f64 / 48_000.0).sin() * 20_000.0) as i16;
            pcm[i * 2..i * 2 + 2].copy_from_slice(&value.to_le_bytes());
        }
        let pcm_bytes = pcm.len() as u32;
        bytes.splice(start..start + 256, pcm);
        bytes[start - 4..start].copy_from_slice(&pcm_bytes.to_le_bytes());
        let sdta = bytes.windows(4).position(|w| w == b"sdta").unwrap();
        bytes[sdta - 4..sdta].copy_from_slice(&(pcm_bytes + 12).to_le_bytes());
        let shdr = bytes.windows(4).position(|w| w == b"shdr").unwrap() + 8;
        bytes[shdr + 24..shdr + 28].copy_from_slice(&48_000u32.to_le_bytes());
        bytes[shdr + 32..shdr + 36].copy_from_slice(&47_999u32.to_le_bytes());
        let riff_bytes = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&riff_bytes.to_le_bytes());
        std::fs::write(&path, bytes).unwrap();
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let font = session.import_soundfont(&path).unwrap();
        let job = session.timbre_map_job().unwrap();
        session.fonts.clear();
        let map = job.run(&TimbreMapControl::default()).unwrap();
        assert!(
            map.sounds
                .iter()
                .any(|s| s.preset.is_some_and(|p| p.font == font))
        );
    }
}
