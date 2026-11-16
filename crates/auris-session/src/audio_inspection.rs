//! File-free inspection of the rendered sound and authored notes of a document snapshot.

use crate::{RenderJob, Session, prelude::*};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};

/// An authored note intersecting the inspected range (before performance transforms).
#[derive(Debug, Serialize, Deserialize)]
pub struct ScoreNote {
    /// Stable track ID.
    pub track: u64,
    /// MIDI pitch.
    pub pitch: u8,
    /// Seconds relative to the inspected range.
    pub start_seconds: f64,
    /// Seconds relative to the inspected range.
    pub end_seconds: f64,
}

/// The longest stretch one inspection renders; the picture has 512 columns whatever the length.
pub const MAX_SECONDS: f64 = 60.0;
/// The most bars an explicit `start_bar`/`bars` range may span.
pub const MAX_BARS: u32 = 32;
/// The fewest semitones the fitted piano roll shows.
const MIN_FITTED_SEMITONES: u8 = 24;
/// The narrowest mel panel, in hertz.
const MIN_HZ_SPAN: f64 = 100.0;

/// What one inspection measures and how its picture is framed. Every field is optional: with
/// nothing given it is the whole song (if it fits in [`MAX_SECONDS`]), the mix, a piano roll
/// fitted to the notes and the full mel range.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InspectionRequest {
    /// A section label from the song's sections, matched exactly after trimming.
    pub section: Option<String>,
    /// Which occurrence of `section`, from 1; omitted, the first.
    pub instance: Option<usize>,
    /// First bar of an explicit range, from 1; given together with `bars`.
    pub start_bar: Option<u32>,
    /// Bars in the explicit range, 1..=[`MAX_BARS`].
    pub bars: Option<u32>,
    /// Solo routing of one track; `None` measures the mix.
    pub track: Option<u64>,
    /// Lowest MIDI pitch the piano roll shows.
    pub low_pitch: Option<u8>,
    /// Highest MIDI pitch the piano roll shows.
    pub high_pitch: Option<u8>,
    /// Lowest frequency of the mel panel.
    pub low_hz: Option<f64>,
    /// Highest frequency of the mel panel.
    pub high_hz: Option<f64>,
}

impl InspectionRequest {
    /// The mix over `bars` bars from `start_bar`, framed by default.
    pub fn bars(start_bar: u32, bars: u32) -> Self {
        Self {
            start_bar: Some(start_bar),
            bars: Some(bars),
            ..Self::default()
        }
    }
}

fn top_pitch() -> u8 {
    127
}

/// Measured render data, ready for presentation without opening or writing any files.
#[derive(Debug, Serialize, Deserialize)]
pub struct Inspection {
    /// Document revision captured before rendering.
    pub revision: u64,
    /// Range and measurement metadata, distinct from subjective listening judgements.
    pub measurements: serde_json::Value,
    /// Number of equally spaced time columns.
    pub columns: usize,
    /// Low-to-high HTK mel centre frequencies.
    pub frequencies: Vec<f64>,
    /// Column-major power dB, fixed reference 1, floor -90 dB.
    pub mel_db: Vec<f32>,
    /// Authored notes, bounded to 1024; metadata reports truncation.
    pub notes: Vec<ScoreNote>,
    /// Lowest MIDI pitch of the piano roll panel.
    #[serde(default)]
    pub low_pitch: u8,
    /// Highest MIDI pitch of the piano roll panel.
    #[serde(default = "top_pitch")]
    pub high_pitch: u8,
}

/// Independent render and score snapshot. Run on a worker, never on the UI/audio thread.
pub struct InspectionJob {
    render: RenderJob,
    revision: u64,
    track: Option<TrackId>,
    start: Ticks,
    end: Ticks,
    start_bar: u32,
    end_bar: u32,
    section: Option<(String, usize, usize)>,
    pitches: (Option<u8>, Option<u8>),
    hz: (f64, f64),
}

/// The piano roll's pitch span: whole octaves from C around `notes`, at least two of them, with
/// either end replaced by the one the caller asked for.
fn pitch_frame(
    notes: Option<(u8, u8)>,
    low: Option<u8>,
    high: Option<u8>,
) -> Result<(u8, u8), String> {
    let (mut fitted_low, mut fitted_high) = match notes {
        Some((lowest, highest)) => (lowest / 12 * 12, (highest / 12 * 12 + 11).min(127)),
        None => (0, 127),
    };
    while fitted_high - fitted_low + 1 < MIN_FITTED_SEMITONES {
        if fitted_low >= 12 {
            fitted_low -= 12;
        } else {
            fitted_high = (fitted_high + 12).min(127);
        }
    }
    let (low, high) = match (low, high) {
        (Some(low), Some(high)) => (low, high),
        (Some(low), None) => (low, fitted_high.max(low.saturating_add(11).min(127))),
        (None, Some(high)) => (fitted_low.min(high.saturating_sub(11)), high),
        (None, None) => (fitted_low, fitted_high),
    };
    if high > 127 || low >= high {
        return Err("Use low_pitch < high_pitch, both 0..127".into());
    }
    Ok((low, high))
}

pub(crate) fn bar_range(
    project: &Project,
    start_bar: u32,
    bars: u32,
) -> Result<(Ticks, Ticks), String> {
    if start_bar == 0 || !(1..=8).contains(&bars) {
        return Err("Use start_bar >= 1 and bars 1..8".into());
    }
    let end_bar = start_bar.checked_add(bars).ok_or("Bar range overflow")?;
    let start = project.signatures.bar_start(start_bar);
    let end = project.signatures.bar_start(end_bar);
    let duration =
        project.tempo_map.ticks_to_seconds(end).0 - project.tempo_map.ticks_to_seconds(start).0;
    if !duration.is_finite() || duration <= 0.0 || duration > 30.0 {
        return Err("Audio range must fit in 30 seconds; request fewer bars".into());
    }
    if start >= project.end_tick() {
        return Err("The requested range starts after the arrangement".into());
    }
    Ok((start, end))
}

fn harmony_events(project: &Project, start: Ticks, end: Ticks) -> (Vec<serde_json::Value>, usize) {
    // Include the in-force harmony at the left edge, then every key/chord change. A key
    // change can alter the sounding chord without adding a new numeral to the chord map.
    let ticks: BTreeSet<_> = std::iter::once(start)
        .chain(project.harmony.keys.points().iter().map(|point| point.tick))
        .chain(
            project
                .harmony
                .chords
                .points()
                .iter()
                .map(|point| point.tick),
        )
        .filter(|tick| *tick >= start && *tick < end)
        .collect();
    let count = ticks.len();
    let start_seconds = project.tempo_map.ticks_to_seconds(start).0;
    let events = ticks
        .into_iter()
        .take(128)
        .map(|tick| {
            let (bar, beat, tick_in_beat) = project.signatures.bar_beat_at(tick);
            serde_json::json!({"tick":tick.raw(),"bar":bar,"beat":beat,"tick_in_beat":tick_in_beat,
            "start_seconds":project.tempo_map.ticks_to_seconds(tick).0 - start_seconds,
            "key":project.harmony.key_at(tick).to_text(),
            "chord":project.harmony.chord_at(tick).map(|chord|chord.to_string())})
        })
        .collect();
    (events, count)
}

impl Session {
    /// Prepare an inspection of the requested range, at most [`MAX_SECONDS`], without altering
    /// the document.
    pub fn audio_inspection_job(
        &mut self,
        request: &InspectionRequest,
    ) -> Result<InspectionJob, String> {
        let project = self.project();
        if request
            .track
            .is_some_and(|id| project.track(TrackId(id)).is_none())
        {
            return Err("Unknown track ID".into());
        }
        let regions = crate::modulation::select_regions(
            project,
            &crate::modulation::RegionQuery {
                section: request.section.as_deref(),
                instance: request.instance.or(request.section.as_ref().map(|_| 1)),
                start_bar: request.start_bar,
                bars: request.bars,
                each_section: false,
                max_bars: MAX_BARS,
            },
        )?;
        let region = regions.into_iter().next().ok_or("Nothing to inspect")?;
        let duration = project.tempo_map.ticks_to_seconds(region.end).0
            - project.tempo_map.ticks_to_seconds(region.start).0;
        if !duration.is_finite() || duration <= 0.0 {
            return Err("The requested range is empty".into());
        }
        if duration > MAX_SECONDS {
            let what = match (&region.section, request.start_bar) {
                (Some(label), _) => format!("Section {label:?}"),
                (None, Some(_)) => "The requested range".to_owned(),
                (None, None) => "The whole song".to_owned(),
            };
            return Err(format!(
                "{what} lasts {duration:.0} seconds; inspect at most {MAX_SECONDS:.0} seconds at a time with start_bar and bars"
            ));
        }
        let low_hz = request.low_hz.unwrap_or(0.0);
        let high_hz = request.high_hz.unwrap_or(auris_dsp::mel::TOP_HZ);
        if !(0.0..=auris_dsp::mel::TOP_HZ).contains(&low_hz)
            || !(high_hz <= auris_dsp::mel::TOP_HZ && high_hz - low_hz >= MIN_HZ_SPAN)
        {
            return Err(format!(
                "Use 0 <= low_hz < high_hz <= {:.0}, at least {MIN_HZ_SPAN:.0} Hz apart",
                auris_dsp::mel::TOP_HZ
            ));
        }
        pitch_frame(None, request.low_pitch, request.high_pitch)?;
        let section = match &region.section {
            Some(label) => {
                let occurrences = project
                    .sections
                    .spans_in(Ticks::ZERO, project.end_tick())
                    .iter()
                    .filter(|span| &span.label == label)
                    .count();
                Some((label.clone(), region.instance.unwrap_or(1), occurrences))
            }
            None => None,
        };
        let start_bar = project.signatures.bar_beat_at(region.start).0;
        let end_bar = project
            .signatures
            .bar_beat_at(Ticks(region.end.raw() - 1).max(region.start))
            .0
            + 1;
        Ok(InspectionJob {
            revision: self.revision(),
            render: self.render_job_at(Some(48_000.0)),
            track: request.track.map(TrackId),
            start: region.start,
            end: region.end,
            start_bar,
            end_bar,
            section,
            pitches: (request.low_pitch, request.high_pitch),
            hz: (low_hz, high_hz),
        })
    }
}

impl InspectionJob {
    /// Render the selected range and collect channel-safe mel power, levels and score data.
    pub fn run(mut self, cancel: &AtomicBool) -> Result<Inspection, String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Inspection cancelled".into());
        }
        self.render.validate_complete().map_err(|e| e.to_string())?;
        let project = self.render.project();
        let start_seconds = project.tempo_map.ticks_to_seconds(self.start).0;
        let seconds = project.tempo_map.ticks_to_seconds(self.end).0 - start_seconds;
        let mut notes = Vec::new();
        let mut total_notes = 0;
        let mut pitch_span: Option<(u8, u8)> = None;
        let mut tracks = Vec::new();
        for track in &project.tracks {
            if self.track.is_some_and(|id| track.id != id) {
                continue;
            }
            if tracks.len() < 64 {
                tracks.push(serde_json::json!({"id":track.id.0,"name":track.name,"mute":track.mixer.mute,"solo":track.mixer.solo}));
            }
            for clip in track.kind.note_clips().into_iter().flatten() {
                for note in &clip.notes {
                    let start = (clip.start + note.start).max(self.start);
                    let end = (clip.start + note.start + note.length)
                        .min(clip.end())
                        .min(self.end);
                    if start >= end {
                        continue;
                    }
                    total_notes += 1;
                    pitch_span =
                        Some(pitch_span.map_or((note.pitch, note.pitch), |(low, high)| {
                            (low.min(note.pitch), high.max(note.pitch))
                        }));
                    if notes.len() < 1024 {
                        notes.push(ScoreNote {
                            track: track.id.0,
                            pitch: note.pitch,
                            start_seconds: project.tempo_map.ticks_to_seconds(start).0
                                - start_seconds,
                            end_seconds: project.tempo_map.ticks_to_seconds(end).0 - start_seconds,
                        });
                    }
                }
            }
        }
        let harmony: Vec<_> = (self.start_bar..self.end_bar).map(|bar| {
            let tick = project.signatures.bar_start(bar);
            serde_json::json!({"bar":bar, "key":project.harmony.key_at(tick).to_text(), "chord":project.harmony.chord_at(tick).map(|chord| chord.to_string())})
        }).collect();
        let (harmony_events, harmony_event_count) = harmony_events(project, self.start, self.end);
        let options = OfflineOptions {
            start_frames: (start_seconds * 48000.0).round() as u64,
            end_frames: Some(((start_seconds + seconds) * 48000.0).round() as u64),
            sample_rate: Some(48000.0),
            include_tail: false,
            ..OfflineOptions::default()
        };
        let audio = self
            .render
            .render_target(
                self.track,
                &options,
                &mut RenderProgress::default().cancelled_by(cancel),
            )
            .map_err(|e| e.to_string())?;
        let mut peak = 0.0_f64;
        let mut sum = 0.0_f64;
        let mut samples = 0_usize;
        let mut clipping = 0_usize;
        for channel in audio.iter_channels() {
            for &sample in channel {
                if !sample.is_finite() {
                    return Err("Render contains non-finite audio samples".into());
                }
                let value = f64::from(sample);
                peak = peak.max(value.abs());
                sum += value * value;
                samples += 1;
                clipping += usize::from(value.abs() >= 1.0);
            }
        }
        let rms = (sum / samples.max(1) as f64).sqrt();
        let db = |value: f64| (value > 0.0).then(|| 20.0 * value.log10());
        let mel = auris_dsp::mel::analyse_between(&audio, self.hz.0, self.hz.1, cancel)
            .ok_or("Inspection cancelled or unusable audio")?;
        let (low_pitch, high_pitch) = pitch_frame(pitch_span, self.pitches.0, self.pitches.1)?;
        let outside = notes
            .iter()
            .filter(|note| note.pitch < low_pitch || note.pitch > high_pitch)
            .count();
        let (section, instance, occurrences) = match &self.section {
            Some((label, instance, count)) => (Some(label.as_str()), Some(*instance), Some(*count)),
            None => (None, None, None),
        };
        Ok(Inspection {
            revision: self.revision,
            low_pitch,
            high_pitch,
            measurements: serde_json::json!({"start_bar":self.start_bar,"bars":self.end_bar - self.start_bar,"end_bar":self.end_bar,"start_tick":self.start.raw(),"end_tick":self.end.raw(),"section":section,"instance":instance,"section_occurrences":occurrences,"piano_roll_pitches":[low_pitch,high_pitch],"notes_outside_piano_roll":outside,"mel_hz_range":[self.hz.0,self.hz.1.min(audio.sample_rate() / 2.0)],"start_seconds":start_seconds,"seconds":seconds,"sample_rate":audio.sample_rate(),"peak_dbfs":db(peak),"rms_dbfs":db(rms),"samples_at_or_above_full_scale":clipping,"silent":peak < 0.000001,"track":self.track.map(|id|id.0),"tracks":tracks,"harmony_at_bar_starts":harmony,"harmony_events":harmony_events,"harmony_event_count":harmony_event_count,"harmony_events_truncated":harmony_event_count > harmony_events.len(),"score_note_count":total_notes,"score_truncated":total_notes > notes.len(),"score_representation":"authored notes before performance transforms; includes muted score parts","render_representation":"actual mix with mute/solo, or requested track solo routing; no appended tail","mel_representation":"2048-frame Hann FFT; 64 triangular HTK mel filters; average channel power; fixed power reference 1; display -90 to 0 dB"}),
            columns: mel.columns,
            frequencies: mel.frequencies,
            mel_db: mel.levels,
            notes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harmony_event_limit_counts_changes_without_rounding_or_duplicates() {
        let session = Session::new(crate::SessionOptions::headless()).unwrap();
        let mut project = session.project().clone();
        // Alternating, because a chord restating the one before it is not a change.
        let numerals = [Numeral::parse("I").unwrap(), Numeral::parse("IV").unwrap()];
        for index in 0..200 {
            project
                .harmony
                .chords
                .set_point(Ticks(index * 15), Some(numerals[index as usize % 2]));
        }
        project
            .harmony
            .keys
            .set_point(Ticks(15), MusicalKey::parse("D major").unwrap());
        let (events, count) = harmony_events(&project, Ticks::ZERO, Ticks(200 * 15));
        assert_eq!(count, 200);
        assert_eq!(events.len(), 128);
        assert_eq!(events[1]["tick_in_beat"], 15);
        assert_eq!(events[1]["key"], "D major");
        assert_eq!(events[127]["tick"], 127 * 15);
        let (events, count) = harmony_events(&project, Ticks(2980), Ticks(2985));
        assert_eq!(count, 1);
        assert_eq!(events[0]["tick"], 2980);
        assert_eq!(events[0]["chord"], "D");
    }

    #[test]
    fn inspection_includes_within_bar_harmony_and_carries_the_starting_chord() {
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Harmony").unwrap();
        session
            .add_midi_clip(track, "Range", Ticks::ZERO, Ticks::QUARTER * 16)
            .unwrap();
        session.stamp_progression(
            &Chart::parse("| I | IV V | vi | ii V |").unwrap(),
            Ticks::ZERO,
            4,
        );
        session.remove_chord(Ticks::QUARTER * 4);
        session.set_key(Ticks::QUARTER * 7, MusicalKey::parse("D major").unwrap());
        let before = session.project().clone();
        let job = session
            .audio_inspection_job(&InspectionRequest::bars(2, 2))
            .unwrap();
        // The inspection must retain its snapshot even if the live harmony changes.
        session.clear_harmony(Ticks::ZERO, Ticks::QUARTER * 16);
        let report = job.run(&AtomicBool::new(false)).unwrap().measurements;
        let events = report["harmony_events"].as_array().expect("harmony events");
        assert_eq!(report["harmony_event_count"], 4);
        assert_eq!(report["harmony_events_truncated"], false);
        for (event, (tick, bar, beat, seconds, chord)) in events.iter().zip([
            (3840, 2, 1, 0.0, "C"),
            (5760, 2, 3, 1.0, "G"),
            (6720, 2, 4, 1.5, "A"),
            (7680, 3, 1, 2.0, "Bm"),
        ]) {
            assert_eq!(event["tick"], tick);
            assert_eq!(event["bar"], bar);
            assert_eq!(event["beat"], beat);
            assert_eq!(event["tick_in_beat"], 0);
            assert_eq!(event["start_seconds"], seconds);
            assert_eq!(event["chord"], chord);
        }
        assert_eq!(events[0]["key"], "C major");
        assert_eq!(events[2]["key"], "D major");
        assert_eq!(report["harmony_at_bar_starts"][0]["chord"], "C");
        assert_eq!(before.tracks, session.project().tracks);
    }

    #[test]
    fn inspection_is_a_read_only_snapshot_and_measures_real_gain_and_mute() {
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session
            .set_track_instrument(track, "auris.synth.fm2")
            .unwrap();
        let clip = session
            .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::from_beats(3.0)))
            .unwrap();
        let before = session.project().clone();
        let cancel = AtomicBool::new(false);
        let first = session
            .audio_inspection_job(&InspectionRequest::bars(1, 1))
            .unwrap()
            .run(&cancel)
            .unwrap();
        assert_eq!(session.project(), &before);
        assert!(session.path().is_none());
        assert_eq!(first.notes.len(), 1);
        assert_eq!(first.measurements["silent"], false);
        session.set_param(crate::ParamTarget::TrackGain(track), -12.0);
        let quiet = session
            .audio_inspection_job(&InspectionRequest::bars(1, 1))
            .unwrap()
            .run(&cancel)
            .unwrap();
        let delta = first.measurements["rms_dbfs"].as_f64().unwrap()
            - quiet.measurements["rms_dbfs"].as_f64().unwrap();
        assert!((delta - 12.0).abs() < 0.1, "{delta}");
        let old = session
            .audio_inspection_job(&InspectionRequest::bars(1, 1))
            .unwrap();
        session.set_track_mute(track, true).unwrap();
        assert_eq!(old.run(&cancel).unwrap().measurements["silent"], false);
        let silent = session
            .audio_inspection_job(&InspectionRequest::bars(1, 1))
            .unwrap()
            .run(&cancel)
            .unwrap();
        assert_eq!(silent.measurements["silent"], true);
        assert!(silent.mel_db.iter().all(|level| *level <= -89.0));
        assert!(
            session
                .audio_inspection_job(&InspectionRequest::bars(0, 1))
                .is_err()
        );
        assert!(
            session
                .audio_inspection_job(&InspectionRequest::bars(1, MAX_BARS + 1))
                .is_err()
        );
        assert!(
            session
                .audio_inspection_job(&InspectionRequest {
                    track: Some(u64::MAX),
                    ..InspectionRequest::bars(1, 1)
                })
                .is_err()
        );
        cancel.store(true, Ordering::Relaxed);
        assert!(
            session
                .audio_inspection_job(&InspectionRequest::bars(1, 1))
                .unwrap()
                .run(&cancel)
                .is_err()
        );
    }

    fn sectioned_song() -> (Session, TrackId) {
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Song", Ticks::ZERO, Ticks::QUARTER * 32)
            .unwrap();
        for (bar, pitch) in [(0, 64), (2, 67), (4, 62), (6, 72)] {
            session
                .add_note(
                    clip,
                    Note::new(pitch, Ticks::QUARTER * (bar * 4), Ticks::QUARTER),
                )
                .unwrap();
        }
        for (beat, label) in [(0, "Aメロ"), (8, "サビ"), (16, "Aメロ"), (24, "サビ")] {
            session.set_section(Ticks::QUARTER * beat, Some(label.into()));
        }
        (session, track)
    }

    #[test]
    fn a_section_is_inspected_by_name_and_occurrence_and_the_whole_song_by_default() {
        let (mut session, _) = sectioned_song();
        let cancel = AtomicBool::new(false);
        let chorus = |instance| InspectionRequest {
            section: Some("サビ".into()),
            instance,
            ..InspectionRequest::default()
        };
        let first = session
            .audio_inspection_job(&chorus(None))
            .unwrap()
            .run(&cancel)
            .unwrap();
        assert_eq!(first.measurements["start_bar"], 3);
        assert_eq!(first.measurements["end_bar"], 5);
        assert_eq!(first.measurements["bars"], 2);
        assert_eq!(first.measurements["instance"], 1);
        assert_eq!(first.measurements["section_occurrences"], 2);
        assert_eq!(first.notes.len(), 1);
        assert_eq!(first.notes[0].pitch, 67);
        let second = session
            .audio_inspection_job(&chorus(Some(2)))
            .unwrap()
            .run(&cancel)
            .unwrap();
        assert_eq!(second.measurements["start_bar"], 7);
        assert_eq!(second.notes[0].pitch, 72);
        let whole = session
            .audio_inspection_job(&InspectionRequest::default())
            .unwrap()
            .run(&cancel)
            .unwrap();
        assert_eq!(whole.measurements["start_bar"], 1);
        assert_eq!(whole.measurements["bars"], 8);
        assert_eq!(whole.notes.len(), 4);
        assert!(session.audio_inspection_job(&chorus(Some(3))).is_err());
        let both = InspectionRequest {
            start_bar: Some(1),
            ..chorus(None)
        };
        assert!(session.audio_inspection_job(&both).is_err());
        assert!(
            session
                .audio_inspection_job(&InspectionRequest::bars(1, MAX_BARS + 1))
                .is_err()
        );
    }

    #[test]
    fn a_range_longer_than_a_minute_is_refused_with_a_way_forward() {
        let (mut session, _) = sectioned_song();
        session.set_tempo_at(Ticks::ZERO, 30.0);
        let error = session
            .audio_inspection_job(&InspectionRequest::default())
            .err()
            .unwrap();
        assert!(
            error.contains("whole song") && error.contains("start_bar"),
            "{error}"
        );
        assert!(
            session
                .audio_inspection_job(&InspectionRequest::bars(1, 4))
                .is_ok()
        );
    }

    #[test]
    fn the_piano_roll_fits_the_notes_unless_told_otherwise() {
        assert_eq!(pitch_frame(Some((62, 72)), None, None), Ok((60, 83)));
        assert_eq!(pitch_frame(Some((60, 61)), None, None), Ok((48, 71)));
        assert_eq!(pitch_frame(Some((2, 5)), None, None), Ok((0, 23)));
        assert_eq!(pitch_frame(Some((120, 127)), None, None), Ok((96, 127)));
        assert_eq!(pitch_frame(None, None, None), Ok((0, 127)));
        assert_eq!(pitch_frame(Some((62, 72)), Some(64), None), Ok((64, 83)));
        assert_eq!(pitch_frame(Some((62, 72)), Some(80), None), Ok((80, 91)));
        assert_eq!(pitch_frame(Some((62, 72)), None, Some(66)), Ok((55, 66)));
        assert!(pitch_frame(None, Some(70), Some(60)).is_err());
        assert!(pitch_frame(None, None, Some(200)).is_err());
    }

    #[test]
    fn the_frame_reaches_the_report() {
        let (mut session, _) = sectioned_song();
        let report = session
            .audio_inspection_job(&InspectionRequest {
                high_pitch: Some(70),
                low_hz: Some(100.0),
                high_hz: Some(2000.0),
                ..InspectionRequest::default()
            })
            .unwrap()
            .run(&AtomicBool::new(false))
            .unwrap();
        assert_eq!((report.low_pitch, report.high_pitch), (59, 70));
        assert_eq!(report.measurements["notes_outside_piano_roll"], 1);
        assert_eq!(report.measurements["mel_hz_range"][1], 2000.0);
        assert!(report.frequencies[0] > 100.0 && report.frequencies[63] < 2000.0);
        let bad_hz = InspectionRequest {
            low_hz: Some(500.0),
            high_hz: Some(550.0),
            ..InspectionRequest::default()
        };
        assert!(session.audio_inspection_job(&bad_hz).is_err());
    }
}
