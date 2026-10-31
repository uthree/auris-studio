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
    BackgroundImage, Ffmpeg, FrameRenderer, NoteColoring, NoteRollStyle, NoteScore, VideoCodec,
    VideoError, VideoSize,
};

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
    /// The notes a video of the open document would draw.
    pub fn note_score(&self) -> NoteScore {
        NoteScore::from_project(self.project())
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
