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
// Named apart from this module's own `PluginWindow`, which is the panel drawn inside the
// application; this one is the window the plugin draws for itself.
use auris_session::HasWindowHandle;
use auris_session::PluginWindow as HostedWindow;
use auris_session::prelude::*;
use gpui::{AnyElement, Pixels, Point, Size, div, point, prelude::*, px, size};

use crate::app::{AurisApp, Drag};
use crate::theme::{Metrics, Theme};
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

    /// The hosted plugin whose *own* window this subject stands for.
    ///
    /// The panel this module draws and the window a CLAP plugin opens are two different windows
    /// onto the same plugin, and this is the one line where the two namings meet.
    pub fn hosted_window(self) -> HostedWindow {
        match self {
            PluginSubject::Instrument(track) => HostedWindow::Instrument(track),
            PluginSubject::Insert { slot, .. } => HostedWindow::Effect(slot),
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

/// How tall the caution strip is drawn.
const CAUTION_HEIGHT: Pixels = px(30.0);

/// What a plugin's window has to warn about, given the state of its switch.
///
/// One plugin and one switch, and both are named here rather than asked of the plugin, for the
/// reason `auris_session::guide::plugins` gives about the equalizer's curve: a `warning` method on
/// the `Instrument` trait would put a frontend's concern into the plugin contract, and the
/// sentence is a frontend's to translate in any case.
///
/// Why it exists at all: turning the sampler's envelope on is the one control in the application
/// that *takes something away* — polyphony, and a drum kit's choke groups — and it takes it away
/// somewhere the person who flipped the switch cannot see. A cost that only shows up as "the
/// sixteenth note of my chord went missing" is a cost that gets blamed on the wrong thing.
pub fn caution(plugin_id: &str, envelope_on: bool) -> Option<Key> {
    match plugin_id == SAMPLER_ID && envelope_on {
        true => Some(Key::SamplerEnvelopeOn),
        false => None,
    }
}

/// The caution strip, under whatever picture the window is carrying.
fn caution_strip(text: &'static str, theme: &Theme) -> AnyElement {
    div()
        .h(CAUTION_HEIGHT)
        .w_full()
        .flex_shrink_0()
        .flex()
        .items_center()
        .px_2()
        .border_b_1()
        .border_color(theme.border)
        .bg(Theme::translucent(theme.warning, 0.12))
        .text_xs()
        .text_color(theme.warning)
        .child(text)
        .into_any_element()
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
    /// How wide a window of numbers is drawn.
    pub const WIDTH: Pixels = px(300.0);
    /// How wide one carrying a curve is.
    ///
    /// A graph is a thing aimed at, and three hundred pixels of it spans thirty hertz to
    /// eighteen kilohertz — under a pixel for the whole bottom octave. The extra width buys
    /// resolution nothing else in the window needs, which is why it is not simply the width.
    pub const CURVE_WIDTH: Pixels = px(440.0);
    /// Tallest the list of controls may grow before it scrolls instead.
    pub const MAX_LIST_HEIGHT: Pixels = px(420.0);

    /// The gap between two control rows, and the padding around the body holding them.
    ///
    /// The body's `gap_1` and `p_2`, written down so [`PluginWindow::height`] can estimate the
    /// same box the builder actually makes. They were the bug: the old estimate counted the rows
    /// and forgot the four pixels between each pair, so a compressor came out twenty-seven pixels
    /// short and its last control was cut through the middle.
    const ROW_GAP: Pixels = px(4.0);
    const BODY_PADDING: Pixels = px(8.0);

    /// How wide the window is drawn, and the tallest it may grow.
    ///
    /// A *ceiling*, not a size: the body sizes itself to the rows it holds and stops here. That is
    /// the whole reason this is not the estimate below — a ceiling that is too low clips, and an
    /// estimate that is too low only moves the window a few pixels.
    ///
    /// `above` is everything drawn between the title bar and the list — graphs and cautions. None
    /// of it scrolls, so it is added to the ceiling rather than taken out of it: otherwise giving
    /// a plugin a picture would have taken a third of its sliders away with it, and a warning
    /// would have cost a control the moment it appeared.
    pub fn frame(has_curve: bool, above: Pixels) -> Size<Pixels> {
        size(
            match has_curve {
                true => Self::CURVE_WIDTH,
                false => Self::WIDTH,
            },
            Self::MAX_LIST_HEIGHT + above,
        )
    }

    /// How tall a window with `param_count` controls and `above` pixels over them comes out, for
    /// deciding where it will fit.
    ///
    /// Only ever used to nudge the window inside the viewport, which is why an estimate is
    /// allowed to be one here at all.
    pub fn height(param_count: usize, has_curve: bool, above: Pixels) -> Pixels {
        let rows = param_count.max(1) as f32;
        let body = Metrics::CONTROL_HEIGHT * rows
            + Self::ROW_GAP * (rows - 1.0)
            + Self::BODY_PADDING * 2.0;
        (Metrics::PANEL_HEADER_HEIGHT + body + above).min(Self::frame(has_curve, above).height)
    }

    /// Where the window is actually drawn, given the viewport it has to fit in.
    ///
    /// Clamped rather than flipped, unlike [`crate::ui::context_menu::ContextMenu::origin`]. A
    /// menu flips to the other side of the pointer because the pointer is about to click through
    /// it; a window is not about to swallow a click, and flipping it would move the title bar out
    /// from under the hand that is reaching for it. A window larger than the viewport pins to the
    /// top-left, because that is the corner the title bar is in.
    pub fn origin(&self, viewport: Size<Pixels>, window: Size<Pixels>) -> Point<Pixels> {
        let x = self
            .anchor
            .x
            .min((viewport.width - window.width).max(px(0.0)))
            .max(px(0.0));
        let y = self
            .anchor
            .y
            .min((viewport.height - window.height).max(px(0.0)))
            .max(px(0.0));
        point(x, y)
    }
}

impl AurisApp {
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
    /// Opens the plugin's own window, or takes it away if it is already up.
    ///
    /// The application's window is named to the session on the way past. This is the one moment a
    /// `&Window` is in scope and the session needs it again later, from a tick where nothing is —
    /// a plugin window that has to move to the other instance of its pair has to be told all over
    /// again what to float above.
    pub(crate) fn toggle_hosted_window(&mut self, subject: PluginSubject, window: &gpui::Window) {
        // Through the trait, not the inherent method of the same name: gpui's own
        // `window_handle` answers with its handle to *this view*, which is not a thing any other
        // process has heard of.
        let parent = HasWindowHandle::window_handle(window)
            .ok()
            .map(|handle| handle.as_raw());
        self.session.set_plugin_window_parent(parent);

        let which = subject.hosted_window();
        let open = !self.session.plugin_window_is_open(which);
        if let Err(error) = self.session.set_plugin_window_open(which, open) {
            let line = self.failure(Key::CmdOpenPluginWindow, &error);
            self.set_failed_status(line);
        }
    }

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
        let equalizer = self.eq_view(subject, &plugin_id);
        if equalizer.is_some() {
            self.session.watch_strip(subject.strip());
        } else {
            self.session.stop_watching();
        }
        let analyser = equalizer.map(|view| self.analyser_display(subject, view, cx));
        let envelope = self.envelope_of(subject, &plugin_id);
        let caution = caution(
            &plugin_id,
            self.switch_is_on(subject, &plugin_id, SAMPLER_ENVELOPE_KEY),
        );

        let theme = self.theme.clone();
        // Asked by *slot* rather than by plugin id wherever there is one. The registry cannot
        // answer for a hosted plugin at all, and could not answer correctly even in principle:
        // two slots may hold the same plugin loaded from two different files.
        let (name, descriptors) = match subject {
            PluginSubject::Insert { slot, .. } => (
                self.effect_label(slot, &plugin_id),
                self.session.effect_descriptors(slot),
            ),
            PluginSubject::Instrument(track) => (
                self.instrument_label(track, &plugin_id),
                self.session.instrument_descriptors(track),
            ),
        };
        // Asked every frame rather than remembered: it is a question about a plugin instance, and
        // a graph rebuild can put a different one behind the same slot between frames.
        let own_window = self.session.plugin_window_exists(subject.hosted_window());
        let has_curve = analyser.is_some();
        // Everything above the scrolling list, none of which scrolls, so all of it is room the
        // window needs on top of whatever the controls come to.
        let above = match has_curve {
            true => px(crate::ui::analyser::HEIGHT),
            false => px(0.0),
        } + match envelope.is_some() {
            true => px(crate::ui::envelope::GRAPH_HEIGHT),
            false => px(0.0),
        } + match caution.is_some() {
            true => CAUTION_HEIGHT,
            false => px(0.0),
        };
        let frame = PluginWindow::frame(has_curve, above);
        let origin = window.origin(
            viewport,
            size(
                frame.width,
                PluginWindow::height(descriptors.len(), has_curve, above),
            ),
        );
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
                .w(frame.width)
                .max_h(frame.height)
                .flex()
                .flex_col()
                .rounded(Metrics::RADIUS_LG)
                .bg(theme.surface_raised)
                .border_1()
                .border_color(theme.border)
                .shadow_lg()
                // A floating window that let the pointer through. gpui's hit test walks every
                // hitbox under the pointer until one blocks, so a press over a plugin's slider
                // was reaching the mixer strip behind it as well: the fader moved, and so did
                // whatever the click landed on underneath.
                .occlude()
                // …and occluding is what would then stop the sliders working. A drag is followed
                // on the root, and the hit test stops dead at the first blocking hitbox — so
                // while this is up the root reads as un-hovered and never sees another pointer
                // move. An overlay that occludes carries the drag itself; see
                // `AurisApp::on_mouse_move`.
                .on_mouse_move(cx.listener(AurisApp::on_mouse_move))
                .on_mouse_up(gpui::MouseButton::Left, cx.listener(AurisApp::on_mouse_up))
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
                        // Only for a plugin that has one. A button that did nothing on every
                        // built-in in the application would be a button nobody pressed on the one
                        // plugin where it works.
                        .children(own_window.then(|| {
                            chain_button(
                                "pw-own",
                                Icon::Window,
                                &theme,
                                cx.listener(move |this, _, window: &mut gpui::Window, cx| {
                                    this.toggle_hosted_window(subject, window);
                                    cx.notify();
                                }),
                            )
                        }))
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
                .children(analyser)
                .children(envelope.map(|env| self.envelope_display(subject, env, cx)))
                .children(caution.map(|key| caution_strip(self.t(key), &theme)))
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

    /// Whether a named toggle parameter of this plugin is switched on.
    ///
    /// `false` when the plugin has no such parameter, which is every plugin but one.
    fn switch_is_on(&mut self, subject: PluginSubject, plugin_id: &str, key: &str) -> bool {
        let descriptors = self.session.param_descriptors(plugin_id);
        descriptors
            .iter()
            .find(|descriptor| descriptor.key == key)
            .is_some_and(|descriptor| {
                self.session
                    .param_value(subject.param_target(descriptor.id), descriptor)
                    >= 0.5
            })
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
    fn a_subject_names_the_window_the_plugin_would_draw_for_itself() {
        // An insert is addressed by its slot and an instrument by its track, and the two ids are
        // both bare integers — so a mapping that crossed them would open track 3's synth from
        // slot 3's compressor, and nothing about the types would object.
        assert_eq!(
            PluginSubject::Instrument(TrackId(3)).hosted_window(),
            HostedWindow::Instrument(TrackId(3))
        );
        assert_eq!(
            PluginSubject::Insert {
                track: Some(TrackId(1)),
                slot: EffectSlotId(3)
            }
            .hosted_window(),
            HostedWindow::Effect(EffectSlotId(3)),
            "an insert is its slot wherever the slot sits"
        );
        assert_eq!(
            PluginSubject::Insert {
                track: None,
                slot: EffectSlotId(3)
            }
            .hosted_window(),
            HostedWindow::Effect(EffectSlotId(3))
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

    /// A window of `count` controls and no graph, at the size the layout would give it.
    fn frame_of(count: usize) -> Size<Pixels> {
        size(
            PluginWindow::frame(false, NO_GRAPH).width,
            PluginWindow::height(count, false, NO_GRAPH),
        )
    }

    /// A window with nothing drawn above its controls.
    const NO_GRAPH: Pixels = px(0.0);

    #[test]
    fn a_window_opened_at_the_edge_is_pushed_back_inside() {
        let viewport = size(px(800.0), px(600.0));
        let wanted = frame_of(4);

        let corner = PluginWindow {
            subject: PluginSubject::Instrument(TrackId(0)),
            anchor: point(px(780.0), px(590.0)),
        };
        let origin = corner.origin(viewport, wanted);
        assert!(origin.x + wanted.width <= viewport.width);
        assert!(origin.y + wanted.height <= viewport.height);

        // One that already fits is left exactly where it was asked for.
        let roomy = PluginWindow {
            subject: PluginSubject::Instrument(TrackId(0)),
            anchor: point(px(100.0), px(80.0)),
        };
        assert_eq!(roomy.origin(viewport, wanted), point(px(100.0), px(80.0)));
    }

    #[test]
    fn a_window_too_large_for_the_viewport_pins_to_the_corner_the_title_bar_is_in() {
        let tiny = size(px(120.0), px(60.0));
        let window = PluginWindow {
            subject: PluginSubject::Instrument(TrackId(0)),
            anchor: point(px(400.0), px(400.0)),
        };
        assert_eq!(window.origin(tiny, frame_of(8)), point(px(0.0), px(0.0)));
    }

    #[test]
    fn the_window_stops_growing_and_scrolls_instead() {
        assert!(
            PluginWindow::height(1, false, NO_GRAPH) < PluginWindow::height(6, false, NO_GRAPH)
        );
        assert_eq!(
            PluginWindow::height(400, false, NO_GRAPH),
            PluginWindow::MAX_LIST_HEIGHT
        );
    }

    #[test]
    fn a_row_costs_the_gap_beside_it_as_well_as_its_own_height() {
        // The bug this replaced: the estimate counted six rows at twenty-two pixels and left the
        // body's `gap_1` out, so a compressor's last control was cut through the middle.
        let step =
            PluginWindow::height(7, false, NO_GRAPH) - PluginWindow::height(6, false, NO_GRAPH);
        assert_eq!(step, Metrics::CONTROL_HEIGHT + PluginWindow::ROW_GAP);

        // And the whole thing is at least as tall as what has to go in it. A window that has
        // reached the ceiling is allowed to be shorter — that is what the scrollbar is for.
        let rows = 7.0;
        let needed = Metrics::PANEL_HEADER_HEIGHT
            + Metrics::CONTROL_HEIGHT * rows
            + PluginWindow::ROW_GAP * (rows - 1.0)
            + PluginWindow::BODY_PADDING * 2.0;
        assert!(PluginWindow::height(7, false, NO_GRAPH) >= needed);
        assert!(
            needed < PluginWindow::MAX_LIST_HEIGHT,
            "still short of the cap"
        );
    }

    #[test]
    fn a_window_with_a_picture_keeps_the_list_it_would_have_had_without_one() {
        // A graph sits outside the scrolling body, so counting it against the same ceiling would
        // have taken a third of the sliders away in exchange for drawing them a picture.
        let curve = px(crate::ui::analyser::HEIGHT);
        let plain = PluginWindow::frame(false, NO_GRAPH);
        let curved = PluginWindow::frame(true, curve);
        assert_eq!(plain.height, PluginWindow::MAX_LIST_HEIGHT);
        assert_eq!(curved.height, plain.height + curve);
        assert!(curved.width > plain.width, "and a curve is drawn wider");

        // The envelope graph is the same bargain at a different height — the sampler grew one
        // without growing a curve, which is what made this a number rather than a flag.
        let envelope = px(crate::ui::envelope::GRAPH_HEIGHT);
        assert_eq!(
            PluginWindow::frame(false, envelope).height,
            plain.height + envelope
        );
        assert_eq!(
            PluginWindow::height(5, false, envelope) - PluginWindow::height(5, false, NO_GRAPH),
            envelope
        );
    }

    #[test]
    fn the_only_switch_that_warns_is_the_one_that_costs_something() {
        // The sampler's envelope takes polyphony and a drum kit's choke groups away from a user
        // who cannot see either going. Every other toggle in the application does what it says.
        assert_eq!(caution(SAMPLER_ID, true), Some(Key::SamplerEnvelopeOn));
        assert_eq!(caution(SAMPLER_ID, false), None);
        assert_eq!(caution("auris.synth.chiptune", true), None);
        assert_eq!(caution("auris.fx.eq", true), None);
    }

    #[test]
    fn the_warning_makes_room_for_itself() {
        // Same lesson as the compressor's clipped row: anything drawn above the scrolling list is
        // height the window needs, and a strip that appeared without being counted would push the
        // last control out of sight the moment the switch went on.
        assert_eq!(
            PluginWindow::height(6, false, CAUTION_HEIGHT)
                - PluginWindow::height(6, false, NO_GRAPH),
            CAUTION_HEIGHT
        );
    }
}
