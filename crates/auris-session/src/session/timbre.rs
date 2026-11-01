//! Read-only library audition, acoustic indexing and explicit adoption of a discovered sound.

use super::Session;
use super::SoundEnvelope;
use crate::{Edit, SessionError};
use crate::{EnvelopeTargets, TimbreCharacter, TimbreTargets};
use auris_core::plugin::{NoteEvent, PluginState, PrepareContext, ProcessContext};
use auris_core::project::PresetRef;
use auris_core::{AudioBuffer, ParamTarget, PluginRegistry, TrackId};
use auris_dsp::timbre::{
    TimbreProjection, project_timbres_refined, timbre_character, timbre_features,
};
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

/// One populated acoustic cluster, summarized using the full standardized feature space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimbreCluster {
    /// Label shared with `TimbreMap::clusters`; stable only within this scan.
    pub id: usize,
    /// Number of measured sources assigned to this cluster.
    pub sound_count: usize,
    /// Source closest to the cluster centroid; a useful default display name.
    pub representative: TimbreSound,
    /// Up to eight source names, ordered by distance to the centroid.
    pub examples: Vec<String>,
    /// Family counts from every member's name, including unknown sounds.
    pub icon_counts: Vec<(crate::SoundIcon, usize)>,
}

impl TimbreCluster {
    /// A family shared by at least two thirds of all members, otherwise a generic icon.
    pub fn icon(&self) -> crate::SoundIcon {
        self.icon_counts
            .iter()
            .find(|(_, count)| *count * 3 >= self.sound_count * 2)
            .map_or(crate::SoundIcon::Sound, |(icon, _)| *icon)
    }
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
    pub(super) fn check(&self) -> Result<(), SessionError> {
        if self.cancelled.load(Ordering::Relaxed) {
            Err(failure("cancelled"))
        } else {
            Ok(())
        }
    }
}

/// Makes a fresh, prepared instrument for a mapped sound.
type NewInstrument<'a> =
    dyn Fn(&TimbreSound) -> Result<Box<dyn auris_core::Instrument>, SessionError> + 'a;

/// Immutable library snapshot. Move to an ordinary background thread before calling `run`.
pub struct TimbreMapJob {
    registry: Arc<PluginRegistry>,
    sounds: Vec<TimbreSound>,
    /// Each font's file, for the envelope cache's key; stamped on the worker.
    font_files: Vec<(auris_core::project::SoundFontId, std::path::PathBuf)>,
    sample_rate: f64,
}

/// A reusable acoustic index and audition buffers. Nothing here is a project edit.
pub struct TimbreMap {
    /// Audible sources in the same order as coordinates and group labels.
    pub sounds: Vec<TimbreSound>,
    /// Sources omitted because at least one reference trigger was silent.
    pub skipped: Vec<String>,
    projection: TimbreProjection,
    characters: Vec<TimbreCharacter>,
    envelopes: Vec<Option<SoundEnvelope>>,
    summaries: Vec<TimbreCluster>,
    previews: Vec<Arc<AudioBuffer>>,
    sample_rate: f64,
    registry: Arc<PluginRegistry>,
}

/// A single mapped note rendered from the scan's immutable library snapshot on a worker.
pub struct TimbrePreviewJob {
    registry: Arc<PluginRegistry>,
    sound: TimbreSound,
    pitch: u8,
    sample_rate: f64,
    cached: Option<Arc<AudioBuffer>>,
}

impl TimbrePreviewJob {
    /// Renders the instrument's actual note, with the map's hold, release and level matching.
    pub fn run(self, control: &TimbreMapControl) -> Result<Arc<AudioBuffer>, SessionError> {
        control.check()?;
        if let Some(cached) = self.cached {
            return Ok(cached);
        }
        let mut instrument = self.registry.create_instrument(&self.sound.instrument_id)?;
        let mut state = PluginState::default();
        if let Some(preset) = self.sound.preset {
            auris_sampler::store_preset(&mut state, preset);
        }
        instrument.load_state(&state);
        instrument.prepare(&PrepareContext::new(self.sample_rate, BLOCK, 2));
        let mut audio = render_reference(
            instrument.as_mut(),
            self.sample_rate,
            self.pitch,
            0.85,
            control,
        )?;
        audio.sanitize();
        if audio.peak() <= 1e-8 {
            return Err(failure("the selected sound is silent at this pitch"));
        }
        level_match(&mut audio);
        Ok(Arc::new(audio))
    }
}

impl Session {
    /// Prepares a mapped note for independent output audition, without requiring a track.
    ///
    /// Run the job on a worker, then pass its buffer to [`Self::play_output_preview`].
    /// Middle C reuses the measured reference; other pitches trigger fresh instruments rather
    /// than pitch-shifting the reference. The scan's SoundFonts remain alive for the job.
    pub fn timbre_preview_job(
        &self,
        map: &TimbreMap,
        index: usize,
        pitch: u8,
    ) -> Result<TimbrePreviewJob, SessionError> {
        if self.is_recording() {
            return Err(SessionError::RecordingInProgress);
        }
        if pitch > 127 {
            return Err(failure("preview pitch must be a MIDI note from 0 to 127"));
        }
        if map.sample_rate != self.sample_rate() {
            return Err(failure("the audio rate changed; rescan the library"));
        }
        let sound = map
            .sounds
            .get(index)
            .ok_or_else(|| failure("unknown mapped sound"))?;
        Ok(TimbrePreviewJob {
            registry: Arc::clone(&map.registry),
            sound: sound.clone(),
            pitch,
            sample_rate: map.sample_rate,
            cached: map.previews.get(index).filter(|_| pitch == 60).cloned(),
        })
    }

    /// Persistent identity for a library correction. Font IDs are document-local, so fonts use
    /// their resolved path and exact bank/patch instead. This performs no filesystem access.
    pub fn library_sound_key(
        &self,
        instrument_id: &str,
        preset: Option<PresetRef>,
    ) -> Option<String> {
        match preset {
            None => Some(format!("instrument:{instrument_id}")),
            Some(preset) => {
                let font = self.soundfonts().find(|font| font.id == preset.font)?;
                let path = font.path.resolve(self.project_folder())?;
                serde_json::to_string(&(instrument_id, path, preset.bank, preset.patch)).ok()
            }
        }
    }

    /// Snapshots built-in melodic instruments and loaded melodic SoundFont presets, up to 512.
    ///
    /// Fonts are shared immutably in a new bank so a later project load cannot change this job.
    /// Each source is measured at MIDI 48, 60 and 72, at velocities 0.45 and 0.85, with a 600 ms
    /// hold and 400 ms release. Neither live tracks nor their mixer state enter the analysis.
    pub fn timbre_map_job(&self) -> Result<TimbreMapJob, SessionError> {
        let fonts = auris_sampler::SoundFontBank::shared();
        let mut font_files = Vec::new();
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
                if let Some(path) = reference.path.resolve(self.project_folder()) {
                    font_files.push((reference.id, path));
                }
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
            font_files,
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
        let mut characters = Vec::new();
        let mut envelopes = Vec::new();
        let mut previews = Vec::new();
        let mut skipped = Vec::new();
        let new_instrument = |sound: &TimbreSound| -> Result<_, SessionError> {
            let mut instrument = self.registry.create_instrument(&sound.instrument_id)?;
            let mut state = PluginState::default();
            if let Some(preset) = sound.preset {
                auris_sampler::store_preset(&mut state, preset);
            }
            instrument.load_state(&state);
            instrument.prepare(&PrepareContext::new(self.sample_rate, BLOCK, 2));
            Ok(instrument)
        };
        for (index, sound) in self.sounds.iter().enumerate() {
            control.check()?;
            let mut features = Vec::new();
            let mut reference_characters = Vec::new();
            let mut preview = None;
            let mut silent = false;
            for pitch in [48, 60, 72] {
                for velocity in [0.45, 0.85] {
                    let mut instrument = new_instrument(sound)?;
                    let mut audio = render_reference(
                        instrument.as_mut(),
                        self.sample_rate,
                        pitch,
                        velocity,
                        control,
                    )?;
                    match timbre_features(&audio, HOLD).map_err(failure)? {
                        Some(values) => {
                            reference_characters.push(timbre_character(&audio, HOLD, &values));
                            features.extend(values);
                        }
                        None => silent = true,
                    }
                    if pitch == 60 && velocity == 0.85 {
                        level_match(&mut audio);
                        preview = Some(Arc::new(audio));
                    }
                }
            }
            if silent {
                skipped.push(sound.name.clone());
            } else {
                envelopes.push(self.envelope(sound, &new_instrument, control)?);
                sounds.push(sound.clone());
                rows.push(features);
                characters.push(TimbreCharacter::mean(&reference_characters));
                previews.push(preview.expect("the reference conditions include middle C"));
            }
            control.completed.store(index + 1, Ordering::Relaxed);
        }
        control.check()?;
        super::envelope::save();
        let groups = (sounds.len() as f64).sqrt().round() as usize;
        let projection = project_timbres_refined(&rows, groups.clamp(1, 12)).map_err(failure)?;
        let summaries = cluster_summaries(&sounds, &projection);
        Ok(TimbreMap {
            sounds,
            skipped,
            projection,
            characters,
            envelopes,
            summaries,
            previews,
            sample_rate: self.sample_rate,
            registry: self.registry,
        })
    }

    /// The sound's measured envelope, from the disk cache when its font file is unchanged.
    fn envelope(
        &self,
        sound: &TimbreSound,
        new_instrument: &NewInstrument<'_>,
        control: &TimbreMapControl,
    ) -> Result<Option<SoundEnvelope>, SessionError> {
        let key = sound.preset.and_then(|preset| {
            let (_, path) = self.font_files.iter().find(|(id, _)| *id == preset.font)?;
            Some(super::sound_search::font_envelope_key(
                path,
                preset.bank,
                preset.patch,
            ))
        });
        if let Some(cached) = key.as_deref().and_then(super::envelope::cached) {
            return Ok(Some(cached));
        }
        let measured = super::envelope::measure_sound(|pitch| {
            let mut instrument = new_instrument(sound).map_err(|e| e.to_string())?;
            super::envelope::measure_note(instrument.as_mut(), self.sample_rate, pitch, control)
                .map_err(|e| e.to_string())
        })
        .map_err(|e| control.check().err().unwrap_or_else(|| failure(e)))?;
        if let (Some(key), Some(envelope)) = (key, &measured) {
            super::envelope::remember(key, envelope.clone());
        }
        Ok(measured)
    }
}

impl TimbreMap {
    /// Fixed-scale descriptors shared by map axes and impression search.
    pub fn characters(&self) -> &[TimbreCharacter] {
        &self.characters
    }

    /// Each mapped sound's measured envelope, in the order of [`sounds`](Self::sounds); `None`
    /// where no measured pitch sounded.
    pub fn envelopes(&self) -> &[Option<SoundEnvelope>] {
        &self.envelopes
    }

    /// Coordinates from the measured envelope: attack time, sustain level and release time,
    /// times on a log scale from 1 ms to 10 s. A sound with no measurement sits at the centre.
    pub fn envelope_positions(&self) -> Vec<[f64; 3]> {
        let time = |ms: f64| (ms.max(1.).log10() / 4.).clamp(0., 1.) * 2. - 1.;
        self.envelopes
            .iter()
            .map(|e| {
                e.as_ref().map_or([0.; 3], |e| {
                    let e = e.summary;
                    [
                        time(e.attack_ms),
                        (e.sustain_db / 60. + 1.).clamp(0., 1.) * 2. - 1.,
                        time(e.release_ms),
                    ]
                })
            })
            .collect()
    }

    /// Coordinates with stable meanings: brightness, attack speed and sustain.
    pub fn character_positions(&self) -> Vec<[f64; 3]> {
        self.characters
            .iter()
            .map(|c| {
                [
                    c.brightness * 2. - 1.,
                    c.attack * 2. - 1.,
                    c.sustain * 2. - 1.,
                ]
            })
            .collect()
    }

    /// Rank measured sounds using shared interpreted targets and envelope ranges — the same
    /// distance the agent's `search_instruments` uses; names can be filtered separately.
    pub fn search_character(
        &self,
        targets: &TimbreTargets,
        envelope: &EnvelopeTargets,
    ) -> Vec<(usize, f64)> {
        let mut matches: Vec<_> = self
            .characters
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let shape = if envelope.is_empty() {
                    0.
                } else {
                    self.envelopes[i]
                        .as_ref()
                        .map_or(1., |e| envelope.distance(&e.summary))
                };
                (i, targets.distance(*c) + shape)
            })
            .collect();
        matches.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        matches
    }

    /// Populated clusters, with representatives and bounded examples for display or naming.
    pub fn cluster_summaries(&self) -> &[TimbreCluster] {
        &self.summaries
    }
    /// Copy summaries using per-sound family corrections for the naming request.
    /// Acoustic membership and representatives remain unchanged.
    pub fn cluster_summaries_with_icons(
        &self,
        mut family: impl FnMut(&TimbreSound) -> crate::SoundIcon,
    ) -> Vec<TimbreCluster> {
        let mut summaries = self.summaries.clone();
        for summary in &mut summaries {
            let mut counts = std::collections::BTreeMap::new();
            for (sound, cluster) in self.sounds.iter().zip(self.clusters()) {
                if *cluster == summary.id {
                    *counts.entry(family(sound)).or_insert(0) += 1;
                }
            }
            summary.icon_counts = counts.into_iter().collect();
        }
        summaries
    }
    /// Find a measured library sound by identity, never by its display name.
    ///
    /// Built-in entries represent default patches only. Callers must not use their labels for
    /// edited or factory patches that were not measured. Fonts are matched by id, bank and patch.
    pub fn cluster_for(&self, instrument_id: &str, preset: Option<PresetRef>) -> Option<usize> {
        self.sounds
            .iter()
            .position(|sound| sound.instrument_id == instrument_id && sound.preset == preset)
            .map(|index| self.projection.clusters[index])
    }
    /// Raw three-component PCA coordinates in source order.
    pub fn positions(&self) -> &[[f64; 3]] {
        &self.projection.positions
    }
    /// Zero-based group labels computed in standardized feature space.
    pub fn clusters(&self) -> &[usize] {
        &self.projection.clusters
    }
    /// Fraction of feature variance retained by the three displayed components.
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

fn cluster_summaries(sounds: &[TimbreSound], projection: &TimbreProjection) -> Vec<TimbreCluster> {
    let mut summaries = Vec::new();
    for id in 0..=projection.clusters.iter().copied().max().unwrap_or(0) {
        let mut members: Vec<_> = projection
            .clusters
            .iter()
            .enumerate()
            .filter_map(|(i, cluster)| (*cluster == id).then_some(i))
            .collect();
        let Some(&first) = members.first() else {
            continue;
        };
        let mut centroid = vec![0.; projection.standardized[first].len()];
        for &index in &members {
            for (mean, value) in centroid.iter_mut().zip(&projection.standardized[index]) {
                *mean += value / members.len() as f64;
            }
        }
        let distance = |index: usize| {
            projection.standardized[index]
                .iter()
                .zip(&centroid)
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f64>()
        };
        members.sort_by(|a, b| distance(*a).total_cmp(&distance(*b)).then(a.cmp(b)));
        summaries.push(TimbreCluster {
            id,
            sound_count: members.len(),
            icon_counts: {
                let mut counts = std::collections::BTreeMap::new();
                for &i in &members {
                    *counts
                        .entry(crate::SoundIcon::infer(&sounds[i].name))
                        .or_insert(0) += 1;
                }
                counts.into_iter().collect()
            },
            representative: sounds[members[0]].clone(),
            examples: members
                .iter()
                .take(8)
                .map(|&i| sounds[i].name.clone())
                .collect(),
        });
    }
    summaries
}

pub(super) fn render_reference(
    instrument: &mut dyn auris_core::Instrument,
    sample_rate: f64,
    pitch: u8,
    velocity: f32,
    control: &TimbreMapControl,
) -> Result<AudioBuffer, SessionError> {
    let frames = (sample_rate * SECONDS).round() as usize;
    let off = (sample_rate * HOLD).round() as usize;
    let mut audio = AudioBuffer::stereo(frames, sample_rate);
    let mut block = AudioBuffer::stereo(BLOCK, sample_rate);
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
                sample_rate,
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

    Ok(audio)
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
    fn correction_keys_follow_font_paths_and_exact_presets_across_documents() {
        let mut first = Session::new(SessionOptions::headless()).unwrap();
        let mut second = Session::new(SessionOptions::headless()).unwrap();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Guitar.sf2");
        let other = root.path().join("Other.sf2");
        let a = first
            .project
            .add_soundfont("Same", auris_core::AssetPath::external(&path), 0);
        second
            .project
            .add_soundfont("Same", auris_core::AssetPath::external(&other), 0);
        let b = second
            .project
            .add_soundfont("Same", auris_core::AssetPath::external(&path), 0);
        assert_ne!(a, b);
        let preset = PresetRef {
            font: a,
            bank: 0,
            patch: 24,
        };
        let key = first.library_sound_key("sampler", Some(preset));
        assert_eq!(
            key,
            second.library_sound_key("sampler", Some(PresetRef { font: b, ..preset }))
        );
        assert_ne!(
            key,
            first.library_sound_key(
                "sampler",
                Some(PresetRef {
                    patch: 25,
                    ..preset
                })
            )
        );
        assert_ne!(
            key,
            first.library_sound_key("sampler", Some(PresetRef { bank: 1, ..preset }))
        );
    }

    #[test]
    fn mixed_cluster_icons_use_all_members_rather_than_only_the_representative() {
        let sounds: Vec<_> = ["Guitar", "Piano", "Piano", "Flute"]
            .into_iter()
            .map(|name| TimbreSound {
                name: name.into(),
                instrument_id: name.into(),
                preset: None,
            })
            .collect();
        let projection = TimbreProjection {
            standardized: vec![vec![0.]; 4],
            positions: vec![[0.; 3]; 4],
            clusters: vec![0; 4],
            explained_variance: 0.,
        };
        let summaries = cluster_summaries(&sounds, &projection);
        assert_eq!(summaries[0].representative.name, "Guitar");
        assert_eq!(summaries[0].icon(), crate::SoundIcon::Sound);
        assert_eq!(
            summaries[0]
                .icon_counts
                .iter()
                .map(|(_, count)| count)
                .sum::<usize>(),
            4
        );
    }

    #[test]
    fn summaries_use_full_features_and_identity_not_names_or_display_coordinates() {
        let sounds: Vec<_> = (0..4)
            .map(|i| TimbreSound {
                name: "Shared name".into(),
                instrument_id: if i == 0 {
                    "builtin"
                } else {
                    auris_sampler::SAMPLER_ID
                }
                .into(),
                preset: (i != 0).then_some(PresetRef {
                    font: auris_core::SoundFontId(i as u64),
                    bank: 0,
                    patch: 4,
                }),
            })
            .collect();
        let projection = TimbreProjection {
            standardized: vec![vec![0., 0.], vec![0., 1.], vec![0., 9.], vec![10., 0.]],
            positions: vec![[0.; 3]; 4],
            clusters: vec![2, 2, 2, 5],
            explained_variance: 0.,
        };
        let summaries = cluster_summaries(&sounds, &projection);
        assert_eq!(
            summaries.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![2, 5]
        );
        assert_eq!(summaries[0].sound_count, 3);
        assert_eq!(summaries[0].representative, sounds[1]);
        let map = TimbreMap {
            sounds,
            characters: vec![],
            envelopes: vec![],
            skipped: vec![],
            projection,
            summaries,
            previews: vec![],
            sample_rate: 48_000.,
            registry: crate::default_registry(auris_sampler::SoundFontBank::shared()),
        };
        assert_eq!(map.cluster_for("builtin", None), Some(2));
        assert_eq!(
            map.cluster_for(auris_sampler::SAMPLER_ID, map.sounds[3].preset),
            Some(5)
        );
        assert_eq!(
            map.cluster_for(
                auris_sampler::SAMPLER_ID,
                Some(PresetRef {
                    bank: 128,
                    ..map.sounds[3].preset.unwrap()
                })
            ),
            None
        );
        assert_eq!(map.cluster_for(auris_sampler::SAMPLER_ID, None), None);
        assert_eq!(map.cluster_for("unmeasured", None), None);
        let corrected = map.cluster_summaries_with_icons(|_| crate::SoundIcon::Guitar);
        for summary in corrected {
            assert_eq!(
                summary.icon_counts,
                vec![(crate::SoundIcon::Guitar, summary.sound_count)]
            );
        }
        assert!(
            map.cluster_summaries()
                .iter()
                .all(|summary| summary.icon() == crate::SoundIcon::Sound)
        );
    }

    #[test]
    fn adopting_the_current_builtin_restores_its_measured_sound_in_one_undo_step() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let descriptors = session.param_descriptors(crate::DEFAULT_INSTRUMENT);
        let waveform = descriptors.iter().find(|d| d.key == "hardness").unwrap();
        let target = ParamTarget::Instrument {
            track,
            param: waveform.id,
        };
        session.set_param(target, 0.9);
        assert!(session.set_automation_point(target, Ticks::ZERO, 0.9));
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
    fn mapped_notes_preview_without_tracks_and_preserve_the_document() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let tracks: Vec<_> = session
            .project()
            .tracks
            .iter()
            .map(|track| track.id)
            .collect();
        for track in tracks {
            session.remove_track(track).unwrap();
        }
        session.forget_history();
        assert!(session.audition_track(None).is_none());
        let before = session.project().clone();
        let revision = session.revision();
        let map = session
            .timbre_map_job()
            .unwrap()
            .run(&TimbreMapControl::default())
            .unwrap();
        let index = map
            .sounds
            .iter()
            // A periodic oscillator makes zero crossings useful; the physical piano has
            // inharmonic partials whose crossings do not measure its fundamental pitch.
            .position(|s| s.instrument_id == "auris.synth.chiptune")
            .unwrap();
        let middle = session
            .timbre_preview_job(&map, index, 60)
            .unwrap()
            .run(&TimbreMapControl::default())
            .unwrap();
        assert!(Arc::ptr_eq(&middle, &map.previews[index]));
        let upper = session
            .timbre_preview_job(&map, index, 72)
            .unwrap()
            .run(&TimbreMapControl::default())
            .unwrap();
        let crossings = |audio: &AudioBuffer| {
            let start = (audio.sample_rate() * 0.1) as usize;
            let end = (audio.sample_rate() * 0.5) as usize;
            audio.channel(0)[start..end]
                .windows(2)
                .filter(|s| s[0] <= 0. && s[1] > 0.)
                .count() as f64
        };
        let ratio = crossings(&upper) / crossings(&middle);
        assert!((ratio - 2.).abs() < 0.04, "octave frequency ratio: {ratio}");
        assert!(upper.peak() > 0.01 && upper.peak() <= 0.801);
        assert_eq!(upper.frame_count(), middle.frame_count());
        session.play_output_preview(upper).unwrap();
        assert_eq!(session.project(), &before);
        assert_eq!(session.revision(), revision);
        assert!(!session.can_undo());
        let cancelled = TimbreMapControl::default();
        cancelled.cancel();
        assert!(
            session
                .timbre_preview_job(&map, index, 61)
                .unwrap()
                .run(&cancelled)
                .is_err()
        );
        assert!(session.timbre_preview_job(&map, usize::MAX, 60).is_err());
        assert!(session.timbre_preview_job(&map, index, 128).is_err());
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
        assert_eq!(map.envelopes().len(), map.sounds.len());
        assert!(
            map.envelopes()
                .iter()
                .all(|e| e.as_ref().is_some_and(|e| e.by_pitch.len() == 3))
        );
        assert!(
            map.envelope_positions()
                .iter()
                .flatten()
                .all(|v| (-1. ..=1.).contains(v))
        );
        // An envelope range alone ranks the slowest swell first.
        let swell = map.search_character(
            &TimbreTargets::default(),
            &EnvelopeTargets {
                attack_ms: Some(crate::Bounds {
                    min: Some(2000.),
                    max: None,
                }),
                ..Default::default()
            },
        );
        let slowest = (0..map.sounds.len())
            .max_by(|a, b| {
                let attack = |i: usize| map.envelopes()[i].as_ref().unwrap().summary.attack_ms;
                attack(*a).total_cmp(&attack(*b))
            })
            .unwrap();
        assert_eq!(swell[0].0, slowest);
        assert_eq!(
            map.cluster_summaries()
                .iter()
                .map(|c| c.sound_count)
                .sum::<usize>(),
            map.sounds.len()
        );
        for summary in map.cluster_summaries() {
            assert_eq!(
                map.cluster_for(
                    &summary.representative.instrument_id,
                    summary.representative.preset
                ),
                Some(summary.id)
            );
            assert_eq!(summary.examples[0], summary.representative.name);
            assert!(summary.examples.len() <= 8);
        }
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
        let index = map
            .sounds
            .iter()
            .position(|s| s.preset.is_some_and(|p| p.font == font))
            .unwrap();
        let preview = session
            .timbre_preview_job(&map, index, 67)
            .unwrap()
            .run(&TimbreMapControl::default())
            .unwrap();
        assert!(
            preview.peak() > 0.01,
            "the keyboard uses the retained SoundFont preset"
        );
    }
}
