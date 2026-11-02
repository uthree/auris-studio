//! The lyric subtitles: the line being sung, centred along the bottom of the frame.
//!
//! Drawn the way the chord caption is — outlined from fonts already installed, nothing bundled —
//! with one difference that matters: the chord names are Latin letters and a few signs, and the
//! words are usually Japanese. The faces tried first are therefore the platforms' Japanese ones,
//! and every character a face lacks falls through to the next, so a line mixing kanji, kana and
//! an English word draws whole wherever any installed face has each glyph. A machine with no
//! font draws no subtitles rather than failing the export.

use std::collections::HashMap;
use std::sync::OnceLock;

use tiny_skia::{LineCap, LineJoin, Paint, Path as SkPath, PathBuilder, Pixmap, Stroke, Transform};

use crate::chords::{FontData, system_font, unit_byte};
use crate::{NoteRollStyle, NoteScore};

/// Height of the text's em square, as a fraction of the frame's shorter side.
const SIZE: f64 = 0.062;
/// Space from the frame's bottom edge to the baseline, as a fraction of its shorter side.
const BOTTOM: f64 = 0.07;
/// The widest a line is drawn, as a fraction of the frame's width; longer lines are shrunk.
const MAX_WIDTH: f64 = 0.9;
/// How long a line takes to fade in and out, in seconds.
const FADE_SECONDS: f64 = 0.15;

/// Japanese faces tried in order: macOS, Windows, then what a Linux machine has.
const FAMILIES: [&str; 10] = [
    "Hiragino Sans",
    "Hiragino Kaku Gothic ProN",
    "Yu Gothic",
    "Meiryo",
    "Noto Sans CJK JP",
    "Noto Sans JP",
    "Source Han Sans JP",
    "IPAexGothic",
    "IPAGothic",
    "Noto Sans CJK SC",
];

/// The installed faces subtitles may draw from, best first, read once per process.
fn lyric_fonts() -> &'static [FontData] {
    static FONTS: OnceLock<Vec<FontData>> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut database = fontdb::Database::new();
        database.load_system_fonts();
        let mut fonts = Vec::new();
        let mut seen = Vec::new();
        for family in FAMILIES {
            let Some(id) = database.query(&fontdb::Query {
                families: &[fontdb::Family::Name(family)],
                weight: fontdb::Weight::BOLD,
                ..fontdb::Query::default()
            }) else {
                continue;
            };
            if seen.contains(&id) {
                continue;
            }
            seen.push(id);
            if let Some(font) = database.with_face_data(id, |data, index| FontData {
                bytes: data.to_vec(),
                index,
            }) {
                fonts.push(font);
            }
            // One Japanese face and one fallback behind it is plenty.
            if fonts.len() == 2 {
                break;
            }
        }
        if let Some(latin) = system_font() {
            fonts.push(FontData {
                bytes: latin.bytes.clone(),
                index: latin.index,
            });
        }
        fonts
    })
}

/// A line outlined in em units: x from the left of the line, y up from the baseline.
struct Line {
    path: Option<SkPath>,
    advance: f64,
}

/// The subtitles for one score, every line already outlined.
pub(crate) struct LyricOverlay {
    lines: HashMap<String, Line>,
}

impl LyricOverlay {
    /// The overlay for `score`, or `None` when it has no lyrics or the machine has no font.
    pub(crate) fn new(score: &NoteScore) -> Option<Self> {
        if score.lyrics.is_empty() {
            return None;
        }
        let faces: Vec<_> = lyric_fonts()
            .iter()
            .filter_map(|font| ttf_parser::Face::parse(&font.bytes, font.index).ok())
            .collect();
        if faces.is_empty() {
            return None;
        }
        let mut lines = HashMap::new();
        for line in &score.lyrics {
            lines
                .entry(line.text.clone())
                .or_insert_with(|| outline(&faces, &line.text));
        }
        Some(Self { lines })
    }

    /// Draws the subtitle for the moment `seconds` into `frame`.
    pub(crate) fn draw(
        &self,
        score: &NoteScore,
        style: &NoteRollStyle,
        seconds: f64,
        frame: &mut Pixmap,
    ) {
        let Some(index) = score.lyric_at(seconds) else {
            return;
        };
        let line = &score.lyrics[index];
        let Some(outlined) = self.lines.get(&line.text) else {
            return;
        };
        let Some(path) = &outlined.path else {
            return;
        };
        let width = f64::from(style.size.width);
        let height = f64::from(style.size.height);
        let short = width.min(height);
        let em = (short * SIZE).min(width * MAX_WIDTH / outlined.advance.max(1e-6));
        let drawn = outlined.advance * em;
        let x = (width - drawn) / 2.0;
        let baseline = height - short * BOTTOM;

        let fade_in = ((seconds - line.start) / FADE_SECONDS).clamp(0.0, 1.0);
        let fade_out = ((line.end - seconds) / FADE_SECONDS).clamp(0.0, 1.0);
        let opacity = fade_in.min(fade_out);
        let place = Transform::from_row(em as f32, 0.0, 0.0, -em as f32, x as f32, baseline as f32);

        // A dark rim under the letters keeps them legible over bright notes and pictures.
        let mut rim = Paint {
            anti_alias: true,
            ..Paint::default()
        };
        rim.set_color_rgba8(0, 0, 0, unit_byte(0.7 * opacity));
        let stroke = Stroke {
            // The stroke is in em units, because the transform scales it.
            width: 0.12,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        frame.stroke_path(path, &rim, &stroke, place, None);
        // The playhead colour, the frame's own ink, rather than the chord caption's colour.
        let (r, g, b) = style.playhead_color.rgb();
        let mut ink = Paint {
            anti_alias: true,
            ..Paint::default()
        };
        ink.set_color_rgba8(r, g, b, unit_byte(opacity));
        frame.fill_path(path, &ink, tiny_skia::FillRule::Winding, place, None);
    }

    /// How wide `text` is drawn at an em of `em` pixels.
    #[cfg(test)]
    fn text_width(&self, text: &str, em: f64) -> f64 {
        self.lines.get(text).map_or(0.0, |line| line.advance * em)
    }
}

/// Outlines `text` in em units, each character from the first face that has it.
fn outline(faces: &[ttf_parser::Face<'_>], text: &str) -> Line {
    let mut builder = Scaled {
        path: PathBuilder::new(),
        x: 0.0,
        scale: 1.0,
    };
    for character in text.chars() {
        let found = faces
            .iter()
            .find_map(|face| face.glyph_index(character).map(|glyph| (face, glyph)));
        let Some((face, glyph)) = found else {
            builder.x += 0.5;
            continue;
        };
        let per_em = f32::from(face.units_per_em().max(1));
        builder.scale = 1.0 / per_em;
        face.outline_glyph(glyph, &mut builder);
        builder.x += f32::from(face.glyph_hor_advance(glyph).unwrap_or(0)) / per_em;
    }
    Line {
        advance: f64::from(builder.x),
        path: builder.path.finish(),
    }
}

/// Collects glyph outlines into one path in em units, each moved along by the advances before it.
struct Scaled {
    path: PathBuilder,
    x: f32,
    scale: f32,
}

impl ttf_parser::OutlineBuilder for Scaled {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to(self.x + x * self.scale, y * self.scale);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to(self.x + x * self.scale, y * self.scale);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let s = self.scale;
        self.path
            .quad_to(self.x + x1 * s, y1 * s, self.x + x * s, y * s);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let s = self.scale;
        self.path.cubic_to(
            self.x + x1 * s,
            y1 * s,
            self.x + x2 * s,
            y2 * s,
            self.x + x * s,
            y * s,
        );
    }

    fn close(&mut self) {
        self.path.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FrameRenderer, ScoreLyric, VideoSize};
    use auris_core::Color;

    fn score() -> NoteScore {
        let line = |start: f64, end: f64, text: &str| ScoreLyric {
            start,
            end,
            text: text.into(),
        };
        NoteScore {
            lyrics: vec![line(0.0, 2.0, "桜咲いた"), line(3.0, 4.0, "Hello 春")],
            ..NoteScore::default()
        }
    }

    fn style(lyrics: bool) -> NoteRollStyle {
        NoteRollStyle {
            size: VideoSize::new(640, 360),
            playhead_line: false,
            background: Color(0x000000),
            playhead_color: Color(0xffffff),
            chords: false,
            lyrics,
            ..NoteRollStyle::default()
        }
    }

    /// Lit pixels in the bottom fifth of the frame, where the subtitle sits.
    fn lit(frame: &Pixmap) -> usize {
        (frame.height() * 4 / 5..frame.height())
            .flat_map(|y| (0..frame.width()).map(move |x| (x, y)))
            .filter(|&(x, y)| frame.data()[((y * frame.width() + x) * 4) as usize] > 128)
            .count()
    }

    fn drawn(lyrics: bool, seconds: f64) -> Option<Pixmap> {
        if lyric_fonts().is_empty() {
            return None;
        }
        let renderer = FrameRenderer::with_background(&score(), &style(lyrics), None);
        let mut frame = renderer.canvas();
        renderer.draw(seconds, &mut frame);
        Some(frame)
    }

    #[test]
    fn the_line_being_sung_is_drawn_along_the_bottom_and_nothing_between_lines() {
        let Some(sung) = drawn(true, 1.0) else {
            eprintln!("no font installed; skipping the subtitles");
            return;
        };
        assert!(lit(&sung) > 100, "the line is drawn");
        assert_eq!(lit(&drawn(true, 2.5).unwrap()), 0, "silent between lines");
        assert_eq!(lit(&drawn(true, 9.0).unwrap()), 0, "and after the last");
        assert_eq!(lit(&drawn(false, 1.0).unwrap()), 0, "and none when off");
    }

    #[test]
    fn a_line_is_centred_and_shrinks_to_fit_a_narrow_frame() {
        let Some(frame) = drawn(true, 1.0) else {
            return;
        };
        let columns: Vec<u32> = (0..640)
            .filter(|&x| (300..360).any(|y| frame.data()[((y * 640 + x) * 4) as usize] > 128))
            .collect();
        let (left, right) = (columns[0], *columns.last().unwrap());
        assert!(
            (i64::from(left) - (640 - i64::from(right))).abs() < 24,
            "centred: {left}..{right}"
        );

        let long = NoteScore {
            lyrics: vec![ScoreLyric {
                start: 0.0,
                end: 1.0,
                text: "あ".repeat(60),
            }],
            ..NoteScore::default()
        };
        let overlay = LyricOverlay::new(&long).unwrap();
        let em = 360.0 * SIZE;
        assert!(
            overlay.text_width(&"あ".repeat(60), em) > 640.0,
            "too long at full size"
        );
        let renderer = FrameRenderer::with_background(&long, &style(true), None);
        let mut frame = renderer.canvas();
        renderer.draw(0.5, &mut frame);
        for y in 0..360 {
            for x in [0u32, 1, 638, 639] {
                assert!(
                    frame.data()[((y * 640 + x) * 4) as usize] < 128,
                    "inside the frame"
                );
            }
        }
    }

    #[test]
    fn japanese_and_latin_glyphs_are_both_outlined() {
        let faces: Vec<_> = lyric_fonts()
            .iter()
            .filter_map(|font| ttf_parser::Face::parse(&font.bytes, font.index).ok())
            .collect();
        if faces.is_empty() {
            return;
        }
        let line = outline(&faces, "Hello 春");
        assert!(line.path.is_some());
        assert!(line.advance > 2.0, "{}", line.advance);
    }
}
