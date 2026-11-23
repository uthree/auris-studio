//! Read-only library audition, acoustic indexing and explicit adoption of a discovered sound.

use super::Session;
use super::SoundEnvelope;
use crate::{Edit, SessionError};
use crate::{EnvelopeTargets, SoundAxes, TimbreCharacter, TimbreTargets};
use auris_core::plugin::{NoteEvent, PluginState, PrepareContext, ProcessContext};
use auris_core::project::PresetRef;
use auris_core::{AudioBuffer, ParamTarget, PluginRegistry, TrackId};
use auris_dsp::timbre::{TimbreGroups, group_timbres_refined};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

const BLOCK: usize = 512;
const HOLD: f64 = 0.6;
const SECONDS: f64 = 1.0;

/// Stable library identity; names are presentation only and never enter feature extraction.
#[derive(Clone, Debug, PartialEq)]
pub struct TimbreSound {
    /// Display name, including the library name for SoundFont and plugin presets.
    pub name: String,
    /// What plays it.
    pub source: TimbreSource,
}

/// Which library a saved patch came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PatchOrigin {
    /// A sound the user saved.
    User,
    /// A sound shipped with the application.
    Factory,
}

/// Where a library sound comes from, exactly enough to play it again.
#[derive(Clone, Debug, PartialEq)]
pub enum TimbreSource {
    /// A built-in instrument at its default patch.
    Builtin(String),
    /// A loaded SoundFont preset.
    Font(PresetRef),
    /// A CLAP instrument, at its default state or with one advertised preset.
    Clap {
        /// The plugin file or bundle.
        file: std::path::PathBuf,
        /// The CLAP plugin id, without the Auris prefix.
        id: String,
        /// The preset to load, if any.
        preset: Option<auris_clap::ClapPreset>,
    },
    /// A VST3 instrument, at its default state or with one program or preset file.
    Vst3 {
        /// The plugin bundle.
        file: std::path::PathBuf,
        /// The VST3 class id, without the Auris prefix.
        id: String,
        /// The program or preset file to load, if any.
        preset: Option<auris_vst3::Vst3Preset>,
    },
    /// A saved instrument with its insert chain.
    Patch {
        /// The library it is saved in.
        origin: PatchOrigin,
        /// The patch itself.
        sound: Arc<crate::SoundPreset>,
    },
}

impl TimbreSound {
    /// A built-in instrument at its default patch.
    pub fn builtin(name: impl Into<String>, instrument_id: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            source: TimbreSource::Builtin(instrument_id.into()),
        }
    }

    /// A SoundFont preset.
    pub fn font(name: impl Into<String>, preset: PresetRef) -> Self {
        Self {
            name: name.into(),
            source: TimbreSource::Font(preset),
        }
    }

    /// The instrument identifier a track playing this sound would hold.
    pub fn instrument_id(&self) -> String {
        match &self.source {
            TimbreSource::Builtin(id) => id.clone(),
            TimbreSource::Font(_) => auris_sampler::SAMPLER_ID.into(),
            TimbreSource::Clap { id, .. } => format!("{}{id}", auris_clap::ID_PREFIX),
            TimbreSource::Vst3 { id, .. } => format!("{}{id}", auris_vst3::ID_PREFIX),
            TimbreSource::Patch { sound, .. } => sound.instrument_id.clone(),
        }
    }

    /// The SoundFont preset, where this is one.
    pub fn preset(&self) -> Option<PresetRef> {
        match &self.source {
            TimbreSource::Font(preset) => Some(*preset),
            _ => None,
        }
    }

    /// Whether playing this sound loads third-party code or a file, and so happens in the
    /// plugin worker process rather than in this one.
    pub fn plays_in_worker(&self) -> bool {
        match &self.source {
            TimbreSource::Builtin(_) | TimbreSource::Font(_) => false,
            TimbreSource::Clap { .. } | TimbreSource::Vst3 { .. } => true,
            TimbreSource::Patch { sound, .. } => {
                !super::sound_search::native::patch_is_builtin(sound)
            }
        }
    }
}

/// One populated acoustic cluster, summarized using the full standardized feature space.
#[derive(Clone, Debug, PartialEq)]
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

/// Shareable cancellation and progress for a library scan.
#[derive(Clone, Default)]
pub struct TimbreMapControl {
    cancelled: Arc<AtomicBool>,
    completed: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
    worker_sounds: Arc<AtomicUsize>,
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
    /// Number of sounds this scan measures; zero while the library is still being listed.
    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }
    /// How many of those play in the plugin worker process — CLAP and VST3 instruments and
    /// saved sounds that need a file. They take seconds each the first time, and are kept on
    /// disk afterwards; a frontend should say so while they are being measured.
    pub fn worker_sounds(&self) -> usize {
        self.worker_sounds.load(Ordering::Relaxed)
    }
    pub(super) fn report(&self, completed: usize, total: usize) {
        self.completed.store(completed, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
    }
    pub(super) fn set_worker_sounds(&self, count: usize) {
        self.worker_sounds.store(count, Ordering::Relaxed);
    }
    /// The cancellation flag, for work that polls one itself.
    pub(super) fn flag(&self) -> &AtomicBool {
        &self.cancelled
    }
    /// Whether `other` is a clone of this control rather than another run's.
    pub(super) fn same_run(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.cancelled, &other.cancelled)
    }
    pub(super) fn check(&self) -> Result<(), SessionError> {
        if self.cancelled.load(Ordering::Relaxed) {
            Err(failure("cancelled"))
        } else {
            Ok(())
        }
    }
}

/// Immutable library snapshot. Move to an ordinary background thread before calling `run`.
///
/// It is the same snapshot the agent's `search_instruments` reads, and the map is drawn from
/// the same acoustic index: whichever of the two measures the library first, the other reuses.
pub struct TimbreMapJob {
    library: super::SoundLibraryJob,
    refresh: bool,
}

/// A reusable acoustic index. Nothing here is a project edit.
pub struct TimbreMap {
    /// Audible sources in the same order as coordinates and group labels.
    pub sounds: Vec<TimbreSound>,
    /// Sources that could not be measured, each with the reason.
    pub skipped: Vec<String>,
    /// Whether measurable sounds were left out to keep the index within its bounds.
    pub limited: bool,
    /// How many mapped sounds play in the plugin worker process.
    pub worker_sounds: usize,
    groups: TimbreGroups,
    characters: Vec<TimbreCharacter>,
    envelopes: Vec<Option<SoundEnvelope>>,
    summaries: Vec<TimbreCluster>,
    sample_rate: f64,
    registry: Arc<PluginRegistry>,
}

/// A single mapped note rendered from the scan's immutable library snapshot on a worker.
pub struct TimbrePreviewJob {
    registry: Arc<PluginRegistry>,
    sound: TimbreSound,
    pitch: u8,
    sample_rate: f64,
}

impl TimbrePreviewJob {
    /// Renders the instrument's actual note, with the map's hold, release and level matching.
    /// A sound that needs a file or third-party code renders in the plugin worker process.
    pub fn run(self, control: &TimbreMapControl) -> Result<Arc<AudioBuffer>, SessionError> {
        control.check()?;
        let mut audio = match super::sound_search::worker_sound(&self.sound.source) {
            Some(sound) => {
                let channels = crate::plugin_worker::preview_isolated(
                    sound,
                    self.sample_rate,
                    self.pitch,
                    control.flag(),
                )?;
                super::sound_search::native::audio_from_channels(channels, self.sample_rate)
                    .map_err(failure)?
            }
            None => {
                let mut instrument = super::sound_search::in_process_instrument(
                    &self.sound.source,
                    &self.registry,
                    &PrepareContext::new(self.sample_rate, BLOCK, 2),
                )
                .map_err(failure)?;
                render_reference(
                    instrument.as_mut(),
                    self.sample_rate,
                    self.pitch,
                    0.85,
                    control,
                )?
            }
        };
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
    /// Every pitch triggers a fresh instrument rather than pitch-shifting a reference.
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
        })
    }

    /// Persistent identity for a library correction. Font IDs are document-local, so fonts use
    /// their resolved path and exact bank/patch instead; plugins use their file, id and preset;
    /// a saved sound its library and name. This performs no filesystem access.
    pub fn library_sound_key(&self, sound: &TimbreSound) -> Option<String> {
        match &sound.source {
            TimbreSource::Builtin(id) => Some(format!("instrument:{id}")),
            TimbreSource::Font(preset) => self.library_font_key(*preset),
            TimbreSource::Clap { file, id, preset } => serde_json::to_string(&(
                "clap",
                file,
                id,
                preset.as_ref().map(|p| (&p.name, &p.location, &p.load_key)),
            ))
            .ok(),
            TimbreSource::Vst3 { file, id, preset } => serde_json::to_string(&(
                "vst3",
                file,
                id,
                preset.as_ref().map(|p| format!("{p:?}")),
            ))
            .ok(),
            TimbreSource::Patch { origin, sound } => {
                serde_json::to_string(&("patch", format!("{origin:?}"), &sound.name)).ok()
            }
        }
    }

    /// [`Self::library_sound_key`] for a built-in instrument or a SoundFont preset, which is
    /// all a library row outside the map can name.
    pub fn library_instrument_key(
        &self,
        instrument_id: &str,
        preset: Option<PresetRef>,
    ) -> Option<String> {
        match preset {
            None => Some(format!("instrument:{instrument_id}")),
            Some(preset) => self.library_font_key(preset),
        }
    }

    fn library_font_key(&self, preset: PresetRef) -> Option<String> {
        let font = self.soundfonts().find(|font| font.id == preset.font)?;
        let path = font.path.resolve(self.project_folder())?;
        serde_json::to_string(&(auris_sampler::SAMPLER_ID, path, preset.bank, preset.patch)).ok()
    }

    /// Snapshots every sound the library offers — built-in instruments, loaded SoundFont
    /// presets, saved user and factory sounds, and the CLAP and VST3 instruments under the
    /// configured plugin paths — without loading or walking anything yet.
    ///
    /// Each source is measured at MIDI 48, 60 and 72, at velocities 0.45 and 0.85, with a 600 ms
    /// hold and 400 ms release, and its envelope at C2, C4 and C6. Neither live tracks nor their
    /// mixer state enter the analysis.
    pub fn timbre_map_job(&self, plugin_paths: &[std::path::PathBuf]) -> TimbreMapJob {
        TimbreMapJob {
            library: self.sound_library_job(plugin_paths),
            refresh: false,
        }
    }

    /// The timbre cluster that each track's sound was measured into, in track order.
    ///
    /// Only what the map actually measured is answered for: a melodic track playing a
    /// SoundFont preset, matched by font, bank and patch, or a built-in instrument still on the
    /// default patch the reference recording used. An edited built-in, a hosted plugin, a drum
    /// kit, a singer, audio and buses are left out — a colour claims a resemblance, and none of
    /// those was listened to. The cluster ids are the map's own and mean nothing outside it.
    pub fn timbre_track_clusters(&self, map: &TimbreMap) -> Vec<(TrackId, usize)> {
        self.project
            .tracks
            .iter()
            .filter_map(|track| {
                let auris_core::project::TrackKind::Instrument(inner) = &track.kind else {
                    return None;
                };
                if inner.file.is_some() {
                    return None;
                }
                let cluster = if inner.instrument_id == auris_sampler::SAMPLER_ID {
                    let preset = auris_sampler::stored_preset(&inner.instrument_state)?;
                    map.cluster_for(&inner.instrument_id, Some(preset))
                } else if inner.instrument_state.is_empty() {
                    map.cluster_for(&inner.instrument_id, None)
                } else {
                    None
                }?;
                Some((track.id, cluster))
            })
            .collect()
    }

    /// Adopts a mapped sound on a track as one undoable instrument or preset edit.
    /// A built-in sound restores its measured default patch and clears instrument automation,
    /// even when the track already uses that instrument. Mixer settings remain in place.
    pub fn use_timbre_sound(
        &mut self,
        track: TrackId,
        sound: &TimbreSound,
    ) -> Result<(), SessionError> {
        match &sound.source {
            TimbreSource::Font(preset) => self.set_track_preset(track, *preset),
            TimbreSource::Clap { .. } | TimbreSource::Vst3 { .. } | TimbreSource::Patch { .. } => {
                self.adopt_library_source(track, &sound.name, &sound.source)
                    .map_err(failure)
            }
            TimbreSource::Builtin(instrument_id) => {
                self.set_track_instrument(track, instrument_id)?;
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
}

impl TimbreMapJob {
    /// Leaves installed CLAP and VST3 plugins out of the map, for tests that must not start
    /// the plugin worker process.
    pub fn without_plugins(self) -> Self {
        Self {
            library: self.library.without_plugins(),
            ..self
        }
    }

    /// Lists the library again and measures what changed, rather than reusing the index a
    /// previous scan or an agent's search left behind. Plugins installed or updated since are
    /// found only this way; sounds whose files are unchanged come back from the disk cache.
    pub fn refreshed(self) -> Self {
        Self {
            refresh: true,
            ..self
        }
    }

    /// Lists the library, measures it (or reuses a finished measurement of the same library),
    /// and builds feature-space clusters and the map's coordinates.
    pub fn run(self, control: &TimbreMapControl) -> Result<TimbreMap, SessionError> {
        let sample_rate = self.library.sample_rate();
        let index = self
            .library
            .map_index(control, self.refresh)
            .map_err(|e| control.check().err().unwrap_or_else(|| failure(e)))?;
        control.check()?;
        if index.sounds.is_empty() {
            return Err(failure("no library sound could be measured"));
        }
        let initial = (index.sounds.len() as f64).sqrt().round() as usize;
        let groups = group_timbres_refined(&index.rows, initial.clamp(1, 12)).map_err(failure)?;
        let summaries = cluster_summaries(&index.sounds, &groups);
        Ok(TimbreMap {
            sounds: index.sounds,
            skipped: index.skipped,
            limited: index.limited,
            worker_sounds: index.worker_sounds,
            groups,
            characters: index.characters,
            envelopes: index.envelopes,
            summaries,
            sample_rate,
            registry: index.registry,
        })
    }
}

impl TimbreMap {
    /// Fixed-scale reference-note descriptors, in the order of [`sounds`](Self::sounds).
    pub fn characters(&self) -> &[TimbreCharacter] {
        &self.characters
    }

    /// Each mapped sound's measured envelope, in the order of [`sounds`](Self::sounds); `None`
    /// where no measured pitch sounded.
    pub fn envelopes(&self) -> &[Option<SoundEnvelope>] {
        &self.envelopes
    }

    /// Each sound's place on the map's axes, in the order of [`sounds`](Self::sounds).
    pub fn axes(&self) -> Vec<SoundAxes> {
        self.characters
            .iter()
            .zip(&self.envelopes)
            .map(|(c, e)| SoundAxes::new(*c, e.as_ref().map(|e| &e.summary)))
            .collect()
    }

    /// The map's coordinates, each −1 to +1: brightness, attack speed and held level.
    /// They have fixed meanings, so adding a library does not move the sounds already there.
    pub fn positions(&self) -> Vec<[f64; 3]> {
        self.axes().into_iter().map(SoundAxes::position).collect()
    }

    /// Rank measured sounds by targets on the map's axes and by envelope ranges — the same
    /// distance the agent's `search_instruments` uses; names can be filtered separately.
    pub fn search_character(
        &self,
        targets: &TimbreTargets,
        envelope: &EnvelopeTargets,
    ) -> Vec<(usize, f64)> {
        let mut matches: Vec<_> = self
            .axes()
            .into_iter()
            .enumerate()
            .map(|(i, axes)| {
                let shape = if envelope.is_empty() {
                    0.
                } else {
                    self.envelopes[i]
                        .as_ref()
                        .map_or(1., |e| envelope.distance(&e.summary))
                };
                (i, targets.distance(axes) + shape)
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
            .position(|sound| match (&sound.source, preset) {
                (TimbreSource::Builtin(id), None) => id == instrument_id,
                (TimbreSource::Font(mapped), Some(preset)) => {
                    *mapped == preset && instrument_id == auris_sampler::SAMPLER_ID
                }
                _ => false,
            })
            .map(|index| self.groups.clusters[index])
    }
    /// Find a measured sound by its exact source — a plugin preset or a saved sound as much as
    /// a built-in or a SoundFont preset.
    pub fn cluster_for_source(&self, source: &TimbreSource) -> Option<usize> {
        self.sounds
            .iter()
            .position(|sound| sound.source == *source)
            .map(|index| self.groups.clusters[index])
    }
    /// Zero-based group labels computed in standardized feature space.
    pub fn clusters(&self) -> &[usize] {
        &self.groups.clusters
    }
    /// Up to `limit` other sources ordered by full-feature Euclidean distance.
    pub fn nearest(&self, index: usize, limit: usize) -> Vec<(usize, f64)> {
        let Some(query) = self.groups.standardized.get(index) else {
            return Vec::new();
        };
        let mut neighbors: Vec<_> = self
            .groups
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

fn cluster_summaries(sounds: &[TimbreSound], groups: &TimbreGroups) -> Vec<TimbreCluster> {
    let mut summaries = Vec::new();
    for id in 0..=groups.clusters.iter().copied().max().unwrap_or(0) {
        let mut members: Vec<_> = groups
            .clusters
            .iter()
            .enumerate()
            .filter_map(|(i, cluster)| (*cluster == id).then_some(i))
            .collect();
        let Some(&first) = members.first() else {
            continue;
        };
        let mut centroid = vec![0.; groups.standardized[first].len()];
        for &index in &members {
            for (mean, value) in centroid.iter_mut().zip(&groups.standardized[index]) {
                *mean += value / members.len() as f64;
            }
        }
        let distance = |index: usize| {
            groups.standardized[index]
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
            events.push(NoteEvent::note_on(0, pitch, velocity));
        }
        if (start..start + count).contains(&off) {
            events.push(NoteEvent::NoteOff {
                frame: (off - start) as u32,
                key: pitch,
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
        let key = first.library_instrument_key("sampler", Some(preset));
        assert_eq!(
            key,
            second.library_instrument_key("sampler", Some(PresetRef { font: b, ..preset }))
        );
        assert_ne!(
            key,
            first.library_instrument_key(
                "sampler",
                Some(PresetRef {
                    patch: 25,
                    ..preset
                })
            )
        );
        assert_ne!(
            key,
            first.library_instrument_key("sampler", Some(PresetRef { bank: 1, ..preset }))
        );
    }

    #[test]
    fn mixed_cluster_icons_use_all_members_rather_than_only_the_representative() {
        let sounds: Vec<_> = ["Guitar", "Piano", "Piano", "Flute"]
            .into_iter()
            .map(|name| TimbreSound::builtin(name, name))
            .collect();
        let groups = TimbreGroups {
            standardized: vec![vec![0.]; 4],
            clusters: vec![0; 4],
        };
        let summaries = cluster_summaries(&sounds, &groups);
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
            .map(|i| {
                if i == 0 {
                    TimbreSound::builtin("Shared name", "builtin")
                } else {
                    TimbreSound::font(
                        "Shared name",
                        PresetRef {
                            font: auris_core::SoundFontId(i as u64),
                            bank: 0,
                            patch: 4,
                        },
                    )
                }
            })
            .collect();
        let groups = TimbreGroups {
            standardized: vec![vec![0., 0.], vec![0., 1.], vec![0., 9.], vec![10., 0.]],
            clusters: vec![2, 2, 2, 5],
        };
        let summaries = cluster_summaries(&sounds, &groups);
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
            limited: false,
            worker_sounds: 0,
            groups,
            summaries,
            sample_rate: 48_000.,
            registry: crate::default_registry(auris_sampler::SoundFontBank::shared()),
        };
        assert_eq!(map.cluster_for("builtin", None), Some(2));
        assert_eq!(
            map.cluster_for(auris_sampler::SAMPLER_ID, map.sounds[3].preset()),
            Some(5)
        );
        assert_eq!(
            map.cluster_for(
                auris_sampler::SAMPLER_ID,
                Some(PresetRef {
                    bank: 128,
                    ..map.sounds[3].preset().unwrap()
                })
            ),
            None
        );
        assert_eq!(map.cluster_for(auris_sampler::SAMPLER_ID, None), None);
        assert_eq!(map.cluster_for("unmeasured", None), None);
        let corrected = map.cluster_summaries_with_icons(|_| crate::SoundIcon::Guitar);
        track_clusters_follow_measured_sounds_only(&map);
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
        let sound = TimbreSound::builtin("Reference", crate::DEFAULT_INSTRUMENT);

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
            .timbre_map_job(&[])
            .without_plugins()
            .run(&TimbreMapControl::default())
            .unwrap();
        let index = map
            .sounds
            .iter()
            // A periodic oscillator makes zero crossings useful; the physical piano has
            // inharmonic partials whose crossings do not measure its fundamental pitch.
            .position(|s| s.source == TimbreSource::Builtin("auris.synth.chiptune".into()))
            .unwrap();
        let middle = session
            .timbre_preview_job(&map, index, 60)
            .unwrap()
            .run(&TimbreMapControl::default())
            .unwrap();
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
        let job = session.timbre_map_job(&[]).without_plugins();
        let map = job.run(&control).unwrap();
        assert_eq!(&before, session.project());
        assert_eq!(control.completed(), control.total());
        assert_eq!(
            control.worker_sounds(),
            0,
            "nothing built in plays in the worker"
        );
        assert!(
            map.sounds.iter().any(|s| matches!(
                s.source,
                TimbreSource::Patch {
                    origin: PatchOrigin::Factory,
                    ..
                }
            )),
            "factory sounds are on the map"
        );
        assert!(map.sounds.len() >= 2);
        assert_eq!(map.positions().len(), map.sounds.len());
        assert_eq!(map.envelopes().len(), map.sounds.len());
        assert!(
            map.envelopes()
                .iter()
                .all(|e| e.as_ref().is_some_and(|e| e.by_pitch.len() == 3))
        );
        assert!(
            map.positions()
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
                map.cluster_for_source(&summary.representative.source),
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
        let preview = session
            .timbre_preview_job(&map, 0, 60)
            .unwrap()
            .run(&TimbreMapControl::default())
            .unwrap();
        assert!(
            preview
                .channels()
                .iter()
                .flatten()
                .all(|v| v.is_finite() && v.abs() <= 0.801)
        );
        let cancelled = TimbreMapControl::default();
        cancelled.cancel();
        assert!(
            session
                .timbre_map_job(&[])
                .without_plugins()
                .run(&cancelled)
                .is_err()
        );
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
        let job = session.timbre_map_job(&[]).without_plugins();
        session.fonts.clear();
        let map = job.run(&TimbreMapControl::default()).unwrap();
        assert!(
            map.sounds
                .iter()
                .any(|s| s.preset().is_some_and(|p| p.font == font))
        );
        let index = map
            .sounds
            .iter()
            .position(|s| s.preset().is_some_and(|p| p.font == font))
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

    /// Called from the summaries test, which owns the only hand-built map: `builtin` is cluster
    /// 2 and the preset on font 3 is cluster 5.
    fn track_clusters_follow_measured_sounds_only(map: &TimbreMap) {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let measured = map.sounds[3].preset().unwrap();
        let project = &mut session.project;
        let sampler = |project: &mut auris_core::project::Project, name, preset| {
            let id = project.add_instrument_track(name, auris_sampler::SAMPLER_ID);
            let inner = project
                .track_mut(id)
                .and_then(|t| t.kind.as_instrument_mut())
                .unwrap();
            auris_sampler::store_preset(&mut inner.instrument_state, preset);
            id
        };
        let preset = sampler(project, "Preset", measured);
        let _other = sampler(
            project,
            "Other",
            PresetRef {
                patch: 9,
                ..measured
            },
        );
        let _unset = project.add_instrument_track("No preset", auris_sampler::SAMPLER_ID);
        let plain = project.add_instrument_track("Plain", "builtin");
        let edited = project.add_instrument_track("Edited", "builtin");
        project
            .track_mut(edited)
            .and_then(|t| t.kind.as_instrument_mut())
            .unwrap()
            .instrument_state
            .params
            .insert("cutoff".into(), 0.5);
        let _drums = project.add_drum_track("Drums", "builtin");
        let _audio = project.add_audio_track("Audio");
        assert_eq!(
            session.timbre_track_clusters(map),
            vec![(preset, 5), (plain, 2)],
            "unmeasured presets, edited patches and drum kits get no cluster"
        );
    }
}
