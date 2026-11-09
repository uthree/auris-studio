//! What can go wrong while making a video.

/// A failure while drawing or encoding a video.
#[derive(Debug, thiserror::Error)]
pub enum VideoError {
    /// No `ffmpeg` was found on this machine.
    #[error("ffmpeg is not installed (install it with: {0})")]
    FfmpegMissing(&'static str),
    /// `ffmpeg` was found but could not be used, or stopped with an error.
    #[error("ffmpeg failed: {0}")]
    Ffmpeg(String),
    /// The background picture could not be read.
    #[error("could not read the background image {path}: {detail}")]
    Image {
        /// The picture that was asked for.
        path: std::path::PathBuf,
        /// What the decoder said.
        detail: String,
    },
    /// Reading or writing a file failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The caller asked the render to stop.
    #[error("video export cancelled")]
    Cancelled,
}
