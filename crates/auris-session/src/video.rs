//! Exporting the song as a note-roll video: the notes flowing past a playhead over the mix.
//!
//! The drawing and the encoding are [`auris_video`]'s; what is here is the part only a session
//! can do — take a consistent snapshot of the document and render its audio — and the one place
//! every frontend reaches the rest through.
//!
//! The encoder is an `ffmpeg` already installed on the machine, never one Auris carries. A
//! frontend asks [`Ffmpeg::detect`] before it offers the command, and points a person at
//! [`ffmpeg_install_command`] when nothing is found.

use std::path::Path;

pub use auris_video::{
    BackgroundImage, Ffmpeg, FrameRenderer, NoteColoring, NoteRollStyle, NoteScore, ScoreChord,
    VideoCodec, VideoError, VideoSize,
};

use auris_analysis::AnalysisControl;
use auris_analysis::chords::{ChordOptions, ChordState};
use auris_core::theory::chord::Chord;

use crate::prelude::*;
use crate::{RenderJob, Session, SessionError};

/// The command that installs ffmpeg with this platform's usual package manager.
pub fn ffmpeg_install_command() -> &'static str {
    auris_video::install_command()
}

/// Where ffmpeg can be downloaded by hand.
pub fn ffmpeg_install_page() -> &'static str {
    auris_video::install_page()
}

/// What a video export produced.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct VideoSummary {
    /// Length in seconds, the effect tail included.
    pub seconds: f64,
    /// Frames encoded.
    pub frames: u64,
    /// The encoder ffmpeg was asked to use.
    pub codec: VideoCodec,
}

/// Everything a video export needs, captured from the session so it can run on another thread
/// while the document goes on being edited — the same arrangement as [`RenderJob`], which it
/// carries for the soundtrack.
pub struct VideoJob {
    audio: RenderJob,
    score: NoteScore,
    style: NoteRollStyle,
}

impl Session {
    /// The notes a video of the open document would draw, and the chords to caption them with.
    ///
    /// The chords are the ones written into the harmony. A song with none written gets the
    /// chords its pitched notes spell instead, read beat by beat by the same recogniser as
    /// *Detect Chords*, so the overlay works on a song nobody has charted — and a chart, once
    /// written, always wins over a guess.
    pub fn note_score(&self) -> NoteScore {
        let mut score = NoteScore::from_project(self.project());
        if score.chords.is_empty() && !score.notes.is_empty() {
            let spans = self.recognised_chords();
            score.set_chords(self.project(), spans);
        }
        score
    }

    /// The chords the pitched notes spell, as spans in ticks named for the key in force.
    ///
    /// A beat the recogniser cannot decide carries the chord before it on, as applying an
    /// analysis does; a beat with no notes ends it. Anything that stops the analysis — a song
    /// too long for its grid, too many notes — shows no chords rather than failing the video.
    fn recognised_chords(&self) -> Vec<(Ticks, Ticks, String)> {
        let project = self.project();
        let end = project.end_tick();
        if end <= Ticks::ZERO {
            return Vec::new();
        }
        let report = match self
            .chord_analysis_job(&[], Ticks::ZERO, end, ChordOptions::default())
            .and_then(|job| job.run(&AnalysisControl::default()))
        {
            Ok(report) => report,
            Err(error) => {
                log::debug!("no chords recognised for the video: {error}");
                return Vec::new();
            }
        };
        let mut spans: Vec<(Ticks, Ticks, String)> = Vec::new();
        let mut open = false;
        for segment in &report.segments {
            match segment.reading.state {
                ChordState::Recognized => {
                    let Some(chord) = segment
                        .reading
                        .candidates
                        .first()
                        .and_then(|candidate| Chord::parse(&candidate.symbol))
                    else {
                        open = false;
                        continue;
                    };
                    let name = chord.name_in(project.harmony.key_at(segment.start));
                    spans.push((segment.start, segment.end, name));
                    open = true;
                }
                ChordState::Unknown if open => {
                    if let Some(last) = spans.last_mut() {
                        last.1 = segment.end;
                    }
                }
                ChordState::Unknown | ChordState::NoChord => open = false,
            }
        }
        spans
    }

    /// Captures the document for a video export in `style`.
    pub fn video_job(&mut self, style: NoteRollStyle) -> VideoJob {
        VideoJob {
            score: self.note_score(),
            audio: self.render_job(),
            style,
        }
    }
}

impl VideoJob {
    /// The notes this job will draw.
    pub fn score(&self) -> &NoteScore {
        &self.score
    }

    /// Renders the mix, draws the notes over it and has `ffmpeg` encode both into `path`.
    ///
    /// The background picture is read first, so a missing file fails before minutes of audio
    /// have been rendered for nothing. The mix goes through a private WAV in a temporary folder
    /// at the project's rate in 32-bit float, so the only lossy step is ffmpeg's AAC encode.
    /// `path` is replaced only once ffmpeg has finished; a cancelled export leaves it alone.
    pub fn render(
        &mut self,
        ffmpeg: &Ffmpeg,
        path: &Path,
        progress: &mut RenderProgress<'_>,
    ) -> Result<VideoSummary, SessionError> {
        let renderer = FrameRenderer::new(&self.score, &self.style)?;
        let folder = tempfile::tempdir().map_err(|error| SessionError::Video(error.into()))?;
        let mix = folder.path().join("mix.wav");
        let settings = WavExportSettings {
            bit_depth: WavBitDepth::Float32,
            sample_rate: self.audio.project().sample_rate.round().max(1.0) as u32,
            dither: false,
        };
        // The audio is the shorter half of the work: an offline render runs many times faster
        // than real time, and the encoder runs at a few times it.
        let audio = progress.within(0.0, 0.2, |progress| {
            self.audio
                .render_to_wav(&mix, &settings, &OfflineOptions::whole_project(), progress)
        })?;
        let frames = progress.within(0.2, 0.8, |progress| {
            auris_video::render_video(
                &renderer,
                ffmpeg,
                Some(&mix),
                audio.seconds,
                path,
                &mut |fraction| {
                    progress.report(fraction);
                    !progress.is_cancelled()
                },
            )
        })?;
        Ok(VideoSummary {
            seconds: audio.seconds,
            frames,
            codec: ffmpeg.codec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionOptions;

    #[test]
    fn a_session_exports_a_video_with_its_mix() {
        let Ok(ffmpeg) = Ffmpeg::detect() else {
            eprintln!("ffmpeg is not installed; skipping the encode");
            return;
        };
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let track = session
            .add_instrument_track("Lead", crate::DEFAULT_INSTRUMENT)
            .unwrap();
        let clip = session
            .add_midi_clip(track, "Lead", Ticks::ZERO, Ticks(TICKS_PER_QUARTER * 4))
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks(TICKS_PER_QUARTER)))
            .unwrap();
        assert_eq!(session.note_score().notes.len(), 1);
        assert!(
            session.note_score().chords.is_empty(),
            "one note spells no chord"
        );

        let style = NoteRollStyle {
            size: VideoSize::new(320, 180),
            fps: 12,
            ..NoteRollStyle::default()
        };
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("song.mp4");
        let mut job = session.video_job(style);
        let mut last = 0.0f32;
        let mut report = |fraction: f32| last = last.max(fraction);
        let summary = job
            .render(&ffmpeg, &path, &mut RenderProgress::reporting(&mut report))
            .unwrap();
        assert!(summary.seconds > 0.0);
        assert_eq!(summary.frames, (summary.seconds * 12.0).ceil() as u64);
        assert!(last > 0.99, "progress reached the end: {last}");
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp");
    }

    #[test]
    fn a_song_with_no_chart_is_captioned_with_the_chords_its_notes_spell() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let track = session
            .add_instrument_track("Keys", crate::DEFAULT_INSTRUMENT)
            .unwrap();
        let bar = TICKS_PER_QUARTER * 4;
        let clip = session
            .add_midi_clip(track, "Keys", Ticks::ZERO, Ticks(bar * 2))
            .unwrap();
        // A bar of C major, then a bar of A minor.
        for (pitches, start) in [([60, 64, 67], 0), ([57, 60, 64], bar)] {
            for pitch in pitches {
                session
                    .add_note(clip, Note::new(pitch, Ticks(start), Ticks(bar)))
                    .unwrap();
            }
        }
        let score = session.note_score();
        let names: Vec<_> = score
            .chords
            .iter()
            .map(|chord| chord.name.as_str())
            .collect();
        assert_eq!(names, ["C", "Am"]);
        let seconds_per_bar = 4.0 * 60.0 / session.project().tempo_map.bpm_at(Ticks::ZERO);
        assert!((score.chords[0].end - seconds_per_bar).abs() < 1e-6);
        assert!((score.chords[1].end - 2.0 * seconds_per_bar).abs() < 1e-6);

        // A written chart wins over the guess.
        session.set_chord(Ticks::ZERO, Numeral::parse("IV").unwrap());
        let names: Vec<_> = session
            .note_score()
            .chords
            .iter()
            .map(|chord| chord.name.clone())
            .collect();
        assert_eq!(names, ["F"]);
    }

    #[test]
    fn the_song_wide_lyrics_become_subtitles_in_seconds() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let bar = Ticks(TICKS_PER_QUARTER * 4);
        session.write_lyric_line(bar, bar * 2, "桜咲いた");
        let score = session.note_score();
        assert_eq!(score.lyrics.len(), 1);
        assert_eq!(score.lyrics[0].text, "桜咲いた");
        let seconds_per_bar = 4.0 * 60.0 / session.project().tempo_map.bpm_at(Ticks::ZERO);
        assert!((score.lyrics[0].start - seconds_per_bar).abs() < 1e-6);
        assert!((score.lyrics[0].end - 2.0 * seconds_per_bar).abs() < 1e-6);
        assert_eq!(score.lyric_at(seconds_per_bar * 1.5), Some(0));
        assert_eq!(score.lyric_at(seconds_per_bar * 2.5), None);
    }

    #[test]
    fn a_missing_background_fails_before_any_audio_is_rendered() {
        let ffmpeg_free = Ffmpeg::probe(Path::new("/nowhere/ffmpeg"));
        assert!(ffmpeg_free.is_err());
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let style = NoteRollStyle {
            background_image: Some("/nowhere/picture.png".into()),
            ..NoteRollStyle::default()
        };
        let mut job = session.video_job(style);
        let Ok(ffmpeg) = Ffmpeg::detect() else {
            return;
        };
        let folder = tempfile::tempdir().unwrap();
        let mut called = false;
        let mut report = |_: f32| called = true;
        let error = job
            .render(
                &ffmpeg,
                &folder.path().join("x.mp4"),
                &mut RenderProgress::reporting(&mut report),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            SessionError::Video(VideoError::Image { .. })
        ));
        assert!(!called);
    }
}
