//! Synthesized-note measurement and explicit application of acoustic drum assignments.
//!
//! Requests are snapshots for a disposable process. A worker restores one independent instance,
//! feeds it the same note route the engine uses, and analyzes only its raw output. The live graph,
//! track effects, fader, authored labels and existing notes do not enter the classifier.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Instant;

use auris_core::plugin::{
    Instrument, NoteEvent, Parameterized, PluginState, PrepareContext, ProcessContext,
};
use auris_core::project::{DrumMap, DrumRole, TrackKind, notes_digest};
use auris_core::{AudioBuffer, SoundFontId, TrackId};
use auris_dsp::drum_analysis::{
    AcousticCharacter, DrumAcoustics, analyze_drum_audio, drum_role_fitness,
};
use serde::{Deserialize, Serialize};

use super::Session;
use crate::{Edit, SessionError};

const BLOCK: usize = 512;

/// Bounded measurement conditions, independent of musical labels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DrumScanOptions {
    /// MIDI addresses to trigger. Their numbers never enter acoustic classification.
    pub notes: Vec<u8>,
    /// Attack strengths in (0, 1], measured separately.
    pub velocities: Vec<f32>,
    /// Repeated triggers per strength to expose sample variation.
    pub repetitions: u8,
    /// Seconds recorded with the key held, revealing natural decay instead of a gated release.
    pub seconds_per_note: f64,
    /// Minimum worst-case acoustic fitness for automatic assignment, in (0, 1].
    pub minimum_fitness: f64,
    /// Cooperative wall-clock limit. The supervising process must enforce a hard timeout too.
    pub timeout_seconds: u32,
}

impl Default for DrumScanOptions {
    fn default() -> Self {
        Self {
            notes: (0..=127).collect(),
            velocities: vec![0.35, 0.7, 1.0],
            repetitions: 2,
            seconds_per_note: 2.0,
            minimum_fitness: 0.55,
            timeout_seconds: 120,
        }
    }
}

impl DrumScanOptions {
    /// Rejects invalid input and bounds CPU, memory and plugin invocations before spawning.
    pub fn validate(&self) -> Result<(), SessionError> {
        let valid = !self.notes.is_empty()
            && self.notes.len() <= 128
            && self.notes.iter().all(|n| *n <= 127)
            && self.notes.iter().copied().collect::<BTreeSet<_>>().len() == self.notes.len()
            && !self.velocities.is_empty()
            && self.velocities.len() <= 4
            && self
                .velocities
                .iter()
                .all(|v| v.is_finite() && *v > 0.0 && *v <= 1.0)
            && (1..=4).contains(&self.repetitions)
            && self.seconds_per_note.is_finite()
            && (0.25..=6.0).contains(&self.seconds_per_note)
            && self.minimum_fitness.is_finite()
            && self.minimum_fitness > 0.0
            && self.minimum_fitness <= 1.0
            && (1..=600).contains(&self.timeout_seconds)
            && self.notes.len() as f64
                * self.velocities.len() as f64
                * f64::from(self.repetitions)
                * self.seconds_per_note
                <= 4096.0;
        if valid {
            Ok(())
        } else {
            Err(failure("invalid or excessive drum scan options"))
        }
    }
}

/// A font the worker must load under the document's existing id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DrumProbeFont {
    /// Id referenced by the snapshotted preset.
    pub id: SoundFontId,
    /// Resolved absolute library path.
    pub path: PathBuf,
}

/// An exact source snapshot for a disposable measurement worker.
///
/// This is an IPC payload, not an untrusted plugin discovery request. It may contain opaque
/// preset data; keep it local and delete temporary payloads once the worker finishes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DrumProbeRequest {
    /// Track to which an accepted report belongs.
    pub track: TrackId,
    /// Registry or hosted instrument id, used only to instantiate the source.
    pub instrument_id: String,
    /// Current parameters and, for a hosted instrument, a successfully saved opaque state.
    pub state: PluginState,
    /// Hosted binary or bundle, if any.
    pub file: Option<PathBuf>,
    /// The selected SoundFont, if any.
    pub soundfont: Option<DrumProbeFont>,
    /// Render rate, matching the document.
    pub sample_rate: f64,
    /// Tempo at the captured playhead, used for tempo-synchronized instruments.
    pub bpm: f64,
    /// Measurement budget and triggering conditions.
    pub options: DrumScanOptions,
    /// Source signature used to reject stale results. Excludes musical drum assignments.
    pub source_fingerprint: String,
}

/// One recorded trigger, before aggregation across dynamics or repetitions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DrumProbeSample {
    /// Trigger strength.
    pub velocity: f32,
    /// Zero-based repetition at this strength.
    pub repetition: u8,
    /// Measurements computed only from synthesized PCM.
    pub acoustics: DrumAcoustics,
}

/// Acoustic evidence for a note address; labels and intended uses are absent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DrumVoiceAnalysis {
    /// Address passed to the instrument; not a classifier feature.
    pub note: u8,
    /// Individual measurements, including silent triggers.
    pub samples: Vec<DrumProbeSample>,
    /// Lowest fitness across all triggers, preventing one good velocity hiding bad ones.
    pub fitness: BTreeMap<DrumRole, f64>,
    /// Largest within-role fitness range across repeated triggers and velocities.
    pub fitness_variation: f64,
    /// Every trigger was silent.
    pub silent: bool,
}

/// A measured kit and a separate proposed musical assignment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DrumKitAnalysis {
    /// Track whose source was snapshotted.
    pub track: TrackId,
    /// Source signature for stale-result rejection.
    pub source_fingerprint: String,
    /// Rate used for synthesis and every spectral frequency measurement.
    pub sample_rate: f64,
    /// Constant tempo used by every trigger.
    pub bpm: f64,
    /// Measurement settings for reproducibility.
    pub options: DrumScanOptions,
    /// Acoustic evidence per note address.
    pub voices: Vec<DrumVoiceAnalysis>,
    /// Best eligible note per role; absent roles have no acceptable measured voice.
    pub proposed_map: DrumMap,
}

impl DrumKitAnalysis {
    /// Validates bounded report structure, finite measurements and every derived score.
    ///
    /// A restored JSON report is evidence only when its conditions and arithmetic agree. This
    /// does not authenticate the origin of PCM; the disposable worker is responsible for that.
    pub fn validate(&self) -> Result<(), SessionError> {
        self.options.validate()?;
        if !self.sample_rate.is_finite()
            || !(8_000.0..=192_000.0).contains(&self.sample_rate)
            || !self.bpm.is_finite()
            || !(1.0..=1000.0).contains(&self.bpm)
            || self.voices.len() != self.options.notes.len()
            || self.voices.iter().map(|v| v.note).collect::<BTreeSet<_>>()
                != self.options.notes.iter().copied().collect()
        {
            return Err(failure("the drum report has inconsistent scan conditions"));
        }
        for voice in &self.voices {
            if voice.samples.len()
                != self.options.velocities.len() * usize::from(self.options.repetitions)
            {
                return Err(failure("the drum report has an incomplete trigger set"));
            }
            for (i, sample) in voice.samples.iter().enumerate() {
                if sample.velocity
                    != self.options.velocities[i / usize::from(self.options.repetitions)]
                    || usize::from(sample.repetition) != i % usize::from(self.options.repetitions)
                {
                    return Err(failure(
                        "the drum report has inconsistent trigger conditions",
                    ));
                }
                validate_acoustics(
                    &sample.acoustics,
                    self.sample_rate,
                    self.options.seconds_per_note,
                )?;
            }
            let (fitness, variation, silent) = aggregate_samples(&voice.samples);
            if !same_scores(&fitness, &voice.fitness)
                || !voice.fitness_variation.is_finite()
                || (variation - voice.fitness_variation).abs() > 1e-9
                || silent != voice.silent
            {
                return Err(failure(
                    "the drum report's aggregate does not match its triggers",
                ));
            }
        }
        if propose_map(&self.voices, self.options.minimum_fitness) != self.proposed_map {
            return Err(failure(
                "the proposed map does not match its measured evidence",
            ));
        }
        Ok(())
    }
}

impl Session {
    /// Captures the actual selected source without touching notes or the live render graph.
    ///
    /// Hosted plugins must support state save; unavailable or unsnapshotable instruments fail
    /// explicitly. Execute the returned request in a disposable process with a hard deadline.
    pub fn drum_probe_request(
        &mut self,
        track: TrackId,
        options: &DrumScanOptions,
    ) -> Result<DrumProbeRequest, SessionError> {
        options.validate()?;
        let inner = self
            .project
            .track(track)
            .ok_or(SessionError::UnknownTrack(track.0))?
            .kind
            .as_instrument()
            .ok_or_else(|| failure("select an instrument track"))?
            .clone();
        let file = match &inner.file {
            Some(path) => Some(
                path.resolve(self.project_folder())
                    .ok_or_else(|| failure("the plugin path cannot be resolved"))?,
            ),
            None => None,
        };
        let mut state = if file.is_none() {
            inner.instrument_state.clone()
        } else if inner.instrument_id.starts_with(auris_vst3::ID_PREFIX) {
            self.vst3.drum_probe_state(track, &inner.instrument_state)?
        } else {
            self.hosted
                .drum_probe_state(track, &inner.instrument_state)?
        };
        if let Some(object) = state.extra.as_object_mut() {
            object.remove(DrumMap::STATE_KEY);
        }
        if state
            .extra
            .as_object()
            .is_some_and(|object| object.is_empty())
        {
            state.extra = serde_json::Value::Null;
        }
        let soundfont = if inner.instrument_id == auris_sampler::SAMPLER_ID {
            let preset = auris_sampler::stored_preset(&state)
                .ok_or_else(|| failure("select an explicit SoundFont preset before measuring"))?;
            if !self.fonts.contains(preset.font) {
                return Err(failure("the selected SoundFont is not loaded"));
            }
            let font = self
                .project
                .soundfonts
                .get(&preset.font)
                .ok_or(SessionError::UnknownSoundFont(preset.font.0))?;
            let path = font
                .path
                .resolve(self.project_folder())
                .ok_or_else(|| failure("the SoundFont path cannot be resolved"))?;
            Some(DrumProbeFont {
                id: preset.font,
                path,
            })
        } else {
            None
        };
        let mut request = DrumProbeRequest {
            track,
            instrument_id: inner.instrument_id,
            state,
            file,
            soundfont,
            sample_rate: self.project.sample_rate,
            bpm: self.project.tempo_map.bpm_at(self.playhead()),
            options: options.clone(),
            source_fingerprint: String::new(),
        };
        request.source_fingerprint = fingerprint(&request)?;
        Ok(request)
    }

    /// Measures a kit synchronously on fresh instances; intended for a disposable CLI worker.
    ///
    /// Frontends that own a window must use [`Self::drum_probe_request`] and supervise a worker
    /// process instead, since a native plugin can hang inside a single processing call.
    pub fn analyze_drum_kit(
        &mut self,
        track: TrackId,
        options: &DrumScanOptions,
    ) -> Result<DrumKitAnalysis, SessionError> {
        probe_drum_request(&self.drum_probe_request(track, options)?)
    }

    /// Stores a measured proposal, optionally remapping explicitly tagged generated notes.
    ///
    /// Measurements are never rewritten. Untagged notes keep their pitches. Missing roles are
    /// never guessed; remapping is refused if a generated voice needs an absent role.
    pub fn apply_drum_map(
        &mut self,
        report: &DrumKitAnalysis,
        remap_generated: bool,
    ) -> Result<bool, SessionError> {
        report.validate()?;
        let current = self.drum_probe_request(report.track, &report.options)?;
        if current.source_fingerprint != report.source_fingerprint
            || report.sample_rate != current.sample_rate
            || report.bpm != current.bpm
        {
            return Err(failure(
                "the source changed after measurement; scan it again",
            ));
        }
        let original = self
            .project
            .track(report.track)
            .unwrap()
            .kind
            .as_instrument()
            .unwrap();
        let mut next = original.clone();
        report.proposed_map.store(&mut next.instrument_state);
        if remap_generated {
            for clip in &mut next.clips {
                let Some(recipe) = &mut clip.recipe else {
                    continue;
                };
                if !recipe.preset.is_drums() {
                    continue;
                }
                let was_unedited =
                    recipe.text_digest == 0 || recipe.text_digest == notes_digest(&clip.notes);
                let roles: BTreeMap<String, DrumRole> = if recipe.drum_voices.is_empty() {
                    auris_compose::roles_of(recipe.preset)
                        .iter()
                        .filter_map(|role| Some((role.name().to_string(), role.drum_role()?)))
                        .collect()
                } else {
                    recipe
                        .drum_voices
                        .iter()
                        .map(|voice| (voice.name.clone(), voice.role))
                        .collect()
                };
                for note in &mut clip.notes {
                    if let Some(role) = roles.get(&note.drum_voice) {
                        note.pitch = *report.proposed_map.voices.get(role).ok_or_else(|| {
                            failure("a generated voice has no acceptable measured assignment")
                        })?;
                    }
                }
                auris_compose::apply_drum_map(recipe, &report.proposed_map);
                if was_unedited {
                    recipe.text_digest = notes_digest(&clip.notes);
                }
            }
        }
        if &next == original {
            return Ok(false);
        }
        self.record(Edit::ApplyDrumMap);
        self.project.track_mut(report.track).unwrap().kind = TrackKind::Instrument(next);
        self.invalidate_graph();
        Ok(true)
    }
}

/// Runs a snapshotted scan without using any live instance, labels or existing project notes.
///
/// External plugins should run in a disposable supervised process. The internal budget checks
/// between process calls do not interrupt a plugin that hangs inside one call.
pub fn probe_drum_request(request: &DrumProbeRequest) -> Result<DrumKitAnalysis, SessionError> {
    request.options.validate()?;
    if !request.sample_rate.is_finite()
        || !(8_000.0..=192_000.0).contains(&request.sample_rate)
        || !request.bpm.is_finite()
        || !(1.0..=1000.0).contains(&request.bpm)
    {
        return Err(failure("unsupported scan sample rate"));
    }
    if fingerprint(request)? != request.source_fingerprint {
        return Err(failure("the source snapshot changed before the scan"));
    }
    let prepare = PrepareContext::new(request.sample_rate, BLOCK, 2).with_max_block_events(4);
    let started = Instant::now();
    let report = match &request.file {
        None => {
            let fonts = auris_sampler::SoundFontBank::shared();
            if let Some(font) = &request.soundfont {
                fonts.insert(font.id, auris_io::load_soundfont(&font.path)?);
            }
            let registry = crate::registry::default_registry(fonts);
            let mut instrument = registry
                .create_instrument(&request.instrument_id)
                .map_err(|_| SessionError::UnknownPlugin(request.instrument_id.clone()))?;
            instrument.load_state(&request.state);
            instrument.prepare(&prepare);
            scan(instrument.as_mut(), request, started, &mut || Ok(()))
        }
        Some(file) if request.instrument_id.starts_with(auris_vst3::ID_PREFIX) => {
            let id = request
                .instrument_id
                .strip_prefix(auris_vst3::ID_PREFIX)
                .unwrap();
            let plugin = auris_vst3::Vst3Plugin::load(file, id, &prepare)?;
            let bytes = request
                .state
                .hosted_bytes()
                .ok_or_else(|| failure("the VST3 snapshot has no opaque state"))?;
            plugin.load_state(&bytes)?;
            for descriptor in plugin.parameters() {
                if let Some(value) = request.state.params.get(descriptor.key.as_ref()) {
                    plugin.set_param(descriptor.id, *value)?;
                }
            }
            let mut instrument = plugin.instrument()?;
            instrument.prepare(&prepare);
            let result = scan(&mut instrument, request, started, &mut || Ok(()));
            if instrument.processing_failed() {
                Err(failure(
                    "the VST3 instrument failed while rendering; no acoustic result was accepted",
                ))
            } else {
                result
            }
            // The instrument drops before its main-thread owner.
        }
        Some(file) => {
            // SAFETY: this is the selected instrument's explicit local worker request.
            let library = unsafe { auris_clap::ClapLibrary::load(file) }?;
            scan_clap_library(&library, request, &prepare, started)
        }
    }?;
    if fingerprint(request)? != request.source_fingerprint {
        return Err(failure("the instrument file changed during measurement"));
    }
    Ok(report)
}

fn scan_clap_library(
    library: &auris_clap::ClapLibrary,
    request: &DrumProbeRequest,
    prepare: &PrepareContext,
    started: Instant,
) -> Result<DrumKitAnalysis, SessionError> {
    let mut plugin = library.instantiate(
        request
            .instrument_id
            .strip_prefix("clap:")
            .unwrap_or(&request.instrument_id),
    )?;
    let bytes = request
        .state
        .hosted_bytes()
        .ok_or_else(|| failure("the CLAP snapshot has no opaque state"))?;
    plugin.load_state(&bytes)?;
    for attempt in 0..3 {
        if started.elapsed().as_secs_f64() > f64::from(request.options.timeout_seconds) {
            return Err(failure("the drum scan exceeded its time budget"));
        }
        service_clap_requests(&mut plugin, false)?;
        if !plugin.refresh_parameters() {
            return Err(failure(
                "the CLAP instrument was active while restoring its parameter layout",
            ));
        }
        // An opaque restore usually includes these exact parameter values already. Replaying all
        // values can retrigger a preset or mode switch after activation. Compare against the
        // plugin itself, and retain every difference needed to reproduce the snapshotted state.
        let mut remaining = PluginState::empty();
        for descriptor in plugin.parameters().to_vec() {
            let Some(&wanted) = request.state.params.get(descriptor.key.as_ref()) else {
                continue;
            };
            let current = plugin.value(descriptor.id).ok_or_else(|| {
                failure("the CLAP instrument could not verify a restored parameter")
            })?;
            if current != wanted {
                remaining.params.insert(descriptor.key.to_string(), wanted);
            }
        }
        if plugin.note_language().is_none() {
            return Err(failure(
                "the CLAP instrument has no note input route supported by this host",
            ));
        }
        let mut instrument = plugin.activate_instrument(prepare)?;
        instrument.load_state(&remaining);
        instrument.prepare(prepare);
        instrument.reset();
        // Some plugins finish restoring their preset on the first audio block, then ask for a
        // fresh layout. No PCM evidence exists yet: return the rendering half and reactivate with
        // the refreshed descriptors, with a strict limit on this startup negotiation. Once it
        // settles, do not reset again or restart after even one measured trigger.
        let mut startup_restart = None;
        let startup = settle_previous_hit(&mut instrument, request, started, &mut || {
            let restart = service_clap_requests(&mut plugin, false)?;
            if restart.is_some() {
                startup_restart = restart;
                Err(failure("the CLAP instrument needs startup reconfiguration"))
            } else {
                Ok(())
            }
        });
        if let Some(reason) = startup_restart {
            plugin.deactivate_instrument(instrument);
            if attempt == 2 {
                return Err(failure(format!(
                    "the CLAP instrument did not settle after three startup activations ({reason})"
                )));
            }
            continue;
        }
        let result = startup.and_then(|()| {
            scan_prepared(&mut instrument, request, started, &mut || {
                service_clap_requests(&mut plugin, true).map(|_| ())
            })
        });
        let result = if instrument.processing_failed() {
            Err(failure(
                "the CLAP instrument failed while rendering; no acoustic result was accepted",
            ))
        } else {
            result
        };
        plugin.deactivate_instrument(instrument);
        return result;
    }
    unreachable!("each final startup attempt either scans or returns an error")
}

fn service_clap_requests(
    plugin: &mut auris_clap::ClapPlugin,
    measuring: bool,
) -> Result<Option<&'static str>, SessionError> {
    plugin.tick_timers();
    let mut restart = None;
    for _ in 0..32 {
        let mut pending = plugin.take_requests();
        // CLAP INFO changes names/modules/display flags and is explicitly allowed while active.
        // This worker neither displays nor classifies those labels. ALL invalidates the audio
        // parameter contract and must still be rejected after measurement starts.
        pending.restart = pending.restart_requested || pending.parameter_rescan;
        if pending.restart {
            restart = Some(clap_restart_reason(pending));
        }
        if !clap_callback_needed(pending, measuring)? {
            return Ok(restart);
        }
        plugin.run_callback();
    }
    Err(failure(
        "the CLAP instrument did not settle its main-thread callbacks",
    ))
}

fn clap_callback_needed(
    pending: auris_clap::PendingRequests,
    measuring: bool,
) -> Result<bool, SessionError> {
    // A state restore may legitimately request a restart while already inactive. Its first
    // activation will use the restored ports and refreshed parameters. During a scan the same
    // request would invalidate measurements already collected, so it remains an error.
    if measuring && (pending.restart_requested || pending.parameter_rescan) {
        return Err(failure(format!(
            "the CLAP instrument changed during measurement ({}); finish loading its preset and try again",
            clap_restart_reason(pending)
        )));
    }
    Ok(pending.callback)
}

fn clap_restart_reason(pending: auris_clap::PendingRequests) -> &'static str {
    match (pending.restart_requested, pending.parameter_rescan) {
        (true, true) => "processing restart and parameter descriptor rescan",
        (false, true) => "parameter descriptor rescan",
        _ => "processing restart",
    }
}

fn scan(
    instrument: &mut dyn Instrument,
    request: &DrumProbeRequest,
    started: Instant,
    service: &mut dyn FnMut() -> Result<(), SessionError>,
) -> Result<DrumKitAnalysis, SessionError> {
    instrument.reset();
    scan_prepared(instrument, request, started, service)
}

fn scan_prepared(
    instrument: &mut dyn Instrument,
    request: &DrumProbeRequest,
    started: Instant,
    service: &mut dyn FnMut() -> Result<(), SessionError>,
) -> Result<DrumKitAnalysis, SessionError> {
    let mut voices = Vec::new();
    for &note in &request.options.notes {
        let mut samples = Vec::new();
        for &velocity in &request.options.velocities {
            for repetition in 0..request.options.repetitions {
                let audio = render_trigger(instrument, request, note, velocity, started, service)?;
                samples.push(DrumProbeSample {
                    velocity,
                    repetition,
                    acoustics: analyze_drum_audio(&audio).map_err(failure)?,
                });
            }
        }
        let (fitness, variation, silent) = aggregate_samples(&samples);
        voices.push(DrumVoiceAnalysis {
            note,
            samples,
            fitness,
            fitness_variation: variation,
            silent,
        });
    }
    let proposed_map = propose_map(&voices, request.options.minimum_fitness);
    Ok(DrumKitAnalysis {
        track: request.track,
        source_fingerprint: request.source_fingerprint.clone(),
        sample_rate: request.sample_rate,
        bpm: request.bpm,
        options: request.options.clone(),
        voices,
        proposed_map,
    })
}

fn propose_map(voices: &[DrumVoiceAnalysis], minimum_fitness: f64) -> DrumMap {
    let mut proposed_map = DrumMap::default();
    for role in DrumRole::ALL {
        if let Some(voice) = voices
            .iter()
            .filter(|v| {
                !v.silent
                    && v.note <= 127
                    && v.fitness.get(&role).is_some_and(|score| {
                        score.is_finite() && *score >= minimum_fitness && *score <= 1.0
                    })
            })
            .max_by(|a, b| a.fitness[&role].total_cmp(&b.fitness[&role]))
        {
            proposed_map.voices.insert(role, voice.note);
        }
    }
    proposed_map
}

fn aggregate_samples(samples: &[DrumProbeSample]) -> (BTreeMap<DrumRole, f64>, f64, bool) {
    let mut fitness = BTreeMap::new();
    let mut variation = 0.0f64;
    for role in DrumRole::ALL {
        let lo = samples
            .iter()
            .map(|s| s.acoustics.fitness.get(&role).copied().unwrap_or(0.0))
            .fold(1.0f64, f64::min);
        let hi = samples
            .iter()
            .map(|s| s.acoustics.fitness.get(&role).copied().unwrap_or(0.0))
            .fold(0.0f64, f64::max);
        variation = variation.max(hi - lo);
        fitness.insert(role, lo);
    }
    (
        fitness,
        variation,
        samples
            .iter()
            .all(|s| s.acoustics.character == AcousticCharacter::Silent),
    )
}

fn validate_acoustics(audio: &DrumAcoustics, rate: f64, seconds: f64) -> Result<(), SessionError> {
    let unit = |v: f64| v.is_finite() && (0.0..=1.0 + 1e-9).contains(&v);
    let time = |v: f64| v.is_finite() && (0.0..=seconds + 1.0 / rate).contains(&v);
    let valid = audio.peak.is_finite()
        && audio.peak >= 0.0
        && audio.rms.is_finite()
        && audio.rms >= 0.0
        && audio.rms <= audio.peak + 1e-9
        && time(audio.onset_seconds)
        && time(audio.energy_duration_seconds)
        && unit(audio.sustained_energy)
        && audio.pitch_fall_semitones.is_finite()
        && (-96.0..=96.0).contains(&audio.pitch_fall_semitones)
        && [&audio.spectrum, &audio.attack, &audio.body, &audio.tail]
            .iter()
            .all(|s| {
                unit(s.low)
                    && unit(s.body)
                    && unit(s.high)
                    && s.low + s.body + s.high <= 1.0 + 1e-9
                    && unit(s.flatness)
                    && unit(s.concentration)
                    && s.centroid_hz.is_finite()
                    && (0.0..=rate / 2.0).contains(&s.centroid_hz)
                    && s.low_peak_hz.is_finite()
                    && (0.0..=800.0).contains(&s.low_peak_hz)
            })
        && same_scores(&audio.fitness, &drum_role_fitness(audio));
    if !valid {
        return Err(failure(
            "the drum report contains invalid measurements or fitness",
        ));
    }
    let expected = if audio.peak < 1e-7 {
        AcousticCharacter::Silent
    } else if audio.spectrum.concentration > 0.55 {
        AcousticCharacter::Tonal
    } else if audio.spectrum.concentration < 0.08 {
        AcousticCharacter::Noisy
    } else {
        AcousticCharacter::Mixed
    };
    if expected != audio.character {
        return Err(failure(
            "the acoustic character disagrees with its measured spectrum",
        ));
    }
    Ok(())
}

fn same_scores(a: &BTreeMap<DrumRole, f64>, b: &BTreeMap<DrumRole, f64>) -> bool {
    a.len() == b.len()
        && a.iter().all(|(role, score)| {
            score.is_finite()
                && (0.0..=1.0).contains(score)
                && b.get(role)
                    .is_some_and(|other| other.is_finite() && (score - other).abs() < 1e-9)
        })
}

fn render_trigger(
    instrument: &mut dyn Instrument,
    request: &DrumProbeRequest,
    pitch: u8,
    velocity: f32,
    started: Instant,
    service: &mut dyn FnMut() -> Result<(), SessionError>,
) -> Result<AudioBuffer, SessionError> {
    let frames = (request.sample_rate * request.options.seconds_per_note).round() as usize;
    let mut audio = AudioBuffer::stereo(frames, request.sample_rate);
    let mut block = AudioBuffer::stereo(BLOCK, request.sample_rate);
    settle_previous_hit(instrument, request, started, service)?;
    // Let the sound itself decay. Releasing a sustained bass after 250 ms would manufacture a
    // percussive envelope and make the measured key gate masquerade as a property of the sound.
    let off = frames.saturating_sub(1);
    for start in (0..frames).step_by(BLOCK) {
        if started.elapsed().as_secs_f64() > f64::from(request.options.timeout_seconds) {
            return Err(failure("the drum scan exceeded its time budget"));
        }
        service()?;
        let count = (frames - start).min(BLOCK);
        block.set_frame_count(count);
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
                sample_rate: request.sample_rate,
                block_frames: count,
                playhead_samples: start as u64,
                bpm: request.bpm,
                is_playing: true,
                is_offline: true,
            },
        );
        service()?;
        for channel in 0..2 {
            audio.channel_mut(channel)[start..start + count]
                .copy_from_slice(block.channel(channel));
        }
    }
    Ok(audio)
}

fn settle_previous_hit(
    instrument: &mut dyn Instrument,
    request: &DrumProbeRequest,
    started: Instant,
    service: &mut dyn FnMut() -> Result<(), SessionError>,
) -> Result<(), SessionError> {
    // Choke voices without restarting the instrument's random or round-robin sequence. Repeated
    // full resets can hide the very variation the repetitions are supposed to measure.
    let mut block = AudioBuffer::stereo(BLOCK, request.sample_rate);
    let mut quiet_blocks = 0;
    for start in (0..(request.sample_rate * 4.0) as usize).step_by(BLOCK) {
        if started.elapsed().as_secs_f64() > f64::from(request.options.timeout_seconds) {
            return Err(failure("the drum scan exceeded its time budget"));
        }
        service()?;
        let events = [NoteEvent::AllSoundOff { frame: 0 }];
        instrument.process(
            if start == 0 { &events } else { &[] },
            &mut block,
            &ProcessContext {
                sample_rate: request.sample_rate,
                block_frames: BLOCK,
                playhead_samples: start as u64,
                bpm: request.bpm,
                is_playing: true,
                is_offline: true,
            },
        );
        service()?;
        if block.channels().iter().flatten().any(|v| !v.is_finite()) {
            return Err(failure(
                "the instrument produced non-finite audio before a trigger",
            ));
        }
        let peak = block
            .channels()
            .iter()
            .flatten()
            .copied()
            .map(f32::abs)
            .fold(0.0f32, f32::max);
        quiet_blocks = if peak < 1e-6 { quiet_blocks + 1 } else { 0 };
        if quiet_blocks >= 3 {
            return Ok(());
        }
    }
    Err(failure(
        "the source did not become silent between triggers; its tail would contaminate measurements",
    ))
}

fn fingerprint(request: &DrumProbeRequest) -> Result<String, SessionError> {
    // FNV-1a is a deterministic change detector, not a security or audio-classification claim.
    let mut hash = 0xcbf29ce484222325u64;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        }
    };
    let source = (
        &request.instrument_id,
        &request.state,
        &request.file,
        &request.soundfont,
        request.sample_rate,
        request.bpm,
    );
    feed(&serde_json::to_vec(&source).map_err(|e| failure(e.to_string()))?);
    for path in request
        .file
        .iter()
        .chain(request.soundfont.iter().map(|font| &font.path))
    {
        let mut pending = vec![path.clone()];
        let mut entries = 0;
        while let Some(path) = pending.pop() {
            entries += 1;
            if entries > 20_000 {
                return Err(failure("the plugin bundle has too many files to snapshot"));
            }
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|e| failure(format!("cannot inspect source {}: {e}", path.display())))?;
            feed(path.to_string_lossy().as_bytes());
            feed(&metadata.len().to_le_bytes());
            let modified = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |t| t.as_nanos());
            feed(&modified.to_le_bytes());
            if metadata.is_dir() {
                let mut children = std::fs::read_dir(&path)
                    .map_err(|e| failure(e.to_string()))?
                    .map(|entry| entry.map(|e| e.path()))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| failure(e.to_string()))?;
                children.sort();
                pending.extend(children);
            }
        }
    }
    Ok(format!("{hash:016x}"))
}

fn failure(message: impl Into<String>) -> SessionError {
    SessionError::DrumAnalysis(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::session;

    fn quick_options() -> DrumScanOptions {
        DrumScanOptions {
            notes: vec![36, 60],
            velocities: vec![0.6, 1.0],
            repetitions: 2,
            seconds_per_note: 0.75,
            ..Default::default()
        }
    }

    #[test]
    fn scan_renders_actual_synth_and_does_not_edit_the_document() {
        let mut session = session();
        let track = session
            .add_instrument_track("misleading hat label", "auris.synth.noisedrum")
            .unwrap();
        let original = session.project().clone();
        let result = session.analyze_drum_kit(track, &quick_options()).unwrap();
        assert_eq!(session.project(), &original);
        assert_eq!(result.voices.len(), 2);
        assert_eq!(result.voices[0].samples.len(), 4);
        assert!(
            result.voices[0]
                .samples
                .iter()
                .all(|s| s.acoustics.rms > 0.001)
        );
        assert!(
            result.voices[0].samples[0]
                .acoustics
                .spectrum
                .centroid_hz
                .is_finite()
        );
    }

    #[test]
    fn apply_is_explicit_undoable_and_rejects_changed_source() {
        let mut session = session();
        let track = session
            .add_instrument_track("Kit", "auris.synth.noisedrum")
            .unwrap();
        let report = session.analyze_drum_kit(track, &quick_options()).unwrap();
        let before = session.project().clone();
        assert!(session.apply_drum_map(&report, false).unwrap());
        assert!(session.undo().is_some());
        assert_eq!(session.project(), &before);
        session
            .project
            .track_mut(track)
            .unwrap()
            .kind
            .as_instrument_mut()
            .unwrap()
            .instrument_state
            .params
            .insert("decay".into(), 0.777);
        assert!(session.apply_drum_map(&report, false).is_err());
    }

    #[test]
    fn invalid_budget_and_missing_host_state_are_not_silent_scans() {
        let mut options = quick_options();
        options.notes = vec![128];
        assert!(options.validate().is_err());
        options = quick_options();
        options.seconds_per_note = f64::NAN;
        assert!(options.validate().is_err());
    }

    #[test]
    fn independent_clap_scan_restores_state_and_leaves_live_voices_alone() {
        use auris_clap::testkit::{TONE_ID, instrument_library};
        let library = instrument_library();
        let mut live = library.instantiate(TONE_ID).unwrap();
        live.load_state(&0.25f32.to_le_bytes()).unwrap();
        let prepare = PrepareContext::new(48_000.0, BLOCK, 2);
        let mut playing = live.activate_instrument(&prepare).unwrap();
        let mut block = AudioBuffer::stereo(BLOCK, 48_000.0);
        let context = ProcessContext::realtime(48_000.0, BLOCK, 0, 120.0, true);
        playing.process(
            &[NoteEvent::NoteOn {
                frame: 0,
                pitch: 36,
                velocity: 1.0,
            }],
            &mut block,
            &context,
        );
        let before = block.channel(0)[10];
        assert!((before - 0.25).abs() < 1e-5);
        let mut state = PluginState::empty();
        state.set_hosted_bytes(&live.save_state().unwrap());
        let mut request = DrumProbeRequest {
            track: TrackId(1),
            instrument_id: format!("clap:{TONE_ID}"),
            state,
            file: None,
            soundfont: None,
            sample_rate: 48_000.0,
            bpm: 120.0,
            options: DrumScanOptions {
                notes: vec![36],
                velocities: vec![1.0],
                repetitions: 1,
                seconds_per_note: 0.5,
                ..Default::default()
            },
            source_fingerprint: "fixture".into(),
        };
        let report = scan_clap_library(&library, &request, &prepare, Instant::now()).unwrap();
        assert!((report.voices[0].samples[0].acoustics.rms - 0.25).abs() < 1e-4);
        assert!(
            report.proposed_map.voices.is_empty(),
            "a held constant source is not percussive"
        );
        playing.process(&[], &mut block, &context);
        assert!((block.channel(0)[10] - before).abs() < 1e-5);
        request.state.set_hosted_bytes(&[]);
        assert!(scan_clap_library(&library, &request, &prepare, Instant::now()).is_err());
        drop(playing);
        live.release();
    }

    #[test]
    fn saved_assignment_does_not_invalidate_its_own_measurement() {
        let mut session = session();
        let track = session
            .add_instrument_track("Kit", "auris.synth.noisedrum")
            .unwrap();
        let report = session.analyze_drum_kit(track, &quick_options()).unwrap();
        assert!(session.apply_drum_map(&report, false).unwrap());
        assert!(!session.apply_drum_map(&report, false).unwrap());
        assert!(
            session
                .project()
                .track(track)
                .unwrap()
                .kind
                .as_instrument()
                .unwrap()
                .clips
                .is_empty()
        );
    }

    #[test]
    fn complete_builtin_scan_is_bounded_and_leaves_absent_keys_unclassified() {
        let mut session = session();
        let track = session
            .add_instrument_track("Kit", "auris.synth.drumkit")
            .unwrap();
        let report = session
            .analyze_drum_kit(track, &DrumScanOptions::default())
            .unwrap();
        assert_eq!(report.voices.len(), 128);
        assert!(report.voices[0].silent);
        assert!(report.voices[0].fitness.values().all(|v| *v == 0.0));
        assert!(report.proposed_map.voices.contains_key(&DrumRole::Kick));
        assert!(report.proposed_map.voices.contains_key(&DrumRole::Snare));
        assert!(report.proposed_map.voices.contains_key(&DrumRole::OpenHat));
        assert!(report.proposed_map.voices.contains_key(&DrumRole::Crash));
        assert!(report.proposed_map.voices.contains_key(&DrumRole::Tom));
        assert!(
            report
                .proposed_map
                .voices
                .contains_key(&DrumRole::ClosedHat)
        );
        // Known synthesis fixtures verify acoustic consequences; their note addresses still
        // enter only the probe, never the fitness function.
        assert!(report.voices[38].fitness[&DrumRole::Snare] > report.options.minimum_fitness);
        assert!(
            report.voices[46].fitness[&DrumRole::OpenHat]
                > report.voices[46].fitness[&DrumRole::ClosedHat]
        );
        assert!(
            report.voices[41].fitness[&DrumRole::Tom] > report.voices[41].fitness[&DrumRole::Kick]
        );
        let assigned_duration = |role| {
            let note = report.proposed_map.voices[&role];
            report.voices[usize::from(note)].samples[0]
                .acoustics
                .energy_duration_seconds
        };
        assert!(assigned_duration(DrumRole::ClosedHat) < assigned_duration(DrumRole::OpenHat));
        assert!(assigned_duration(DrumRole::OpenHat) < assigned_duration(DrumRole::Crash));
        report.validate().unwrap();
    }

    #[test]
    fn serialized_evidence_roundtrips_and_tampering_is_rejected() {
        let mut session = session();
        let track = session
            .add_instrument_track("Kit", "auris.synth.drumkit")
            .unwrap();
        let mut options = quick_options();
        options.notes = vec![36, 0];
        let report = session.analyze_drum_kit(track, &options).unwrap();
        let mut loaded: DrumKitAnalysis =
            serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
        loaded.validate().unwrap();
        loaded.voices[1].fitness.insert(DrumRole::Kick, 1.0);
        assert!(loaded.validate().is_err());
        loaded = report.clone();
        loaded.voices[0].samples[0].acoustics.spectrum.high = f64::NAN;
        assert!(loaded.validate().is_err());
        loaded = report.clone();
        loaded.voices[0].samples.pop();
        assert!(loaded.validate().is_err());
        session.set_bpm(report.bpm + 10.0);
        assert!(session.apply_drum_map(&report, false).is_err());
    }

    #[test]
    fn worker_services_due_clap_timers() {
        use auris_clap::testkit::{FIXTURE_ID, fixture_library};
        let library = fixture_library();
        let mut plugin = library.instantiate(FIXTURE_ID).unwrap();
        let before = plugin.value(auris_core::param::ParamId(2)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(25));
        service_clap_requests(&mut plugin, false).unwrap();
        assert!(plugin.value(auris_core::param::ParamId(2)).unwrap() > before);
    }

    #[test]
    fn explicit_remap_preserves_untagged_notes_and_future_generation_uses_saved_map() {
        use auris_core::time::Ticks;
        use auris_core::{ClipPreset, ClipRecipe, Note};
        let mut session = session();
        let track = session
            .add_instrument_track("Kit", "auris.synth.drumkit")
            .unwrap();
        let options = DrumScanOptions {
            notes: vec![35],
            velocities: vec![1.0],
            repetitions: 1,
            seconds_per_note: 1.0,
            ..Default::default()
        };
        let report = session.analyze_drum_kit(track, &options).unwrap();
        assert_eq!(report.proposed_map.voices.get(&DrumRole::Kick), Some(&35));
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                Ticks::QUARTER * 4,
                ClipRecipe::new(ClipPreset::Kick, 1),
            )
            .unwrap();
        session
            .add_note(clip, Note::new(100, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        session.apply_drum_map(&report, true).unwrap();
        let notes = &session.midi_clip(clip).unwrap().notes;
        assert!(
            notes
                .iter()
                .any(|n| n.drum_voice.is_empty() && n.pitch == 100)
        );
        assert!(
            notes
                .iter()
                .filter(|n| !n.drum_voice.is_empty())
                .all(|n| n.pitch == 35)
        );
        session.regenerate_clip(clip).unwrap();
        assert!(
            session
                .midi_clip(clip)
                .unwrap()
                .notes
                .iter()
                .all(|n| n.pitch == 35)
        );
    }

    #[test]
    fn repetition_preserves_round_robin_state_and_passes_the_captured_tempo() {
        use auris_core::param::{ParamDescriptor, ParamId};
        use auris_core::plugin::{PluginCategory, PluginDescriptor};
        #[derive(Default)]
        struct Alternating {
            hits: u32,
            age: usize,
            active: bool,
            bpm: f64,
        }
        impl Parameterized for Alternating {
            fn parameters(&self) -> &[ParamDescriptor] {
                &[]
            }
            fn param(&self, _: ParamId) -> f32 {
                0.0
            }
            fn set_param(&mut self, _: ParamId, _: f32) {}
        }
        impl Instrument for Alternating {
            fn descriptor(&self) -> PluginDescriptor {
                PluginDescriptor::instrument(
                    "test.roundrobin",
                    "Any misleading label",
                    "",
                    PluginCategory::Other,
                )
            }
            fn prepare(&mut self, _: &PrepareContext) {}
            fn reset(&mut self) {
                *self = Self::default();
            }
            fn process(
                &mut self,
                events: &[NoteEvent],
                out: &mut AudioBuffer,
                context: &ProcessContext,
            ) {
                self.bpm = context.bpm;
                for frame in 0..out.frame_count() {
                    for event in events.iter().filter(|e| e.frame() as usize == frame) {
                        match event {
                            NoteEvent::NoteOn { .. } => {
                                self.hits += 1;
                                self.age = 0;
                                self.active = true;
                            }
                            NoteEvent::NoteOff { .. } | NoteEvent::AllSoundOff { .. } => {
                                self.active = false
                            }
                            _ => {}
                        }
                    }
                    let t = self.age as f64 / context.sample_rate;
                    let frequency = if self.hits % 2 == 1 { 80.0 } else { 6000.0 };
                    let sample = if self.active {
                        ((std::f64::consts::TAU * frequency * t).sin() * (-t / 0.04).exp()) as f32
                    } else {
                        0.0
                    };
                    for channel in out.channels_mut() {
                        channel[frame] = sample;
                    }
                    self.age += 1;
                }
            }
        }
        let request = DrumProbeRequest {
            track: TrackId(1),
            instrument_id: "irrelevant".into(),
            state: PluginState::empty(),
            file: None,
            soundfont: None,
            sample_rate: 24_000.0,
            bpm: 173.0,
            options: DrumScanOptions {
                notes: vec![99],
                velocities: vec![1.0],
                repetitions: 2,
                seconds_per_note: 0.5,
                ..Default::default()
            },
            source_fingerprint: "fixture".into(),
        };
        let mut source = Alternating::default();
        let mut callbacks = 0;
        let report = scan(&mut source, &request, Instant::now(), &mut || {
            callbacks += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(source.bpm, 173.0);
        assert!(callbacks > 4);
        assert!(report.voices[0].samples[0].acoustics.spectrum.low > 0.99);
        assert!(report.voices[0].samples[1].acoustics.spectrum.high > 0.99);
        assert!(report.voices[0].fitness_variation > 0.9);
        assert!(report.proposed_map.voices.is_empty());
    }

    #[test]
    fn inactive_restart_is_acknowledged_but_active_restart_invalidates_measurements() {
        let pending = auris_clap::PendingRequests {
            restart: true,
            restart_requested: true,
            callback: true,
            ..Default::default()
        };
        assert!(clap_callback_needed(pending, false).unwrap());
        assert!(clap_callback_needed(pending, true).is_err());
        let presentation = auris_clap::PendingRequests {
            restart: true,
            parameter_info_changed: true,
            callback: true,
            ..Default::default()
        };
        assert!(clap_callback_needed(presentation, true).unwrap());
        assert!(
            clap_callback_needed(
                auris_clap::PendingRequests {
                    parameter_rescan: true,
                    ..presentation
                },
                true
            )
            .is_err()
        );
        let library = auris_clap::testkit::instrument_library();
        let mut plugin = library.instantiate(auris_clap::testkit::TONE_ID).unwrap();
        plugin.load_state(&0.25f32.to_le_bytes()).unwrap();
        assert!(plugin.refresh_parameters());
        let instrument = plugin
            .activate_instrument(&PrepareContext::new(48_000.0, BLOCK, 2))
            .unwrap();
        assert!(!plugin.refresh_parameters());
        drop(instrument);
        assert!(plugin.release());
        assert!(plugin.refresh_parameters());
    }
}
