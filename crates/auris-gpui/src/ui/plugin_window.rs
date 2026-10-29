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
use auris_session::PluginWindow as HostedWindow;
use auris_session::prelude::*;
use gpui::{AnyElement, Pixels, div, prelude::*, px};

use crate::app::AurisApp;
use crate::theme::{Metrics, Theme};
use crate::ui::context_menu::{ContextMenu, MenuCommand, MenuEntry};
use crate::ui::icons::Icon;
use crate::ui::plugin_editor::plugin_header;
use crate::ui::widgets::chain_button;
use crate::ui::widgets::{ButtonStyle, button};

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
        if self
            .plugin_window
            .is_some_and(|editor| editor.subject == subject)
        {
            self.raise_auxiliary = Some(crate::auxiliary_window::Surface::Plugin);
            return;
        }
        self.close_physical_view();
        self.parameter_list = Default::default();
        self.plugin_window = Some(PluginWindow { subject });
        self.raise_auxiliary = Some(crate::auxiliary_window::Surface::Plugin);
    }

    /// Closes the editor, reporting whether one was open.
    pub(crate) fn close_plugin_window(&mut self) -> bool {
        self.close_physical_view();
        let session = &self.session;
        close_after(&mut self.plugin_window, || session.stop_watching())
    }

    /// Opens the native editor directly, falling back to Auris's editor when none is offered.
    pub(crate) fn show_plugin_editor(&mut self, subject: PluginSubject) {
        if self.session.plugin_window_exists(subject.hosted_window()) {
            self.show_hosted_window(subject);
        } else {
            self.open_plugin_window(subject);
        }
    }

    /// Whether the primary editor action will bring an existing window to the front.
    pub(crate) fn plugin_editor_is_open(&mut self, subject: PluginSubject) -> bool {
        if self.session.plugin_window_exists(subject.hosted_window()) {
            self.session.plugin_window_is_open(subject.hosted_window())
        } else {
            self.plugin_window
                .is_some_and(|editor| editor.subject == subject)
        }
    }

    /// The visible primary action shared by instrument and effect slots.
    pub(crate) fn plugin_editor_button(
        &mut self,
        id: impl Into<gpui::ElementId>,
        subject: PluginSubject,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let open = self.plugin_editor_is_open(subject);
        crate::ui::widgets::bounded_button_enabled(
            id,
            self.t(if open {
                Key::PluginRaiseEditor
            } else {
                Key::PluginOpenEditor
            }),
            ButtonStyle::Normal,
            crate::ui::widgets::ButtonState::available(false, true),
            self.theme.accent_soft,
            &self.theme,
            cx.listener(move |this, _, _, cx| {
                this.show_plugin_editor(subject);
                cx.notify();
            }),
        )
        .flex_shrink_0()
    }

    /// A compact strip keeps the plugin's name above its full-width editor command.
    pub(crate) fn compact_plugin_controls(
        &mut self,
        id: impl Into<gpui::ElementId>,
        subject: PluginSubject,
        name: String,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let id = id.into();
        let menu_id = gpui::SharedString::from(format!("{id}-menu"));
        let name_id = gpui::SharedString::from(format!("{id}-name"));
        let row_id = id.clone();
        div()
            .debug_selector(move || format!("{row_id}-row"))
            .flex()
            .flex_col()
            .gap_0p5()
            .min_w_0()
            .w_full()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .child(
                        div()
                            .id(name_id)
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .child(name.clone())
                            .tooltip(crate::ui::tooltip::keyed_tip(name.clone(), "", &self.theme)),
                    )
                    .child(self.plugin_menu_button(menu_id, subject, name, cx)),
            )
            .child(self.plugin_editor_button(id, subject, cx).w_full())
            .into_any_element()
    }

    /// Secondary commands stay discoverable beside the editor action.
    pub(crate) fn plugin_menu_button(
        &self,
        id: impl Into<gpui::ElementId>,
        subject: PluginSubject,
        name: String,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let open = self.menu.as_ref().is_some_and(|menu| menu.entries.iter().any(|entry| {
            matches!(entry, MenuEntry::Item(item) if item.command == MenuCommand::PluginParameters(subject))
        }));
        crate::ui::widgets::button_with_content(
            id,
            crate::ui::icons::icon(Icon::More, px(12.), self.theme.text),
            ButtonStyle::Ghost,
            crate::ui::widgets::ButtonState::available(open, true),
            self.theme.accent_soft,
            &self.theme,
            Self::opens_menu(cx, move |this, at| match subject {
                PluginSubject::Insert { track, slot } => {
                    this.effect_menu(at, track, slot, name.clone())
                }
                PluginSubject::Instrument(_) => {
                    let menu = ContextMenu::new(at, name.clone());
                    this.plugin_window_menu_entries(menu, subject)
                }
            }),
        )
        .w(Metrics::CONTROL_HEIGHT)
        .px_0()
        .flex_shrink_0()
        .tooltip(self.tip(Key::PluginActions, ""))
    }

    /// Adds parameter access and a separate close command to a plugin's menu.
    pub(crate) fn plugin_window_menu_entries(
        &mut self,
        menu: ContextMenu,
        subject: PluginSubject,
    ) -> ContextMenu {
        let menu = menu.item(
            self.t(Key::PluginParameters),
            MenuCommand::PluginParameters(subject),
        );
        if self.plugin_editor_is_open(subject) {
            menu.item(
                self.t(Key::PluginCloseEditor),
                MenuCommand::ClosePluginEditor(subject),
            )
        } else {
            menu
        }
    }

    /// Closes the primary editor without changing the plugin's enabled state or parameters.
    pub(crate) fn close_plugin_editor(&mut self, subject: PluginSubject) {
        if self.session.plugin_window_exists(subject.hosted_window()) {
            if let Err(error) = self
                .session
                .set_plugin_window_open(subject.hosted_window(), false)
            {
                let line = self.failure(Key::PluginCloseEditor, &error);
                self.set_failed_status(line);
            }
        } else if self
            .plugin_window
            .is_some_and(|editor| editor.subject == subject)
        {
            self.close_plugin_window();
        }
    }

    /// Opens the plugin's own window, or raises the existing editor without closing it.
    pub(crate) fn show_hosted_window(&mut self, subject: PluginSubject) {
        let which = subject.hosted_window();
        if let Err(error) = self.session.set_plugin_window_open(which, true) {
            let line = self.failure(Key::CmdOpenPluginWindow, &error);
            self.set_failed_status(line);
        }
    }

    /// Draws the editor while its track and plugin still exist.
    pub(crate) fn render_plugin_window(
        &mut self,
        host_window: &gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> Option<AnyElement> {
        let window = self.plugin_window.take()?;
        let subject = window.subject;
        let Some((plugin_id, enabled)) = self.resolve_plugin(subject) else {
            // The track, slot, or instrument disappeared while its EQ was open. The window is
            // gone now too, so the analysis it kept alive must end on the same transition.
            self.session.stop_watching();
            self.close_physical_view();
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
        let wide = host_window.viewport_size().width >= host_window.rem_size() * 48.;
        let physical = self.physical_model_display(wide, cx);
        // The native editor leaves this window open behind it. Building thousands of hidden
        // controls on every meter tick otherwise starves the same UI thread that drives it.
        let body = if let Some(physical) = physical {
            let spacing = theme.atelier.spacing;
            div()
                .id("pw-body")
                .debug_selector(|| "pw-body".into())
                .flex_1()
                .min_h_0()
                .flex()
                .gap(spacing.lg)
                .p(spacing.lg)
                .when(!wide, |body| body.flex_col().overflow_y_scroll())
                .child(
                    div()
                        .id("pw-physical-view")
                        .flex_1()
                        .min_w_0()
                        .when(wide, |pane| pane.min_h_0().overflow_y_scroll())
                        .child(physical),
                )
                .child(
                    div()
                        .id("pw-physical-parameters")
                        .flex()
                        .flex_col()
                        .pt(spacing.lg)
                        .gap(spacing.sm)
                        .w(gpui::rems(20.))
                        .flex_shrink_0()
                        .when(wide, |pane| pane.min_h_0().overflow_y_scroll())
                        .when(!wide, |pane| pane.w_full())
                        .child(atelier::SectionHeader::new(self.t(Key::PhysicalParameters)))
                        .children(self.param_controls(
                            &descriptors,
                            move |param| subject.param_target(param),
                            subject.id_prefix(),
                            cx,
                        )),
                )
                .into_any_element()
        } else if descriptors.len() > 128 {
            self.parameter_list(
                descriptors,
                move |param| subject.param_target(param),
                subject.id_prefix(),
                cx,
            )
        } else {
            div()
                .id("pw-body")
                .debug_selector(|| "pw-body".into())
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .children(self.param_controls(
                    &descriptors,
                    move |param| subject.param_target(param),
                    subject.id_prefix(),
                    cx,
                ))
                .into_any_element()
        };
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
                            button(
                                "pw-own",
                                self.t(
                                    if self.session.plugin_window_is_open(subject.hosted_window()) {
                                        Key::PluginRaiseEditor
                                    } else {
                                        Key::PluginOpenEditor
                                    },
                                ),
                                ButtonStyle::Normal,
                                false,
                                theme.accent_soft,
                                &theme,
                                cx.listener(move |this, _, _, cx| {
                                    this.show_hosted_window(subject);
                                    cx.notify();
                                }),
                            )
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
                .child(body)
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
#[path = "plugin_window_tests.rs"]
mod interaction_tests;

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        static PARAMETER_ROWS_BUILT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    struct LargeParameterEditor {
        app: gpui::Entity<AurisApp>,
        descriptors: std::sync::Arc<Vec<ParamDescriptor>>,
    }

    impl gpui::Render for LargeParameterEditor {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let list = self.app.update(cx, |app, cx| {
                app.parameter_list(
                    self.descriptors.clone(),
                    |param| {
                        PARAMETER_ROWS_BUILT.with(|count| count.set(count.get() + 1));
                        ParamTarget::Instrument {
                            track: TrackId(1),
                            param,
                        }
                    },
                    "large-test",
                    cx,
                )
            });
            div()
                .key_context(crate::actions::KEY_CONTEXT)
                .size_full()
                .flex()
                .flex_col()
                .child(list)
        }
    }

    fn large_parameter_editor(
        cx: &mut gpui::TestAppContext,
    ) -> (gpui::Entity<AurisApp>, &mut gpui::VisualTestContext) {
        let (app, _) = crate::harness::open(cx);
        let descriptors = std::sync::Arc::new(
            (0..3000)
                .map(|id| ParamDescriptor::new(ParamId(id), "test", "Parameter", 0., 1., 0.5))
                .collect(),
        );
        PARAMETER_ROWS_BUILT.with(|count| count.set(0));
        let (_, cx) = cx.add_window_view(|_, cx| {
            cx.observe(&app, |_, _, cx| cx.notify()).detach();
            LargeParameterEditor {
                app: app.clone(),
                descriptors,
            }
        });
        cx.run_until_parked();
        (app, cx)
    }

    #[gpui::test]
    fn large_parameter_editor_builds_only_visible_rows(cx: &mut gpui::TestAppContext) {
        let (_, _) = large_parameter_editor(cx);
        let built = PARAMETER_ROWS_BUILT.with(|count| count.get());
        assert!(
            built > 0 && built < 200,
            "built {built} rows for a 3000-parameter plugin"
        );
    }

    #[gpui::test]
    fn large_parameter_editor_scrolls_to_the_last_parameter(cx: &mut gpui::TestAppContext) {
        let (app, cx) = large_parameter_editor(cx);
        let body = cx.debug_bounds("pw-body").unwrap();
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: body.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-1_000_000.))),
            ..Default::default()
        });
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("large-test-param-2999").is_some());
    }

    #[gpui::test]
    fn large_parameter_editor_tabs_across_unrendered_rows(cx: &mut gpui::TestAppContext) {
        let (app, cx) = large_parameter_editor(cx);
        cx.update(|window, cx| {
            window.focus(&app.read(cx).parameter_list.rows[0]);
            window.focus_next();
        });
        for _ in 0..60 {
            cx.simulate_keystrokes("tab");
            cx.run_until_parked();
        }
        cx.update(|window, cx| {
            let rows = &app.read(cx).parameter_list.rows;
            assert!(
                rows[60].contains_focused(window, cx),
                "focused row: {:?}",
                rows.iter().position(|row| row.contains_focused(window, cx))
            );
        });
        for _ in 0..60 {
            cx.simulate_keystrokes("shift-tab");
            cx.run_until_parked();
        }
        cx.update(|window, cx| {
            assert!(app.read(cx).parameter_list.rows[0].contains_focused(window, cx));
        });
    }

    #[gpui::test]
    #[ignore = "requires AURIS_VST3_SMOKE_PLUGIN; measures the real plugin parameter window"]
    fn external_plugin_window_frame_cost(cx: &mut gpui::TestAppContext) {
        use crate::{auxiliary_window::Surface, harness};
        let path = std::path::PathBuf::from(std::env::var_os("AURIS_VST3_SMOKE_PLUGIN").unwrap());
        let (app, cx) = harness::open(cx);
        let track = app.update(cx, |app, _| {
            let info = app.session.vst3_plugins_in(&path).unwrap().remove(0);
            let track = app
                .session
                .add_default_instrument_track("External")
                .unwrap();
            app.session
                .set_vst3_instrument(track, &path, &info.class_id)
                .unwrap();
            track
        });
        harness::paint(&app, cx);
        app.update(cx, |app, _| {
            app.open_plugin_window(PluginSubject::Instrument(track))
        });
        harness::paint(&app, cx);
        let handle = app.read_with(cx, |app, _| app.auxiliary_windows[&Surface::Plugin]);
        let mut utility = gpui::VisualTestContext::from_window(handle.into(), cx);
        let count = app.update(&mut utility, |app, _| {
            app.session.instrument_descriptors(track).len()
        });
        assert!(
            count > 1000,
            "use a large plugin for this regression: {count}"
        );
        let start = std::time::Instant::now();
        for _ in 0..30 {
            harness::paint(&app, &mut utility);
        }
        eprintln!(
            "{count} parameters, 30 host editor frames: {:?}",
            start.elapsed()
        );
    }

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
