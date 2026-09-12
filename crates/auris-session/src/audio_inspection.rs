//! File-free inspection of the rendered sound and authored notes of a document snapshot.

use crate::{RenderJob, Session, prelude::*};
use serde::{Deserialize, Serialize};
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
}

/// Independent render and score snapshot. Run on a worker, never on the UI/audio thread.
pub struct InspectionJob {
    render: RenderJob,
    revision: u64,
    track: Option<TrackId>,
    start: Ticks,
    end: Ticks,
    start_bar: u32,
    bars: u32,
}

impl Session {
    /// Prepare an inspection of 1..8 bars, at most 30 seconds, without altering the document.
    pub fn audio_inspection_job(
        &mut self,
        start_bar: u32,
        bars: u32,
        track: Option<u64>,
    ) -> Result<InspectionJob, String> {
        if start_bar == 0 || !(1..=8).contains(&bars) {
            return Err("Use start_bar >= 1 and bars 1..8".into());
        }
        let end_bar = start_bar.checked_add(bars).ok_or("Bar range overflow")?;
        let project = self.project();
        if track.is_some_and(|id| project.track(TrackId(id)).is_none()) {
            return Err("Unknown track ID".into());
        }
        let start = project.signatures.bar_start(start_bar);
        let end = project.signatures.bar_start(end_bar);
        let duration =
            project.tempo_map.ticks_to_seconds(end).0 - project.tempo_map.ticks_to_seconds(start).0;
        if !duration.is_finite() || duration <= 0.0 || duration > 30.0 {
            return Err("Inspection must fit in 30 seconds; request fewer bars".into());
        }
        if start >= project.end_tick() {
            return Err("The requested range starts after the arrangement".into());
        }
        Ok(InspectionJob {
            revision: self.revision(),
            render: self.render_job(),
            track: track.map(TrackId),
            start,
            end,
            start_bar,
            bars,
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
        let harmony: Vec<_> = (self.start_bar..self.start_bar + self.bars).map(|bar| {
            let tick = project.signatures.bar_start(bar);
            serde_json::json!({"bar":bar, "key":project.harmony.key_at(tick).to_text(), "chord":project.harmony.chord_at(tick).map(|chord| chord.to_string())})
        }).collect();
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
        let mel = auris_dsp::mel::analyse(&audio, cancel)
            .ok_or("Inspection cancelled or unusable audio")?;
        Ok(Inspection {
            revision: self.revision,
            measurements: serde_json::json!({"start_bar":self.start_bar,"bars":self.bars,"start_seconds":start_seconds,"seconds":seconds,"sample_rate":audio.sample_rate(),"peak_dbfs":db(peak),"rms_dbfs":db(rms),"samples_at_or_above_full_scale":clipping,"silent":peak < 0.000001,"track":self.track.map(|id|id.0),"tracks":tracks,"harmony_at_bar_starts":harmony,"score_note_count":total_notes,"score_truncated":total_notes > notes.len(),"score_representation":"authored notes before performance transforms; includes muted score parts","render_representation":"actual mix with mute/solo, or requested track solo routing; no appended tail","mel_representation":"2048-frame Hann FFT; 64 triangular HTK mel filters; average channel power; fixed power reference 1; display -90 to 0 dB"}),
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
    fn inspection_is_a_read_only_snapshot_and_measures_real_gain_and_mute() {
        let mut session =
            Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap();
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
            .audio_inspection_job(1, 1, None)
            .unwrap()
            .run(&cancel)
            .unwrap();
        assert_eq!(session.project(), &before);
        assert!(session.path().is_none());
        assert_eq!(first.notes.len(), 1);
        assert_eq!(first.measurements["silent"], false);
        session.set_param(crate::ParamTarget::TrackGain(track), -12.0);
        let quiet = session
            .audio_inspection_job(1, 1, None)
            .unwrap()
            .run(&cancel)
            .unwrap();
        let delta = first.measurements["rms_dbfs"].as_f64().unwrap()
            - quiet.measurements["rms_dbfs"].as_f64().unwrap();
        assert!((delta - 12.0).abs() < 0.1, "{delta}");
        let old = session.audio_inspection_job(1, 1, None).unwrap();
        session.set_track_mute(track, true).unwrap();
        assert_eq!(old.run(&cancel).unwrap().measurements["silent"], false);
        let silent = session
            .audio_inspection_job(1, 1, None)
            .unwrap()
            .run(&cancel)
            .unwrap();
        assert_eq!(silent.measurements["silent"], true);
        assert!(silent.mel_db.iter().all(|level| *level <= -89.0));
        assert!(session.audio_inspection_job(0, 1, None).is_err());
        assert!(session.audio_inspection_job(1, 9, None).is_err());
        assert!(session.audio_inspection_job(1, 1, Some(u64::MAX)).is_err());
        cancel.store(true, Ordering::Relaxed);
        assert!(
            session
                .audio_inspection_job(1, 1, None)
                .unwrap()
                .run(&cancel)
                .is_err()
        );
    }
}
