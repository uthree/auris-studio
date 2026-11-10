//! Immutable recognition jobs and explicit, undoable acceptance of their drafts.

use crate::{Edit, Session, SessionError};
use auris_analysis::instruments::{self, INSTRUMENT_RATE, InstrumentAnalysis};
use auris_analysis::{
    AnalysisControl,
    audio::{self, AudioAnalysis},
    chords::{self, ChordOptions, ChordState, SymbolicChordSegment},
    transcription::TranscribedNote,
};
use auris_core::{
    AudioBuffer, AudioClip, ClipId, Note, TrackId,
    harmony::{ChordMap, ChordPoint},
    project::loop_passes,
    theory::{chord::Chord, numeral::Numeral},
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

impl AudioAnalysisJob {
    /// Resamples the trimmed first pass, then analyzes it on the CPU.
    ///
    /// Cancellation is checked before/after the existing synchronous resampler and between
    /// analysis frames. Clip repeats are mapped when the draft is placed into the project.
    pub fn run(&self, control: &AnalysisControl) -> Result<ClipAudioAnalysis, SessionError> {
        check_cancel(control)?;
        let prepared = self.prepared(audio::ANALYSIS_RATE)?;
        let analysis = audio::analyze_audio(&prepared, control).map_err(failure)?;
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
    control: &AnalysisControl,
) -> Result<AudioAnalysis, SessionError> {
    check_cancel(control)?;
    let audio = super::decode_audio(path, audio::ANALYSIS_RATE)?;
    audio::analyze_audio(&audio, control).map_err(failure)
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
    fn recognition_fingerprint(&self) -> Result<String, SessionError> {
        let bytes = serde_json::to_vec(&self.project).map_err(failure)?;
        Ok(hex::encode(Sha256::digest(bytes)))
    }

    /// Prepares written notes from selected tracks (empty means all pitched note tracks).
    ///
    /// Reads stored pitches, clipping and repeats, without playback humanization. Muted clips,
    /// muted tracks, drum tracks, known drum instruments, percussion presets and drum recipes
    /// are skipped.
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
            if (!tracks.is_empty() && !tracks.contains(&track.id))
                || track.mixer.mute
                || track.kind.is_drum()
            {
                continue;
            }
            if self.track_preset(track.id).is_some_and(|p| p.bank == 128)
                || track
                    .kind
                    .as_instrument()
                    .is_some_and(|t| auris_synth::is_drum_kit(&t.instrument_id))
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
                    chord: chord
                        .map(|c| Numeral::from_chord_in(c, self.project.harmony.key_at(tick))),
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
    pub fn audio_analysis_job(&self, clip: ClipId) -> Result<AudioAnalysisJob, SessionError> {
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

    fn create_analysis_track(
        &mut self,
        notes: Vec<Note>,
        start: Ticks,
        name: &str,
        instrument: Option<&str>,
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
            let track = match instrument {
                Some(id) => self.add_instrument_track(name, id)?,
                None => self.add_default_instrument_track(name)?,
            };
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

    /// Adds a recognizer-independent draft as one new physical-piano track and undo step.
    /// Validates all events before editing; preserves existing music, tempo and source timing.
    pub fn import_transcription(
        &mut self,
        draft: &crate::NoteTranscription,
        start: Ticks,
        name: &str,
    ) -> Result<(TrackId, ClipId), SessionError> {
        draft.validate().map_err(failure)?;
        self.import_transcribed_notes(&draft.notes, start, name)
    }

    fn import_transcribed_notes(
        &mut self,
        source: &[TranscribedNote],
        start: Ticks,
        name: &str,
    ) -> Result<(TrackId, ClipId), SessionError> {
        if name.trim().is_empty() || name.len() > 256 || start.raw() < 0 {
            return Err(failure(
                "use a nonempty name up to 256 bytes and a nonnegative start",
            ));
        }
        let origin = self.project.tempo_map.ticks_to_seconds(start).0;
        let mut notes = self.map_transcription(source, origin, 1.0, start, Ticks(i64::MAX));
        if notes.len() != source.len() {
            return Err(failure(
                "some note intervals collapse at the project's tick resolution; lengthen or remove them explicitly before importing",
            ));
        }
        for (note, event) in notes.iter_mut().zip(source) {
            note.velocity = event.strength;
        }
        self.create_analysis_track(
            notes,
            start,
            name,
            Some(crate::registry::DEFAULT_INSTRUMENT),
        )
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionOptions;
    use auris_core::theory::key::Key;
    fn session() -> Session {
        Session::new(SessionOptions::headless()).unwrap()
    }

    #[test]
    fn external_transcription_preserves_source_timing_and_is_one_undo() {
        let mut s = session();
        s.add_default_instrument_track("Existing").unwrap();
        s.set_bpm(90.0);
        let before = s.project().clone();
        let command = serde_json::from_value(serde_json::json!({
            "action":"import_transcription", "name":"External draft", "at_tick":3840,
            "draft":{"kind":"piano","transcription":{
                "source":{"method":"codex.harmonic-fit","model":"gpt-6.1-sol",
                    "artifact_sha256":"1ad425dd3758b046d23c3e3a11bee0ce2815402fda64fc69f77a9cdd873c08b5"},
                "seconds":2.0, "limitations":"Uniform dynamics; review missing notes.",
                "notes":[
                    {"pitch":60,"start":0.25,"end":1.5,"strength":0.7},
                    {"pitch":64,"start":0.25,"end":1.5,"strength":0.7},
                    {"pitch":67,"start":0.25,"end":1.5,"strength":0.7}]
            }}
        })).unwrap();
        assert_eq!(
            serde_json::to_value(&command).unwrap()["draft"]["transcription"]["source"]["method"],
            "codex.harmonic-fit"
        );
        let result: serde_json::Value =
            serde_json::from_str(&s.agent_command(command).unwrap()).unwrap();
        assert_eq!(result["notes"], 3);
        assert_eq!(s.project().tempo_map, before.tempo_map);
        assert_eq!(s.project().tracks[0], before.tracks[0]);
        let (_, clip) = s
            .project()
            .midi_clip(ClipId(result["created"][0]["clip"].as_u64().unwrap()))
            .unwrap();
        assert_eq!(clip.start, Ticks::from_beats(4.0));
        assert_eq!(clip.notes.len(), 3);
        let origin = s.project().tempo_map.ticks_to_seconds(clip.start).0;
        for note in &clip.notes {
            assert!(
                (s.project()
                    .tempo_map
                    .ticks_to_seconds(clip.start + note.start)
                    .0
                    - origin
                    - 0.25)
                    .abs()
                    < 0.002
            );
        }
        s.undo();
        assert_eq!(s.project(), &before);
    }

    #[test]
    fn external_drafts_keep_weak_dynamics_and_refuse_silent_sub_tick_omissions() {
        let mut s = session();
        let mut draft: crate::NoteTranscription = serde_json::from_value(serde_json::json!({
            "source":{"method":"external"},"seconds":1,
            "notes":[{"pitch":60,"start":0,"end":0.5,"strength":0.01}]
        }))
        .unwrap();
        let before = s.project().clone();
        let (_, clip) = s
            .import_transcription(&draft, Ticks::ZERO, "Quiet")
            .unwrap();
        assert_eq!(
            s.project().midi_clip(clip).unwrap().1.notes[0].velocity,
            0.01
        );
        s.undo();
        assert_eq!(s.project(), &before);
        draft.notes.push(TranscribedNote {
            pitch: 64,
            start: 0.0,
            end: 0.000001,
            strength: 0.7,
        });
        assert!(
            s.import_transcription(&draft, Ticks::ZERO, "Too short")
                .unwrap_err()
                .to_string()
                .contains("tick resolution")
        );
        assert_eq!(s.project(), &before);
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
    fn drum_tracks_never_supply_harmony_even_without_percussion_metadata() {
        let mut s = session();
        let piano = s.add_default_instrument_track("Piano").unwrap();
        let builtin = s.add_default_drum_track("Kit").unwrap();
        let custom = s
            .add_drum_track("Custom kit", "auris.synth.chiptune")
            .unwrap();
        let hosted = s.project.add_drum_track("Hosted kit", "clap.custom");
        for track in [piano, builtin, custom, hosted] {
            let clip = s
                .add_midi_clip(track, "Manual", Ticks::ZERO, Ticks::QUARTER)
                .unwrap();
            let pitches = if track == piano {
                [62, 66, 69]
            } else {
                [60, 64, 67]
            };
            for pitch in pitches {
                s.add_note(clip, Note::new(pitch, Ticks::ZERO, Ticks::QUARTER))
                    .unwrap();
            }
        }
        for track in [builtin, custom, hosted] {
            let report = s
                .chord_analysis_job(&[track], Ticks::ZERO, Ticks::QUARTER, Default::default())
                .unwrap()
                .run(&AnalysisControl::default())
                .unwrap();
            assert_eq!(report.segments.len(), 1);
            assert_eq!(report.segments[0].reading.state, ChordState::NoChord);
        }
        let report = s
            .chord_analysis_job(&[], Ticks::ZERO, Ticks::QUARTER, Default::default())
            .unwrap()
            .run(&AnalysisControl::default())
            .unwrap();
        assert_eq!(report.segments.len(), 1);
        assert_eq!(report.segments[0].reading.candidates[0].symbol, "D");
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
                        let n = Numeral::from_chord_in(chord, key);
                        assert_eq!(n.chord_in(key), chord, "{key:?}: {n}");
                        assert_eq!(Numeral::parse(&n.to_text()).unwrap().chord_in(key), chord);
                    }
                }
            }
        }
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
