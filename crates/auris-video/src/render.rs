//! Drawing frames, and the export that strings them together.

use std::path::Path;

use auris_core::Color;
use tiny_skia::{BlendMode, FillRule, Paint, Path as SkPath, PathBuilder, Pixmap, Rect, Transform};

use crate::chords::ChordOverlay;
use crate::lyrics::LyricOverlay;
use crate::style::{NoteColoring, NoteRollStyle};
use crate::{Ffmpeg, NoteScore, ScoreNote, VideoError, VideoWriter};

/// How long the flash on a struck note takes to fade, in seconds.
const FLASH_SECONDS: f64 = 0.3;
/// How far a struck note is lifted towards white at the instant it is struck.
const FLASH_STRENGTH: f32 = 0.55;
/// Semitones of air above the highest note and below the lowest.
const PITCH_MARGIN: f64 = 2.0;
/// The fewest semitones the frame is divided into, so a melody of three notes does not fill the
/// screen with bars a fifth of its height each.
const MIN_PITCH_SPAN: f64 = 24.0;
/// How long the longest-lived spark of a struck note lasts, in seconds.
const SPARK_SECONDS: f64 = 0.75;
/// Sparks a note struck at full strength throws, at a particle amount of 1.
const SPARKS_PER_NOTE: f64 = 20.0;
/// How quickly a spark slows down: the time constant of its drag, in seconds.
const SPARK_DRAG: f64 = 0.22;
/// How much of the scroll a spark is carried along with, so the burst trails off with its note
/// instead of hanging at the playhead.
const SPARK_DRIFT: f64 = 0.5;
/// How far a struck note sinks at a sink amount of 1, as a fraction of one semitone's lane.
const SINK_DEPTH: f64 = 0.3;
/// Time constant of the press, in seconds: quick, like a key going down.
const SINK_PRESS: f64 = 0.025;
/// Time constant of the bounce that the press overshoots by and settles from.
const SINK_SETTLE: f64 = 0.12;
/// How far past its resting depth the press overshoots at first.
const SINK_OVERSHOOT: f64 = 0.6;
/// Time constant of the rise back to rest once the note is released.
const SINK_RELEASE: f64 = 0.07;
/// How much a fully pressed note is squashed, as a fraction of its height.
const SINK_SQUASH: f64 = 0.15;

/// A background picture, decoded once and scaled to the frame when a renderer is made.
#[derive(Clone, Debug)]
pub struct BackgroundImage(image::RgbaImage);

impl BackgroundImage {
    /// Reads a PNG, JPEG or WebP.
    pub fn load(path: &Path) -> Result<Self, VideoError> {
        image::open(path)
            .map(|decoded| Self(decoded.into_rgba8()))
            .map_err(|error| VideoError::Image {
                path: path.to_path_buf(),
                detail: error.to_string(),
            })
    }

    /// The picture's size in pixels.
    pub fn dimensions(&self) -> (u32, u32) {
        self.0.dimensions()
    }
}

/// Draws frames of one score in one style.
///
/// Everything that does not move — the background, the pitch layout, each track's colour — is
/// worked out once here, so a frame is a copy of the backdrop and a pass over the notes in view.
pub struct FrameRenderer {
    style: NoteRollStyle,
    score: NoteScore,
    backdrop: Pixmap,
    /// The chord names, outlined, when the style shows them and there are any.
    chords: Option<ChordOverlay>,
    /// The lyric lines, outlined, when the style shows them and there are any.
    lyrics: Option<LyricOverlay>,
    /// Top and bottom of the drawn pitch range, with the margins.
    high: f64,
    lane: f64,
    longest: f64,
    low_pitch: f64,
    pitch_span: f64,
}

impl FrameRenderer {
    /// A renderer for `score` in `style`, reading the style's background picture if it has one.
    pub fn new(score: &NoteScore, style: &NoteRollStyle) -> Result<Self, VideoError> {
        let background = style
            .background_image
            .as_deref()
            .map(BackgroundImage::load)
            .transpose()?;
        Ok(Self::with_background(score, style, background.as_ref()))
    }

    /// A renderer with an already decoded background, which is how a preview redraws on every
    /// change of colour without reading the picture from disk again.
    pub fn with_background(
        score: &NoteScore,
        style: &NoteRollStyle,
        background: Option<&BackgroundImage>,
    ) -> Self {
        let style = style.sanitised();
        let score = if style.include_drums {
            score.clone()
        } else {
            score.without_drums()
        };
        let (low, high) = score.pitch_range().map_or((48.0, 72.0), |(low, high)| {
            (f64::from(low), f64::from(high))
        });
        let (mut low, mut high) = (low - PITCH_MARGIN, high + PITCH_MARGIN);
        let span = high - low + 1.0;
        if span < MIN_PITCH_SPAN {
            let grow = (MIN_PITCH_SPAN - span) / 2.0;
            low -= grow;
            high += grow;
        }
        let lane = f64::from(style.size.height) / (high - low + 1.0);
        let backdrop = backdrop(&style, background);
        let chords = if style.chords {
            ChordOverlay::new(&score)
        } else {
            None
        };
        let lyrics = if style.lyrics {
            LyricOverlay::new(&score)
        } else {
            None
        };
        Self {
            chords,
            lyrics,
            longest: score.longest(),
            low_pitch: low + PITCH_MARGIN,
            pitch_span: (high - low - 2.0 * PITCH_MARGIN).max(1.0),
            style,
            score,
            backdrop,
            high,
            lane,
        }
    }

    /// The style actually drawn, after [`NoteRollStyle::sanitised`].
    pub fn style(&self) -> &NoteRollStyle {
        &self.style
    }

    /// A frame-sized canvas to draw into.
    pub fn canvas(&self) -> Pixmap {
        self.backdrop.clone()
    }

    /// Draws the moment `seconds` into `frame`, which must come from [`Self::canvas`].
    pub fn draw(&self, seconds: f64, frame: &mut Pixmap) {
        frame.data_mut().copy_from_slice(self.backdrop.data());
        let width = f64::from(self.style.size.width);
        let height = f64::from(self.style.size.height);
        let per_second = width / f64::from(self.style.seconds_visible);
        let playhead_x = f64::from(self.style.playhead) * width;
        let left = seconds - playhead_x / per_second;
        let right = seconds + (width - playhead_x) / per_second;

        let first = self
            .score
            .notes
            .partition_point(|note| note.start < left - self.longest);
        let thickness = (self.lane * f64::from(self.style.note_thickness)).max(1.0);
        let radius = thickness * f64::from(self.style.note_roundness);
        let mut paint = Paint {
            anti_alias: true,
            ..Paint::default()
        };
        for note in &self.score.notes[first..] {
            if note.start > right {
                break;
            }
            if note.end < left {
                continue;
            }
            let x0 = playhead_x + (note.start - seconds) * per_second;
            let x1 = (playhead_x + (note.end - seconds) * per_second).max(x0 + 2.0);
            let press = self.press(note, seconds);
            let centre = self.centre(note, press);
            let thickness = thickness * (1.0 - SINK_SQUASH * press.min(1.0));
            let y0 = centre - thickness / 2.0;

            let (mut r, mut g, mut b) = self.color_of(note.track, note.pitch);
            let alpha = if note.end <= seconds {
                self.style.played_opacity
            } else if note.start > seconds {
                self.style.upcoming_opacity
            } else {
                if self.style.flash {
                    let age = seconds - note.start;
                    let lift = FLASH_STRENGTH * (-(age / FLASH_SECONDS)).exp() as f32;
                    r += (1.0 - r) * lift;
                    g += (1.0 - g) * lift;
                    b += (1.0 - b) * lift;
                }
                1.0
            };
            if alpha <= 0.0 {
                continue;
            }
            paint.set_color_rgba8(unit_byte(r), unit_byte(g), unit_byte(b), unit_byte(alpha));
            if let Some(path) = rounded_rect(x0, y0, x1 - x0, thickness, radius) {
                frame.fill_path(
                    &path,
                    &paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }

        if self.style.playhead_line {
            let (r, g, b) = self.style.playhead_color.rgb();
            paint.set_color_rgba8(r, g, b, 150);
            if let Some(line) = Rect::from_xywh(playhead_x as f32 - 1.0, 0.0, 2.0, height as f32) {
                frame.fill_rect(line, &paint, Transform::identity(), None);
            }
        }

        if self.style.particles > 0.0 {
            let struck = self
                .score
                .notes
                .partition_point(|note| note.start < seconds - SPARK_SECONDS);
            for note in &self.score.notes[struck..] {
                if note.start > seconds {
                    break;
                }
                self.sparks(note, seconds - note.start, playhead_x, per_second, frame);
            }
        }

        if let Some(chords) = &self.chords {
            chords.draw(&self.score, &self.style, seconds, frame);
        }
        if let Some(lyrics) = &self.lyrics {
            lyrics.draw(&self.score, &self.style, seconds, frame);
        }
    }

    /// How far down a note is pressed at `seconds`: 0 at rest, 1 held, briefly more just after
    /// the strike as the press overshoots and settles, and falling back to 0 on release.
    fn press(&self, note: &ScoreNote, seconds: f64) -> f64 {
        if self.style.sink <= 0.0 || seconds < note.start {
            return 0.0;
        }
        let held = |age: f64| {
            (1.0 - (-age / SINK_PRESS).exp()) * (1.0 + SINK_OVERSHOOT * (-age / SINK_SETTLE).exp())
        };
        if seconds < note.end {
            held(seconds - note.start)
        } else {
            held(note.end - note.start) * (-(seconds - note.end) / SINK_RELEASE).exp()
        }
    }

    /// Where a note's bar is centred, lowered by `press`.
    fn centre(&self, note: &ScoreNote, press: f64) -> f64 {
        let rest = (self.high - f64::from(note.pitch) + 0.5) * self.lane;
        rest + press * SINK_DEPTH * f64::from(self.style.sink) * self.lane
    }

    /// The burst of a note struck `age` seconds ago: a flare where it met the playhead and a
    /// spray of sparks that slow, shrink and fade as they are carried off with the music.
    ///
    /// Every spark is a function of the note and the time alone, seeded by the note, so any
    /// frame can be drawn on its own — the preview jumps about, and the export never has to
    /// simulate the frames before the one it is drawing.
    fn sparks(
        &self,
        note: &ScoreNote,
        age: f64,
        playhead_x: f64,
        per_second: f64,
        frame: &mut Pixmap,
    ) {
        let (r, g, b) = self.color_of(note.track, note.pitch);
        let lift = |channel: f32| channel + (1.0 - channel) * 0.4;
        let (r, g, b) = (unit_byte(lift(r)), unit_byte(lift(g)), unit_byte(lift(b)));
        let mut paint = Paint {
            anti_alias: true,
            blend_mode: BlendMode::Plus,
            ..Paint::default()
        };
        let height = f64::from(self.style.size.height);
        let centre = self.centre(note, self.press(note, note.start + age));
        let strength = 0.35 + 0.65 * f64::from(note.velocity);
        let drift = per_second * SPARK_DRIFT * age;

        // The flare: a disc that swells and fades in the first third of the burst.
        let flare = age / (SPARK_SECONDS / 3.0);
        if flare < 1.0 {
            let radius = self.lane * f64::from(self.style.note_thickness) * (0.8 + 2.4 * flare);
            let alpha = 0.7 * strength * (1.0 - flare).powi(2);
            paint.set_color_rgba8(r, g, b, unit_byte(alpha as f32));
            if let Some(disc) = PathBuilder::from_circle(
                (playhead_x - drift) as f32,
                centre as f32,
                radius.max(1.0) as f32,
            ) {
                frame.fill_path(
                    &disc,
                    &paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }

        let count = (SPARKS_PER_NOTE * f64::from(self.style.particles) * strength).round() as u32;
        let mut seed =
            note.start.to_bits() ^ (u64::from(note.pitch) << 56) ^ ((note.track as u64) << 48);
        let size = (self.lane * 0.3).min(height * 0.008).max(1.5);
        for _ in 0..count {
            let mut next = || (splitmix(&mut seed) >> 11) as f64 / (1u64 << 53) as f64;
            let life = SPARK_SECONDS * (0.45 + 0.55 * next());
            let along = age / life;
            let angle = next() * std::f64::consts::TAU;
            let speed = height * (0.25 + 0.75 * next()) * strength;
            let grain = 0.5 + next();
            if along >= 1.0 {
                continue;
            }
            let travel = speed * SPARK_DRAG * (1.0 - (-age / SPARK_DRAG).exp());
            let x = playhead_x - drift + angle.cos() * travel;
            let y = centre + angle.sin() * travel;
            let radius = (size * grain * (1.0 - along)).max(0.6);
            let alpha = (1.0 - along).powf(1.5);
            // A soft halo under a bright core, which is what reads as light rather than as dots.
            for (scale, strength) in [(2.6, 0.22), (1.0, 1.0)] {
                paint.set_color_rgba8(r, g, b, unit_byte((alpha * strength) as f32));
                if let Some(dot) =
                    PathBuilder::from_circle(x as f32, y as f32, (radius * scale) as f32)
                {
                    frame.fill_path(&dot, &paint, FillRule::Winding, Transform::identity(), None);
                }
            }
        }
    }

    /// The colour of a note on `track` at `pitch`, each channel from 0 to 1.
    fn color_of(&self, track: usize, pitch: u8) -> (f32, f32, f32) {
        let color = match self.style.coloring {
            NoteColoring::Track => self
                .score
                .tracks
                .get(track)
                .map_or(self.style.single_color, |track| track.color),
            NoteColoring::Palette => self.style.palette[track % self.style.palette.len()],
            NoteColoring::Single => self.style.single_color,
            NoteColoring::Pitch => {
                let along =
                    ((f64::from(pitch) - self.low_pitch) / self.pitch_span).clamp(0.0, 1.0) as f32;
                let hue = self.style.low_hue + (self.style.high_hue - self.style.low_hue) * along;
                return hsv(hue, 0.78, 1.0);
            }
        };
        unit_rgb(color)
    }
}

/// The next number of a SplitMix64 sequence: cheap, and the same on every platform.
fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The background colour, with the picture scaled to cover the frame and faded towards it.
fn backdrop(style: &NoteRollStyle, background: Option<&BackgroundImage>) -> Pixmap {
    let (width, height) = (style.size.width, style.size.height);
    let mut pixmap = Pixmap::new(width, height).expect("a sanitised size is never zero");
    let (br, bg, bb) = style.background.rgb();
    pixmap.fill(tiny_skia::Color::from_rgba8(br, bg, bb, 255));
    let Some(BackgroundImage(picture)) = background else {
        return pixmap;
    };
    let (iw, ih) = picture.dimensions();
    if iw == 0 || ih == 0 {
        return pixmap;
    }
    // Cover: scale until both sides are filled, then crop the middle.
    let scale = (f64::from(width) / f64::from(iw)).max(f64::from(height) / f64::from(ih));
    let sw = ((f64::from(iw) * scale).ceil() as u32).max(width);
    let sh = ((f64::from(ih) * scale).ceil() as u32).max(height);
    let scaled = image::imageops::resize(picture, sw, sh, image::imageops::FilterType::Triangle);
    let (ox, oy) = ((sw - width) / 2, (sh - height) / 2);
    let dim = style.background_dim;
    let back = [f32::from(br), f32::from(bg), f32::from(bb)];
    let data = pixmap.data_mut();
    for y in 0..height {
        for x in 0..width {
            let source = scaled.get_pixel(x + ox, y + oy).0;
            let alpha = f32::from(source[3]) / 255.0;
            let at = ((y * width + x) * 4) as usize;
            for channel in 0..3 {
                // Over the background colour, then faded towards it: the frame stays opaque, so
                // premultiplied and straight alpha are the same bytes.
                let over = f32::from(source[channel]) * alpha + back[channel] * (1.0 - alpha);
                let faded = over * (1.0 - dim) + back[channel] * dim;
                data[at + channel] = faded.round().clamp(0.0, 255.0) as u8;
            }
            data[at + 3] = 255;
        }
    }
    pixmap
}

fn rounded_rect(x: f64, y: f64, width: f64, height: f64, radius: f64) -> Option<SkPath> {
    let (x, y, w, h) = (x as f32, y as f32, width as f32, height as f32);
    let r = (radius as f32).min(w / 2.0).min(h / 2.0).max(0.0);
    if r < 0.5 {
        return Some(PathBuilder::from_rect(Rect::from_xywh(x, y, w, h)?));
    }
    let mut path = PathBuilder::new();
    path.move_to(x + r, y);
    path.line_to(x + w - r, y);
    path.quad_to(x + w, y, x + w, y + r);
    path.line_to(x + w, y + h - r);
    path.quad_to(x + w, y + h, x + w - r, y + h);
    path.line_to(x + r, y + h);
    path.quad_to(x, y + h, x, y + h - r);
    path.line_to(x, y + r);
    path.quad_to(x, y, x + r, y);
    path.close();
    path.finish()
}

fn unit_rgb(color: Color) -> (f32, f32, f32) {
    let (r, g, b) = color.rgb();
    (
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
    )
}

fn unit_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Hue in degrees, saturation and value from 0 to 1, to RGB from 0 to 1.
fn hsv(hue: f32, saturation: f32, value: f32) -> (f32, f32, f32) {
    let hue = hue.rem_euclid(360.0) / 60.0;
    let chroma = value * saturation;
    let x = chroma * (1.0 - (hue % 2.0 - 1.0).abs());
    let (r, g, b) = match hue as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = value - chroma;
    (r + m, g + m, b + m)
}

/// Draws `seconds` of `score` and encodes it with the audio in `audio` into an `.mp4` at
/// `output`.
///
/// The file is written beside `output` under a temporary name and moved into place only once
/// ffmpeg has finished it, so a cancelled or failed export leaves whatever was there before.
/// `progress` hears the fraction done before every frame and answers whether to go on; `false`
/// stops the export with [`VideoError::Cancelled`].
pub fn render_video(
    renderer: &FrameRenderer,
    ffmpeg: &Ffmpeg,
    audio: Option<&Path>,
    seconds: f64,
    output: &Path,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<u64, VideoError> {
    let style = renderer.style();
    let fps = style.fps;
    let frames = ((seconds.max(0.0) * f64::from(fps)).ceil() as u64).max(1);
    let folder = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let staging = tempfile::Builder::new()
        .prefix(".auris-video-")
        .suffix(".mp4")
        .tempfile_in(folder)?;
    let mut writer = VideoWriter::start(ffmpeg, style.size, fps, audio, staging.path())?;
    let mut frame = renderer.canvas();
    for index in 0..frames {
        if !progress(index as f32 / frames as f32) {
            return Err(VideoError::Cancelled);
        }
        renderer.draw(index as f64 / f64::from(fps), &mut frame);
        writer.write_frame(frame.data())?;
    }
    writer.finish()?;
    // Asked once more after the encoder has finished: the last moment the old file can still be
    // kept.
    if !progress(1.0) {
        return Err(VideoError::Cancelled);
    }
    staging
        .persist(output)
        .map_err(|error| VideoError::Io(error.error))?;
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ScoreTrack, VideoSize};

    fn score() -> NoteScore {
        NoteScore {
            tracks: vec![ScoreTrack {
                name: "Lead".into(),
                color: Color(0xff0000),
                drum: false,
            }],
            notes: vec![
                ScoreNote {
                    start: 1.0,
                    end: 2.0,
                    pitch: 60,
                    velocity: 1.0,
                    track: 0,
                },
                ScoreNote {
                    start: 3.0,
                    end: 4.0,
                    pitch: 72,
                    velocity: 1.0,
                    track: 0,
                },
            ],
            chords: Vec::new(),
            lyrics: Vec::new(),
        }
    }

    fn style() -> NoteRollStyle {
        NoteRollStyle {
            size: VideoSize::new(200, 100),
            seconds_visible: 4.0,
            playhead: 0.5,
            playhead_line: false,
            flash: false,
            particles: 0.0,
            sink: 0.0,
            upcoming_opacity: 1.0,
            played_opacity: 1.0,
            background: Color(0x000000),
            coloring: NoteColoring::Track,
            chords: false,
            lyrics: false,
            ..NoteRollStyle::default()
        }
    }

    fn pixel(frame: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * frame.width() + x) * 4) as usize;
        frame.data()[at..at + 4].try_into().unwrap()
    }

    /// The row a pitch's bar is centred on.
    fn row_of(renderer: &FrameRenderer, pitch: u8) -> u32 {
        ((renderer.high - f64::from(pitch) + 0.5) * renderer.lane) as u32
    }

    #[test]
    fn a_note_crosses_the_playhead_when_it_sounds() {
        let renderer = FrameRenderer::with_background(&score(), &style(), None);
        let mut frame = renderer.canvas();
        // 4 s across 200 px is 50 px a second; the playhead is at x = 100.
        renderer.draw(1.5, &mut frame);
        let row = row_of(&renderer, 60);
        assert_eq!(
            pixel(&frame, 100, row),
            [255, 0, 0, 255],
            "sounding at the playhead"
        );
        assert_eq!(pixel(&frame, 70, row), [0, 0, 0, 255], "began at x = 75");
        assert_eq!(pixel(&frame, 130, row), [0, 0, 0, 255], "ends at x = 125");
        // The second note starts 1.5 s later: 75 px to the right.
        let high = row_of(&renderer, 72);
        assert!(high < row, "higher pitches are drawn higher up");
        assert_eq!(pixel(&frame, 180, high), [255, 0, 0, 255]);
        assert_eq!(pixel(&frame, 170, high), [0, 0, 0, 255]);
    }

    #[test]
    fn played_notes_fade_and_struck_notes_flash() {
        let style = NoteRollStyle {
            played_opacity: 0.5,
            flash: true,
            ..style()
        };
        let renderer = FrameRenderer::with_background(&score(), &style, None);
        let mut frame = renderer.canvas();
        renderer.draw(2.5, &mut frame);
        let row = row_of(&renderer, 60);
        // Finished half a second ago, so it sits 25 px left of the playhead at half strength.
        assert_eq!(pixel(&frame, 70, row), [128, 0, 0, 255]);
        renderer.draw(3.0, &mut frame);
        let high = row_of(&renderer, 72);
        let struck = pixel(&frame, 102, high);
        assert_eq!(struck[0], 255);
        assert!(
            struck[1] > 100,
            "lifted towards white at the strike: {struck:?}"
        );
    }

    #[test]
    fn a_struck_note_throws_sparks_that_die_away() {
        let style = NoteRollStyle {
            particles: 1.0,
            ..style()
        };
        let lit_away_from_the_note = |style: &NoteRollStyle, seconds: f64| {
            let renderer = FrameRenderer::with_background(&score(), style, None);
            let row = i64::from(row_of(&renderer, 60));
            let band = (renderer.lane * 0.5).ceil() as i64;
            let mut frame = renderer.canvas();
            renderer.draw(seconds, &mut frame);
            let lit = (0..frame.height())
                .filter(|&y| (i64::from(y) - row).abs() > band)
                // Left of where the next note, a twelfth higher, comes into view.
                .flat_map(|y| (0..150).map(move |x| (x, y)))
                .filter(|&(x, y)| pixel(&frame, x, y)[..3] != [0, 0, 0])
                .count();
            (lit, frame)
        };
        assert_eq!(
            lit_away_from_the_note(&style, 0.99).0,
            0,
            "nothing before the strike"
        );
        let (burst, first) = lit_away_from_the_note(&style, 1.1);
        assert!(burst > 10, "a burst just after the strike: {burst} pixels");
        assert_eq!(
            first.data(),
            lit_away_from_the_note(&style, 1.1).1.data(),
            "a frame is the same however often it is drawn"
        );
        assert_eq!(
            lit_away_from_the_note(&style, 1.0 + SPARK_SECONDS + 0.01).0,
            0,
            "gone once spent"
        );
        let off = NoteRollStyle {
            particles: 0.0,
            ..style
        };
        assert_eq!(
            lit_away_from_the_note(&off, 1.1).0,
            0,
            "and none when turned off"
        );
    }

    #[test]
    fn a_sounding_note_sinks_and_rises_again_when_released() {
        let style = NoteRollStyle {
            size: VideoSize::new(200, 400),
            sink: 1.0,
            ..style()
        };
        let renderer = FrameRenderer::with_background(&score(), &style, None);
        // The lowest lit row in the note's column, which moves down as it is pressed.
        let bottom = |seconds: f64| {
            let mut frame = renderer.canvas();
            renderer.draw(seconds, &mut frame);
            let x = (100.0 + (1.5 - seconds) * 50.0) as u32;
            (0..frame.height())
                .rev()
                .find(|&y| pixel(&frame, x, y)[0] > 0 && (y as f64) < renderer.lane * 20.0)
                .unwrap()
        };
        let rest = bottom(0.9);
        let held = bottom(1.5);
        let lane = renderer.lane;
        assert!(
            f64::from(held - rest) > lane * 0.15,
            "pressed down while it sounds: {rest} -> {held}, lane {lane}"
        );
        let bounce = bottom(1.04);
        assert!(
            bounce > held,
            "overshoots just after the strike: {bounce} vs {held}"
        );
        assert_eq!(bottom(2.6), rest, "back at rest once released");
        let still =
            FrameRenderer::with_background(&score(), &NoteRollStyle { sink: 0.0, ..style }, None);
        assert_eq!(
            still.press(&score().notes[0], 1.5),
            0.0,
            "and still when turned off"
        );
    }

    #[test]
    fn pitch_colouring_runs_from_the_low_hue_to_the_high_hue() {
        let style = NoteRollStyle {
            coloring: NoteColoring::Pitch,
            low_hue: 120.0,
            high_hue: 240.0,
            ..style()
        };
        let renderer = FrameRenderer::with_background(&score(), &style, None);
        let (r, g, b) = renderer.color_of(0, 60);
        assert!(g > r && g > b, "lowest pitch is green: {r} {g} {b}");
        let (r, g, b) = renderer.color_of(0, 72);
        assert!(b > r && b > g, "highest pitch is blue: {r} {g} {b}");
    }

    #[test]
    fn a_background_picture_covers_the_frame_and_is_dimmed() {
        let picture = BackgroundImage(image::RgbaImage::from_pixel(
            10,
            40,
            image::Rgba([200, 100, 0, 255]),
        ));
        let style = NoteRollStyle {
            background_dim: 0.5,
            background: Color(0x000000),
            ..style()
        };
        let renderer =
            FrameRenderer::with_background(&NoteScore::default(), &style, Some(&picture));
        let mut frame = renderer.canvas();
        renderer.draw(0.0, &mut frame);
        for (x, y) in [(0, 0), (199, 99), (100, 50)] {
            assert_eq!(pixel(&frame, x, y), [100, 50, 0, 255]);
        }
    }

    #[test]
    fn a_video_is_encoded_when_ffmpeg_is_installed() {
        let Ok(ffmpeg) = Ffmpeg::detect() else {
            eprintln!("ffmpeg is not installed; skipping the encode");
            return;
        };
        let folder = tempfile::tempdir().unwrap();
        let output = folder.path().join("roll.mp4");
        let style = NoteRollStyle {
            fps: 10,
            chords: true,
            ..style()
        };
        let mut score = score();
        score.chords.push(crate::ScoreChord {
            start: 0.0,
            end: 2.0,
            name: "C".into(),
        });
        let renderer = FrameRenderer::with_background(&score, &style, None);
        let mut last = 0.0;
        let frames = render_video(&renderer, &ffmpeg, None, 2.0, &output, &mut |fraction| {
            last = fraction;
            true
        })
        .unwrap();
        assert_eq!(frames, 20);
        assert_eq!(last, 1.0);
        let bytes = std::fs::read(&output).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp", "an ISO media file");
        let leftovers = std::fs::read_dir(folder.path()).unwrap().count();
        assert_eq!(leftovers, 1, "the staging file was moved, not copied");
    }

    #[test]
    fn a_cancelled_export_leaves_nothing_behind() {
        let Ok(ffmpeg) = Ffmpeg::detect() else {
            return;
        };
        let folder = tempfile::tempdir().unwrap();
        let output = folder.path().join("roll.mp4");
        let renderer = FrameRenderer::with_background(&score(), &style(), None);
        let result = render_video(&renderer, &ffmpeg, None, 2.0, &output, &mut |_| false);
        assert!(matches!(result, Err(VideoError::Cancelled)));
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
    }
}
