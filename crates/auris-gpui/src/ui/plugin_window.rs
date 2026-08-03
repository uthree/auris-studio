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
use gpui::{AnyElement, Pixels, Point, Size, div, point, prelude::*, px};

use crate::app::{AurisApp, Drag};
use crate::theme::Metrics;
use crate::ui::icons::Icon;
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
        let wanted = Metrics::EDITOR_HEADER_HEIGHT + body + px(16.0);
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

/// One bar per band, drawn as a strip across the top of the window.
fn spectrum_display(bands: Vec<f32>, theme: &crate::theme::Theme) -> AnyElement {
    let bars: Vec<AnyElement> = bands
        .into_iter()
        .map(|level| {
            let height = ((level - SPECTRUM_FLOOR) / -SPECTRUM_FLOOR).clamp(0.0, 1.0);
            div()
                .flex_1()
                .h_full()
                .flex()
                .flex_col()
                .justify_end()
                .child(
                    div()
                        .w_full()
                        .h(gpui::relative(height))
                        .bg(crate::theme::Theme::translucent(theme.accent, 0.75)),
                )
                .into_any_element()
        })
        .collect();

    div()
        .h(px(64.0))
        .w_full()
        .flex_shrink_0()
        .flex()
        .items_end()
        .gap(px(1.0))
        .px_2()
        .py_1()
        .bg(theme.surface_sunken)
        .border_b_1()
        .border_color(theme.border)
        .children(bars)
        .into_any_element()
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
                        .h(Metrics::EDITOR_HEADER_HEIGHT)
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
    fn resolve_plugin(&self, subject: PluginSubject) -> Option<(String, bool)> {
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
    use gpui::size;

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
