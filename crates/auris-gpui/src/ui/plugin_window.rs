//! The floating plugin editor.
//!
//! Logic opens a plugin's controls in a window of their own, and the reason is not decoration:
//! the strip you clicked stays where it was, so the next insert is one click away and the chain
//! is still readable while a parameter moves. Editing in place — which is what the inspector used
//! to do, one expanding card per effect — pushes everything below it down the panel and turns a
//! four-effect chain into a scroll.
//!
//! It is a panel *inside* the main window rather than a second operating-system window, for a
//! reason that is structural rather than aesthetic: [`crate::app::Drag::Param`] is dispatched
//! from the root view's `on_mouse_move`, so a slider living in another window would have nothing
//! driving it once the pointer went down. One is open at a time, the rule the context menu and
//! the rename sheet already follow.

use auris_i18n::Key;
use auris_session::prelude::*;
use gpui::{AnyElement, Bounds, Pixels, Point, Size, Window, div, point, prelude::*, px, size};

use crate::app::{AurisApp, Drag};
use crate::theme::Metrics;
use crate::ui::icons::Icon;
use crate::ui::paint;
use crate::ui::plugin_editor::plugin_header;
use crate::ui::widgets::chain_button;

/// What an open plugin window is editing.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PluginSubject {
    /// A track's instrument.
    Instrument(TrackId),
    /// One insert on a track's chain, or on the master bus when `track` is `None`.
    Insert {
        /// Strip the insert belongs to.
        track: Option<TrackId>,
        /// Which slot in that strip's chain.
        slot: EffectSlotId,
    },
}

impl PluginSubject {
    /// The parameter target this subject's `param` addresses.
    pub fn param_target(self, param: ParamId) -> ParamTarget {
        match self {
            PluginSubject::Instrument(track) => ParamTarget::Instrument { track, param },
            PluginSubject::Insert { track, slot } => ParamTarget::Effect { track, slot, param },
        }
    }

    /// The strip this plugin sits on, or `None` for the master bus.
    pub fn strip(self) -> Option<TrackId> {
        match self {
            PluginSubject::Instrument(track) => Some(track),
            PluginSubject::Insert { track, .. } => track,
        }
    }

    /// Element-id prefix for the controls inside the window.
    ///
    /// Load-bearing, not decorative. `target_element_key` folds an instrument's *track* id and an
    /// effect's *slot* id through the same multiplier, so track 1's instrument and slot 1's
    /// effect come out with the same key; without a differing prefix the two would share hover
    /// state whenever both were reachable.
    pub fn id_prefix(self) -> &'static str {
        match self {
            PluginSubject::Instrument(_) => "pw-inst",
            PluginSubject::Insert { .. } => "pw-fx",
        }
    }
}

/// An open plugin editor.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PluginWindow {
    /// What it is editing.
    pub subject: PluginSubject,
    /// Top-left corner, in window coordinates. Moved by dragging the title bar.
    pub anchor: Point<Pixels>,
}

impl PluginWindow {
    /// How wide the window is drawn.
    pub const WIDTH: Pixels = px(300.0);
    /// Tallest it may grow before its body scrolls instead.
    pub const MAX_HEIGHT: Pixels = px(420.0);

    /// How tall a window with `param_count` controls wants to be.
    pub fn height(param_count: usize) -> Pixels {
        let body = Metrics::CONTROL_HEIGHT * param_count as f32;
        // Title bar, the body, and the padding either side of it.
        let wanted = Metrics::PANEL_HEADER_HEIGHT + body + px(16.0);
        wanted.min(Self::MAX_HEIGHT)
    }

    /// Where the window is actually drawn, given the viewport it has to fit in.
    ///
    /// Clamped rather than flipped, unlike [`crate::ui::context_menu::ContextMenu::origin`]. A
    /// menu flips to the other side of the pointer because the pointer is about to click through
    /// it; a window is not about to swallow a click, and flipping it would move the title bar out
    /// from under the hand that is reaching for it. A window larger than the viewport pins to the
    /// top-left, because that is the corner the title bar is in.
    pub fn origin(&self, viewport: Size<Pixels>, height: Pixels) -> Point<Pixels> {
        let x = self
            .anchor
            .x
            .min((viewport.width - Self::WIDTH).max(px(0.0)))
            .max(px(0.0));
        let y = self
            .anchor
            .y
            .min((viewport.height - height).max(px(0.0)))
            .max(px(0.0));
        point(x, y)
    }
}

/// How many bars the spectrum is drawn as.
///
/// Far fewer than the window has bins. A display three hundred pixels wide cannot show five
/// hundred of them, and a musician reads bands rather than bins — so the bins are gathered into
/// this many, spaced by octave.
const SPECTRUM_BANDS: usize = 48;

/// Lowest frequency the display shows, in hertz.
const SPECTRUM_LOW: f64 = 30.0;
/// Highest frequency the display shows, in hertz.
const SPECTRUM_HIGH: f64 = 18_000.0;
/// Level at the bottom of the display, in dBFS.
const SPECTRUM_FLOOR: f32 = -72.0;

/// Whether this plugin is one whose editor shows what is passing through it.
///
/// A list rather than a method on the `Effect` trait: what a *display* offers is a property of
/// the editor, not of the processor, and asking every plugin author about a window they have
/// never seen would put a frontend's concern into the plugin contract. An equalizer is the one
/// that needs it — its whole job is deciding where to put a curve, and the curve alone does not
/// say where.
fn analyses_spectrum(plugin_id: &str) -> bool {
    plugin_id == "auris.fx.eq"
}

/// The frequencies the scale marks, and whether each carries its number.
///
/// Decades are named and the steps between them are only ruled. A line every octave with a
/// number on it is a ruler; three numbers and a set of ticks is a scale somebody can read while
/// looking at something else, which is what an analyser is for.
const SPECTRUM_TICKS: [(f64, bool); 8] = [
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
const SPECTRUM_SCALE_HEIGHT: f32 = 12.0;

/// The longest run of unmeasured bands [`bridge_gaps`] will draw across.
///
/// Four at the very bottom of the display, where the gaps are widest; the number has one spare.
const SPECTRUM_GAP: usize = 5;

/// Where a frequency sits across the display, from 0 at the left edge to 1 at the right.
///
/// Logarithmic, because pitch is, and because that is how `auris_dsp::bands_from_bins` spaces the
/// bands — the scale and the curve have to agree or the numbers under it are decoration.
fn spectrum_x(hz: f64) -> f32 {
    let span = (SPECTRUM_HIGH / SPECTRUM_LOW).ln();
    ((hz / SPECTRUM_LOW).ln() / span).clamp(0.0, 1.0) as f32
}

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

/// The spectrum as a filled curve over a frequency scale, across the top of the window.
///
/// A curve rather than a bar chart. A bar chart of forty-eight bands is a picture of the
/// *display's* resolution; what an ear hears and what an equalizer is about to move is a shape,
/// and a shape is what a line draws. The numbers underneath are the other half of it — a bump was
/// visible before and there was nothing on screen to say whether it was at 200 Hz or at 2 kHz,
/// which is the only question anybody reaches for an analyser to answer.
fn spectrum_display(mut bands: Vec<f32>, theme: &crate::theme::Theme) -> AnyElement {
    bridge_gaps(&mut bands, Session::spectrum_silence(), SPECTRUM_GAP);
    let theme = theme.clone();
    div()
        .h(px(76.0))
        .w_full()
        .flex_shrink_0()
        .px_2()
        .py_1()
        .bg(theme.surface_sunken)
        .border_b_1()
        .border_color(theme.border)
        .child(
            gpui::canvas(
                |_, _, _| (),
                move |bounds, _, window, cx| paint_spectrum(window, cx, bounds, &bands, &theme),
            )
            .size_full(),
        )
        .into_any_element()
}

/// Draws the scale, then the curve over it.
fn paint_spectrum(
    window: &mut Window,
    cx: &mut gpui::App,
    bounds: Bounds<Pixels>,
    bands: &[f32],
    theme: &crate::theme::Theme,
) {
    let plot_height = (bounds.size.height - px(SPECTRUM_SCALE_HEIGHT)).max(px(1.0));
    let baseline = bounds.origin.y + plot_height;
    let left = f32::from(bounds.origin.x);
    let width = f32::from(bounds.size.width);
    let top = f32::from(bounds.origin.y);
    let height = f32::from(plot_height);

    for (hz, named) in SPECTRUM_TICKS {
        let x = px(left + width * spectrum_x(hz));
        paint::rect(
            window,
            Bounds {
                origin: point(x, bounds.origin.y),
                size: size(px(1.0), plot_height),
            },
            crate::theme::Theme::translucent(theme.border, if named { 1.0 } else { 0.5 }),
        );
        if named {
            // Pulled back inside the panel where a decade would otherwise hang off the right
            // edge: 10 kHz sits at ninety-one per cent of a display that ends at eighteen.
            let at = (x + px(3.0)).min(bounds.origin.x + bounds.size.width - px(19.0));
            paint::label(
                window,
                cx,
                point(at, baseline + px(1.0)),
                hz_label(hz),
                px(9.0),
                theme.text_faint,
            );
        }
    }

    let points: Vec<Point<Pixels>> = bands
        .iter()
        .enumerate()
        .map(|(index, level)| {
            // The middle of the band, because that is the frequency it stands for — the bands are
            // spaced across the same log range the scale is, so the two line up by construction.
            let across = (index as f32 + 0.5) / bands.len().max(1) as f32;
            let up = ((level - SPECTRUM_FLOOR) / -SPECTRUM_FLOOR).clamp(0.0, 1.0);
            point(px(left + width * across), px(top + height * (1.0 - up)))
        })
        .collect();

    paint::area_under(
        window,
        &points,
        baseline,
        crate::theme::Theme::translucent(theme.accent, 0.25),
    );
    paint::polyline(window, &points, px(1.5), theme.accent);
}

impl AurisApp {
    /// The current spectrum, gathered into bands ready to draw.
    ///
    /// A torn read — the audio thread wrote through the copy — returns the floor rather than a
    /// window with a seam in it. The next repaint is sixteen milliseconds away and will find a
    /// settled one, which is cheaper than making the audio thread wait for this.
    fn spectrum_bins(&mut self) -> Vec<f32> {
        let mut bands = vec![Session::spectrum_silence(); SPECTRUM_BANDS];
        self.session
            .spectrum(SPECTRUM_LOW, SPECTRUM_HIGH, &mut bands);
        bands
    }

    /// Opens the editor for one plugin, replacing whatever was open.
    pub(crate) fn open_plugin_window(&mut self, subject: PluginSubject, anchor: Point<Pixels>) {
        self.plugin_window = Some(PluginWindow { subject, anchor });
    }

    /// Closes the editor, reporting whether one was open.
    pub(crate) fn close_plugin_window(&mut self) -> bool {
        self.plugin_window.take().is_some()
    }

    /// Draws the open plugin editor, if there is one and it still names something.
    ///
    /// Takes the field and puts it back only once the subject has resolved, which is one guard
    /// instead of four: an insert removed from a menu, a track deleted, an undo past the point
    /// the effect was added, a project opened — every one of them leaves the window pointing at
    /// nothing, and the next frame quietly closes it.
    pub(crate) fn render_plugin_window(
        &mut self,
        viewport: Size<Pixels>,
        cx: &mut gpui::Context<Self>,
    ) -> Option<AnyElement> {
        let window = self.plugin_window.take()?;
        let subject = window.subject;
        let (plugin_id, enabled) = self.resolve_plugin(subject)?;
        self.plugin_window = Some(window);

        // Point the analysis at whichever strip this plugin sits on. Asked for every frame the
        // window is open, because a rebuild or a change of selection could otherwise leave it
        // reading a strip that has moved; it is one relaxed store.
        if analyses_spectrum(&plugin_id) {
            self.session.watch_strip(subject.strip());
        } else {
            self.session.stop_watching();
        }
        let spectrum = analyses_spectrum(&plugin_id).then(|| self.spectrum_bins());
        let envelope = self.envelope_of(subject, &plugin_id);

        let theme = self.theme.clone();
        let name = self.plugin_label(&plugin_id);
        let descriptors = self.session.param_descriptors(&plugin_id);
        let height = PluginWindow::height(descriptors.len());
        let origin = window.origin(viewport, height);
        let controls = self.param_controls(
            &descriptors,
            move |param| subject.param_target(param),
            subject.id_prefix(),
            cx,
        );

        Some(
            div()
                .absolute()
                .left(origin.x)
                .top(origin.y)
                .w(PluginWindow::WIDTH)
                .max_h(height)
                .flex()
                .flex_col()
                .rounded(Metrics::RADIUS_LG)
                .bg(theme.surface_raised)
                .border_1()
                .border_color(theme.border)
                .shadow_lg()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .h(Metrics::PANEL_HEADER_HEIGHT)
                        .px_1p5()
                        .flex_shrink_0()
                        .border_b_1()
                        .border_color(theme.border)
                        // The whole bar is the grab handle, so the window moves from anywhere
                        // that is not one of its two buttons.
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |this, event: &gpui::MouseDownEvent, _, _| {
                                this.begin_drag(Drag::MovePluginWindow {
                                    grab_offset: point(
                                        event.position.x - origin.x,
                                        event.position.y - origin.y,
                                    ),
                                });
                            }),
                        )
                        .child(div().flex_1().min_w_0().child(plugin_header(
                            gpui::SharedString::from(format!("{}-bypass", subject.id_prefix())),
                            name,
                            enabled,
                            self.t(if enabled { Key::ValueOn } else { Key::ValueOff }),
                            &theme,
                            cx.listener(move |this, _, _, cx| {
                                if let PluginSubject::Insert { track, slot } = subject {
                                    this.toggle_effect(track, slot);
                                }
                                cx.notify();
                            }),
                        )))
                        .child(chain_button(
                            "pw-close",
                            Icon::Cross,
                            &theme,
                            cx.listener(|this, _, _, cx| {
                                this.close_plugin_window();
                                cx.notify();
                            }),
                        )),
                )
                .children(spectrum.map(|bins| spectrum_display(bins, &theme)))
                .children(envelope.map(|env| self.envelope_display(subject, env, cx)))
                .child(
                    div()
                        .id("pw-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_2()
                        .children(controls),
                )
                .into_any_element(),
        )
    }

    /// The plugin id a subject names, and whether it is switched in.
    ///
    /// `None` once the thing it named has gone, which is what closes the window.
    pub(crate) fn resolve_plugin(&self, subject: PluginSubject) -> Option<(String, bool)> {
        match subject {
            PluginSubject::Instrument(track) => {
                let inner = self.project().track(track)?.kind.as_instrument()?;
                Some((inner.instrument_id.clone(), true))
            }
            PluginSubject::Insert { track, slot } => {
                let strip = match track {
                    Some(id) => &self.project().track(id)?.mixer,
                    None => &self.project().master,
                };
                let entry = strip.effects.iter().find(|effect| effect.id == slot)?;
                Some((entry.effect_id.clone(), entry.enabled))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scale_and_the_curve_measure_the_same_axis() {
        // The numbers under a curve are decoration unless they sit over the frequency they name.
        // The bands are spaced across the log range by `bands_from_bins`, and the tick positions
        // have to be worked out the same way or a 1 kHz mark lands on a 700 Hz band.
        assert_eq!(spectrum_x(SPECTRUM_LOW), 0.0);
        assert_eq!(spectrum_x(SPECTRUM_HIGH), 1.0);
        assert_eq!(spectrum_x(1.0), 0.0, "below the display is its left edge");
        assert_eq!(spectrum_x(40_000.0), 1.0, "and above it is the right");

        // The middle of the display is the geometric mean, which is what makes it logarithmic:
        // 30 Hz to 735 Hz is as wide as 735 Hz to 18 kHz, and both are half the panel.
        let middle = (SPECTRUM_LOW * SPECTRUM_HIGH).sqrt();
        assert!((spectrum_x(middle) - 0.5).abs() < 0.001, "{middle} Hz");

        for (hz, _) in SPECTRUM_TICKS {
            let at = spectrum_x(hz);
            assert!(
                (0.02..=0.98).contains(&at),
                "the {hz} Hz tick is at {at}, which is on the frame rather than in the display"
            );
        }
        assert_eq!(hz_label(100.0), "100");
        assert_eq!(hz_label(10_000.0), "10k");
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
    fn a_subject_names_the_parameter_it_edits() {
        assert_eq!(
            PluginSubject::Instrument(TrackId(1)).param_target(ParamId(2)),
            ParamTarget::Instrument {
                track: TrackId(1),
                param: ParamId(2)
            }
        );
        // The master bus is `None`, and it has to survive the round trip: an insert on master
        // that resolved to a track would write another strip's parameter.
        assert_eq!(
            PluginSubject::Insert {
                track: None,
                slot: EffectSlotId(3)
            }
            .param_target(ParamId(4)),
            ParamTarget::Effect {
                track: None,
                slot: EffectSlotId(3),
                param: ParamId(4)
            }
        );
    }

    #[test]
    fn the_two_kinds_of_subject_never_share_an_element_key() {
        // `target_element_key` folds a track id and a slot id through the same multiplier, so
        // these two produce the same number and only the prefix keeps them apart.
        assert_ne!(
            PluginSubject::Instrument(TrackId(1)).id_prefix(),
            PluginSubject::Insert {
                track: None,
                slot: EffectSlotId(1)
            }
            .id_prefix()
        );
    }

    #[test]
    fn a_window_opened_at_the_edge_is_pushed_back_inside() {
        let viewport = size(px(800.0), px(600.0));
        let height = PluginWindow::height(4);

        let corner = PluginWindow {
            subject: PluginSubject::Instrument(TrackId(0)),
            anchor: point(px(780.0), px(590.0)),
        };
        let origin = corner.origin(viewport, height);
        assert!(origin.x + PluginWindow::WIDTH <= viewport.width);
        assert!(origin.y + height <= viewport.height);

        // One that already fits is left exactly where it was asked for.
        let roomy = PluginWindow {
            subject: PluginSubject::Instrument(TrackId(0)),
            anchor: point(px(100.0), px(80.0)),
        };
        assert_eq!(roomy.origin(viewport, height), point(px(100.0), px(80.0)));
    }

    #[test]
    fn a_window_too_large_for_the_viewport_pins_to_the_corner_the_title_bar_is_in() {
        let tiny = size(px(120.0), px(60.0));
        let window = PluginWindow {
            subject: PluginSubject::Instrument(TrackId(0)),
            anchor: point(px(400.0), px(400.0)),
        };
        assert_eq!(
            window.origin(tiny, PluginWindow::height(8)),
            point(px(0.0), px(0.0))
        );
    }

    #[test]
    fn the_window_stops_growing_and_scrolls_instead() {
        assert!(PluginWindow::height(1) < PluginWindow::height(6));
        assert_eq!(PluginWindow::height(400), PluginWindow::MAX_HEIGHT);
    }
}
