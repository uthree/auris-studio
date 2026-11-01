//! Note-roll videos: the notes of a project flowing past a playhead, with the mix underneath.
//!
//! Three parts, each usable on its own:
//!
//! * [`NoteScore`] is what is drawn — every sounding note of the project in seconds, with the
//!   track it belongs to. It is taken from the document once, so a render never has to look at
//!   ticks or tempo again.
//! * [`FrameRenderer`] draws one frame of a [`NoteScore`] in a [`NoteRollStyle`], in software.
//!   The desktop shows a frame of it as the preview, so what is previewed and what is encoded
//!   are the same pixels rather than two drawings that are meant to agree.
//! * [`Ffmpeg`] finds an `ffmpeg` that is already installed and [`VideoWriter`] pipes frames into
//!   it. Auris never ships one: a bundled ffmpeg is tens of megabytes on every platform, and an
//!   H.264 encoder inside it is either GPL (x264) or the platform's own. The installed program
//!   brings whichever encoder its owner chose, and [`Ffmpeg::probe`] picks the best of them.
//!
//! [`render_video`] puts the three together.

#![warn(missing_docs)]

mod error;
mod ffmpeg;
mod render;
mod score;
mod style;

pub use error::VideoError;
pub use ffmpeg::{Ffmpeg, VideoCodec, VideoWriter, install_command, install_page};
pub use render::{BackgroundImage, FrameRenderer, render_video};
pub use score::{NoteScore, ScoreNote, ScoreTrack};
pub use style::{NoteColoring, NoteRollStyle, VideoSize};
