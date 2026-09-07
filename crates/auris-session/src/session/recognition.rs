//! Immutable recognition jobs and explicit, undoable acceptance of their drafts.

use super::mixture::{self, MixtureAnalysis, MixtureOptions};
use crate::{Edit, Session, SessionError};
use auris_analysis::instruments::{self, INSTRUMENT_RATE, InstrumentAnalysis};
use auris_analysis::{
    AnalysisControl,
    audio::{self, AudioAnalysis, AudioOptions, TranscribedNote},
    chords::{self, ChordOptions, ChordState, SymbolicChordSegment},
};
use auris_core::{
    AudioBuffer, AudioClip, ClipId, Note, TrackId,
    harmony::{ChordMap, ChordPoint},
    project::loop_passes,
    theory::{
        chord::Chord,
        key::Key,
        numeral::{Numeral, degree_of},
    },
    time::{Seconds, Ticks},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};

fn failure(error: impl ToString) -> SessionError {
    SessionError::MusicAnalysis(error.to_string())
}

/// A bounded selection of immutable written notes ready for a worker.
#[derive(Clone, Debug)]
pub struct ChordAnalysisJob {
    notes: Vec<Note>,
    from: Ticks,
    to: Ticks,
    options: ChordOptions,
    fingerprint: String,
}

/// Read-only harmony hypotheses with enough provenance to reject stale edits.
#[derive(Clone, Debug, Serialize)]
pub struct ChordAnalysisReport {
    /// Recognition algorithm identifier.
    pub algorithm: &'static str,
    /// Inclusive range start in project ticks.
    pub from: Ticks,
    /// Exclusive range end in project ticks.
    pub to: Ticks,
    /// Detection resolution in ticks.
    pub window: Ticks,
    /// Harmony candidates for each interval.
    pub segments: Vec<SymbolicChordSegment>,
    #[serde(skip)]
    fingerprint: String,
}

impl ChordAnalysisJob {
    /// Computes the report on a worker, leaving the session untouched.
    pub fn run(&self, control: &AnalysisControl) -> Result<ChordAnalysisReport, SessionError> {
        let segments =
            chords::analyze_notes(&self.notes, self.from, self.to, self.options, control)
                .map_err(failure)?;
        Ok(ChordAnalysisReport {
            algorithm: "cpu-chord-templates-v1",
            from: self.from,
            to: self.to,
            window: self.options.window,
            segments,
            fingerprint: self.fingerprint.clone(),
        })
    }
}

/// An immutable audio-clip source and trim, ready for resampling and analysis on a worker.
#[derive(Clone, Debug)]
pub struct AudioAnalysisJob {
    clip: AudioClip,
    audio: Arc<AudioBuffer>,
    options: AudioOptions,
    fingerprint: String,
}

/// Analysis of one pass of a trimmed audio clip, before effects and time stretching.
#[derive(Clone, Debug, Serialize)]
pub struct ClipAudioAnalysis {
    /// Clip that supplied the audio and placement information.
    pub clip: ClipId,
    /// Original source seconds at the start of the analyzed trim.
    pub source_offset_seconds: f64,
    /// Times here are relative to the analyzed trim, in unstretched seconds.
    pub analysis: AudioAnalysis,
    #[serde(skip)]
    job: AudioAnalysisJob,
}

/// Instrument-presence hypotheses for a trimmed, immutable audio source.
#[derive(Clone, Debug, Serialize)]
pub struct ClipInstrumentAnalysis {
    /// Original source seconds at the start of the analyzed trim.
    pub source_offset_seconds: f64,
    /// Overlapping windows relative to the trimmed source, before stretching.
    pub analysis: InstrumentAnalysis,
    #[serde(skip)]
    job: AudioAnalysisJob,
}

/// A noncommercial MuScriptor draft tied to one immutable audio clip source.
#[derive(Clone, Debug, Serialize)]
pub struct ClipMixtureAnalysis {
    /// Original source time at the analyzed trim's start.
    pub source_offset_seconds: f64,
    /// Notes relative to the trimmed first pass, before stretch/effects.
    pub analysis: MixtureAnalysis,
    #[serde(skip)]
    job: AudioAnalysisJob,
}

impl AudioAnalysisJob {
    /// Resamples the trimmed first pass, then analyzes it on the CPU.
    ///
    /// Cancellation is checked before/after the existing synchronous resampler and between
    /// analysis frames. Clip repeats are mapped when the draft is placed into the project.
    pub fn run(&self, control: &AnalysisControl) -> Result<ClipAudioAnalysis, SessionError> {
        check_cancel(control)?;
        let prepared = self.prepared(audio::ANALYSIS_RATE)?;
        let analysis = audio::analyze_audio(&prepared, self.options, control).map_err(failure)?;
        Ok(ClipAudioAnalysis {
            clip: self.clip.id,
            source_offset_seconds: self.source_offset_seconds(),
            analysis,
            job: self.clone(),
        })
    }

    /// Runs optional local YAMNet tagging on the CPU without editing the project.
    pub fn run_instruments(
        &self,
        model: &Path,
        threshold: f32,
        control: &AnalysisControl,
    ) -> Result<ClipInstrumentAnalysis, SessionError> {
        check_cancel(control)?;
        let prepared = self.prepared(INSTRUMENT_RATE)?;
        let analysis = instruments::analyze_instruments(&prepared, model, threshold, control)
            .map_err(failure)?;
        Ok(ClipInstrumentAnalysis {
            source_offset_seconds: self.source_offset_seconds(),
            analysis,
            job: self.clone(),
        })
    }

    fn source_offset_seconds(&self) -> f64 {
        self.clip.offset_frames.min(self.audio.frame_count() as u64) as f64
            / self.audio.sample_rate()
    }

    /// Runs optional MuScriptor after checking this invocation's noncommercial acknowledgement.
    pub fn run_mixture(
        &self,
        options: &MixtureOptions,
        control: &AnalysisControl,
    ) -> Result<ClipMixtureAnalysis, SessionError> {
        options.validate()?;
        check_cancel(control)?;
        let prepared = self.prepared(16000.0)?;
        let analysis = mixture::transcribe_buffer(&prepared, options, control)?;
        Ok(ClipMixtureAnalysis {
            source_offset_seconds: self.source_offset_seconds(),
            analysis,
            job: self.clone(),
        })
    }

    fn prepared(&self, rate: f64) -> Result<AudioBuffer, SessionError> {
        let from = self.clip.offset_frames.min(self.audio.frame_count() as u64) as usize;
        let to = self
            .clip
            .offset_frames
            .saturating_add(self.clip.length_frames)
            .min(self.audio.frame_count() as u64) as usize;
        let trimmed = AudioBuffer::from_planar(
            self.audio
                .iter_channels()
                .map(|c| c[from..to.max(from)].to_vec())
                .collect(),
            self.audio.sample_rate(),
        )?;
        Ok(auris_io::resample_buffer(&trimmed, rate)?)
    }
}

fn check_cancel(control: &AnalysisControl) -> Result<(), SessionError> {
    // A zero-length symbolic job is not used as a cancellation probe: control has its own API.
    if control.is_cancelled() {
        Err(failure("cancelled"))
    } else {
        Ok(())
    }
}

/// Decodes and analyzes a file without opening, importing or changing a project.
///
/// Runs on a worker. The existing decoder is synchronous; cancellation is observed before
/// decoding and by the analysis afterwards. Result timestamps start at the file's beginning.
pub fn analyze_audio_file(
    path: &Path,
    options: AudioOptions,
    control: &AnalysisControl,
) -> Result<AudioAnalysis, SessionError> {
    check_cancel(control)?;
    let audio = super::decode_audio(path, audio::ANALYSIS_RATE)?;
    audio::analyze_audio(&audio, options, control).map_err(failure)
}

/// Decodes a file and tags instrument/voice presence with a local model on the CPU.
/// No model is downloaded and no project is opened or edited.
pub fn analyze_instrument_file(
    path: &Path,
    model: &Path,
    threshold: f32,
    control: &AnalysisControl,
) -> Result<InstrumentAnalysis, SessionError> {
    check_cancel(control)?;
    let audio = super::decode_audio(path, INSTRUMENT_RATE)?;
    instruments::analyze_instruments(&audio, model, threshold, control).map_err(failure)
}

impl Session {
    /// Whether the source and document still match a multi-instrument draft.
    pub fn mixture_analysis_is_current(&self, report: &ClipMixtureAnalysis) -> bool {
        self.recognition_fingerprint()
            .is_ok_and(|f| f == report.job.fingerprint)
            && self
                .bank
                .get(report.job.clip.source)
                .is_some_and(|b| Arc::ptr_eq(b, &report.job.audio))
    }

    /// Accepts a file's instrument-labeled notes as new tracks in one undo step.
    /// Track/clip names retain MuScriptor provenance; playback patches require user selection.
    pub fn create_mixture_tracks(
        &mut self,
        report: &MixtureAnalysis,
        start: Ticks,
    ) -> Result<Vec<TrackId>, SessionError> {
        let groups = self.mixture_groups(report)?;
        let origin = self.project.tempo_map.ticks_to_seconds(start).0;
        let mapped = groups
            .into_iter()
            .map(|(name, notes)| {
                (
                    name,
                    self.map_transcription(&notes, origin, 1.0, start, Ticks(i64::MAX)),
                )
            })
            .collect();
        self.place_mixture_groups(mapped, start)
    }

    /// Accepts all parts of a clip draft, preserving trim, stretch, loops and source audio.
    pub fn create_clip_mixture_tracks(
        &mut self,
        report: &ClipMixtureAnalysis,
    ) -> Result<Vec<TrackId>, SessionError> {
        if !self.mixture_analysis_is_current(report) {
            return Err(failure("the audio or document changed; analyze it again"));
        }
        let groups = self.mixture_groups(&report.analysis)?;
        let source = &report.job.clip;
        let content = self.audio_clip_length_ticks(source);
        if content.raw() <= 0 || source.loop_end.raw() / content.raw() > 200_000 {
            return Err(failure("invalid or excessive audio repeats"));
        }
        let stretch = source.stretch_in(&self.project.tempo_map);
        let mut mapped = Vec::new();
        let mut total = 0;
        for (name, events) in groups {
            let mut notes = Vec::new();
            for (offset, span) in loop_passes(content, source.loop_end) {
                let pass = source.start + offset;
                let origin = self.project.tempo_map.ticks_to_seconds(pass).0;
                let batch =
                    self.map_transcription(&events, origin, stretch, source.start, pass + span);
                total += batch.len();
                if total > 200_000 {
                    return Err(failure("too many mixture notes after repeating"));
                }
                notes.extend(batch);
            }
            mapped.push((name, notes));
        }
        self.place_mixture_groups(mapped, source.start)
    }

    fn mixture_groups(
        &self,
        report: &MixtureAnalysis,
    ) -> Result<std::collections::BTreeMap<String, Vec<TranscribedNote>>, SessionError> {
        if !report.seconds.is_finite()
            || !(0.0..=600.0).contains(&report.seconds)
            || report.notes.is_empty()
        {
            return Err(failure("no usable mixture transcription"));
        }
        let mut notes = report.notes.clone();
        mixture::validate_notes(&mut notes, report.seconds)?;
        let mut groups = std::collections::BTreeMap::<String, Vec<TranscribedNote>>::new();
        for n in notes {
            groups
                .entry(n.instrument)
                .or_default()
                .push(TranscribedNote {
                    pitch: n.pitch,
                    start: n.start,
                    end: n.end,
                    strength: 0.7,
                });
            if groups.len() > 128 {
                return Err(failure("too many instrument groups"));
            }
        }
        Ok(groups)
    }

    fn place_mixture_groups(
        &mut self,
        groups: Vec<(String, Vec<Note>)>,
        start: Ticks,
    ) -> Result<Vec<TrackId>, SessionError> {
        if self.transaction.is_some() {
            return Err(SessionError::EditInProgress);
        }
        if start.raw() < 0 || groups.iter().all(|(_, n)| n.is_empty()) {
            return Err(failure("no notes at the requested placement"));
        }
        self.begin_transaction(Edit::AddInstrumentTrack);
        let outcome = (|| {
            let mut tracks = Vec::new();
            for (instrument, notes) in groups {
                let Some(length) = notes.iter().map(Note::end).max() else {
                    continue;
                };
                let name = format!("MuScriptor [NC] - {instrument}");
                let track = if instrument == "drums"
                    && self.registry.has_instrument(auris_synth::DrumKit::ID)
                {
                    self.add_instrument_track(&name, auris_synth::DrumKit::ID)?
                } else {
                    self.add_default_instrument_track(&name)?
                };
                let clip = self.add_midi_clip(track, &name, start, length)?;
                self.project
                    .midi_clip_mut(clip)
                    .expect("inserted clip")
                    .notes = notes;
                tracks.push(track);
            }
            Ok(tracks)
        })();
        if outcome.is_err() {
            self.revert_transaction();
        } else {
            self.end_transaction();
        }
        outcome
    }

    fn recognition_fingerprint(&self) -> Result<String, SessionError> {
        let bytes = serde_json::to_vec(&self.project).map_err(failure)?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    /// Prepares written notes from selected tracks (empty means all pitched note tracks).
    ///
    /// Reads stored pitches, clipping and repeats, without playback humanization. Muted clips,
    /// muted tracks, known drum instruments, percussion presets and drum recipes are skipped.
    /// Hosted instruments with no percussion metadata can be excluded through the selection.
    pub fn chord_analysis_job(
        &self,
        tracks: &[TrackId],
        from: Ticks,
        to: Ticks,
        options: ChordOptions,
    ) -> Result<ChordAnalysisJob, SessionError> {
        if from.raw() < 0
            || to <= from
            || options.window.raw() <= 0
            || (to - from).raw() / options.window.raw() > 16_384
        {
            return Err(failure("invalid or excessive analysis range"));
        }
        for track in tracks {
            self.require_track(*track)?;
        }
        let mut notes = Vec::new();
        for track in &self.project.tracks {
            if (!tracks.is_empty() && !tracks.contains(&track.id)) || track.mixer.mute {
                continue;
            }
            if self.track_preset(track.id).is_some_and(|p| p.bank == 128)
                || track
                    .kind
                    .as_instrument()
                    .is_some_and(|t| t.instrument_id == "auris.synth.noisedrum")
            {
                continue;
            }
            for clip in track.kind.note_clips().into_iter().flatten() {
                if clip.muted
                    || clip.length.raw() <= 0
                    || clip.recipe.as_ref().is_some_and(|r| r.preset.is_drums())
                {
                    continue;
                }
                if clip.loop_end.raw() / clip.length.raw() > 200_000 {
                    return Err(failure("too many note repeats for analysis"));
                }
                for (offset, span) in loop_passes(clip.length, clip.loop_end) {
                    let base = clip.start + offset;
                    if base >= to || base + span <= from {
                        continue;
                    }
                    for note in clip
                        .playable_notes()
                        .filter(|n| n.start < span && n.drum_voice.is_empty())
                    {
                        let start = base + note.start;
                        let end = (start + note.length.min(span - note.start)).min(to);
                        if end > from && end > start {
                            notes.push(Note {
                                start: start.max(from),
                                length: end - start.max(from),
                                ..note
                            });
                            if notes.len() > 200_000 {
                                return Err(failure("select fewer notes or a shorter range"));
                            }
                        }
                    }
                }
            }
        }
        Ok(ChordAnalysisJob {
            notes,
            from,
            to,
            options,
            fingerprint: self.recognition_fingerprint()?,
        })
    }

    /// Whether the entire document still matches the report's snapshot.
    pub fn chord_analysis_is_current(&self, report: &ChordAnalysisReport) -> bool {
        self.recognition_fingerprint()
            .is_ok_and(|f| f == report.fingerprint)
    }

    /// Applies recognized top candidates and clears silent intervals in one undo step.
    /// Unknown intervals and harmony outside the analyzed range are preserved.
    pub fn apply_chord_analysis(
        &mut self,
        report: &ChordAnalysisReport,
    ) -> Result<usize, SessionError> {
        if !self.chord_analysis_is_current(report) {
            return Err(failure(
                "the document changed; analyze it again before applying harmony",
            ));
        }
        if self.transaction.is_some() {
            return Err(SessionError::EditInProgress);
        }
        if report.from.raw() < 0 || report.to <= report.from || report.segments.len() > 16_384 {
            return Err(failure("invalid chord report range"));
        }
        let mut map = self.project.harmony.chords.clone();
        let mut applied = 0;
        let mut end = report.from;
        for segment in &report.segments {
            if segment.start < end || segment.end <= segment.start || segment.end > report.to {
                return Err(failure("invalid chord intervals"));
            }
            end = segment.end;
            let chord = match segment.reading.state {
                ChordState::Unknown => continue,
                ChordState::NoChord => None,
                ChordState::Recognized => Some(
                    segment
                        .reading
                        .candidates
                        .first()
                        .and_then(|c| Chord::parse(&c.symbol))
                        .ok_or_else(|| failure("invalid chord candidate"))?,
                ),
            };
            let restored = map.numeral_at(segment.end);
            let mut points: Vec<_> = map
                .points()
                .iter()
                .filter(|p| p.tick < segment.start || p.tick >= segment.end)
                .copied()
                .collect();
            let mut boundaries = vec![segment.start];
            boundaries.extend(
                self.project
                    .harmony
                    .keys
                    .points()
                    .iter()
                    .filter(|p| p.tick > segment.start && p.tick < segment.end)
                    .map(|p| p.tick),
            );
            for tick in boundaries {
                points.push(ChordPoint {
                    tick,
                    chord: chord.map(|c| numeral(c, self.project.harmony.key_at(tick))),
                });
            }
            map = ChordMap::new(points);
            map.set_point(segment.end, restored);
            applied += 1;
        }
        if map != self.project.harmony.chords {
            self.record(Edit::SetChord);
            self.project.harmony.chords = map;
        }
        Ok(applied)
    }

    /// Chooses one alternate reading in the report, then applies only that interval.
    pub fn apply_chord_candidate(
        &mut self,
        report: &ChordAnalysisReport,
        segment: usize,
        candidate: usize,
    ) -> Result<usize, SessionError> {
        let mut selection = report.clone();
        let mut span = selection
            .segments
            .get(segment)
            .cloned()
            .ok_or_else(|| failure("unknown chord interval"))?;
        let choice = span
            .reading
            .candidates
            .get(candidate)
            .cloned()
            .ok_or_else(|| failure("unknown chord candidate"))?;
        span.reading.state = ChordState::Recognized;
        span.reading.candidates = vec![choice];
        selection.from = span.start;
        selection.to = span.end;
        selection.segments = vec![span];
        self.apply_chord_analysis(&selection)
    }

    /// Prepares a trimmed audio clip for offline CPU analysis, sharing its source samples.
    pub fn audio_analysis_job(
        &self,
        clip: ClipId,
        options: AudioOptions,
    ) -> Result<AudioAnalysisJob, SessionError> {
        let clip = self
            .project
            .tracks
            .iter()
            .filter_map(|t| t.kind.as_audio())
            .flat_map(|t| &t.clips)
            .find(|c| c.id == clip)
            .ok_or(SessionError::UnknownClip(clip.0))?;
        let audio = self
            .bank
            .get(clip.source)
            .ok_or_else(|| failure("audio source is missing"))?;
        if clip.length_frames as f64 / audio.sample_rate() > 1800.0 || audio.channel_count() > 8 {
            return Err(failure("select at most thirty minutes and eight channels"));
        }
        Ok(AudioAnalysisJob {
            clip: clip.clone(),
            audio: Arc::clone(audio),
            options,
            fingerprint: self.recognition_fingerprint()?,
        })
    }

    /// Checks both the document and the immutable audio buffer before publishing or applying.
    pub fn audio_analysis_is_current(&self, report: &ClipAudioAnalysis) -> bool {
        self.recognition_fingerprint()
            .is_ok_and(|f| f == report.job.fingerprint)
            && self
                .bank
                .get(report.job.clip.source)
                .is_some_and(|b| Arc::ptr_eq(b, &report.job.audio))
    }

    /// Rejects instrument results after document edits or source replacement.
    pub fn instrument_analysis_is_current(&self, report: &ClipInstrumentAnalysis) -> bool {
        self.recognition_fingerprint()
            .is_ok_and(|f| f == report.job.fingerprint)
            && self
                .bank
                .get(report.job.clip.source)
                .is_some_and(|b| Arc::ptr_eq(b, &report.job.audio))
    }

    /// Places a file transcription on an existing note track using the current tempo map.
    /// Raw timing is retained; tempo and existing notes are not changed.
    pub fn place_transcription(
        &mut self,
        report: &AudioAnalysis,
        track: TrackId,
        start: Ticks,
        name: &str,
    ) -> Result<ClipId, SessionError> {
        self.validate_transcription(report)?;
        let origin = self.project.tempo_map.ticks_to_seconds(start).0;
        let notes = self.map_transcription(&report.notes, origin, 1.0, start, Ticks(i64::MAX));
        self.place_analyzed_notes(track, start, name, notes)
    }

    /// Places a clip transcription with its trim, stretch and repeats, in one undo step.
    pub fn place_clip_transcription(
        &mut self,
        report: &ClipAudioAnalysis,
        track: TrackId,
        name: &str,
    ) -> Result<ClipId, SessionError> {
        let notes = self.clip_transcription_notes(report)?;
        self.place_analyzed_notes(track, report.job.clip.start, name, notes)
    }

    fn clip_transcription_notes(
        &self,
        report: &ClipAudioAnalysis,
    ) -> Result<Vec<Note>, SessionError> {
        if !self.audio_analysis_is_current(report) {
            return Err(failure("the audio or document changed; analyze it again"));
        }
        self.validate_transcription(&report.analysis)?;
        let source = &report.job.clip;
        let content = self.audio_clip_length_ticks(source);
        if content.raw() <= 0 || source.loop_end.raw() / content.raw() > 200_000 {
            return Err(failure("invalid or excessive audio repeats"));
        }
        let stretch = source.stretch_in(&self.project.tempo_map);
        let mut notes = Vec::new();
        for (offset, span) in loop_passes(content, source.loop_end) {
            let pass_start = source.start + offset;
            let origin = self.project.tempo_map.ticks_to_seconds(pass_start).0;
            notes.extend(self.map_transcription(
                &report.analysis.notes,
                origin,
                stretch,
                source.start,
                pass_start + span,
            ));
            if notes.len() > 200_000 {
                return Err(failure("too many transcribed notes after repeating"));
            }
        }
        Ok(notes)
    }

    /// Creates a new instrument track for a file's note draft, as one undoable edit.
    pub fn create_transcription_track(
        &mut self,
        report: &AudioAnalysis,
        start: Ticks,
        name: &str,
    ) -> Result<(TrackId, ClipId), SessionError> {
        self.validate_transcription(report)?;
        let origin = self.project.tempo_map.ticks_to_seconds(start).0;
        let notes = self.map_transcription(&report.notes, origin, 1.0, start, Ticks(i64::MAX));
        self.create_analysis_track(notes, start, name)
    }

    /// Creates a new instrument track matching the analyzed clip's placement and repeats.
    pub fn create_clip_transcription_track(
        &mut self,
        report: &ClipAudioAnalysis,
        name: &str,
    ) -> Result<(TrackId, ClipId), SessionError> {
        let notes = self.clip_transcription_notes(report)?;
        self.create_analysis_track(notes, report.job.clip.start, name)
    }

    fn create_analysis_track(
        &mut self,
        notes: Vec<Note>,
        start: Ticks,
        name: &str,
    ) -> Result<(TrackId, ClipId), SessionError> {
        if self.transaction.is_some() {
            return Err(SessionError::EditInProgress);
        }
        if start.raw() < 0 || notes.is_empty() {
            return Err(failure("no notes at the requested placement"));
        }
        let length = notes.iter().map(Note::end).max().unwrap_or(Ticks(1));
        self.begin_transaction(Edit::AddInstrumentTrack);
        let outcome = (|| {
            let track = self.add_default_instrument_track(name)?;
            let clip = self.add_midi_clip(track, name, start, length)?;
            self.project
                .midi_clip_mut(clip)
                .expect("clip just inserted")
                .notes = notes;
            Ok((track, clip))
        })();
        if outcome.is_err() {
            self.revert_transaction();
        } else {
            self.end_transaction();
        }
        outcome
    }

    /// Converts audio chord windows through the clip's trim, stretch and repeat placement.
    pub fn audio_chord_report(
        &self,
        report: &ClipAudioAnalysis,
    ) -> Result<ChordAnalysisReport, SessionError> {
        if !self.audio_analysis_is_current(report) {
            return Err(failure("the audio or document changed; analyze it again"));
        }
        let source = &report.job.clip;
        let content = self.audio_clip_length_ticks(source);
        if content.raw() <= 0 || source.loop_end.raw() / content.raw() > 16_384 {
            return Err(failure("invalid or excessive audio repeats"));
        }
        let stretch = source.stretch_in(&self.project.tempo_map);
        let mut segments = Vec::new();
        for (offset, span) in loop_passes(content, source.loop_end) {
            let pass = source.start + offset;
            let origin = self.project.tempo_map.ticks_to_seconds(pass).0;
            for c in &report.analysis.chords {
                let start = self
                    .project
                    .tempo_map
                    .seconds_to_ticks(Seconds(origin + c.start * stretch));
                let end = self
                    .project
                    .tempo_map
                    .seconds_to_ticks(Seconds(origin + c.end * stretch))
                    .min(pass + span);
                if end > start {
                    segments.push(SymbolicChordSegment {
                        start,
                        end,
                        reading: c.reading.clone(),
                    });
                }
                if segments.len() > 16_384 {
                    return Err(failure("too many repeated chord intervals"));
                }
            }
        }
        let to = segments.last().map_or(source.start, |s| s.end);
        Ok(ChordAnalysisReport {
            algorithm: report.analysis.algorithm,
            from: source.start,
            to,
            window: Ticks::ZERO,
            segments,
            fingerprint: report.job.fingerprint.clone(),
        })
    }

    /// Explicitly adopts a tempo candidate as the audio clip's source tempo.
    /// The project's tempo map is preserved.
    pub fn apply_audio_source_tempo(
        &mut self,
        report: &ClipAudioAnalysis,
        candidate: usize,
    ) -> Result<(), SessionError> {
        if !self.audio_analysis_is_current(report) {
            return Err(failure("the audio or document changed; analyze it again"));
        }
        let tempo = report
            .analysis
            .tempo
            .candidates
            .get(candidate)
            .ok_or_else(|| failure("unknown tempo candidate"))?;
        self.set_clip_source_bpm(report.clip, Some(tempo.bpm))
    }

    fn validate_transcription(&self, report: &AudioAnalysis) -> Result<(), SessionError> {
        if !report.options.transcribe
            || !report.seconds.is_finite()
            || report.seconds <= 0.0
            || report.seconds > 1800.0
            || report.notes.is_empty()
            || report.notes.len() > 200_000
        {
            return Err(failure("no usable monophonic transcription"));
        }
        for n in &report.notes {
            if n.pitch > 127
                || !n.start.is_finite()
                || !n.end.is_finite()
                || n.start < 0.0
                || n.end <= n.start
                || n.end > report.seconds + 1e-6
                || !n.strength.is_finite()
            {
                return Err(failure("invalid transcribed note"));
            }
        }
        Ok(())
    }

    fn map_transcription(
        &self,
        events: &[TranscribedNote],
        origin: f64,
        stretch: f64,
        start: Ticks,
        end: Ticks,
    ) -> Vec<Note> {
        events
            .iter()
            .filter_map(|n| {
                let a = self
                    .project
                    .tempo_map
                    .seconds_to_ticks(Seconds(origin + n.start * stretch));
                let b = self
                    .project
                    .tempo_map
                    .seconds_to_ticks(Seconds(origin + n.end * stretch))
                    .min(end);
                (b > a).then(|| Note {
                    velocity: n.strength.clamp(0.1, 1.0),
                    ..Note::new(n.pitch, a - start, b - a)
                })
            })
            .collect()
    }

    fn place_analyzed_notes(
        &mut self,
        track: TrackId,
        start: Ticks,
        name: &str,
        notes: Vec<Note>,
    ) -> Result<ClipId, SessionError> {
        if self.transaction.is_some() {
            return Err(SessionError::EditInProgress);
        }
        self.require_track(track)?;
        if start.raw() < 0
            || notes.is_empty()
            || !self
                .project
                .track(track)
                .is_some_and(|t| t.kind.holds_notes())
        {
            return Err(failure(
                "select a note track and nonnegative placement with usable notes",
            ));
        }
        let length = notes.iter().map(Note::end).max().unwrap_or(Ticks(1));
        self.begin_transaction(Edit::AddClip);
        let result = self.add_midi_clip(track, name, start, length);
        if let Ok(id) = result {
            self.project
                .midi_clip_mut(id)
                .expect("clip just inserted")
                .notes = notes;
        }
        self.end_transaction();
        result
    }
}

fn numeral(chord: Chord, key: Key) -> Numeral {
    let degree = |pitch| {
        let preferred = degree_of(key, pitch);
        // The composer's inverse can name a parallel-major degree with zero alteration.
        // For a saved absolute measurement, check the actual reading: unaltered VI in
        // C minor resolves to Ab, even when the detected pitch was A. Search spellings
        // through the reader itself so bass notes and non-major modes round-trip exactly.
        std::iter::once(preferred)
            .chain(
                [0, -1, 1, -2, 2]
                    .into_iter()
                    .flat_map(|a| (1..=7).map(move |d| (d, a))),
            )
            .find(|(d, a)| {
                let mut n = Numeral::new(*d, false);
                n.accidental = *a;
                n.chord_in(key).root == pitch
            })
            .expect("chromatic pitch has a representable degree")
    };
    let (d, accidental) = degree(chord.root);
    let mut n = Numeral::new(d, chord.quality.is_minor()).with_quality(chord.quality);
    n.accidental = accidental;
    n.bass_degree = chord.bass.map(degree);
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionOptions;
    fn session() -> Session {
        Session::new(SessionOptions::headless()).unwrap()
    }
    #[test]
    fn mixture_groups_are_one_undo_step_and_source_edits_invalidate_the_draft() {
        let mut s = session();
        let rate = s.project().sample_rate;
        let source = s
            .place_audio(
                Path::new("mixture.wav"),
                AudioBuffer::from_planar(vec![vec![0.0; (rate * 2.0) as usize]], rate).unwrap(),
                Ticks::QUARTER,
            )
            .unwrap();
        let job = s
            .audio_analysis_job(source, AudioOptions::default())
            .unwrap();
        let report = ClipMixtureAnalysis {
            source_offset_seconds: 0.0,
            analysis: MixtureAnalysis {
                algorithm: "test-fixture",
                model_license: "CC-BY-NC-4.0",
                model_sha256: "test".into(),
                checkpoint_sha256: "test".into(),
                seconds: 2.0,
                notes: vec![
                    super::mixture::MixtureNote {
                        pitch: 60,
                        start: 0.25,
                        end: 0.75,
                        instrument: "piano".into(),
                    },
                    super::mixture::MixtureNote {
                        pitch: 48,
                        start: 0.25,
                        end: 1.0,
                        instrument: "bass".into(),
                    },
                ],
            },
            job,
        };
        let before = s.project().clone();
        assert!(s.mixture_analysis_is_current(&report));
        let tracks = s.create_clip_mixture_tracks(&report).unwrap();
        assert_eq!(tracks.len(), 2);
        for track in tracks {
            let track = s.project().track(track).unwrap();
            assert!(track.name.starts_with("MuScriptor [NC]"));
            let clip = &track.kind.as_instrument().unwrap().clips[0];
            assert_eq!(clip.start, Ticks::QUARTER);
            assert_eq!(clip.notes[0].start, Ticks::from_beats(0.5));
        }
        assert!(!s.mixture_analysis_is_current(&report));
        s.undo();
        assert_eq!(s.project(), &before);
        assert!(s.mixture_analysis_is_current(&report));
        s.add_default_instrument_track("edit").unwrap();
        assert!(s.create_clip_mixture_tracks(&report).is_err());
    }
    #[test]
    fn chord_report_is_read_only_apply_preserves_outside_and_undo_restores() {
        let mut s = session();
        let track = s.add_default_instrument_track("piano").unwrap();
        let clip = s
            .add_midi_clip(track, "chords", Ticks(960), Ticks(1920))
            .unwrap();
        for p in [60, 64, 67] {
            s.add_note(clip, Note::new(p, Ticks(0), Ticks(1920)))
                .unwrap();
        }
        s.set_chord(Ticks(0), Numeral::parse("V").unwrap());
        let before = s.project().clone();
        let report = s
            .chord_analysis_job(&[], Ticks(960), Ticks(2880), ChordOptions::default())
            .unwrap()
            .run(&AnalysisControl::default())
            .unwrap();
        assert_eq!(s.project(), &before);
        s.apply_chord_analysis(&report).unwrap();
        assert_eq!(s.harmony().chord_at(Ticks(960)), Chord::parse("C"));
        assert_eq!(s.harmony().chord_at(Ticks(2880)), Chord::parse("G"));
        assert_eq!(s.harmony().chord_at(Ticks(0)), Chord::parse("G"));
        s.undo();
        assert_eq!(s.project(), &before);
        s.add_note(clip, Note::new(61, Ticks(0), Ticks(960)))
            .unwrap();
        assert!(s.apply_chord_analysis(&report).is_err());
    }
    #[test]
    fn drum_recipes_are_excluded_and_note_loops_are_expanded() {
        let mut s = session();
        let track = s.add_default_instrument_track("piano").unwrap();
        let clip = s
            .add_midi_clip(track, "loop", Ticks(960), Ticks(960))
            .unwrap();
        for p in [60, 64, 67] {
            s.add_note(clip, Note::new(p, Ticks(0), Ticks(960)))
                .unwrap();
        }
        s.project.midi_clip_mut(clip).unwrap().loop_end = Ticks(2880);
        // Explicit percussion notes on an otherwise pitched track do not supply harmony.
        let mut drum = Note::new(61, Ticks(0), Ticks(960));
        drum.drum_voice = "snare".into();
        s.add_note(clip, drum).unwrap();
        let muted = s
            .add_midi_clip(track, "muted", Ticks(0), Ticks(960))
            .unwrap();
        s.add_note(muted, Note::new(62, Ticks(0), Ticks(960)))
            .unwrap();
        s.project.midi_clip_mut(muted).unwrap().muted = true;
        let r = s
            .chord_analysis_job(&[track], Ticks(0), Ticks(3840), ChordOptions::default())
            .unwrap()
            .run(&AnalysisControl::default())
            .unwrap();
        assert_eq!(r.segments[0].reading.state, ChordState::NoChord);
        assert!(
            r.segments[1..]
                .iter()
                .all(|s| s.reading.candidates[0].symbol == "C")
        );
    }
    #[test]
    fn absolute_chords_survive_conversion_in_every_major_and_minor_key() {
        use auris_core::theory::{chord::Quality, pitch::PitchClass, scale::ScaleId};
        for tonic in 0..12 {
            for mode in [ScaleId::Major, ScaleId::Minor, ScaleId::HarmonicMinor] {
                let key = Key::new(PitchClass::new(tonic), mode);
                for root in 0..12 {
                    for bass in 0..12 {
                        let chord = Chord::new(PitchClass::new(root), Quality::Dominant7)
                            .over(PitchClass::new(bass));
                        let n = numeral(chord, key);
                        assert_eq!(n.chord_in(key), chord, "{key:?}: {n}");
                        assert_eq!(Numeral::parse(&n.to_text()).unwrap().chord_in(key), chord);
                    }
                }
            }
        }
    }
    #[test]
    fn transcription_uses_seconds_through_tempo_changes_and_is_one_edit() {
        let mut s = session();
        let track = s.add_default_instrument_track("draft").unwrap();
        s.set_tempo_point(Ticks(1920), 60.0);
        let before = s.project().clone();
        let mut report = audio::analyze_audio(
            &AudioBuffer::new(1, 11025, audio::ANALYSIS_RATE),
            AudioOptions { transcribe: true },
            &AnalysisControl::default(),
        )
        .unwrap();
        report.notes = vec![TranscribedNote {
            pitch: 69,
            start: 0.2,
            end: 0.8,
            strength: 0.7,
        }];
        let id = s
            .place_transcription(&report, track, Ticks(960), "draft")
            .unwrap();
        let note = &s.project.midi_clip(id).unwrap().1.notes[0];
        let origin = s.project.tempo_map.ticks_to_seconds(Ticks(960)).0;
        assert!(
            (s.project
                .tempo_map
                .ticks_to_seconds(Ticks(960) + note.start)
                .0
                - origin
                - 0.2)
                .abs()
                < 0.002
        );
        assert!(
            (s.project
                .tempo_map
                .ticks_to_seconds(Ticks(960) + note.end())
                .0
                - origin
                - 0.8)
                .abs()
                < 0.002
        );
        s.undo();
        assert_eq!(s.project(), &before);
        report.notes[0].end = f64::NAN;
        assert!(
            s.place_transcription(&report, track, Ticks(960), "invalid")
                .is_err()
        );
        assert_eq!(s.project(), &before);
    }

    #[test]
    fn clip_trim_stretch_repeats_and_staleness_preserve_source_and_undo() {
        let mut s = session();
        let rate = s.project.sample_rate;
        let buffer = AudioBuffer::from_planar(
            vec![
                (0..(rate * 2.0) as usize)
                    .map(|i| {
                        if i as f64 / rate < 0.5 {
                            0.0
                        } else {
                            (std::f64::consts::TAU * 440.0 * i as f64 / rate).sin() as f32 * 0.4
                        }
                    })
                    .collect(),
            ],
            rate,
        )
        .unwrap();
        let id = s
            .place_audio(Path::new("tone.wav"), buffer, Ticks(960))
            .unwrap();
        {
            let clip = s.project.audio_clip_mut(id).unwrap();
            clip.offset_frames = (rate * 0.5) as u64;
            clip.length_frames = rate as u64;
            clip.source_bpm = Some(240.0);
            clip.follows_tempo = true;
            clip.loop_end = Ticks(9600); // two full stretched passes and a half pass
        }
        let before = s.project.clone();
        let report = s
            .audio_analysis_job(id, AudioOptions { transcribe: true })
            .unwrap()
            .run(&AnalysisControl::default())
            .unwrap();
        assert_eq!(report.source_offset_seconds, 0.5);
        assert!((report.analysis.seconds - 1.0).abs() < 0.001);
        assert!(report.analysis.notes.iter().all(|n| n.pitch == 69));
        let (track, draft) = s.create_clip_transcription_track(&report, "draft").unwrap();
        assert_eq!(s.project.tracks.len(), before.tracks.len() + 1);
        let notes = &s.project.midi_clip(draft).unwrap().1.notes;
        assert_eq!(notes.len(), 3);
        assert!(notes[0].start.raw() < 160);
        assert!((notes[1].start.raw() - 3840).abs() < 160);
        assert!(notes[2].end() <= Ticks(9600));
        assert_eq!(s.project.audio_clip(id), before.audio_clip(id));
        s.undo();
        assert_eq!(s.project(), &before);
        assert!(s.project.track(track).is_none());
        assert!(s.audio_analysis_is_current(&report));
        s.set_tempo_point(Ticks(1920), 90.0);
        assert!(s.create_clip_transcription_track(&report, "stale").is_err());
    }

    #[test]
    fn harmony_crossing_a_key_change_keeps_its_absolute_pitches() {
        let mut s = session();
        let track = s.add_default_instrument_track("piano").unwrap();
        let clip = s
            .add_midi_clip(track, "held", Ticks(0), Ticks(3840))
            .unwrap();
        for p in [58, 66, 70, 73, 76] {
            s.add_note(clip, Note::new(p, Ticks(0), Ticks(3840)))
                .unwrap();
        }
        s.set_key(Ticks(1920), Key::parse("F# minor").unwrap());
        let report = s
            .chord_analysis_job(
                &[],
                Ticks(0),
                Ticks(3840),
                ChordOptions {
                    window: Ticks(3840),
                },
            )
            .unwrap()
            .run(&AnalysisControl::default())
            .unwrap();
        s.apply_chord_candidate(&report, 0, 0).unwrap();
        let expected = Chord::parse(&report.segments[0].reading.candidates[0].symbol);
        assert_eq!(s.harmony().chord_at(Ticks(0)), expected);
        assert_eq!(s.harmony().chord_at(Ticks(1920)), expected);
        assert_eq!(s.harmony().chord_at(Ticks(3839)), expected);
    }
}
