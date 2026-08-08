//! The display across the top of the equalizer's window: what is going into it, and what it is
//! doing to that.
//!
//! Twenty-four sliders is a correct and unusable way to present an equalizer. Nobody hears
//! twenty-four numbers; they hear a shape, and the shape is the thing being adjusted — so the
//! curve is drawn over the spectrum it is being aimed at, with a node on each band that follows
//! the pointer. Logic's Channel EQ is this, and so is every equalizer anybody reaches for.
//!
//! The sliders stay underneath, on [`crate::ui::envelope`]'s reasoning: a graph is how a shape is
//! *found* and a number is how it is *said*, and neither answers for the other.
//!
//! Everything above the view half is a free function with tests, on the rule the rest of the
//! window follows: a gpui view cannot be unit-tested, so what it decides lives outside it.

use auris_session::prelude::*;

/// Lowest frequency the display shows, in hertz.
///
/// Narrower than the range a band may be *set* to, which reaches down to 20 Hz. The bottom octave
/// is where the analyser has fewest bins and the ear fewest opinions, and spending a tenth of the
/// width on it would cost every band above it — so a node dragged to the left edge stops here,
/// and the slider underneath is what reaches the last ten hertz.
pub const LOW_HZ: f64 = 30.0;

/// Highest frequency the display shows, in hertz.
pub const HIGH_HZ: f64 = 18_000.0;

/// Level at the bottom of the spectrum, in dBFS.
pub const FLOOR_DB: f32 = -72.0;

/// Where a frequency sits across the display, from 0 at the left edge to 1 at the right.
///
/// Logarithmic, because pitch is, and because that is how `auris_dsp::bands_from_bins` spaces the
/// bands — the scale, the spectrum and the curve have to agree or the numbers under them are
/// decoration.
pub fn x_of(hz: f64) -> f32 {
    let span = (HIGH_HZ / LOW_HZ).ln();
    ((hz / LOW_HZ).ln() / span).clamp(0.0, 1.0) as f32
}

/// The frequency a position across the display stands for. The inverse of [`x_of`].
pub fn hz_at(x: f32) -> f32 {
    let span = (HIGH_HZ / LOW_HZ).ln();
    (LOW_HZ * (f64::from(x.clamp(0.0, 1.0)) * span).exp()) as f32
}

/// Where a gain sits up the display, from 0 at the bottom to 1 at the top.
///
/// The axis is the gain parameter's own range rather than a prettier number, so every value a
/// band can hold is somewhere a node can be dragged to. A display that stopped at ±18 dB would
/// make the last six unreachable from the graph, which is a control that quietly does less than
/// the one beside it.
pub fn y_of(gain_db: f32, limit: f32) -> f32 {
    let limit = limit.max(f32::EPSILON);
    ((gain_db / limit + 1.0) / 2.0).clamp(0.0, 1.0)
}

/// The gain a height up the display stands for. The inverse of [`y_of`].
pub fn db_at(y: f32, limit: f32) -> f32 {
    (y.clamp(0.0, 1.0) * 2.0 - 1.0) * limit
}

/// Where a band's node sits in the unit box: x rightwards from 0, y *upwards* from 0.
///
/// A shape with no gain sits on the centre line, because that is where its curve crosses: a
/// high-pass has a corner, not a level, and putting its node anywhere else would be drawing a
/// boost nobody asked for.
pub fn node_of(band: EqBandSetting, gain_limit: f32) -> (f32, f32) {
    let gain = if band.kind.has_gain() {
        band.gain_db
    } else {
        0.0
    };
    (x_of(f64::from(band.frequency)), y_of(gain, gain_limit))
}

/// The node nearest `at` and within `radius` of it, or nothing.
///
/// A radius per axis, because the box is wider than it is tall and a grab zone measured in pixels
/// is a different fraction of each. Nearest rather than first, so two bands dragged on top of one
/// another still resolve to the one under the pointer.
///
/// A band that is switched off has no node and cannot be grabbed. It is not on the curve either,
/// so a handle for it would be a control floating over a shape it has no part in; the toggle in
/// the list below is what switches it in.
pub fn node_at(
    bands: &[EqBandSetting],
    gain_limit: f32,
    at: (f32, f32),
    radius: (f32, f32),
) -> Option<usize> {
    let (rx, ry) = (radius.0.max(f32::EPSILON), radius.1.max(f32::EPSILON));
    bands
        .iter()
        .enumerate()
        .filter(|(_, band)| band.enabled)
        .filter_map(|(index, band)| {
            // Scaled into a circle so "within the zone" means the same thing in both directions.
            let node = node_of(*band, gain_limit);
            let dx = (node.0 - at.0) / rx;
            let dy = (node.1 - at.1) / ry;
            let distance = dx * dx + dy * dy;
            (distance <= 1.0).then_some((index, distance))
        })
        .min_by(|one, other| one.1.total_cmp(&other.1))
        .map(|(index, _)| index)
}

/// The band after its node has been dragged to `at` in the unit box.
///
/// Both numbers at once, which is the whole point of the graph: where a band is and how much of
/// it there is are the pair a person is thinking about when they reach for it. A shape with no
/// level keeps the gain it had rather than taking one from the height of the pointer — see
/// [`EqBandKind::has_gain`].
pub fn moved(band: EqBandSetting, at: (f32, f32), gain_limit: f32) -> EqBandSetting {
    EqBandSetting {
        frequency: hz_at(at.0),
        gain_db: if band.kind.has_gain() {
            db_at(at.1, gain_limit)
        } else {
            band.gain_db
        },
        ..band
    }
}

/// How much one notch of the wheel multiplies a band's Q by.
///
/// Multiplied rather than added, because Q is read logarithmically: the step from 0.5 to 1.0 is
/// the same musical change as the step from 4 to 8, and a fixed increment would crawl at the
/// bottom of the range and leap at the top.
///
/// A quarter, which crosses the useful range — a broad 0.5 to a surgical 8 — in a dozen notches.
/// A twelfth was the first guess and it was a control that looked broken: a notch moved a Q of
/// 0.71 to 0.80, which on a curve three hundred pixels wide is nothing anybody can see happening.
const Q_PER_NOTCH: f32 = 1.25;

/// The band's Q after `notches` of the wheel over its node.
///
/// Upwards narrows, which is the direction every equalizer with a wheel has chosen: the gesture
/// is "tighten this", and a wheel pushed away from the hand reads as more of the thing under it.
pub fn resonance_after_scroll(q: f32, notches: f32, range: (f32, f32)) -> f32 {
    let (low, high) = (range.0.min(range.1), range.0.max(range.1));
    (q * Q_PER_NOTCH.powf(notches)).clamp(low, high)
}

/// An equalizer as the display holds it, with the ranges its numbers move in.
///
/// The ranges travel with the values for the reason [`crate::ui::envelope::Adsr`]'s do: every
/// function here has to map a pixel back to a number, and the plugin's descriptors are the only
/// honest source for what that number may be.
#[derive(Clone, Debug, PartialEq)]
pub struct EqView {
    /// The bands, low to high, in the order of `EQ_LAYOUT`.
    pub bands: Vec<EqBandSetting>,
    /// How far a band's gain reaches either side of flat, in dB.
    pub gain_limit: f32,
    /// The least and most resonant a band may be set.
    pub q_range: (f32, f32),
    /// The rate the curve is drawn at, which is the rate the audio is made at.
    pub sample_rate: f64,
}

// ---------------------------------------------------------------- the spectrum

/// How many bars the spectrum is gathered into.
///
/// Far fewer than the window has bins. A display a few hundred pixels wide cannot show five
/// hundred of them, and a musician reads bands rather than bins — so the bins are gathered into
/// this many, spaced by octave.
const SPECTRUM_BANDS: usize = 64;

/// The frequencies the scale marks, and whether each carries its number.
///
/// Decades are named and the steps between them are only ruled. A line every octave with a
/// number on it is a ruler; three numbers and a set of ticks is a scale somebody can read while
/// looking at something else, which is what an analyser is for.
const TICKS: [(f64, bool); 8] = [
    (50.0, false),
    (100.0, true),
    (200.0, false),
    (500.0, false),
    (1_000.0, true),
    (2_000.0, false),
    (5_000.0, false),
    (10_000.0, true),
];

/// How tall the strip carrying the numbers is.
const SCALE_HEIGHT: f32 = 12.0;

/// The longest run of unmeasured bands [`bridge_gaps`] will draw across.
///
/// Five at the very bottom of the display, where the gaps are widest; the number has one spare.
const SPECTRUM_GAP: usize = 6;

/// How a frequency is written on the scale.
fn hz_label(hz: f64) -> String {
    if hz >= 1_000.0 {
        format!("{:.0}k", hz / 1_000.0)
    } else {
        format!("{hz:.0}")
    }
}

/// Bridges the gaps a logarithmic display leaves at the bottom of the spectrum.
///
/// At 30 Hz a band is four hertz wide and the analyser's bins are twenty-odd hertz apart, so most
/// of the bottom octave has no bin in it at all and comes back at the floor. Drawn as bars nobody
/// noticed; drawn as a line it is a comb, and the comb is a property of the display rather than
/// of the sound.
///
/// So a *short* run at the floor between two measured bands is interpolated across: the analyser
/// had nothing to say there, and a line that dives to the floor and back says "silence", which is
/// a stronger claim than it made. A long run is left exactly where it is, because that is what a
/// genuinely quiet part of the spectrum looks like — and so is a run at either end, which has
/// nothing on one side to interpolate from.
fn bridge_gaps(bands: &mut [f32], floor: f32, longest: usize) {
    let mut at = 0;
    while at < bands.len() {
        if bands[at] > floor {
            at += 1;
            continue;
        }
        let mut end = at;
        while end < bands.len() && bands[end] <= floor {
            end += 1;
        }
        if end - at <= longest && at > 0 && end < bands.len() {
            let (before, after) = (bands[at - 1], bands[end]);
            let steps = (end - at + 1) as f32;
            for (offset, index) in (at..end).enumerate() {
                bands[index] = before + (after - before) * (offset + 1) as f32 / steps;
            }
        }
        at = end;
    }
}

// ---------------------------------------------------------------- the view

use gpui::{
    Bounds, IntoElement, MouseDownEvent, Pixels, Point, Window, canvas, div, point, prelude::*, px,
    size,
};

use crate::app::{AurisApp, Drag};
use crate::theme::Theme;
use crate::ui::paint;
use crate::ui::paint::unit_of;
use crate::ui::plugin_window::PluginSubject;

/// How tall the whole display is drawn, scale included.
pub const HEIGHT: f32 = 148.0;

/// How near a node a press has to land to take hold of it, in pixels.
const GRAB: f32 = 11.0;

/// How large a node is drawn.
const NODE_RADIUS: f32 = 4.0;

/// How far apart the curve is sampled, in pixels.
///
/// Every sample redesigns six biquads, so this is the one place the display costs anything. Three
/// pixels is under the width of the stroke — the curve is smooth at any zoom the window reaches —
/// and it holds the arithmetic to a couple of hundred filters a frame.
const CURVE_STEP: f32 = 3.0;

/// The gains the display rules, in dB either side of flat.
const GAIN_MARKS: [f32; 2] = [6.0, 12.0];

impl AurisApp {
    /// The equalizer on `subject`, as the display holds it, or nothing if this is not one.
    ///
    /// A list of one plugin id rather than a method on the `Effect` trait: what a *display* offers
    /// is a property of the editor, not of the processor, and asking every plugin author about a
    /// window they have never seen would put a frontend's concern into the plugin contract. The
    /// equalizer is the one that needs it — its whole job is deciding where to put a curve, and
    /// the numbers alone do not say where.
    pub(crate) fn eq_view(&mut self, subject: PluginSubject, plugin_id: &str) -> Option<EqView> {
        if plugin_id != EQUALIZER_ID {
            return None;
        }
        let descriptors = self.session.param_descriptors(plugin_id);
        let find = |key: &str| descriptors.iter().find(|entry| entry.key == key);

        let mut bands = Vec::with_capacity(EQ_LAYOUT.len());
        let mut gain_limit = 0.0;
        let mut q_range = (0.0, 1.0);
        for entry in EQ_LAYOUT {
            let (on, frequency, gain, q) = (
                find(entry.keys[0])?,
                find(entry.keys[1])?,
                find(entry.keys[2])?,
                find(entry.keys[3])?,
            );
            let read = |descriptor: &ParamDescriptor| {
                self.session
                    .param_value(subject.param_target(descriptor.id), descriptor)
            };
            gain_limit = gain.max;
            q_range = (q.min, q.max);
            bands.push(EqBandSetting {
                kind: entry.kind,
                enabled: read(on) >= 0.5,
                frequency: read(frequency),
                gain_db: read(gain),
                q: read(q),
            });
        }
        Some(EqView {
            bands,
            gain_limit,
            q_range,
            sample_rate: self.session.sample_rate(),
        })
    }

    /// The current spectrum, gathered into bands ready to draw.
    ///
    /// A torn read — the audio thread wrote through the copy — returns the floor rather than a
    /// window with a seam in it. The next repaint is sixteen milliseconds away and will find a
    /// settled one, which is cheaper than making the audio thread wait for this.
    fn spectrum_bins(&mut self) -> Vec<f32> {
        let mut bands = vec![Session::spectrum_silence(); SPECTRUM_BANDS];
        self.session.spectrum(LOW_HZ, HIGH_HZ, &mut bands);
        bands
    }

    /// The spectrum, the curve over it, and a node on each band that is switched in.
    pub(crate) fn analyser_display(
        &mut self,
        subject: PluginSubject,
        view: EqView,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::AnyElement {
        let mut spectrum = self.spectrum_bins();
        bridge_gaps(&mut spectrum, Session::spectrum_silence(), SPECTRUM_GAP);

        let theme = self.theme.clone();
        let recorded = self.canvas.analyser.clone();
        let painted = theme.clone();
        let drawn = view.clone();
        div()
            .h(px(HEIGHT))
            .w_full()
            .flex_shrink_0()
            .px_2()
            .py_1()
            .bg(theme.surface_sunken)
            .border_b_1()
            .border_color(theme.border)
            .cursor_pointer()
            .child(
                canvas(
                    move |bounds, _, _| recorded.set(Some(plot_of(bounds))),
                    move |bounds, _, window, cx| {
                        paint_analyser(window, cx, bounds, &spectrum, &drawn, &painted);
                    },
                )
                .size_full(),
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.press_eq_node(subject, event.position);
                    cx.notify();
                }),
            )
            .on_scroll_wheel(
                cx.listener(move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                    let notches = f32::from(event.delta.pixel_delta(px(16.0)).y) / 16.0;
                    this.scroll_eq_node(subject, event.position, notches);
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// The parameter behind one of the equalizer's keys.
    fn eq_target(&mut self, subject: PluginSubject, key: &str) -> Option<ParamTarget> {
        self.session
            .param_descriptors(EQUALIZER_ID)
            .iter()
            .find(|entry| entry.key == key)
            .map(|entry| subject.param_target(entry.id))
    }

    /// The equalizer behind the open window, and where its graph was drawn.
    fn eq_at_pointer(&mut self, subject: PluginSubject) -> Option<(EqView, Bounds<Pixels>)> {
        let bounds = self.canvas.analyser.get()?;
        let (plugin_id, _) = self.resolve_plugin(subject)?;
        let view = self.eq_view(subject, &plugin_id)?;
        Some((view, bounds))
    }

    /// Takes hold of whichever node the press landed on.
    fn press_eq_node(&mut self, subject: PluginSubject, at: Point<Pixels>) {
        let Some((view, bounds)) = self.eq_at_pointer(subject) else {
            return;
        };
        let radius = (
            GRAB / f32::from(bounds.size.width).max(1.0),
            GRAB / f32::from(bounds.size.height).max(1.0),
        );
        let Some(band) = node_at(&view.bands, view.gain_limit, unit_of(bounds, at), radius) else {
            return;
        };
        // Filed under the frequency, which is the one parameter every shape's node moves.
        let Some(target) = self.eq_target(subject, EQ_LAYOUT[band].keys[1]) else {
            return;
        };
        self.begin_drag(Drag::EqNode {
            subject,
            band,
            target,
        });
    }

    /// Moves the node in hand to where the pointer is.
    ///
    /// The settings are read afresh rather than carried in the drag, on the envelope graph's
    /// reasoning: nothing here can then go stale under an undo or a change of plugin.
    pub(crate) fn drag_eq_node(&mut self, subject: PluginSubject, band: usize, at: Point<Pixels>) {
        let Some((view, bounds)) = self.eq_at_pointer(subject) else {
            return;
        };
        let (Some(before), Some(keys)) = (
            view.bands.get(band).copied(),
            EQ_LAYOUT.get(band).map(|entry| entry.keys),
        ) else {
            return;
        };
        let after = moved(before, unit_of(bounds, at), view.gain_limit);
        if let Some(target) = self.eq_target(subject, keys[1]) {
            self.session.set_param(target, after.frequency);
        }
        if after.kind.has_gain()
            && let Some(target) = self.eq_target(subject, keys[2])
        {
            self.session.set_param(target, after.gain_db);
        }
    }

    /// Narrows or widens the band under the pointer.
    ///
    /// The third number a band has, and the one there is nowhere on a two-axis graph to put. The
    /// wheel is where every equalizer with a curve has put it, and it costs no screen: the graph
    /// sits outside the scrolling list, so the wheel over it meant nothing before.
    fn scroll_eq_node(&mut self, subject: PluginSubject, at: Point<Pixels>, notches: f32) {
        let Some((view, bounds)) = self.eq_at_pointer(subject) else {
            return;
        };
        let radius = (
            GRAB / f32::from(bounds.size.width).max(1.0),
            GRAB / f32::from(bounds.size.height).max(1.0),
        );
        let Some(band) = node_at(&view.bands, view.gain_limit, unit_of(bounds, at), radius) else {
            return;
        };
        let Some(target) = self.eq_target(subject, EQ_LAYOUT[band].keys[3]) else {
            return;
        };
        let q = resonance_after_scroll(view.bands[band].q, notches, view.q_range);
        self.session.set_param(target, q);
    }
}

/// The part of the display the graph occupies, above the strip carrying the numbers.
///
/// Recorded as the canvas rather than the whole rectangle, so a press and the node it is aimed at
/// are measured against the same box.
fn plot_of(bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    Bounds {
        origin: bounds.origin,
        size: size(
            bounds.size.width,
            (bounds.size.height - px(SCALE_HEIGHT)).max(px(1.0)),
        ),
    }
}

/// Draws the scale, the spectrum over it, then the equalizer's curve over that.
fn paint_analyser(
    window: &mut Window,
    cx: &mut gpui::App,
    bounds: Bounds<Pixels>,
    spectrum: &[f32],
    view: &EqView,
    theme: &Theme,
) {
    let plot = plot_of(bounds);
    let baseline = plot.origin.y + plot.size.height;
    let left = f32::from(plot.origin.x);
    let width = f32::from(plot.size.width);
    let top = f32::from(plot.origin.y);
    let height = f32::from(plot.size.height);
    let at = |unit: (f32, f32)| point(px(left + width * unit.0), px(top + height * (1.0 - unit.1)));

    for (hz, named) in TICKS {
        let x = px(left + width * x_of(hz));
        paint::rect(
            window,
            Bounds {
                origin: point(x, plot.origin.y),
                size: size(px(1.0), plot.size.height),
            },
            Theme::translucent(theme.border, if named { 1.0 } else { 0.5 }),
        );
        if named {
            // Pulled back inside the panel where a decade would otherwise hang off the right
            // edge: 10 kHz sits at ninety-one per cent of a display that ends at eighteen.
            let label_x = (x + px(3.0)).min(plot.origin.x + plot.size.width - px(19.0));
            paint::label(
                window,
                cx,
                point(label_x, baseline + px(1.0)),
                hz_label(hz),
                px(9.0),
                theme.text_faint,
            );
        }
    }

    // Flat, and the two rules either side of it. Without them the curve is a shape with no scale
    // and a 3 dB cut looks exactly like a 12 dB one.
    for (gain, strength) in GAIN_MARKS
        .iter()
        .flat_map(|&gain| [(gain, 0.35), (-gain, 0.35)])
        .chain([(0.0, 0.8)])
    {
        let y = px(top + height * (1.0 - y_of(gain, view.gain_limit)));
        paint::hline(window, plot, y, Theme::translucent(theme.border, strength));
    }

    let spectrum_points: Vec<Point<Pixels>> = spectrum
        .iter()
        .enumerate()
        .map(|(index, level)| {
            // The middle of the band, because that is the frequency it stands for — the bands are
            // spaced across the same log range the scale is, so the two line up by construction.
            let across = (index as f32 + 0.5) / spectrum.len().max(1) as f32;
            let up = ((level - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0);
            at((across, up))
        })
        .collect();
    // Grey where the curve is the accent colour, and no fainter than that: it is the thing being
    // aimed at, and a target nobody can see is a graph with a decoration behind it. Drawn first,
    // so where the two cross it is the curve that is on top.
    paint::area_under(
        window,
        &spectrum_points,
        baseline,
        Theme::translucent(theme.text_muted, 0.35),
    );
    paint::polyline(window, &spectrum_points, px(1.0), theme.text_muted);

    // The curve, in the accent colour the nodes are in: it is the thing being edited, and the
    // spectrum behind it is what it is being aimed at.
    let steps = (width / CURVE_STEP).ceil().max(1.0) as usize;
    let curve: Vec<Point<Pixels>> = (0..=steps)
        .map(|step| {
            let across = step as f32 / steps as f32;
            let db = eq_response_db(&view.bands, view.sample_rate, hz_at(across));
            at((across, y_of(db, view.gain_limit)))
        })
        .collect();
    paint::polyline(window, &curve, px(1.75), theme.accent);

    for band in view.bands.iter().filter(|band| band.enabled) {
        let centre = at(node_of(*band, view.gain_limit));
        paint::rounded_rect(
            window,
            Bounds {
                origin: point(centre.x - px(NODE_RADIUS), centre.y - px(NODE_RADIUS)),
                size: size(px(NODE_RADIUS * 2.0), px(NODE_RADIUS * 2.0)),
            },
            px(NODE_RADIUS),
            theme.accent,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band(kind: EqBandKind, frequency: f32, gain_db: f32) -> EqBandSetting {
        EqBandSetting {
            kind,
            enabled: true,
            frequency,
            gain_db,
            q: 1.0,
        }
    }

    #[test]
    fn the_scale_the_spectrum_and_the_curve_measure_the_same_axis() {
        // The numbers under a curve are decoration unless they sit over the frequency they name.
        // The spectrum's bands are spaced across the log range by `bands_from_bins`, and the tick
        // positions have to be worked out the same way or a 1 kHz mark lands on a 700 Hz band.
        assert_eq!(x_of(LOW_HZ), 0.0);
        assert_eq!(x_of(HIGH_HZ), 1.0);
        assert_eq!(x_of(1.0), 0.0, "below the display is its left edge");
        assert_eq!(x_of(40_000.0), 1.0, "and above it is the right");

        // The middle of the display is the geometric mean, which is what makes it logarithmic:
        // 30 Hz to 735 Hz is as wide as 735 Hz to 18 kHz, and both are half the panel.
        let middle = (LOW_HZ * HIGH_HZ).sqrt();
        assert!((x_of(middle) - 0.5).abs() < 0.001, "{middle} Hz");

        for (hz, _) in TICKS {
            let at = x_of(hz);
            assert!(
                (0.02..=0.98).contains(&at),
                "the {hz} Hz tick is at {at}, which is on the frame rather than in the display"
            );
        }
        assert_eq!(hz_label(100.0), "100");
        assert_eq!(hz_label(10_000.0), "10k");
    }

    #[test]
    fn a_frequency_survives_the_trip_to_the_axis_and_back() {
        // The node is drawn through `x_of` and dragged through `hz_at`, so a value that did not
        // round-trip would make the node slide out from under the pointer holding it.
        for hz in [30.0f32, 100.0, 440.0, 3_000.0, 18_000.0] {
            let back = hz_at(x_of(f64::from(hz)));
            assert!((back / hz - 1.0).abs() < 1e-4, "{hz} Hz came back {back}");
        }
        // And the display's range is the drag's range: a band set below it from the slider comes
        // back to the left edge rather than to where it was.
        assert_eq!(hz_at(0.0), LOW_HZ as f32);
        assert_eq!(hz_at(1.0), HIGH_HZ as f32);
    }

    #[test]
    fn a_gain_survives_the_trip_to_the_axis_and_back() {
        for db in [-24.0f32, -6.0, 0.0, 6.0, 24.0] {
            let back = db_at(y_of(db, 24.0), 24.0);
            assert!((back - db).abs() < 1e-3, "{db} dB came back {back}");
        }
        assert_eq!(y_of(0.0, 24.0), 0.5, "flat is the middle of the display");
        // Every value the parameter holds is somewhere the pointer can reach, in both directions.
        assert_eq!(db_at(1.0, 24.0), 24.0);
        assert_eq!(db_at(0.0, 24.0), -24.0);
    }

    #[test]
    fn a_shape_with_no_level_sits_on_the_centre_line() {
        // A high-pass has a corner, not a level. Its node has to sit where its curve crosses, or
        // the graph would be drawing a boost the audio is not making.
        let pass = node_of(band(EqBandKind::HighPass, 100.0, 18.0), 24.0);
        assert_eq!(pass.1, 0.5);

        // …and dragging it up and down moves nothing, because there is nothing there to move.
        let dragged = moved(band(EqBandKind::HighPass, 100.0, 0.0), (0.5, 0.9), 24.0);
        assert_eq!(dragged.gain_db, 0.0);
        assert!(dragged.frequency > 100.0, "but it still moves sideways");

        // A shape that does have one takes both from the pointer at once.
        let bell = moved(band(EqBandKind::Peaking, 100.0, 0.0), (0.5, 0.75), 24.0);
        assert!((bell.gain_db - 12.0).abs() < 1e-3);
        assert!((bell.frequency - hz_at(0.5)).abs() < 1e-3);
    }

    #[test]
    fn dragging_a_node_to_where_it_already_is_changes_nothing() {
        // The property that keeps a grabbed node from jumping on the first pointer move.
        for kind in [
            EqBandKind::HighPass,
            EqBandKind::LowShelf,
            EqBandKind::Peaking,
            EqBandKind::HighShelf,
            EqBandKind::LowPass,
        ] {
            let before = band(kind, 800.0, -7.0);
            let after = moved(before, node_of(before, 24.0), 24.0);
            assert!(
                (after.frequency / before.frequency - 1.0).abs() < 1e-3,
                "{kind:?} moved to its own position and went to {} Hz",
                after.frequency
            );
            if kind.has_gain() {
                assert!((after.gain_db - before.gain_db).abs() < 1e-3, "{kind:?}");
            }
        }
    }

    #[test]
    fn a_press_takes_the_node_it_landed_on_and_nothing_else() {
        let bands = [
            band(EqBandKind::LowShelf, 120.0, 4.0),
            band(EqBandKind::Peaking, 3_000.0, -6.0),
            EqBandSetting {
                enabled: false,
                ..band(EqBandKind::LowPass, 8_000.0, 0.0)
            },
        ];
        let radius = (0.03, 0.06);
        for (index, entry) in bands.iter().enumerate().take(2) {
            assert_eq!(
                node_at(&bands, 24.0, node_of(*entry, 24.0), radius),
                Some(index)
            );
        }
        // A band that is switched off is not on the curve, so it has no handle over the curve.
        assert_eq!(node_at(&bands, 24.0, node_of(bands[2], 24.0), radius), None);
        assert_eq!(
            node_at(&bands, 24.0, (0.9, 0.9), radius),
            None,
            "empty space claimed a node"
        );
    }

    #[test]
    fn the_wheel_narrows_by_the_same_musical_step_wherever_it_is_turned() {
        // Multiplied rather than added: one notch has to mean the same change at 0.5 as at 8, or
        // the control crawls at one end of its range and leaps at the other.
        let range = (0.1, 18.0);
        let low = resonance_after_scroll(0.5, 1.0, range) / 0.5;
        let high = resonance_after_scroll(8.0, 1.0, range) / 8.0;
        assert!((low - high).abs() < 1e-4, "{low} against {high}");
        assert!(low > 1.0, "the wheel forwards narrows the band");

        // And one notch has to be a step somebody can see happen. A twelfth was the first guess:
        // it took a default Q of 0.71 to 0.80, which on a curve this wide is nothing at all, and
        // the control read as broken rather than as fine.
        assert!(low >= 1.2, "one notch moved the band by {low}×");
        // A dozen of them crosses the range anybody works in, from broad to surgical.
        let mut q = 0.5;
        for _ in 0..12 {
            q = resonance_after_scroll(q, 1.0, range);
        }
        assert!(q > 6.0, "twelve notches only reached {q}");

        // And it comes back: a notch each way is where it started.
        let there = resonance_after_scroll(2.0, 3.0, range);
        assert!((resonance_after_scroll(there, -3.0, range) - 2.0).abs() < 1e-3);

        // Never outside what the plugin accepts, in either direction.
        assert_eq!(resonance_after_scroll(0.2, -80.0, range), 0.1);
        assert_eq!(resonance_after_scroll(9.0, 80.0, range), 18.0);
    }

    #[test]
    fn a_short_gap_is_drawn_across_and_a_long_one_is_not() {
        // The bottom octave has fewer bins than bands, so it comes back full of holes that are a
        // property of the analyser rather than of the sound. A line that dives to the floor and
        // back through one of them claims a silence nobody measured.
        let floor = -90.0;
        let mut bands = [-20.0, floor, floor, -24.0];
        bridge_gaps(&mut bands, floor, 3);
        assert!(
            bands[1] < -20.0 && bands[1] > -24.0 && bands[2] < bands[1],
            "the hole was not filled from its neighbours: {bands:?}"
        );

        // A long run is a quiet stretch of the spectrum and stays exactly where it is.
        let mut long = [-20.0, floor, floor, floor, floor, -24.0];
        bridge_gaps(&mut long, floor, 3);
        assert_eq!(long, [-20.0, floor, floor, floor, floor, -24.0]);

        // A run at either end has nothing on one side to interpolate from.
        let mut edges = [floor, -20.0, floor];
        bridge_gaps(&mut edges, floor, 3);
        assert_eq!(edges, [floor, -20.0, floor]);

        // Silence stays silence, whatever the length.
        let mut silent = [floor; 8];
        bridge_gaps(&mut silent, floor, 3);
        assert_eq!(silent, [floor; 8]);
        bridge_gaps(&mut [], floor, 3);
    }

    #[test]
    fn the_graph_is_measured_above_the_strip_carrying_the_numbers() {
        // A press is compared against the recorded plot, and the curve is drawn into it. If the
        // two disagreed by the height of the scale, every node would sit below the pointer that
        // grabbed it.
        let bounds = Bounds {
            origin: point(px(10.0), px(20.0)),
            size: size(px(400.0), px(HEIGHT)),
        };
        let plot = plot_of(bounds);
        assert_eq!(plot.origin, bounds.origin);
        assert_eq!(plot.size.width, bounds.size.width);
        assert_eq!(plot.size.height, px(HEIGHT - SCALE_HEIGHT));

        // A display squeezed shorter than its own scale still has a graph rather than a negative
        // one, which would put every node at the top of the window.
        let squeezed = plot_of(Bounds {
            size: size(px(400.0), px(4.0)),
            ..bounds
        });
        assert!(squeezed.size.height > px(0.0));
    }
}
