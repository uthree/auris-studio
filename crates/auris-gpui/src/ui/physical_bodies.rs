//! The bodies that are not plain strings, each drawn as the thing it is.
//!
//! Atelier draws a string ([`atelier::viz::StringLane`]) and a head ([`atelier::viz::Heatmap`]);
//! everything else a physical model rings with is drawn here, in the same ink: the rest position
//! on `grid`, the fixed parts of the instrument on `text_faint`, the voice in its series colour
//! with trails fading behind it, and a released voice at rest strength. A piano string has its
//! hammer under it and its damper over it, a mallet bar lies thick on its two nodal supports, a
//! bell's rim is a ring, and a bore or a vocal tract is a tube whose air is shaded by pressure.
//!
//! Every lane keeps [`super::physical_model`]'s ten-pixel inset at each end, so a press is read
//! as the same place along the body whichever of these draws it.

use std::f32::consts::TAU;

use gpui::{
    App, Bounds, Hsla, IntoElement, Pixels, Point, RenderOnce, Window, canvas, point, prelude::*,
    px, size,
};

use super::paint;
use crate::theme::Theme;

/// The margin each lane leaves at both ends, shared with [`atelier::viz::StringLane`].
pub(crate) const LANE_INSET: f32 = 10.;
/// The node positions of a free bar's first mode, where a xylophone bar is held.
pub(crate) const BAR_NODES: [f32; 2] = [0.224, 0.776];
/// How strong a released voice and its trails are drawn, as Atelier's string lane does.
const RESTING: f32 = 0.35;
const TRAILS: f32 = 0.35;

/// What a non-string body is, with what each one needs beyond its motion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Shape {
    /// A piano string: the hammer that struck it below, its damper above.
    Piano {
        /// Where the hammer meets the string, from zero to one.
        hammer: Option<f32>,
        /// Whether the hammer is against the string now.
        striking: bool,
        /// Whether the damper is down on the string.
        damped: bool,
    },
    /// A free bar on two supports at its nodes, and where the mallet lands on it.
    Bar {
        /// Where the mallet strikes, from zero to one.
        mallet: Option<f32>,
    },
    /// A bell's rim seen from above: a ring, and where on it the clapper struck.
    Ring {
        /// Where around the rim, as a fraction of a turn from the top.
        contact: Option<f32>,
    },
    /// An air column from its mouthpiece to its open end.
    Bore(BoreEnd),
    /// A vocal tract from the glottis to the lips, as wide as its eight tubes.
    Tract([f32; 8]),
}

/// How a bore is set sounding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoreEnd {
    /// A reed on a closed mouthpiece, and a flared bell at the far end: a clarinet.
    Reed,
    /// A fipple's plug and window, and an open end: a whistle.
    Fipple,
}

/// One body drawn as its [`Shape`].
#[derive(IntoElement)]
pub(crate) struct BodyPlot {
    shape: Shape,
    points: Vec<f32>,
    trails: Vec<Vec<f32>>,
    active: bool,
    ink: Ink,
}

/// The theme's colours a body is painted in, gathered off the theme once.
#[derive(Clone, Copy)]
struct Ink {
    voice: Hsla,
    rest: Hsla,
    fixed: Hsla,
    outline: Hsla,
}

impl BodyPlot {
    /// `shape` displaced by `points`, each from -1 to 1, in the voice colour `color`.
    pub(crate) fn new(shape: Shape, points: Vec<f32>, color: Hsla, theme: &Theme) -> Self {
        let colors = &theme.atelier.colors;
        Self {
            shape,
            points,
            trails: Vec::new(),
            active: true,
            ink: Ink {
                voice: color,
                rest: colors.grid,
                fixed: colors.text_faint,
                outline: colors.text,
            },
        }
    }

    /// Earlier frames, oldest first, drawn fading behind the current one.
    pub(crate) fn trails(mut self, trails: Vec<Vec<f32>>) -> Self {
        self.trails = trails;
        self
    }

    /// A released or silent body, drawn at rest strength.
    pub(crate) fn inactive(mut self) -> Self {
        self.active = false;
        self
    }
}

impl RenderOnce for BodyPlot {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                paint::clipped(window, bounds, |window| match self.shape {
                    Shape::Piano {
                        hammer,
                        striking,
                        damped,
                    } => paint_piano(window, bounds, &self, hammer, striking, damped),
                    Shape::Bar { mallet } => paint_bar(window, bounds, &self, mallet),
                    Shape::Ring { contact } => paint_ring(window, bounds, &self, contact),
                    Shape::Bore(end) => paint_bore(window, bounds, &self, end),
                    Shape::Tract(areas) => paint_tract(window, bounds, &self, &areas),
                });
            },
        )
        .size_full()
    }
}

/// A lane's frame: where along it each point lies and where its rest line is.
struct Lane {
    left: Pixels,
    width: Pixels,
    middle: Pixels,
    half: Pixels,
}

impl Lane {
    fn of(bounds: Bounds<Pixels>) -> Self {
        let inset = px(LANE_INSET);
        Self {
            left: bounds.left() + inset,
            width: (bounds.size.width - inset * 2.).max(px(1.)),
            middle: bounds.top() + bounds.size.height / 2.,
            half: bounds.size.height / 2. * 0.8,
        }
    }

    /// The x of a place `at` along the body, from zero to one.
    fn x(&self, at: f32) -> Pixels {
        self.left + self.width * at.clamp(0., 1.)
    }

    /// The x of point `index` of `count`.
    fn x_of(&self, index: usize, count: usize) -> Pixels {
        self.x(index as f32 / count.saturating_sub(1).max(1) as f32)
    }

    /// The points of a curve displaced up to `half` from the rest line.
    fn curve(&self, values: &[f32], half: Pixels) -> Vec<Point<Pixels>> {
        values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                point(
                    self.x_of(index, values.len()),
                    self.middle - half * value.clamp(-1., 1.),
                )
            })
            .collect()
    }
}

impl BodyPlot {
    fn strength(&self) -> f32 {
        if self.active { 1. } else { RESTING }
    }

    /// The voice colour where the voice is sounding, the instrument's own where it rests.
    fn part(&self) -> Hsla {
        if self.active {
            self.ink.voice
        } else {
            self.ink.fixed
        }
    }

    /// Value at a place along the body, from the nearest point.
    fn value_at(&self, at: f32) -> f32 {
        let last = self.points.len().saturating_sub(1);
        let index = (last as f32 * at.clamp(0., 1.)).round() as usize;
        self.points.get(index).copied().unwrap_or(0.)
    }

    /// Faded earlier frames, each through `curve`.
    fn paint_trails(
        &self,
        window: &mut Window,
        curve: impl Fn(&[f32]) -> Vec<Point<Pixels>>,
        closed: bool,
    ) {
        let count = self.trails.len();
        for (age, trail) in self.trails.iter().enumerate() {
            let strength = (age + 1) as f32 / (count + 1) as f32 * TRAILS;
            let mut points = curve(trail);
            if closed && let Some(first) = points.first().copied() {
                points.push(first);
            }
            paint::polyline(window, &points, px(1.), self.ink.voice.opacity(strength));
        }
    }

    /// The end posts a string is stretched between, as Atelier's lane draws them.
    fn paint_anchors(&self, window: &mut Window, lane: &Lane) {
        for end in [lane.left, lane.left + lane.width] {
            paint::rounded_rect(
                window,
                Bounds::new(
                    point(end - px(1.5), lane.middle - px(8.)),
                    size(px(3.), px(16.)),
                ),
                px(1.5),
                self.ink.fixed,
            );
        }
    }

    /// A struck place: a dot in the voice colour, ringed so it reads on the curve.
    fn paint_contact(&self, window: &mut Window, at: Point<Pixels>) {
        let radius = px(5.);
        window.paint_quad(
            gpui::fill(
                Bounds::new(
                    point(at.x - radius, at.y - radius),
                    size(radius * 2., radius * 2.),
                ),
                self.ink.voice,
            )
            .corner_radii(radius)
            .border_widths(px(1.5))
            .border_color(self.ink.outline),
        );
    }
}

/// Fills a closed polygon.
fn polygon(window: &mut Window, points: &[Point<Pixels>], color: Hsla) {
    let Some((first, rest)) = points.split_first() else {
        return;
    };
    if rest.len() < 2 {
        return;
    }
    let mut path = gpui::PathBuilder::fill();
    path.move_to(*first);
    for at in rest {
        path.line_to(*at);
    }
    path.close();
    if let Ok(built) = path.build() {
        window.paint_path(built, color);
    }
}

fn paint_piano(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    plot: &BodyPlot,
    hammer: Option<f32>,
    striking: bool,
    damped: bool,
) {
    let lane = Lane::of(bounds);
    paint::polyline(
        window,
        &[
            point(lane.left, lane.middle),
            point(lane.left + lane.width, lane.middle),
        ],
        px(1.),
        plot.ink.rest,
    );
    plot.paint_anchors(window, &lane);
    // The damper sits over the string just past the hammer line, felt on a wire from above.
    let damper_at = hammer.unwrap_or(0.12) + 0.08;
    let damper = size(px(16.), px(6.));
    let lifted = if damped { px(0.) } else { px(9.) };
    let damper_top = lane.middle - px(1.) - damper.height - lifted;
    let damper_x = lane.x(damper_at) - damper.width / 2.;
    paint::rect(
        window,
        Bounds::new(
            point(damper_x + damper.width / 2. - px(0.5), bounds.top()),
            size(px(1.), (damper_top - bounds.top()).max(px(0.))),
        ),
        plot.ink.fixed,
    );
    paint::rounded_rect(
        window,
        Bounds::new(point(damper_x, damper_top), damper),
        px(2.),
        plot.ink.fixed.opacity(if damped { 1. } else { 0.6 }),
    );
    plot.paint_trails(window, |values| lane.curve(values, lane.half), false);
    let now = lane.curve(&plot.points, lane.half);
    paint::polyline(
        window,
        &now,
        px(2.),
        plot.ink.voice.opacity(plot.strength()),
    );
    if let Some(at) = hammer {
        // A felt head on its shank, against the string while it strikes and just below it after.
        let head = size(px(10.), px(7.));
        let x = lane.x(at);
        let string = lane.middle - lane.half * plot.value_at(at).clamp(-1., 1.);
        let top = if striking {
            string + px(1.)
        } else {
            lane.middle + px(9.)
        };
        paint::rect(
            window,
            Bounds::new(
                point(x - px(1.), top + head.height),
                size(px(2.), (bounds.bottom() - top - head.height).max(px(0.))),
            ),
            plot.ink.fixed,
        );
        paint::rounded_rect(
            window,
            Bounds::new(point(x - head.width / 2., top), head),
            px(3.),
            if striking {
                plot.ink.voice
            } else {
                plot.part()
            },
        );
    }
}

fn paint_bar(window: &mut Window, bounds: Bounds<Pixels>, plot: &BodyPlot, mallet: Option<f32>) {
    let lane = Lane::of(bounds);
    let half = lane.half * 0.65;
    let thickness = (bounds.size.height * 0.14).clamp(px(4.), px(10.));
    // The cords the bar hangs on cross under it at the nodes, which barely move.
    for node in BAR_NODES {
        let x = lane.x(node);
        let top = lane.middle + thickness / 2. + px(2.);
        polygon(
            window,
            &[
                point(x, top),
                point(x - px(5.), top + px(8.)),
                point(x + px(5.), top + px(8.)),
            ],
            plot.ink.fixed,
        );
    }
    paint::polyline(
        window,
        &[
            point(lane.left, lane.middle),
            point(lane.left + lane.width, lane.middle),
        ],
        px(1.),
        plot.ink.rest,
    );
    plot.paint_trails(window, |values| lane.curve(values, half), false);
    let centre = lane.curve(&plot.points, half);
    let upper = centre.iter().map(|at| point(at.x, at.y - thickness / 2.));
    let lower = centre
        .iter()
        .rev()
        .map(|at| point(at.x, at.y + thickness / 2.));
    let outline: Vec<_> = upper.chain(lower).collect();
    polygon(
        window,
        &outline,
        plot.ink.voice.opacity(if plot.active { 0.9 } else { 0.3 }),
    );
    if let (Some(at), true) = (mallet, plot.active) {
        let y = lane.middle - half * plot.value_at(at) - thickness / 2. - px(6.);
        plot.paint_contact(window, point(lane.x(at), y));
    }
}

/// Where on a ring around `centre` a turn `fraction` from the top lies, clockwise.
pub(crate) fn ring_point(centre: Point<Pixels>, radius: Pixels, fraction: f32) -> Point<Pixels> {
    let angle = fraction * TAU;
    point(
        centre.x + radius * angle.sin(),
        centre.y - radius * angle.cos(),
    )
}

/// The turn from the top, clockwise, at which `position` lies around `centre`: the inverse of
/// [`ring_point`], so a press on a bell's rim reads as the place it is drawn.
pub(crate) fn ring_fraction(centre: Point<Pixels>, position: Point<Pixels>) -> f32 {
    let offset = position - centre;
    let angle = f32::from(offset.x).atan2(-f32::from(offset.y));
    angle.rem_euclid(TAU) / TAU
}

/// The radius of a bell's rim at rest in `bounds`, and how far a full displacement moves it.
pub(crate) fn ring_radius(bounds: Bounds<Pixels>) -> (Pixels, Pixels) {
    let radius = bounds.size.width.min(bounds.size.height) / 2. * 0.62;
    (radius, radius * 0.35)
}

fn paint_ring(window: &mut Window, bounds: Bounds<Pixels>, plot: &BodyPlot, contact: Option<f32>) {
    let centre = bounds.center();
    let (radius, reach) = ring_radius(bounds);
    // The perimeter is periodic: the last point is the first again.
    let ring = |values: &[f32]| -> Vec<Point<Pixels>> {
        let turn = values.len().saturating_sub(1).max(1);
        values[..turn.min(values.len())]
            .iter()
            .enumerate()
            .map(|(index, value)| {
                ring_point(
                    centre,
                    radius + reach * value.clamp(-1., 1.),
                    index as f32 / turn as f32,
                )
            })
            .collect()
    };
    let rest: Vec<_> = (0..=64)
        .map(|step| ring_point(centre, radius, step as f32 / 64.))
        .collect();
    paint::polyline(window, &rest, px(1.), plot.ink.rest);
    // The crown the bell hangs from, at the centre of its rim.
    let crown = px(3.);
    paint::rounded_rect(
        window,
        Bounds::new(
            point(centre.x - crown, centre.y - crown),
            size(crown * 2., crown * 2.),
        ),
        crown,
        plot.ink.fixed,
    );
    plot.paint_trails(window, ring, true);
    let mut now = ring(&plot.points);
    if let Some(first) = now.first().copied() {
        now.push(first);
    }
    paint::polyline(
        window,
        &now,
        px(2.),
        plot.ink.voice.opacity(plot.strength()),
    );
    if let (Some(at), true) = (contact, plot.active) {
        let value = plot.value_at(at);
        plot.paint_contact(
            window,
            ring_point(centre, radius + reach * value.clamp(-1., 1.), at),
        );
    }
}

/// Fills the air of a tube column by column, as strong as the pressure in it.
fn paint_air(window: &mut Window, lane: &Lane, plot: &BodyPlot, half_at: impl Fn(f32) -> Pixels) {
    let count = plot.points.len();
    let step = lane.width / count.saturating_sub(1).max(1) as f32;
    for (index, value) in plot.points.iter().enumerate() {
        let at = index as f32 / count.saturating_sub(1).max(1) as f32;
        let half = half_at(at);
        let amount = value.abs().clamp(0., 1.) * 0.55 * plot.strength();
        if amount <= 0.01 {
            continue;
        }
        let x = lane.x(at) - step / 2.;
        paint::rect(
            window,
            Bounds::new(
                point(x.max(lane.left), lane.middle - half),
                size(step + px(0.5), half * 2.),
            ),
            plot.ink.voice.opacity(amount),
        );
    }
}

fn paint_bore(window: &mut Window, bounds: Bounds<Pixels>, plot: &BodyPlot, end: BoreEnd) {
    let lane = Lane::of(bounds);
    let bore = (bounds.size.height * 0.27).clamp(px(6.), px(18.));
    let flare = |at: f32| -> Pixels {
        match end {
            // A clarinet's bell opens over the last tenth of its length.
            BoreEnd::Reed if at > 0.9 => bore * (1. + ((at - 0.9) / 0.1).powi(2) * 0.7),
            _ => bore,
        }
    };
    paint_air(window, &lane, plot, flare);
    let wall = |sign: f32, from: f32| -> Vec<Point<Pixels>> {
        (0..=40)
            .map(|step| from + (1. - from) * step as f32 / 40.)
            .map(|at| point(lane.x(at), lane.middle + flare(at) * sign))
            .collect()
    };
    match end {
        BoreEnd::Reed => {
            paint::polyline(window, &wall(-1., 0.), px(2.), plot.ink.fixed);
            paint::polyline(window, &wall(1., 0.), px(2.), plot.ink.fixed);
            // The mouthpiece is closed by the reed, which lies along its underside.
            paint::rounded_rect(
                window,
                Bounds::new(
                    point(lane.left - px(3.), lane.middle - bore),
                    size(px(3.), bore * 2.),
                ),
                px(1.),
                plot.ink.fixed,
            );
            paint::polyline(
                window,
                &[
                    point(lane.left - px(6.), lane.middle + bore + px(3.)),
                    point(lane.x(0.07), lane.middle + bore),
                ],
                px(2.),
                plot.part(),
            );
        }
        BoreEnd::Fipple => {
            // A plug fills the head but for the windway along its top, which blows across the
            // window onto the labium's edge.
            let window_at = (0.05, 0.1);
            paint::polyline(
                window,
                &[
                    point(lane.left, lane.middle - bore),
                    point(lane.x(window_at.0), lane.middle - bore),
                ],
                px(2.),
                plot.ink.fixed,
            );
            paint::polyline(window, &wall(-1., window_at.1), px(2.), plot.ink.fixed);
            paint::polyline(window, &wall(1., 0.), px(2.), plot.ink.fixed);
            paint::rounded_rect(
                window,
                Bounds::new(
                    point(lane.left, lane.middle - bore + px(4.)),
                    size(lane.x(window_at.0) - lane.left, bore * 2. - px(4.)),
                ),
                px(1.),
                plot.ink.fixed.opacity(0.6),
            );
            polygon(
                window,
                &[
                    point(lane.x(window_at.1) - px(6.), lane.middle - bore),
                    point(lane.x(window_at.1), lane.middle - bore - px(2.)),
                    point(lane.x(window_at.1), lane.middle - bore + px(2.)),
                ],
                plot.part(),
            );
        }
    }
    // Tone holes along the top, drawn open: the model does not finger them.
    let holes: &[f32] = match end {
        BoreEnd::Reed => &[0.38, 0.45, 0.52, 0.6, 0.67, 0.74, 0.81],
        BoreEnd::Fipple => &[0.45, 0.53, 0.61, 0.71, 0.79, 0.87],
    };
    for at in holes {
        let radius = px(2.5);
        paint::rounded_outline(
            window,
            Bounds::new(
                point(lane.x(*at) - radius, lane.middle - bore - radius),
                size(radius * 2., radius * 2.),
            ),
            radius,
            px(1.),
            plot.ink.fixed,
        );
    }
    plot.paint_trails(window, |values| lane.curve(values, bore * 0.85), false);
    let now = lane.curve(&plot.points, bore * 0.85);
    paint::polyline(
        window,
        &now,
        px(2.),
        plot.ink.voice.opacity(plot.strength()),
    );
}

/// A tract's half-width at `at`, from its eight tubes' areas relative to the widest: the
/// radius follows the square root of the area, joined straight between the tubes' centres, and
/// never quite closes, since a tube shut tight would hide what is in it.
pub(crate) fn tract_width(areas: &[f32; 8], at: f32) -> f32 {
    let position = (at.clamp(0., 1.) * 8. - 0.5).clamp(0., 7.);
    let below = position.floor() as usize;
    let above = (below + 1).min(7);
    let blend = position - below as f32;
    let radius = |area: f32| area.clamp(0., 1.).sqrt();
    let radius = radius(areas[below]) * (1. - blend) + radius(areas[above]) * blend;
    0.18 + 0.82 * radius
}

fn paint_tract(window: &mut Window, bounds: Bounds<Pixels>, plot: &BodyPlot, areas: &[f32; 8]) {
    let lane = Lane::of(bounds);
    let widest = bounds.size.height * 0.42;
    let half_at = |at: f32| widest * tract_width(areas, at);
    paint_air(window, &lane, plot, half_at);
    let wall = |sign: f32| -> Vec<Point<Pixels>> {
        (0..=48)
            .map(|step| step as f32 / 48.)
            .map(|at| point(lane.x(at), lane.middle + half_at(at) * sign))
            .collect()
    };
    paint::polyline(window, &wall(-1.), px(2.), plot.ink.fixed);
    paint::polyline(window, &wall(1.), px(2.), plot.ink.fixed);
    // The folds at the glottis, which set the air going, and the lips it leaves by.
    let glottis = half_at(0.);
    for sign in [-1., 1.] {
        paint::rounded_rect(
            window,
            Bounds::new(
                point(
                    lane.left - px(1.5),
                    lane.middle + if sign < 0. { -glottis } else { px(2.) },
                ),
                size(px(3.), glottis - px(2.)),
            ),
            px(1.5),
            plot.part(),
        );
        let lips = half_at(1.);
        let lip = size(px(5.), px(4.));
        paint::rounded_rect(
            window,
            Bounds::new(
                point(
                    lane.left + lane.width - lip.width / 2.,
                    lane.middle + lips * sign - lip.height / 2.,
                ),
                lip,
            ),
            px(2.),
            plot.ink.fixed,
        );
    }
    let curve = |values: &[f32]| -> Vec<Point<Pixels>> {
        values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let at = index as f32 / values.len().saturating_sub(1).max(1) as f32;
                point(
                    lane.x(at),
                    lane.middle - half_at(at) * 0.85 * value.clamp(-1., 1.),
                )
            })
            .collect()
    };
    plot.paint_trails(window, curve, false);
    let now = curve(&plot.points);
    paint::polyline(
        window,
        &now,
        px(2.),
        plot.ink.voice.opacity(plot.strength()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn a_press_on_the_rim_reads_back_as_the_place_it_is_drawn() {
        let centre = point(px(100.), px(50.));
        for fraction in [0., 0.125, 0.25, 0.5, 0.8] {
            let at = ring_point(centre, px(30.), fraction);
            assert!(close(ring_fraction(centre, at), fraction), "{fraction}");
        }
        // Clockwise from the top: a quarter turn is the right-hand side.
        let right = ring_point(centre, px(30.), 0.25);
        assert!(close(f32::from(right.x), 130.) && close(f32::from(right.y), 50.));
    }

    #[test]
    fn a_ring_fits_its_box_with_room_to_move() {
        let (radius, reach) =
            ring_radius(Bounds::new(point(px(0.), px(0.)), size(px(200.), px(100.))));
        assert!(close(f32::from(radius), 31.));
        assert!(radius + reach < px(50.), "a full displacement stays inside");
    }

    #[test]
    fn a_tract_is_as_wide_as_its_tubes() {
        let mut areas = [1.; 8];
        areas[0] = 0.;
        // At a tube's centre the width is that tube's own.
        assert!(close(tract_width(&areas, 0.5 / 8.), 0.18));
        assert!(close(tract_width(&areas, 4.5 / 8.), 1.));
        // Halfway between the shut tube and the open one, the radius is halfway too.
        assert!(close(tract_width(&areas, 1. / 8.), 0.18 + 0.82 * 0.5));
        // A quarter of the area is half the radius.
        assert!(close(tract_width(&[0.25; 8], 0.3), 0.18 + 0.82 * 0.5));
    }

    #[test]
    fn a_bar_is_held_at_its_first_modes_nodes() {
        // A free-free beam's first bending mode, which a tuned bar's fundamental is: the
        // supports sit where it does not move.
        let (beta, sigma) = (4.730_04_f32, 0.982_5_f32);
        let shape = |x: f32| {
            let b = beta * x;
            b.cosh() + b.cos() - sigma * (b.sinh() + b.sin())
        };
        for node in BAR_NODES {
            assert!(shape(node).abs() < 0.01, "{node}: {}", shape(node));
        }
        assert!(shape(0.5).abs() > 1., "while the middle swings");
    }
}
