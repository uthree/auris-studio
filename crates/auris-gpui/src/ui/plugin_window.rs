//! The floating plugin editor.
//!
//! Logic opens a plugin's controls in a window of their own, and the reason is not decoration:
//! the strip you clicked stays where it was, so the next insert is one click away and the chain
//! is still readable while a parameter moves. Editing in place — which is what the inspector used
//! to do, one expanding card per effect — pushes everything below it down the panel and turns a
//! four-effect chain into a scroll.
//!
//! The native utility window supplies shared gesture and keyboard routing.

use auris_i18n::Key;
// Distinguish Auris's parameter editor from the hosted plugin's own editor.
use auris_session::HasWindowHandle;
use auris_session::PluginWindow as HostedWindow;
use auris_session::prelude::*;
use gpui::{AnyElement, Pixels, div, prelude::*, px};

use crate::app::AurisApp;
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

/// How tall the row naming the track an effect is keyed from is drawn.
const SIDECHAIN_HEIGHT: Pixels = px(28.0);

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
}

/// Takes a plugin window only after ending the per-block work it kept alive.
fn close_after<T>(window: &mut Option<T>, stop_watching: impl FnOnce()) -> bool {
    stop_watching();
    window.take().is_some()
}

impl AurisApp {
    /// Opens the editor for one plugin, replacing whatever was open.
    pub(crate) fn open_plugin_window(&mut self, subject: PluginSubject) {
        self.plugin_window = Some(PluginWindow { subject });
        self.raise_auxiliary = Some(crate::auxiliary_window::Surface::Plugin);
    }

    /// Closes the editor, reporting whether one was open.
    pub(crate) fn close_plugin_window(&mut self) -> bool {
        let session = &self.session;
        close_after(&mut self.plugin_window, || session.stop_watching())
    }

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

    /// Draws the editor while its track and plugin still exist.
    pub(crate) fn render_plugin_window(
        &mut self,
        cx: &mut gpui::Context<Self>,
    ) -> Option<AnyElement> {
        let window = self.plugin_window.take()?;
        let subject = window.subject;
        let Some((plugin_id, enabled)) = self.resolve_plugin(subject) else {
            // The track, slot, or instrument disappeared while its EQ was open. The window is
            // gone now too, so the analysis it kept alive must end on the same transition.
            self.session.stop_watching();
            return None;
        };
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
        // Which track keys this effect, for the one row that can say so — and `None` for every
        // effect with no reading for a key, which is what keeps the row off the other plugins.
        // The name is resolved here rather than in the row so a source deleted between frames
        // shows as nothing rather than as a number.
        let keyed_from = match subject {
            PluginSubject::Insert { track, slot }
                if self.session.effect_wants_sidechain(track, slot) =>
            {
                let source = self
                    .session
                    .effect_sidechain(track, slot)
                    .and_then(|id| self.project().track(id))
                    .map(|entry| entry.name.clone());
                Some((track, slot, source))
            }
            _ => None,
        };
        let controls = self.param_controls(
            &descriptors,
            move |param| subject.param_target(param),
            subject.id_prefix(),
            cx,
        );

        Some(
            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(theme.surface_raised)
                .border_1()
                .border_color(theme.border)
                // The body occludes its parent, so it also routes slider drags.
                .occlude()
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
                        .child(div().flex_1().min_w_0().child(plugin_header(
                            gpui::SharedString::from(format!("{}-bypass", subject.id_prefix())),
                            name,
                            enabled,
                            matches!(subject, PluginSubject::Insert { .. }),
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
                            .tooltip(self.tip(Key::CmdOpenPluginWindow, ""))
                        }))
                        .child(
                            chain_button(
                                "pw-close",
                                Icon::Cross,
                                &theme,
                                cx.listener(|this, _, _, cx| {
                                    this.close_plugin_window();
                                    cx.notify();
                                }),
                            )
                            .tooltip(self.tip(Key::Close, "")),
                        ),
                )
                .children(analyser)
                .children(envelope.map(|env| self.envelope_display(subject, env, cx)))
                // Under the header and above the controls, because it is a fact about the whole
                // plugin rather than one of its parameters — and because the answer changes what
                // every parameter below it is doing.
                .children(keyed_from.map(|(track, slot, source)| {
                    div()
                        .h(SIDECHAIN_HEIGHT)
                        .w_full()
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_1()
                        .px_2()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child(self.t(Key::MenuSidechain)),
                        )
                        .child(crate::ui::widgets::button(
                            "pw-key",
                            source.unwrap_or_else(|| self.t(Key::MenuSidechainNone).to_string()),
                            crate::ui::widgets::ButtonStyle::Ghost,
                            true,
                            theme.accent_soft,
                            &theme,
                            Self::opens_menu(cx, move |this, at| {
                                this.sidechain_menu(at, track, slot)
                            }),
                        ))
                        .into_any_element()
                }))
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
    fn closing_a_plugin_window_stops_its_analysis_before_taking_it() {
        let stopped = std::cell::Cell::new(false);
        let mut window = Some(1);
        assert!(close_after(&mut window, || stopped.set(true)));
        assert!(stopped.get());
        assert!(window.is_none());
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

    #[test]
    fn the_only_switch_that_warns_is_the_one_that_costs_something() {
        // The sampler's envelope takes polyphony and a drum kit's choke groups away from a user
        // who cannot see either going. Every other toggle in the application does what it says.
        assert_eq!(caution(SAMPLER_ID, true), Some(Key::SamplerEnvelopeOn));
        assert_eq!(caution(SAMPLER_ID, false), None);
        assert_eq!(caution("auris.synth.chiptune", true), None);
        assert_eq!(caution("auris.fx.eq", true), None);
    }
}
