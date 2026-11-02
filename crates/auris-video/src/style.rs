//! How a note-roll video looks.

use std::path::PathBuf;

use auris_core::Color;
use serde::{Deserialize, Serialize};

/// The pixel size of a video.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoSize {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl VideoSize {
    /// The sizes offered in a menu: the three landscape sizes people upload, a square, and the
    /// portrait frame short-video sites want.
    pub const CHOICES: [VideoSize; 5] = [
        VideoSize::new(1280, 720),
        VideoSize::new(1920, 1080),
        VideoSize::new(3840, 2160),
        VideoSize::new(1080, 1080),
        VideoSize::new(1080, 1920),
    ];

    /// A size of `width` by `height`.
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// The nearest size an H.264 encoder accepts: both sides even, at least 16 and at most 8K.
    ///
    /// 4:2:0 chroma halves both axes, so an odd side is refused by every encoder rather than
    /// rounded by any of them.
    pub fn encodable(self) -> Self {
        let even = |side: u32| (side.clamp(16, 7680) / 2) * 2;
        Self::new(even(self.width), even(self.height))
    }
}

/// Which colour a note is drawn in.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoteColoring {
    /// The colour the track has in the arrangement, so a video matches the window it came from
    /// and recolouring a part is done where it is done for everything else.
    Track,
    /// The style's own palette, one colour per track in arrangement order, repeating.
    ///
    /// The default, because new tracks of one kind share one arrangement colour: a song of
    /// four instrument tracks would otherwise be four parts in the same blue.
    #[default]
    Palette,
    /// A hue running from the lowest pitch to the highest, whatever track a note is on.
    Pitch,
    /// One colour for every note.
    Single,
}

/// Everything about the look of a note-roll video.
///
/// A preference of the person rather than a property of the song, so it lives in the settings and
/// a second project starts from the look the first one ended with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NoteRollStyle {
    /// Frame size.
    pub size: VideoSize,
    /// Frames per second.
    pub fps: u32,
    /// Seconds of music across the width of the frame. Smaller is faster.
    pub seconds_visible: f32,
    /// Where "now" sits, as a fraction of the width from the left.
    pub playhead: f32,
    /// Draw a vertical line at the playhead.
    pub playhead_line: bool,
    /// Colour of the playhead line.
    pub playhead_color: Color,
    /// How notes are coloured.
    pub coloring: NoteColoring,
    /// The colours [`NoteColoring::Palette`] hands out, in order.
    pub palette: Vec<Color>,
    /// The colour [`NoteColoring::Single`] draws every note in.
    pub single_color: Color,
    /// Hue in degrees of the lowest pitch under [`NoteColoring::Pitch`].
    pub low_hue: f32,
    /// Hue in degrees of the highest pitch under [`NoteColoring::Pitch`].
    pub high_hue: f32,
    /// Fill behind everything, and the colour the background picture is dimmed towards.
    pub background: Color,
    /// A picture filling the frame behind the notes, cropped to cover it.
    pub background_image: Option<PathBuf>,
    /// How far the picture is faded towards [`Self::background`], from 0 (untouched) to 1 (gone).
    pub background_dim: f32,
    /// Height of a note as a fraction of the space one semitone gets.
    pub note_thickness: f32,
    /// Corner rounding as a fraction of a note's height: 0 is square, 0.5 a capsule.
    pub note_roundness: f32,
    /// Opacity of notes that have not started yet.
    pub upcoming_opacity: f32,
    /// Opacity of notes that have finished.
    pub played_opacity: f32,
    /// Flash a note towards white when it is struck.
    pub flash: bool,
    /// How many sparks burst from the playhead when a note reaches it, as a multiple of the usual
    /// amount: 0 turns them off, 2 is twice as many.
    pub particles: f32,
    /// How far a sounding note sinks, as if pressed like a key, as a multiple of the usual
    /// depth: 0 keeps notes still.
    pub sink: f32,
    /// Draw drum tracks as well as pitched ones.
    pub include_drums: bool,
    /// Show the chord sounding now in the top corner, with the next one beside it.
    ///
    /// The chords are the ones written into the project, or — where none are written — the
    /// ones recognised from its pitched notes. Drawn in [`Self::playhead_color`], which is the
    /// colour of everything in the frame that is not a note.
    pub chords: bool,
    /// Show the line being sung as a subtitle along the bottom of the frame.
    ///
    /// The lines are the song-wide lyrics annotation, not the words on the notes: those are
    /// what a voice sings, often a mora at a time or in kana, and this is what a person reads.
    pub lyrics: bool,
}

impl Default for NoteRollStyle {
    fn default() -> Self {
        Self {
            size: VideoSize::new(1920, 1080),
            fps: 60,
            seconds_visible: 8.0,
            playhead: 0.35,
            playhead_line: true,
            playhead_color: Color(0xffffff),
            coloring: NoteColoring::Palette,
            palette: vec![
                Color(0x3d7bff),
                Color(0x35d0e0),
                Color(0x7cf08a),
                Color(0xd8f04a),
                Color(0xffb13d),
                Color(0xff5f7a),
                Color(0xb58cff),
                Color(0x6fe3c1),
            ],
            single_color: Color(0x6fc3ff),
            low_hue: 100.0,
            high_hue: 260.0,
            background: Color(0x000000),
            background_image: None,
            background_dim: 0.55,
            note_thickness: 0.7,
            note_roundness: 0.25,
            upcoming_opacity: 0.85,
            played_opacity: 0.45,
            flash: true,
            particles: 1.0,
            sink: 1.0,
            include_drums: false,
            chords: true,
            lyrics: true,
        }
    }
}

impl NoteRollStyle {
    /// Frame rates offered in a menu.
    pub const FPS_CHOICES: [u32; 3] = [24, 30, 60];
    /// The most [`Self::particles`] can be turned up to.
    pub const MAX_PARTICLES: f32 = 3.0;
    /// The most [`Self::sink`] can be turned up to.
    pub const MAX_SINK: f32 = 3.0;

    /// The same style with every number in a range a renderer and an encoder can use.
    ///
    /// Settings files are edited by hand and by older builds; a zero frame rate or a negative
    /// width of time would otherwise reach a division.
    pub fn sanitised(&self) -> Self {
        let unit = |value: f32, fallback: f32| {
            if value.is_finite() {
                value.clamp(0.0, 1.0)
            } else {
                fallback
            }
        };
        let defaults = Self::default();
        Self {
            size: self.size.encodable(),
            fps: self.fps.clamp(1, 120),
            seconds_visible: if self.seconds_visible.is_finite() {
                self.seconds_visible.clamp(1.0, 60.0)
            } else {
                defaults.seconds_visible
            },
            playhead: unit(self.playhead, defaults.playhead),
            background_dim: unit(self.background_dim, defaults.background_dim),
            note_thickness: unit(self.note_thickness, defaults.note_thickness).max(0.05),
            note_roundness: unit(self.note_roundness, defaults.note_roundness).min(0.5),
            upcoming_opacity: unit(self.upcoming_opacity, defaults.upcoming_opacity),
            played_opacity: unit(self.played_opacity, defaults.played_opacity),
            particles: if self.particles.is_finite() {
                self.particles.clamp(0.0, Self::MAX_PARTICLES)
            } else {
                defaults.particles
            },
            sink: if self.sink.is_finite() {
                self.sink.clamp(0.0, Self::MAX_SINK)
            } else {
                defaults.sink
            },
            low_hue: finite_or(self.low_hue, defaults.low_hue),
            high_hue: finite_or(self.high_hue, defaults.high_hue),
            palette: if self.palette.is_empty() {
                defaults.palette
            } else {
                self.palette.clone()
            },
            ..self.clone()
        }
    }
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodable_sizes_are_even_and_bounded() {
        assert_eq!(
            VideoSize::new(1921, 1081).encodable(),
            VideoSize::new(1920, 1080)
        );
        assert_eq!(VideoSize::new(1, 3).encodable(), VideoSize::new(16, 16));
        assert_eq!(
            VideoSize::new(100_000, 7).encodable(),
            VideoSize::new(7680, 16)
        );
    }

    #[test]
    fn sanitising_repairs_numbers_a_hand_edit_could_break() {
        let style = NoteRollStyle {
            fps: 0,
            seconds_visible: f32::NAN,
            playhead: 4.0,
            palette: Vec::new(),
            ..NoteRollStyle::default()
        }
        .sanitised();
        assert_eq!(style.fps, 1);
        assert_eq!(style.seconds_visible, 8.0);
        assert_eq!(style.playhead, 1.0);
        assert_eq!(style.palette.len(), 8);
    }

    #[test]
    fn an_older_settings_file_fills_in_what_it_lacks() {
        let style: NoteRollStyle = serde_json::from_str(r#"{"fps":30}"#).unwrap();
        assert_eq!(style.fps, 30);
        assert_eq!(style.size, VideoSize::new(1920, 1080));
    }
}
