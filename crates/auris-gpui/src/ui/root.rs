//! The window's root layout, global pointer handling and action dispatch.

use auris_i18n::{Key, messages};
use auris_session::prelude::*;

use gpui::{
    AnyElement, Axis, Context, IntoElement, MouseMoveEvent, MouseUpEvent, Render, Window, div,
    prelude::*, px, relative,
};

use crate::actions;
use crate::app::{AurisApp, Drag, Pane};
use crate::dock::{Dock, Panel};
use crate::gestures::past_drag_threshold;
use crate::menu::MenuRow;
use crate::theme::Theme;
use crate::ui::context_menu::MenuCommand;
use crate::ui::drop::{drop_action, lanes_offset};
use crate::ui::menu_bar;
use crate::ui::widgets::splitter;

impl Render for AurisApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Pointer coordinates arrive in window space, and the resize handlers need to know how
        // tall the window is to bound the bottom panel; record it once per frame.
        self.viewport_height = window.viewport_size().height;
        // Sourced from where the lanes were actually painted, so it stays right through a
        // panel resize instead of being re-derived from constants that no longer hold.
        self.arrangement_width = self
            .canvas
            .lanes
            .get()
            .map_or(window.viewport_size().width, |bounds| bounds.size.width);

        // Keep the playhead on screen while the transport rolls, but never fight the user's
        // own scrolling when it is stopped.
        if self.is_playing() {
            let playhead = self.playhead_ticks();
            let width = self.arrangement_width;
            self.timeline.scroll_to_reveal(playhead, width);
        }

        // The taskbar and the Alt-Tab list are where a user looks to tell one project's window
        // from another's, and they were both showing a constant. Only on a change: setting it is
        // a call into the platform, and this runs on every repaint.
        let title = self.window_title();
        if title != self.titled {
            window.set_window_title(&title);
            self.titled = title;
        }

        // Before anything is built: a sheet needs the keyboard for the platform to type into it,
        // and a panel needs it back once the sheet is gone.
        self.reconcile_focus(window);

        let theme = self.theme.clone();
        let menu_bar = self.render_menu_bar(window, cx);
        let transport = self.render_transport(window, cx);
        let arrangement = self.render_arrangement(window, cx);
        let plugin_window = self.render_plugin_window(window.viewport_size(), cx);
        // Built before the layout so each one can borrow the window to ask whether it has the
        // keyboard, which the layout below is too deep inside a builder chain to do.
        let left = self.render_dock(Dock::Left, window, cx);
        let bottom = self.render_dock(Dock::Bottom, window, cx);
        let right = self.render_dock(Dock::Right, window, cx);
        let left_divider = left.is_some().then(|| self.dock_divider(Dock::Left, cx));
        let bottom_divider = bottom
            .is_some()
            .then(|| self.dock_divider(Dock::Bottom, cx));
        let right_divider = right.is_some().then(|| self.dock_divider(Dock::Right, cx));
        let arrangement_pane = self.pane(Pane::Arrangement, window, cx);
        let status = self.render_status_bar(cx);
        let export_overlay = self.render_export_overlay(cx);
        let song_sheet = self.render_song_sheet(cx);
        let prompt = self.render_prompt(cx);
        let palette = self.render_palette(cx);
        let menu = self.render_context_menu(window, cx);
        // Files dragged in from the desktop, taken by the whole window rather than by one panel:
        // what a file is, is its extension, and which rectangle it was let go over is not
        // something a person should have to get right for a drop to be understood.
        let drop_ring = self.render_drop_ring(cx);

        div()
            .id("root")
            // While something is being typed into, the application's own bindings must not fire:
            // `i` has to type an `i`, not toggle the inspector. Swapping the root's context for
            // one nothing is bound to disables the window's bindings in a single move; the panes
            // drop their contexts at the same time, in `pane_context`, which is what stops a
            // pane-scoped binding firing from the pane that still holds focus behind the sheet.
            .key_context(if self.keys_are_claimed() {
                actions::context::PROMPT
            } else {
                actions::KEY_CONTEXT
            })
            .track_focus(&self.focus)
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .text_color(theme.text)
            .font(crate::theme::ui_font())
            .text_sm()
            .on_action(cx.listener(Self::on_toggle_play))
            .on_action(cx.listener(Self::on_stop))
            .on_action(cx.listener(Self::on_return_to_zero))
            .on_action(cx.listener(Self::on_toggle_loop))
            .on_action(cx.listener(Self::on_toggle_metronome))
            .on_action(cx.listener(Self::on_toggle_recording))
            .on_action(cx.listener(Self::on_toggle_monitoring))
            .on_action(cx.listener(Self::on_toggle_punch))
            .on_action(cx.listener(Self::on_new_project))
            .on_action(cx.listener(Self::on_open_project))
            .on_action(cx.listener(Self::on_quit))
            .on_action(cx.listener(Self::on_compose_song))
            .on_action(cx.listener(Self::on_compose_from_spec))
            .on_action(cx.listener(Self::on_accompany_melody))
            .on_action(cx.listener(Self::on_save_project))
            .on_action(cx.listener(Self::on_save_project_as))
            .on_action(cx.listener(Self::on_import_audio))
            .on_action(cx.listener(Self::on_import_soundfont))
            .on_action(cx.listener(Self::on_import_midi))
            .on_action(cx.listener(Self::on_export_midi))
            .on_action(cx.listener(Self::on_collect_assets))
            .on_action(cx.listener(Self::on_export_audio))
            .on_action(cx.listener(Self::on_export_cycle))
            .on_action(cx.listener(Self::on_add_instrument_track))
            .on_action(cx.listener(Self::on_add_audio_track))
            .on_action(cx.listener(Self::on_add_bus_track))
            .on_action(cx.listener(Self::on_duplicate_track))
            .on_action(cx.listener(Self::on_toggle_track_mute))
            .on_action(cx.listener(Self::on_toggle_track_solo))
            .on_action(cx.listener(Self::on_delete_track))
            .on_action(cx.listener(Self::on_delete_selection))
            .on_action(cx.listener(Self::on_select_all_notes))
            .on_action(cx.listener(Self::on_duplicate_notes))
            .on_action(cx.listener(Self::on_cut_notes))
            .on_action(cx.listener(Self::on_copy_notes))
            .on_action(cx.listener(Self::on_paste_notes))
            .on_action(cx.listener(Self::on_cut_clips))
            .on_action(cx.listener(Self::on_copy_clips))
            .on_action(cx.listener(Self::on_paste_clips))
            .on_action(cx.listener(Self::on_transpose_up))
            .on_action(cx.listener(Self::on_transpose_down))
            .on_action(cx.listener(Self::on_octave_up))
            .on_action(cx.listener(Self::on_octave_down))
            .on_action(cx.listener(Self::on_select_all_clips))
            .on_action(cx.listener(Self::on_duplicate_clip))
            .on_action(cx.listener(Self::on_split_clip))
            .on_action(cx.listener(Self::on_toggle_clip_mute))
            .on_action(cx.listener(Self::on_toggle_clip_loop))
            .on_action(cx.listener(Self::on_quantize_starts))
            .on_action(cx.listener(Self::on_quantize_lengths))
            .on_action(cx.listener(Self::on_quantize_notes))
            .on_action(cx.listener(Self::on_next_tool))
            .on_action(cx.listener(Self::on_set_tempo))
            .on_action(cx.listener(Self::on_set_time_signature))
            .on_action(cx.listener(Self::on_cycle_grid))
            .on_action(cx.listener(Self::on_go_to_position))
            .on_action(cx.listener(Self::on_undo))
            .on_action(cx.listener(Self::on_redo))
            .on_action(cx.listener(Self::on_panic_stop))
            .on_action(cx.listener(Self::on_zoom_in))
            .on_action(cx.listener(Self::on_zoom_out))
            .on_action(cx.listener(Self::on_toggle_library))
            .on_action(cx.listener(Self::on_toggle_inspector))
            .on_action(cx.listener(Self::on_toggle_piano_roll))
            .on_action(cx.listener(Self::on_toggle_mixer))
            .on_action(cx.listener(Self::on_toggle_log))
            .on_action(cx.listener(Self::on_toggle_structure_lane))
            .on_action(cx.listener(Self::on_toggle_harmony_lane))
            .on_action(cx.listener(Self::on_toggle_tempo_marks))
            .on_action(cx.listener(Self::on_toggle_bend_lane))
            .on_action(cx.listener(Self::on_toggle_modulation_lane))
            .on_action(cx.listener(Self::on_open_settings))
            .on_action(cx.listener(Self::on_open_command_palette))
            .on_action(cx.listener(Self::on_open_menu_bar))
            .on_action(cx.listener(Self::on_focus_next_pane))
            .on_action(cx.listener(Self::on_focus_previous_pane))
            // A click anywhere else dismisses an open menu-bar menu, the way a native menu bar
            // behaves. Capture phase, so it is seen even where a panel stops the event before it
            // reaches the root — but not over the bar itself, which decides for itself whether a
            // click is opening a menu or toggling one shut and needs the state this would clear.
            .capture_any_mouse_down(cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                if event.position.y <= crate::ui::menu_bar::HEIGHT {
                    return;
                }
                if this.close_menu_bar() {
                    cx.notify();
                }
            }))
            // Drags are tracked on the root so they keep working after the pointer leaves the
            // control that started them, which is what makes a fader usable.
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(gpui::MouseButton::Left, cx.listener(Self::on_mouse_up))
            // …and a release *outside* the window ends them too. Letting go over the desktop is
            // what a user does when they have dragged a fader to the end of its travel, and
            // without this the drag never finishes: the pointer comes back still holding the
            // clip, and the transaction it opened silently eats every edit made afterwards.
            .on_mouse_up_out(gpui::MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_key_down(cx.listener(Self::on_key_down))
            .children(menu_bar)
            .child(transport)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    // The left dock leads, then the arrangement with the bottom dock under it,
                    // then the right dock — so the bottom one spans the middle column rather than
                    // the whole window, which is what leaves the side docks full height. Only the
                    // middle column carries `flex_1().min_w_0()`, so a narrowing window takes the
                    // timeline and never a panel: a panel that shrank would move every hit test in
                    // it out from under the pointer.
                    .children(left)
                    .children(left_divider)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            // No `min_h_0` here: the arrangement asserts its own minimum height,
                            // and a wrapper that allowed shrinking past it would let the bottom
                            // dock push the lanes out of existence.
                            .child(arrangement_pane.flex().flex_1().child(arrangement))
                            .children(bottom_divider)
                            .children(bottom),
                    )
                    .children(right_divider)
                    .children(right),
            )
            .child(status)
            .children(export_overlay)
            .children(song_sheet)
            .child(drop_ring)
            // These come last so they paint — and are hit-tested — above the panels. The plugin
            // editor sits below the menu because a right-click inside it opens one.
            .children(prompt)
            .children(palette)
            .children(plugin_window)
            .children(menu)
    }
}

impl AurisApp {
    /// The wrapper that makes a panel a place the keyboard can be.
    ///
    /// Four things, and a panel missing any one of them is a panel whose bindings sometimes work:
    /// a focus handle to be focused, a key context so its own bindings are on the dispatch path
    /// while it is, a tab stop so the keyboard can get there without the mouse, and a ring so the
    /// user can see where it went.
    fn pane(&self, pane: Pane, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let focused = self.pane_focused(pane, window, cx);
        let ring = if focused {
            self.theme.accent_soft
        } else {
            // Painted either way rather than added on focus: a border that appeared would move
            // every hit test in the panel by a pixel at the moment the user clicked into it.
            gpui::transparent_black()
        };
        div()
            // The tab order is on the handle rather than here: `div().tab_index(n)` is copied
            // onto a handle gpui made itself and silently ignored for one the application owns.
            // See `PaneFocus::new`.
            .track_focus(self.panes.handle(pane))
            .when_some(self.pane_context(pane), |this, context| {
                this.key_context(context)
            })
            // Capture, so the panel takes the keyboard however deep in it the press lands and
            // whatever that press goes on to do. A click on a fader is still a click on the
            // mixer, and the handler that moves the fader stops the event before the bubble
            // phase would ever reach here.
            .capture_any_mouse_down(cx.listener(
                move |this, _: &gpui::MouseDownEvent, window, cx| {
                    this.focus_pane(pane, window);
                    cx.notify();
                },
            ))
            .border_1()
            .border_color(ring)
    }

    /// One dock, as the panel it is showing, or nothing when it is showing none.
    ///
    /// A dock is a size and a place; what fills it is whichever of its panels is up, so the
    /// wrapper is built here and the panel itself has no idea where it has been put.
    fn render_dock(
        &mut self,
        dock: Dock,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        let panel = self.panels.showing(dock)?;
        let size = self.panels.size(dock);
        let wrapper = self
            .pane(panel.pane(), window, cx)
            .flex_shrink_0()
            .flex()
            // Clipped, because a panel can now be given a dock it was never laid out for: the
            // roll's header strip is wider than a side column, and without this it would paint
            // its zoom slider across the arrangement next door.
            .overflow_hidden();
        let content = self.render_panel(panel, window, cx);
        Some(
            match dock.is_side() {
                true => wrapper.w(size),
                false => wrapper.h(size),
            }
            .child(content),
        )
    }

    /// What one panel draws.
    fn render_panel(
        &mut self,
        panel: Panel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match panel {
            Panel::Library => self.render_library(window, cx).into_any_element(),
            Panel::PianoRoll => self.render_piano_roll(window, cx),
            Panel::Mixer => self.render_mixer(window, cx).into_any_element(),
            Panel::Inspector => self.render_inspector(window, cx).into_any_element(),
            Panel::Log => self.render_log(window, cx).into_any_element(),
        }
    }

    /// The strip between a dock and the arrangement, which drags to resize it.
    fn dock_divider(&self, dock: Dock, cx: &mut Context<Self>) -> AnyElement {
        let axis = match dock.is_side() {
            true => Axis::Vertical,
            false => Axis::Horizontal,
        };
        splitter(
            ("split-dock", dock as usize),
            axis,
            &self.theme,
            cx.listener(move |this, event: &gpui::MouseDownEvent, _, _| {
                let start_size = this.panels.size(dock);
                // The axis the dock is measured along, which is the one its divider slides on.
                let start = match dock.is_side() {
                    true => event.position.x,
                    false => event.position.y,
                };
                this.begin_drag(Drag::ResizeDock {
                    dock,
                    start,
                    start_size,
                });
            }),
        )
        .into_any_element()
    }

    /// Moves the keyboard to the next panel, or the previous one.
    ///
    /// gpui walks the tab stops that were actually painted, so a hidden library or a shut dock
    /// drops out of the cycle without anything here having to know which panels are up.
    fn on_focus_next_pane(
        &mut self,
        _: &actions::FocusNextPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus_next();
        cx.notify();
    }

    fn on_focus_previous_pane(
        &mut self,
        _: &actions::FocusPreviousPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus_prev();
        cx.notify();
    }

    /// A modal-ish overlay shown while an export runs.
    fn render_export_overlay(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let export = self.export.as_ref()?;
        let theme = self.theme.clone();
        let fraction = export.fraction();
        let finished = export.result.is_some();
        let failed = matches!(export.result, Some(Err(_)));
        let message = match &export.result {
            Some(Ok(summary)) => summary.clone(),
            Some(Err(error)) => error.clone(),
            None => messages::rendering(self.language(), &export.path.display().to_string()),
        };

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(Theme::translucent(theme.background, 0.72))
                // Export is the longest thing this application does, and the screen went dim
                // while every click still landed on the arrangement underneath it.
                .occlude()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .w(px(420.0))
                        .p_4()
                        .rounded(crate::theme::Metrics::RADIUS_LG)
                        .bg(theme.surface_raised)
                        .border_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.text)
                                .child(self.t(Key::Export)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(if failed {
                                    theme.danger
                                } else {
                                    theme.text_muted
                                })
                                .child(message),
                        )
                        .child(
                            div()
                                .h(px(6.0))
                                .w_full()
                                .rounded(crate::theme::Metrics::RADIUS_SM)
                                .overflow_hidden()
                                .bg(theme.surface_sunken)
                                .child(
                                    // A render that failed used to fill this bar to the end in
                                    // the accent colour, which is exactly what a finished one
                                    // looks like — a completed export with no file at the end
                                    // of it. It stops where it got to, in the colour of a
                                    // failure.
                                    div()
                                        .h_full()
                                        .w(relative(if finished && !failed {
                                            1.0
                                        } else {
                                            fraction
                                        }))
                                        .bg(if failed { theme.danger } else { theme.accent }),
                                ),
                        )
                        .when(finished, |this| {
                            this.child(crate::ui::widgets::button(
                                "export-close",
                                self.t(Key::Close),
                                crate::ui::widgets::ButtonStyle::Primary,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(|this, _, _, cx| {
                                    this.export = None;
                                    cx.notify();
                                }),
                            ))
                        }),
                ),
        )
    }

    /// The window's drop target: it takes the file, and says so before the file is let go.
    ///
    /// Absolute and always present rather than added when a drag arrives: a border that appeared
    /// would inset the whole window by two pixels at the moment a file crossed into it, moving
    /// every lane out from under the pointer that is about to let go.
    ///
    /// The drop is taken *here* rather than on the root, and not only so that the rectangle which
    /// lights up is the rectangle that acts. A `drag_over` style is applied to a hitbox, and gpui
    /// gives an element a hitbox for a drop listener but not for a drag-over style alone — with
    /// the listener on the root this element had none, and the border never lit up at all.
    ///
    /// It stays dark for a drag holding nothing readable, so a folder or a PDF says beforehand
    /// that it is not going to be understood.
    fn render_drop_ring(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let accent = self.theme.accent;
        div()
            .id("drop-ring")
            .absolute()
            .inset_0()
            .border_2()
            .border_color(gpui::transparent_black())
            .on_drop(cx.listener(Self::on_files_dropped))
            .drag_over::<gpui::ExternalPaths>(move |style, paths, _, _| {
                match drop_action(paths.paths()).takes_anything() {
                    true => style.border_color(accent),
                    false => style,
                }
            })
    }

    /// Files dragged onto the window from the desktop.
    ///
    /// Only *where* is decided here; what a drop means is [`crate::ui::drop::drop_action`]'s. Audio
    /// lands where it was let go when that was over the lanes, snapped to the grid the way a clip
    /// dragged there would be, and at the playhead otherwise — over a panel, the transport or the
    /// track headers there is no position under the pointer to read.
    fn on_files_dropped(
        &mut self,
        paths: &gpui::ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let start = match lanes_offset(window.mouse_position(), self.canvas.lanes.get()) {
            Some(x) => self.snap(self.timeline.x_to_tick(x)).max_zero(),
            None => self.playhead_ticks(),
        };
        self.accept_drop(paths.paths().to_vec(), start, cx);
    }

    /// Follows the gesture in progress, wherever the pointer has got to.
    ///
    /// Registered on the root so a drag survives the pointer leaving the control that began it,
    /// which is what makes a fader usable. **An overlay that occludes has to register it again**:
    /// the hit test stops at the first blocking hitbox, so everything painted before that one —
    /// the root among them — reads as un-hovered, and gpui runs a bubble-phase `on_mouse_move`
    /// only on a hitbox that is hovered. Without it a slider inside the overlay takes the press
    /// and then sits still however far the pointer travels.
    pub(crate) fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.drag.clone() else {
            return;
        };
        match drag {
            Drag::Playhead => {
                let x = event.position.x - self.timeline_origin().x;
                let tick = self.snap_unless_held(self.timeline.x_to_tick(x), event.modifiers);
                self.seek(tick);
            }
            Drag::AuditionHarmony => {
                let x = event.position.x - self.timeline_origin().x;
                self.audition_chord(self.timeline.x_to_tick(x).max_zero());
            }
            // The point has moved, so the drag has to follow it: the next pointer move would
            // otherwise look for it where it no longer is and move nothing at all.
            Drag::CurvePoint { clip, which, at } => {
                if let Some(landed) = self.drag_curve_point(clip, which, at, event)
                    && let Some(Drag::CurvePoint { at, .. }) = &mut self.drag
                {
                    *at = landed;
                }
            }
            Drag::HarmonyChord { at, grab_offset } => {
                let x = event.position.x - self.timeline_origin().x;
                let landed = self
                    .session
                    .snap_harmony(self.timeline.x_to_tick(x) - grab_offset);
                // The chord has moved, so the drag has to follow it: the next pointer move will
                // otherwise look for it where it no longer is and move nothing at all.
                if self.session.move_chord(at, landed)
                    && let Some(Drag::HarmonyChord { at, .. }) = &mut self.drag
                {
                    *at = landed;
                }
            }
            Drag::SectionLabel { at, grab_offset } => {
                let x = event.position.x - self.timeline_origin().x;
                let wanted = (self.timeline.x_to_tick(x) - grab_offset).max_zero();
                // `move_section` snaps to the bar itself; the drag has to follow the boundary
                // for the same reason the chord drag does.
                if self.session.move_section(at, wanted) {
                    let landed = self
                        .session
                        .project()
                        .sections
                        .change_at(wanted)
                        .unwrap_or(wanted);
                    if let Some(Drag::SectionLabel { at, .. }) = &mut self.drag {
                        *at = landed;
                    }
                }
            }
            Drag::MixerScroll {
                start_x,
                start_offset,
            } => {
                let offset = crate::ui::widgets::scrollbar_dragged(
                    f32::from(start_offset),
                    f32::from(event.position.x - start_x),
                    f32::from(self.mixer_scroll.max_offset().width),
                    f32::from(self.mixer_scroll.bounds().size.width),
                );
                self.mixer_scroll
                    .set_offset(gpui::point(px(offset), self.mixer_scroll.offset().y));
            }
            Drag::LoopRegion { anchor } => {
                let x = event.position.x - self.timeline_origin().x;
                let tick = self
                    .snap_unless_held(self.timeline.x_to_tick(x), event.modifiers)
                    .max_zero();
                let (start, end) = if tick < anchor {
                    (tick, anchor)
                } else {
                    (anchor, tick)
                };
                self.session.set_loop_region(start, end);
            }
            Drag::ClipMove {
                clip,
                grab_offset,
                ref origins,
                ref origin_lanes,
                grab_lane,
                pressed_at,
            } => {
                // A press that has not travelled is a selection, not a move. Without this a
                // one-pixel wobble snapped an off-grid clip onto the grid — undoable, but it
                // reads as the arrangement rearranging itself under the pointer.
                if let Some(from) = pressed_at {
                    if !past_drag_threshold(from, event.position) {
                        return;
                    }
                    if let Some(Drag::ClipMove { pressed_at, .. }) = &mut self.drag {
                        *pressed_at = None;
                    }
                }
                let origin = self.lanes_origin();
                let tick = self.timeline.x_to_tick(event.position.x - origin.x);
                let start = self
                    .snap_unless_held(tick - grab_offset, event.modifiers)
                    .max_zero();
                // The delta comes from the clip under the pointer, so that one lands exactly on
                // the grid and the rest keep their spacing relative to it.
                let anchor = origins
                    .iter()
                    .find(|(id, _)| *id == clip)
                    .map(|(_, from)| *from)
                    .unwrap_or(start);
                self.session.move_clips(origins, start - anchor);

                // The same idea vertically: the lane under the pointer decides how far the whole
                // selection shifts, so a pair of clips on adjacent tracks stays a pair.
                let lanes = origin_lanes.clone();
                if let Some((under, _)) = self.track_at_y(self.lane_y(event.position.y)) {
                    self.move_clips_by_lane(&lanes, grab_lane, under);
                }
            }
            Drag::ClipResize { clip, edge } => {
                let x = event.position.x - self.lanes_origin().x;
                let tick = self.snap_unless_held(self.timeline.x_to_tick(x), event.modifiers);
                // Either edge rewrites the notes of a clip that has a recipe, and trimming the
                // front rebases the notes of one that has not — so the selected indices no longer
                // name the notes the user chose. Asked before the drag rather than after, since a
                // clip dragged down to nothing has nothing left to ask.
                let rewritten = self.project().midi_clip(clip).is_some_and(|(_, midi)| {
                    midi.is_generated() || edge == crate::app::ClipEdge::Start
                });
                let done = match edge {
                    crate::app::ClipEdge::End => self.session.resize_clip(clip, tick),
                    crate::app::ClipEdge::Start => self.session.trim_clip_start(clip, tick),
                };
                if done.is_ok() && rewritten {
                    self.forget_rewritten_notes(clip);
                }
            }
            Drag::ClipLoop { clip } => {
                let x = event.position.x - self.lanes_origin().x;
                let tick = self.snap_unless_held(self.timeline.x_to_tick(x), event.modifiers);
                // Measured from the clip's own start, which is what the field means. Nothing is
                // clamped here: dragged back inside the clip the session reads it as "no
                // repeats", which is how the gesture turns the loop off again.
                if let Some(start) = self.session.clip_start(clip) {
                    let _ = self.session.set_clip_loop(clip, tick - start);
                }
            }
            Drag::AutomationPoint { target, at } => {
                // The point has to be followed rather than remembered: it lands where the lane
                // put it, which is not always where it was asked to go — dropped onto another
                // point it absorbs that one, and the next move must look for it there.
                if let Some(landed) = self.drag_automation_point(target, at, event)
                    && let Some(Drag::AutomationPoint { at, .. }) = &mut self.drag
                {
                    *at = landed;
                }
            }
            Drag::TrackReorder { track, pressed_at } => {
                // The same wobble guard the clips and the notes have: a click on a header to
                // select a track must not reorder the list because the hand moved a pixel.
                if let Some(from) = pressed_at {
                    if !past_drag_threshold(from, event.position) {
                        return;
                    }
                    if let Some(Drag::TrackReorder { pressed_at, .. }) = &mut self.drag {
                        *pressed_at = None;
                    }
                }
                let y = self.lane_y(event.position.y);
                if let Some(to) = crate::ui::automation::reorder_target(&self.lane_rows(), track, y)
                {
                    let _ = self.session.move_track(track, to);
                }
            }
            Drag::ClipFade { clip, edge } => {
                // Unsnapped on purpose: a fade is shaped by ear against the waveform, and no
                // grid position has anything to do with where a breath ends.
                let x = event.position.x - self.lanes_origin().x;
                let tick = self.timeline.x_to_tick(x);
                self.drag_clip_fade(clip, edge, tick);
            }
            Drag::NoteMove {
                clip,
                origin_tick,
                origin_pitch,
                ref origins,
                pressed_at,
            } => {
                // The same wobble guard the clips have. Rows are floor-binned, so a click
                // drifting one pixel across a row boundary transposed the whole selection —
                // and auditioned the wrong pitch — before the hand had decided anything.
                if let Some(from) = pressed_at {
                    if !past_drag_threshold(from, event.position) {
                        return;
                    }
                    if let Some(Drag::NoteMove { pressed_at, .. }) = &mut self.drag {
                        *pressed_at = None;
                    }
                }
                let origin = self.roll_origin();
                let tick = self.timeline.x_to_tick(event.position.x - origin.x);
                let pitch = self.pitch.y_to_pitch(event.position.y - origin.y);
                let Some(clip_start) = self.session.midi_clip(clip).map(|c| c.start) else {
                    return;
                };
                let delta_ticks = self.snap_unless_held(tick - clip_start, event.modifiers)
                    - self.snap(origin_tick);
                let delta_pitch = pitch as i32 - origin_pitch as i32;
                let _ = self
                    .session
                    .move_notes(clip, origins, delta_ticks, delta_pitch);
                // Sound where the note has landed, the way pressing one does. Dragging a note up
                // a third was otherwise silent, so the pitch had to be counted off the keyboard
                // at the side of the roll instead of simply heard. Only on a change, or every
                // pointer move would retrigger the note and turn a drag into a stutter.
                if !self.is_auditioning(pitch) {
                    self.audition(pitch);
                }
            }
            Drag::NoteVelocity {
                clip,
                start_y,
                ref origins,
                ..
            } => {
                // No origin to measure against and no snapping to do, so the roll's own y is not
                // wanted here: the drag is a distance from where the button went down.
                self.drag_velocity(clip, start_y, origins, event.position.y);
            }
            Drag::NoteResize {
                clip,
                index,
                pressed_at,
            } => {
                // Guarded only for a grabbed existing note — `pressed_at` is `None` while
                // drawing a new one — so a click wobble cannot snap an off-grid end onto the
                // grid.
                if let Some(from) = pressed_at {
                    if !past_drag_threshold(from, event.position) {
                        return;
                    }
                    if let Some(Drag::NoteResize { pressed_at, .. }) = &mut self.drag {
                        *pressed_at = None;
                    }
                }
                let origin = self.roll_origin();
                let tick = self.timeline.x_to_tick(event.position.x - origin.x);
                let Some(clip_start) = self.session.midi_clip(clip).map(|c| c.start) else {
                    return;
                };
                let end = self.snap_unless_held(tick - clip_start, event.modifiers);
                let _ = self.session.resize_note(clip, index, end);
            }
            Drag::Param {
                target,
                start_value,
                start_x,
            } => {
                let delta = f32::from(event.position.x - start_x);
                self.drag_param(target, start_value, delta);
            }
            Drag::EnvelopeHandle {
                subject, handle, ..
            } => self.drag_envelope_handle(subject, handle, event.position),
            Drag::EqNode { subject, band, .. } => self.drag_eq_node(subject, band, event.position),
            Drag::PartDial {
                clip,
                dial,
                start_fraction,
                start_x,
            } => {
                let delta = f32::from(event.position.x - start_x);
                self.drag_dial(clip, dial, start_fraction, delta);
            }
            Drag::SongDial {
                target,
                start_fraction,
                start_x,
            } => {
                let delta = f32::from(event.position.x - start_x);
                self.drag_song_dial(target, start_fraction, delta);
            }
            Drag::TimeZoom {
                start_fraction,
                start_x,
            } => {
                // A full sweep of the slider is a full sweep of the range, so the drag is
                // measured against the width the widget was drawn at.
                let travel = f32::from(crate::ui::widgets::ZOOM_SLIDER_WIDTH).max(1.0);
                let delta = f32::from(event.position.x - start_x) / travel;
                self.timeline.set_zoom_fraction(start_fraction + delta);
            }
            Drag::Tempo {
                at,
                start_bpm,
                start_x,
            } => {
                // Half a beat per pixel would be unusable; 0.25 BPM/px lets a short drag cover
                // the musically interesting range while still landing on exact values.
                let delta = f64::from(f32::from(event.position.x - start_x)) * 0.25;
                self.session.set_tempo_at(at, start_bpm + delta);
            }
            Drag::MovePluginWindow { grab_offset } => {
                if let Some(window) = self.plugin_window.as_mut() {
                    window.anchor = gpui::point(
                        event.position.x - grab_offset.x,
                        event.position.y - grab_offset.y,
                    );
                }
            }
            Drag::ResizeDock {
                dock,
                start,
                start_size,
            } => {
                let now = match dock.is_side() {
                    true => event.position.x,
                    false => event.position.y,
                };
                self.resize_dock(dock, start_size, now - start);
            }
            Drag::ResizeHeaders {
                start_x,
                start_width,
            } => self.resize_headers(start_width, event.position.x - start_x),
            Drag::RubberBand { .. } => {
                // The band's far corner follows the pointer, and the selection is recomputed
                // from scratch each move — sweeping back over something has to unselect it.
                if let Some(Drag::RubberBand { current, .. }) = &mut self.drag {
                    *current = event.position;
                }
                self.apply_rubber_band();
            }
        }
        cx.notify();
    }

    /// Ends the gesture in progress. Re-registered by an occluding overlay for the reason
    /// [`Self::on_mouse_move`] is, so that a drag begun in one finishes where it was let go
    /// rather than through the root's out-of-bounds path.
    pub(crate) fn on_mouse_up(
        &mut self,
        _event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stop_audition();
        self.end_drag(window, cx);
        cx.notify();
    }

    /// Keys the open rename sheet claims before anything else sees them.
    ///
    /// Only the ones the platform does not deliver as text: characters reach the field through
    /// the input handler instead, which is what lets an IME compose into it.
    fn on_key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.palette_key(event, window, cx)
            || self.prompt_key(event, window, cx)
            || self.menu_key(event, window, cx)
            || self.menu_bar_key(event, window, cx)
        {
            cx.stop_propagation();
            cx.notify();
        }
    }

    /// Arrow keys, Return and Escape, aimed at the menu bar this window draws for itself.
    ///
    /// The bar could be opened with a key and then only answered with the pointer, which is the
    /// half of a keyboard path that is worse than none: the hand leaves the keyboard anyway, and
    /// it has to find a menu that has already dropped open under the cursor.
    ///
    /// Left and right walk the titles, which is what a menu bar does that a lone menu does not —
    /// the bar is one row of menus, and stepping off the end of File onto Edit is how anyone who
    /// has used one expects to get there.
    fn menu_bar_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(open) = self.menu_bar else {
            return false;
        };
        let sections = crate::menu::model(self.language());
        let Some(section) = sections.get(open.index) else {
            // The menus are rebuilt from the language on every frame; an index left over from a
            // shorter set is not a state to try to recover, only one to stop being in.
            self.close_menu_bar();
            return true;
        };

        match event.keystroke.key.as_str() {
            "escape" => {
                self.close_menu_bar();
            }
            "left" | "right" => {
                let delta = if event.keystroke.key == "left" { -1 } else { 1 };
                let index = menu_bar::stepped_section(sections.len(), open.index, delta);
                self.menu_bar = Some(menu_bar::OpenMenu::at(index));
            }
            "down" | "up" | "home" | "end" => {
                let key = event.keystroke.key.as_str();
                let delta = if matches!(key, "down" | "home") {
                    1
                } else {
                    -1
                };
                // Home and End are the same step taken from nothing, which is where a step lands
                // on whichever end the direction implies.
                let from = matches!(key, "down" | "up")
                    .then_some(open.highlighted)
                    .flatten();
                self.menu_bar = Some(menu_bar::OpenMenu {
                    highlighted: menu_bar::stepped(&section.rows, from, delta),
                    ..open
                });
            }
            "enter" => {
                let action = open
                    .highlighted
                    .and_then(|index| section.rows.get(index))
                    .and_then(|row| match row {
                        MenuRow::Command { action, .. } => Some(action.boxed_clone()),
                        MenuRow::Separator | MenuRow::System { .. } => None,
                    });
                // Closed either way. Return on a menu nobody has walked through means "I am done
                // here", not "wait for an answer nobody is going to give".
                self.close_menu_bar();
                if let Some(action) = action {
                    // Dispatched rather than called, the same as a click on the row: the root
                    // view already handles every one of these for the keymap and the system menu
                    // bar, and routing through it keeps the three in step.
                    window.dispatch_action(action, cx);
                }
            }
            _ => return false,
        }
        true
    }

    fn on_open_menu_bar(
        &mut self,
        _: &actions::OpenMenuBar,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Nothing to open where the system owns the bar; macOS has its own way in to that.
        if !Self::wants_menu_bar() {
            return;
        }
        self.menu = None;
        self.menu_bar = match self.menu_bar {
            Some(_) => None,
            None => Some(menu_bar::OpenMenu::at(0)),
        };
        cx.notify();
    }

    /// Arrow keys and Return, aimed at the open context menu.
    ///
    /// The menus had no keyboard path at all: a right-click opened one and only the pointer
    /// could answer it. Escape already closes it, through the same handler that means "never
    /// mind" everywhere else.
    fn menu_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(menu) = self.menu.as_mut() else {
            return false;
        };
        match event.keystroke.key.as_str() {
            // Handled here rather than left to the Escape *binding*, which no longer fires: an
            // open menu takes every binding out of reach so that walking it with the arrow keys
            // cannot also have a letter run a command underneath it.
            "escape" => {
                self.close_menu();
                return true;
            }
            "down" => menu.step(1),
            "up" => menu.step(-1),
            "home" => {
                menu.highlighted = None;
                menu.step(1);
            }
            "end" => {
                menu.highlighted = None;
                menu.step(-1);
            }
            "enter" => {
                let Some(command) = menu.highlighted_command() else {
                    // Return on a menu nobody has moved through closes it, rather than being
                    // swallowed by an overlay that looks like it is waiting for an answer.
                    self.close_menu();
                    return true;
                };
                self.close_menu();
                self.run_menu_command(command, cx);
            }
            _ => return false,
        }
        true
    }

    fn on_toggle_play(
        &mut self,
        _: &actions::TogglePlay,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_play();
        cx.notify();
    }

    fn on_stop(&mut self, _: &actions::StopPlayback, _window: &mut Window, cx: &mut Context<Self>) {
        self.session.stop();
        cx.notify();
    }

    fn on_return_to_zero(
        &mut self,
        _: &actions::ReturnToZero,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.seek(Ticks::ZERO);
        cx.notify();
    }

    fn on_toggle_loop(
        &mut self,
        _: &actions::ToggleLoop,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_loop();
        cx.notify();
    }

    /// Writes a band behind the selected clip's melody.
    ///
    /// The selected clip, because that is the one the editors are pointed at — this is aimed at
    /// something a person is looking at rather than at the song as a whole, which is the whole
    /// difference between it and the two commands above it in the menu.
    fn on_accompany_melody(
        &mut self,
        _: &actions::AccompanyMelody,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.selected_clip {
            Some(clip) => self.run_menu_command(MenuCommand::AccompanyClip(clip), cx),
            None => self.set_status(self.t(Key::NoClipToAccompany)),
        }
        cx.notify();
    }

    fn on_toggle_metronome(
        &mut self,
        _: &actions::ToggleMetronome,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_metronome();
        cx.notify();
    }

    fn on_toggle_recording(
        &mut self,
        _: &actions::ToggleRecording,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_recording(window, cx);
        cx.notify();
    }

    fn on_toggle_monitoring(
        &mut self,
        _: &actions::ToggleMonitoring,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Onto whatever a take would land on, which is where somebody wants to hear themselves.
        // The per-track button is for monitoring one track while looking at another.
        match self
            .session
            .monitored_track()
            .or_else(|| self.record_target())
        {
            Some(track) => self.toggle_monitoring(track),
            None => {
                let line =
                    self.failure(Key::CmdToggleMonitoring, &SessionError::NothingToRecordOnto);
                self.set_failed_status(line);
            }
        }
        cx.notify();
    }

    fn on_toggle_punch(
        &mut self,
        _: &actions::TogglePunch,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_punch();
        cx.notify();
    }

    fn on_new_project(
        &mut self,
        _: &actions::NewProject,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_project_asking();
        cx.notify();
    }

    fn on_quit(&mut self, _: &actions::Quit, _window: &mut Window, cx: &mut Context<Self>) {
        // Handled here rather than on the application, because `App::quit` does not run the
        // window's close guard and a document with unsaved changes has to get its say.
        if self.confirm_discard(crate::ui::prompt::PendingAction::Quit) {
            cx.quit();
        }
        cx.notify();
    }

    fn on_open_project(
        &mut self,
        _: &actions::OpenProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_project(window, cx);
    }

    fn on_compose_song(
        &mut self,
        _: &actions::ComposeSong,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_song_sheet();
        cx.notify();
    }

    fn on_compose_from_spec(
        &mut self,
        _: &actions::ComposeFromSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.compose_from_spec(window, cx);
    }

    fn on_save_project(
        &mut self,
        _: &actions::SaveProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save(window, cx);
        cx.notify();
    }

    fn on_save_project_as(
        &mut self,
        _: &actions::SaveProjectAs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_as(window, cx);
    }

    fn on_import_audio(
        &mut self,
        _: &actions::ImportAudio,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.import_audio(window, cx);
    }

    fn on_import_soundfont(
        &mut self,
        _: &actions::ImportSoundFont,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.import_soundfont(window, cx);
    }

    fn on_import_midi(
        &mut self,
        _: &actions::ImportMidi,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.import_midi(window, cx);
    }

    fn on_export_midi(
        &mut self,
        _: &actions::ExportMidi,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.export_midi(window, cx);
    }

    fn on_collect_assets(
        &mut self,
        _: &actions::CollectAssets,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.collect_assets(cx);
        cx.notify();
    }

    fn on_export_audio(
        &mut self,
        _: &actions::ExportAudio,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_export(window, cx);
    }

    fn on_export_cycle(
        &mut self,
        _: &actions::ExportCycle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_export_cycle(window, cx);
    }

    fn on_add_instrument_track(
        &mut self,
        _: &actions::AddInstrumentTrack,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_instrument_track();
        cx.notify();
    }

    fn on_add_audio_track(
        &mut self,
        _: &actions::AddAudioTrack,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_audio_track();
        cx.notify();
    }

    fn on_delete_track(
        &mut self,
        _: &actions::DeleteTrack,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_selected_track();
        cx.notify();
    }

    fn on_delete_selection(
        &mut self,
        _: &actions::DeleteSelection,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_selection();
        cx.notify();
    }

    // ---------------------------------------------------------------- from the context menus
    //
    // Each of these ran the same [`MenuCommand`] a right-click already ran, and did so by calling
    // `run_menu_command` rather than by reimplementing it: a keystroke and a menu row that mean
    // the same thing must *be* the same thing, or the pair drift and one of them starts leaving
    // the selection somewhere the other does not.
    //
    // What they act on is the selection, because a keystroke has no pointer to be under. The
    // clip commands take `selected_clip`, which is what the arrangement's own menu passes when it
    // is opened over a clip that is already selected.

    fn on_add_bus_track(
        &mut self,
        _: &actions::AddBusTrack,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_bus_track();
        cx.notify();
    }

    fn on_duplicate_track(
        &mut self,
        _: &actions::DuplicateTrack,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(track) = self.selected_track {
            self.run_menu_command(MenuCommand::DuplicateTrack(track), cx);
        }
        cx.notify();
    }

    fn on_toggle_track_mute(
        &mut self,
        _: &actions::ToggleTrackMute,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(track) = self.selected_track {
            self.toggle_mute(track);
        }
        cx.notify();
    }

    fn on_toggle_track_solo(
        &mut self,
        _: &actions::ToggleTrackSolo,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(track) = self.selected_track {
            self.toggle_solo(track);
        }
        cx.notify();
    }

    fn on_select_all_notes(
        &mut self,
        _: &actions::SelectAllNotes,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_menu_command(MenuCommand::SelectAllNotes, cx);
        cx.notify();
    }

    fn on_duplicate_notes(
        &mut self,
        _: &actions::DuplicateNotes,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_menu_command(MenuCommand::DuplicateNotes, cx);
        cx.notify();
    }

    fn on_cut_notes(
        &mut self,
        _: &actions::CutNotes,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_menu_command(MenuCommand::CutNotes, cx);
        cx.notify();
    }

    fn on_copy_notes(
        &mut self,
        _: &actions::CopyNotes,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_menu_command(MenuCommand::CopyNotes, cx);
        cx.notify();
    }

    fn on_paste_notes(
        &mut self,
        _: &actions::PasteNotes,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_menu_command(MenuCommand::PasteNotes, cx);
        cx.notify();
    }

    fn on_cut_clips(
        &mut self,
        _: &actions::CutClips,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(clip) = self.selected_clip {
            self.run_menu_command(MenuCommand::CutClips(clip), cx);
        }
        cx.notify();
    }

    fn on_copy_clips(
        &mut self,
        _: &actions::CopyClips,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(clip) = self.selected_clip {
            self.run_menu_command(MenuCommand::CopyClips(clip), cx);
        }
        cx.notify();
    }

    /// Lays the clipboard's clips onto the selected track, at the playhead.
    ///
    /// The playhead rather than where the clips came from, which is the one position a paste with
    /// no pointer behind it can mean — and the selected track rather than the one the material was
    /// copied off, so a block can be moved to another part of the arrangement in two keystrokes.
    fn on_paste_clips(
        &mut self,
        _: &actions::PasteClips,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(track) = self.paste_target_track() else {
            self.set_status(self.t(Key::NoTrackToPasteOnto));
            cx.notify();
            return;
        };
        let at = self.playhead_ticks();
        self.run_menu_command(MenuCommand::PasteClips { track, at }, cx);
        cx.notify();
    }

    /// Which track a keyboard paste lands on: the selected one, or the first that could hold it.
    ///
    /// The fallback is what makes ⌘V work on a project nobody has clicked in yet. It is the first
    /// track rather than none at all because a paste that silently does nothing looks like a
    /// broken keystroke, and the status line cannot say "nowhere" any more usefully than the
    /// arrangement can show the material landing on row one.
    fn paste_target_track(&self) -> Option<TrackId> {
        self.selected_track
            .filter(|id| self.project().track(*id).is_some())
            .or_else(|| self.project().tracks.first().map(|track| track.id))
    }

    fn on_transpose_up(
        &mut self,
        _: &actions::TransposeUp,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_menu_command(MenuCommand::TransposeNotes(1), cx);
        cx.notify();
    }

    fn on_transpose_down(
        &mut self,
        _: &actions::TransposeDown,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_menu_command(MenuCommand::TransposeNotes(-1), cx);
        cx.notify();
    }

    fn on_octave_up(
        &mut self,
        _: &actions::OctaveUp,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_menu_command(MenuCommand::TransposeNotes(12), cx);
        cx.notify();
    }

    fn on_octave_down(
        &mut self,
        _: &actions::OctaveDown,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_menu_command(MenuCommand::TransposeNotes(-12), cx);
        cx.notify();
    }

    /// Selects every clip in the song, on every track.
    ///
    /// Not only the clips of the selected track: ⌘A means everything in the thing you are looking
    /// at, and what the arrangement shows is the whole song.
    fn on_select_all_clips(
        &mut self,
        _: &actions::SelectAllClips,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let clips: std::collections::BTreeSet<ClipId> = self
            .project()
            .tracks
            .iter()
            .flat_map(|track| {
                let midi = track
                    .kind
                    .as_instrument()
                    .into_iter()
                    .flat_map(|instrument| instrument.clips.iter().map(|clip| clip.id));
                let audio = track
                    .kind
                    .as_audio()
                    .into_iter()
                    .flat_map(|audio| audio.clips.iter().map(|clip| clip.id));
                midi.chain(audio)
            })
            .collect();
        // The editors keep pointing at the clip they were on, so selecting everything does not
        // swap the piano roll out from under the user.
        let primary = self.selected_clip.filter(|id| clips.contains(id));
        self.select_clips(clips, primary);
        cx.notify();
    }

    fn on_duplicate_clip(
        &mut self,
        _: &actions::DuplicateClip,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(clip) = self.selected_clip {
            self.run_menu_command(MenuCommand::DuplicateClip(clip), cx);
        }
        cx.notify();
    }

    fn on_split_clip(
        &mut self,
        _: &actions::SplitClip,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(clip) = self.selected_clip {
            self.run_menu_command(MenuCommand::SplitClipAtPlayhead(clip), cx);
        }
        cx.notify();
    }

    fn on_toggle_clip_mute(
        &mut self,
        _: &actions::ToggleClipMute,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(clip) = self.selected_clip {
            self.run_menu_command(MenuCommand::ToggleClipMute(clip), cx);
        }
        cx.notify();
    }

    fn on_toggle_clip_loop(
        &mut self,
        _: &actions::ToggleClipLoop,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(clip) = self.selected_clip {
            self.run_menu_command(MenuCommand::ToggleClipLoop(clip), cx);
        }
        cx.notify();
    }

    fn on_quantize_starts(
        &mut self,
        _: &actions::QuantizeNoteStarts,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_menu_command(MenuCommand::QuantizeNotes(Quantize::Starts), cx);
        cx.notify();
    }

    fn on_quantize_lengths(
        &mut self,
        _: &actions::QuantizeNoteLengths,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_menu_command(MenuCommand::QuantizeNotes(Quantize::Lengths), cx);
        cx.notify();
    }

    fn on_quantize_notes(
        &mut self,
        _: &actions::QuantizeNotes,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_menu_command(MenuCommand::QuantizeNotes(Quantize::Both), cx);
        cx.notify();
    }

    fn on_undo(&mut self, _: &actions::Undo, _window: &mut Window, cx: &mut Context<Self>) {
        self.undo();
        cx.notify();
    }

    fn on_redo(&mut self, _: &actions::Redo, _window: &mut Window, cx: &mut Context<Self>) {
        self.redo();
        cx.notify();
    }

    fn on_panic_stop(
        &mut self,
        _: &actions::PanicStop,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Escape takes back the nearest thing first. Panicking the engine while a menu is up
        // would be a surprising answer to "never mind", and so would silencing a drag that the
        // user only wanted to abandon — so each of these returns rather than falling through.
        //
        // The menus close themselves on Escape now, in their own key handlers, because a menu
        // takes every binding out of reach while it is open and this one is a binding. Kept
        // because the action is dispatchable from the palette as well as from the keyboard, and
        // "silence everything" arriving from there should still put a menu away rather than
        // leave it hanging over a stopped engine.
        if self.close_menu() || self.close_menu_bar() {
            cx.notify();
            return;
        }
        // A gesture in progress goes back where it started. Clearing the field alone would
        // leave the session's transaction open, and every edit after that inaudible.
        if self.abort_drag() {
            self.stop_audition();
            cx.notify();
            return;
        }
        // A finished export is a sheet with one button on it, and Escape is how a sheet closes.
        // A running one has no cancel, so Escape leaves it alone rather than pretending.
        if self
            .export
            .as_ref()
            .is_some_and(|export| export.result.is_some())
        {
            self.export = None;
            cx.notify();
            return;
        }
        if self.close_plugin_window() {
            cx.notify();
            return;
        }
        self.session.panic();
        self.set_status(self.t(Key::PanicStopped));
        cx.notify();
    }

    fn on_zoom_in(&mut self, _: &actions::ZoomIn, _window: &mut Window, cx: &mut Context<Self>) {
        self.timeline.zoom_by(1.3, px(0.0));
        cx.notify();
    }

    fn on_zoom_out(&mut self, _: &actions::ZoomOut, _window: &mut Window, cx: &mut Context<Self>) {
        self.timeline.zoom_by(1.0 / 1.3, px(0.0));
        cx.notify();
    }

    /// Puts the next of the roll's tools in hand, and says which one that is.
    ///
    /// The status line rather than the tool strip alone, because the strip is only on screen when
    /// the piano roll is: the editor panel can be hidden or showing the mixer, and a mode changed
    /// where nothing says so is the whole hazard of having tools at all. A gesture in progress
    /// keeps the tool it started with — swapping tools out from under a drag would leave it half
    /// one thing and half another.
    fn on_next_tool(
        &mut self,
        _: &actions::NextTool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.drag.is_some() {
            return;
        }
        self.tool = self.tool.next();
        self.set_status(messages::tool_in_hand(
            self.language(),
            self.t(self.tool.label()),
        ));
        cx.notify();
    }

    /// Opens the tempo sheet, aimed where the readout is.
    ///
    /// These four reach the readouts in the middle of the transport bar, which until now answered
    /// to the mouse and to nothing else — which meant they were absent from the command palette,
    /// from the settings window's list of keys, and from the reach of anybody who works from the
    /// keyboard.
    fn on_set_tempo(
        &mut self,
        _: &actions::SetTempo,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prompt_for_tempo();
        cx.notify();
    }

    fn on_set_time_signature(
        &mut self,
        _: &actions::SetTimeSignature,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prompt_for_signature();
        cx.notify();
    }

    fn on_cycle_grid(
        &mut self,
        _: &actions::CycleGrid,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_grid();
        // The grid button is in the corner of the transport bar and a keystroke does not point at
        // it, so the status line is what says the division changed.
        self.set_status(messages::grid_set(self.language(), self.grid_label()));
        cx.notify();
    }

    fn on_go_to_position(
        &mut self,
        _: &actions::GoToPosition,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prompt_for_position();
        cx.notify();
    }

    fn on_toggle_library(
        &mut self,
        _: &actions::ToggleLibrary,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_panel(Panel::Library);
        cx.notify();
    }

    fn on_toggle_inspector(
        &mut self,
        _: &actions::ToggleInspector,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_panel(Panel::Inspector);
        cx.notify();
    }

    fn on_toggle_piano_roll(
        &mut self,
        _: &actions::TogglePianoRoll,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_panel(Panel::PianoRoll);
        cx.notify();
    }

    fn on_toggle_mixer(
        &mut self,
        _: &actions::ToggleMixer,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_panel(Panel::Mixer);
        cx.notify();
    }

    fn on_toggle_log(
        &mut self,
        _: &actions::ToggleLog,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_panel(Panel::Log);
        cx.notify();
    }

    fn on_toggle_structure_lane(
        &mut self,
        _: &actions::ToggleStructureLane,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.panels.lanes.structure = !self.panels.lanes.structure;
        self.remember_layout();
        cx.notify();
    }

    fn on_toggle_harmony_lane(
        &mut self,
        _: &actions::ToggleHarmonyLane,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.panels.lanes.harmony = !self.panels.lanes.harmony;
        self.remember_layout();
        cx.notify();
    }

    fn on_toggle_tempo_marks(
        &mut self,
        _: &actions::ToggleTempoMarks,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.panels.lanes.tempo = !self.panels.lanes.tempo;
        self.remember_layout();
        cx.notify();
    }

    fn on_toggle_bend_lane(
        &mut self,
        _: &actions::ToggleBendLane,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_curve_lane(ClipCurve::Bend);
        cx.notify();
    }

    fn on_toggle_modulation_lane(
        &mut self,
        _: &actions::ToggleModulationLane,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_curve_lane(ClipCurve::Modulation);
        cx.notify();
    }

    /// Shows or hides one of the roll's curve strips, and remembers it.
    fn toggle_curve_lane(&mut self, which: ClipCurve) {
        let shown = self.panels.curve_lane(which);
        self.panels.set_curve_lane(which, !shown);
        self.remember_layout();
    }

    fn on_open_settings(
        &mut self,
        _: &actions::OpenSettings,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings(cx);
        cx.notify();
    }

    fn on_open_command_palette(
        &mut self,
        _: &actions::OpenCommandPalette,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_palette();
        cx.notify();
    }
}
