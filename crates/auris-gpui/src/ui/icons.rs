//! Vector icons, drawn rather than typed.
//!
//! The transport used to be text — `▶`, `⏹`, `🔁` — which looks fine until the platform decides
//! one of those codepoints deserves colour emoji presentation and the row stops matching itself.
//! Drawing the shapes removes the font from the question entirely: every icon is the same
//! weight, the same colour, and lines up on the same optical centre.

use gpui::{
    Bounds, Corners, Hsla, IntoElement, PathBuilder, Pixels, Point, Window, canvas, div, fill,
    point, prelude::*, px, size,
};

/// An icon the UI can draw.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Icon {
    /// Return the playhead to the start.
    ToStart,
    /// Start playback.
    Play,
    /// Pause playback.
    Pause,
    /// Stop playback and rewind.
    Stop,
    /// Start recording a take.
    Record,
    /// Toggle the cycle region.
    Loop,
    /// Toggle the punch region: the cycle's outline with a record disc inside it.
    Punch,
    /// Toggle the click: the wedge and its pendulum.
    Metronome,
    /// Add something.
    Plus,
    /// Move up in a list.
    ChevronUp,
    /// Move down in a list.
    ChevronDown,
    /// A branch of a tree that is shut. Points at what opening it would reveal.
    ChevronRight,
    /// Remove something.
    Cross,
    /// A menu item that is currently on.
    Check,
    /// The sound library: a list of things to choose from.
    Library,
    /// The piano roll: notes lying at different places along a grid.
    Notes,
    /// The mixer: a bank of vertical faders.
    Faders,
    /// The inspector: horizontal sliders, the controls it is made of.
    Sliders,
    /// The log: lines of text, ragged the way written lines are.
    Log,
    /// An instrument: a keyboard, seen from the front.
    Keyboard,
    /// An effect: a knob with its pointer.
    Knob,
    /// A sound — a font's preset, or the shelf a set of them sits on.
    Wave,
}

/// An element that draws `icon` at `size`, centred in whatever box it is given.
pub fn icon(icon: Icon, size: Pixels, color: Hsla) -> impl IntoElement + use<> {
    div().size(size).child(
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| paint_icon(window, bounds, icon, color),
        )
        .size_full(),
    )
}

/// Draws `icon` filling `bounds`.
///
/// Every shape is laid out in the same square so a row of icons shares one optical weight; the
/// insets below are tuned against each other rather than derived, which is why they differ.
pub fn paint_icon(window: &mut Window, bounds: Bounds<Pixels>, icon: Icon, color: Hsla) {
    let side = f32::from(bounds.size.width).min(f32::from(bounds.size.height));
    if side < 4.0 {
        return;
    }
    // Work in a centred square so a non-square button still gets a centred icon.
    let origin = point(
        f32::from(bounds.origin.x) + (f32::from(bounds.size.width) - side) / 2.0,
        f32::from(bounds.origin.y) + (f32::from(bounds.size.height) - side) / 2.0,
    );
    let at = |x: f32, y: f32| point(px(origin.x + x * side), px(origin.y + y * side));
    let bar = |window: &mut Window, x0: f32, y0: f32, x1: f32, y1: f32| {
        rounded(
            window,
            Bounds {
                origin: at(x0, y0),
                size: size(px((x1 - x0) * side), px((y1 - y0) * side)),
            },
            px(((x1 - x0) * side * 0.35).min(2.0)),
            color,
        )
    };
    // A control's handle: a capsule centred on a point, rather than measured from a corner, so a
    // fader's setting is written as the place it sits.
    let knob = |window: &mut Window, cx: f32, cy: f32, w: f32, h: f32| {
        rounded(
            window,
            Bounds {
                origin: at(cx - w / 2.0, cy - h / 2.0),
                size: size(px(w * side), px(h * side)),
            },
            px(w.min(h) * side / 2.0),
            color,
        )
    };

    // A circle, four quadratic arcs with their controls at the corners of the bounding square.
    // Not a true circle — the corners bulge by about three per cent of the radius — and at the
    // ten pixels an icon is drawn at, three per cent is a third of nothing.
    let ring = |window: &mut Window, cx: f32, cy: f32, r: f32, width: f32| {
        let mut builder = PathBuilder::stroke(px((side * width).max(1.0)));
        builder.move_to(at(cx, cy - r));
        builder.curve_to(at(cx + r, cy), at(cx + r, cy - r));
        builder.curve_to(at(cx, cy + r), at(cx + r, cy + r));
        builder.curve_to(at(cx - r, cy), at(cx - r, cy + r));
        builder.curve_to(at(cx, cy - r), at(cx - r, cy - r));
        if let Ok(path) = builder.build() {
            window.paint_path(path, color);
        }
    };

    match icon {
        Icon::Play => triangle(
            window,
            at(0.30, 0.20),
            at(0.30, 0.80),
            at(0.78, 0.50),
            color,
        ),
        Icon::Pause => {
            bar(window, 0.30, 0.22, 0.43, 0.78);
            bar(window, 0.57, 0.22, 0.70, 0.78);
        }
        Icon::Stop => rounded(
            window,
            Bounds {
                origin: at(0.28, 0.28),
                size: size(px(0.44 * side), px(0.44 * side)),
            },
            px((0.08 * side).min(3.0)),
            color,
        ),
        Icon::ToStart => {
            bar(window, 0.26, 0.22, 0.34, 0.78);
            triangle(
                window,
                at(0.76, 0.20),
                at(0.76, 0.80),
                at(0.38, 0.50),
                color,
            );
        }
        Icon::Loop => {
            // Logic's cycle button: the outline of a cycle region, nothing more. Arrowheads
            // were tried and rejected — at this size a pair of them reads as "swap", and one
            // riding the edge reads as a flag stuck to a box. The bare rounded rectangle is
            // unambiguous because it is a picture of the thing it toggles.
            let (x0, x1, y0, y1, r) = (0.14, 0.86, 0.28, 0.72, 0.16);
            let mut builder = PathBuilder::stroke(px((side * 0.10).max(1.25)));
            builder.move_to(at(x0 + r, y0));
            builder.line_to(at(x1 - r, y0));
            builder.curve_to(at(x1, y0 + r), at(x1, y0));
            builder.line_to(at(x1, y1 - r));
            builder.curve_to(at(x1 - r, y1), at(x1, y1));
            builder.line_to(at(x0 + r, y1));
            builder.curve_to(at(x0, y1 - r), at(x0, y1));
            builder.line_to(at(x0, y0 + r));
            builder.curve_to(at(x0 + r, y0), at(x0, y0));
            if let Ok(path) = builder.build() {
                window.paint_path(path, color);
            }
        }
        Icon::Punch => {
            // The cycle's own outline with a record disc inside it, because that is exactly what
            // the thing is: a region, and what happens in it. Drawn wider and shallower than the
            // cycle box so the disc has somewhere to sit without touching the walls — a dot that
            // fills the box reads as a full stop in brackets.
            let (x0, x1, y0, y1, r) = (0.08, 0.92, 0.30, 0.70, 0.13);
            let mut builder = PathBuilder::stroke(px((side * 0.09).max(1.1)));
            builder.move_to(at(x0 + r, y0));
            builder.line_to(at(x1 - r, y0));
            builder.curve_to(at(x1, y0 + r), at(x1, y0));
            builder.line_to(at(x1, y1 - r));
            builder.curve_to(at(x1 - r, y1), at(x1, y1));
            builder.line_to(at(x0 + r, y1));
            builder.curve_to(at(x0, y1 - r), at(x0, y1));
            builder.line_to(at(x0, y0 + r));
            builder.curve_to(at(x0 + r, y0), at(x0, y0));
            if let Ok(path) = builder.build() {
                window.paint_path(path, color);
            }
            knob(window, 0.50, 0.50, 0.26, 0.26);
        }
        Icon::Metronome => {
            // The wedge and the rod. Every metronome anybody has seen is this shape, and at
            // eleven pixels it is the only drawing that still reads as one: a wedge on its own
            // is a tent, and a rod on its own is a slash.
            let stroke = px((side * 0.09).max(1.25));
            let mut builder = PathBuilder::stroke(stroke);
            builder.move_to(at(0.40, 0.16));
            builder.line_to(at(0.18, 0.84));
            builder.line_to(at(0.82, 0.84));
            builder.line_to(at(0.60, 0.16));
            builder.line_to(at(0.40, 0.16));
            if let Ok(path) = builder.build() {
                window.paint_path(path, color);
            }
            // Leaning, because a rod standing upright in the middle is a pole in a tent.
            stroke_line(window, at(0.50, 0.80), at(0.66, 0.28), stroke, color);
        }
        Icon::Plus => {
            bar(window, 0.43, 0.22, 0.57, 0.78);
            bar(window, 0.22, 0.43, 0.78, 0.57);
        }
        Icon::ChevronUp => {
            triangle(
                window,
                at(0.26, 0.62),
                at(0.74, 0.62),
                at(0.50, 0.34),
                color,
            );
        }
        Icon::ChevronDown => {
            triangle(
                window,
                at(0.26, 0.38),
                at(0.74, 0.38),
                at(0.50, 0.66),
                color,
            );
        }
        Icon::ChevronRight => {
            triangle(
                window,
                at(0.38, 0.26),
                at(0.38, 0.74),
                at(0.66, 0.50),
                color,
            );
        }
        Icon::Check => {
            // The short stroke is drawn first so the joint at the bottom is covered by the long
            // one, which keeps the corner sharp instead of showing two overlapping caps.
            stroke_line(
                window,
                at(0.22, 0.52),
                at(0.42, 0.72),
                px(side * 0.11),
                color,
            );
            stroke_line(
                window,
                at(0.42, 0.72),
                at(0.78, 0.28),
                px(side * 0.11),
                color,
            );
        }
        // The four panel marks. They are drawn at twelve pixels in the status bar, where a picture
        // of a thing is out of the question and a silhouette is all there is: what tells these
        // apart is which way their strokes run, so no two of them run the same way.
        Icon::Library => {
            // A list with a rail down its left. The rail is the only vertical stroke in the set,
            // which is what tells the library from the piano roll at that size.
            bar(window, 0.22, 0.22, 0.31, 0.78);
            bar(window, 0.40, 0.24, 0.78, 0.34);
            bar(window, 0.40, 0.45, 0.78, 0.55);
            bar(window, 0.40, 0.66, 0.78, 0.76);
        }
        Icon::Notes => {
            // Three notes at different places along a grid: the roll from far enough away.
            bar(window, 0.18, 0.24, 0.50, 0.35);
            bar(window, 0.38, 0.45, 0.82, 0.56);
            bar(window, 0.26, 0.66, 0.62, 0.77);
        }
        Icon::Faders => {
            // Two channel faders. Their knobs are at different heights on purpose: level with
            // each other the pair reads as an equals sign.
            bar(window, 0.30, 0.16, 0.38, 0.84);
            bar(window, 0.62, 0.16, 0.70, 0.84);
            knob(window, 0.34, 0.62, 0.30, 0.15);
            knob(window, 0.66, 0.38, 0.30, 0.15);
        }
        Icon::Sliders => {
            // The same controls lying down, which is how the inspector arranges them.
            bar(window, 0.16, 0.30, 0.84, 0.38);
            bar(window, 0.16, 0.62, 0.84, 0.70);
            knob(window, 0.62, 0.34, 0.15, 0.30);
            knob(window, 0.36, 0.66, 0.15, 0.30);
        }
        Icon::Log => {
            // Four lines of writing. Ragged right, because a block of equal bars reads as a
            // table or a fader bank — the uneven ends are the whole of what says "text".
            bar(window, 0.16, 0.22, 0.84, 0.30);
            bar(window, 0.16, 0.40, 0.68, 0.48);
            bar(window, 0.16, 0.58, 0.80, 0.66);
            bar(window, 0.16, 0.76, 0.52, 0.84);
        }
        // The three kinds of thing the library holds. They sit on the leaf rows, where the tree
        // has run out of headings to say what something is — an instrument, an effect and a sound
        // are three different clicks, and until these arrived the rows looked identical.
        Icon::Keyboard => {
            // A keyboard seen from the front: the case across the top, two keys hanging off it.
            // Gaps are the only thing that can say "keys" in a single-colour silhouette, so the
            // keys are separate bars rather than one block with lines drawn on it.
            //
            // Two keys and not three. Three left gaps of a tenth of the box, which at the size a
            // library row draws this is under a pixel — so they vanished or greyed into the keys,
            // and the glyph read as a lower-case m. One gap can be half as wide again as three
            // could, and the only thing this has to survive at is small.
            bar(window, 0.16, 0.24, 0.84, 0.38);
            bar(window, 0.20, 0.38, 0.42, 0.76);
            bar(window, 0.58, 0.38, 0.80, 0.76);
        }
        Icon::Knob => {
            ring(window, 0.50, 0.52, 0.30, 0.10);
            // The pointer runs from the middle out through the ring, which is what makes the
            // circle a control rather than a full stop.
            bar(window, 0.455, 0.14, 0.545, 0.52);
        }
        Icon::Record => {
            // A filled disc, drawn as a square rounded until it has no corners left. Smaller than
            // the other transport glyphs on purpose: a circle of the same width reads as heavier
            // than a triangle or a pair of bars, which is why every hardware transport ever built
            // draws its record button a size down.
            knob(window, 0.50, 0.50, 0.46, 0.46);
        }
        Icon::Wave => {
            // A sine and a half. One period reads as a bracket; the extra half is what makes it
            // periodic, and periodic is what makes it a sound.
            let mut builder = PathBuilder::stroke(px((side * 0.10).max(1.25)));
            builder.move_to(at(0.14, 0.50));
            builder.curve_to(at(0.38, 0.50), at(0.26, 0.16));
            builder.curve_to(at(0.62, 0.50), at(0.50, 0.84));
            builder.curve_to(at(0.86, 0.50), at(0.74, 0.16));
            if let Ok(path) = builder.build() {
                window.paint_path(path, color);
            }
        }
        Icon::Cross => {
            // Two bars rotated 45°, drawn as paths because quads cannot be rotated.
            stroke_line(
                window,
                at(0.30, 0.30),
                at(0.70, 0.70),
                px(side * 0.09),
                color,
            );
            stroke_line(
                window,
                at(0.70, 0.30),
                at(0.30, 0.70),
                px(side * 0.09),
                color,
            );
        }
    }
}

fn rounded(window: &mut Window, bounds: Bounds<Pixels>, radius: Pixels, color: Hsla) {
    if bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0) {
        return;
    }
    // Icon bars are only a pixel or two thick, so an unclamped radius would exceed the shape.
    let limit = bounds.size.width.min(bounds.size.height) / 2.0;
    let radius = if radius > limit { limit } else { radius };
    window.paint_quad(fill(bounds, color).corner_radii(Corners::all(radius)));
}

fn triangle(
    window: &mut Window,
    a: Point<Pixels>,
    b: Point<Pixels>,
    c: Point<Pixels>,
    color: Hsla,
) {
    let mut builder = PathBuilder::fill();
    builder.move_to(a);
    builder.line_to(b);
    builder.line_to(c);
    builder.close();
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn stroke_line(
    window: &mut Window,
    from: Point<Pixels>,
    to: Point<Pixels>,
    width: Pixels,
    color: Hsla,
) {
    let mut builder = PathBuilder::stroke(width);
    builder.move_to(from);
    builder.line_to(to);
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}
